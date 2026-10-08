use url::Url;

use super::{
    decide_paired, discard_fallback_token, ensure_transport_is_safe, load_from, read_public_config,
    store_token, stored_service_in, write_public_config,
};

fn service(address: &str) -> Url {
    Url::parse(address).expect("a valid service URL")
}

/// A fresh directory of this test's own, so nothing reaches the real configuration.
fn scratch(name: &str) -> std::path::PathBuf {
    let directory =
        std::env::temp_dir().join(format!("rd-capture-config-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the scratch directory");
    directory
}

/// The defect: a successful pairing left an older plaintext token on disk, and `load` falls
/// back to exactly that file whenever the keyring cannot be read.
#[test]
fn a_successful_keyring_write_takes_the_plaintext_token_with_it() {
    let directory = scratch("keyring-write");
    let path = directory.join("capture.token");
    std::fs::write(&path, "an-older-token-that-was-revoked").expect("write the old token");

    store_token(&directory, &"n".repeat(32), |_| Ok(())).expect("store the token");
    assert!(
        !path.exists(),
        "a token the keyring accepted must not stay readable on disk as well"
    );

    // Removing what is not there is not an error; pairing twice must not fail.
    discard_fallback_token(&directory).expect("a missing file is nothing to report");
    std::fs::remove_dir_all(&directory).expect("clean up");
}

/// The fallback still exists for the host that has no working keyring.
#[test]
fn a_failed_keyring_write_still_leaves_a_usable_token() {
    let directory = scratch("keyring-fail");
    let result = store_token(&directory, &"n".repeat(32), |_| {
        Err(anyhow::anyhow!("no secret service on this host"))
    });
    if cfg!(unix) {
        result.expect("the Unix fallback writes the file");
        assert!(directory.join("capture.token").exists());
    } else {
        assert!(
            result.is_err(),
            "without a keyring and without a Unix fallback there is nowhere to put it"
        );
    }
    std::fs::remove_dir_all(&directory).expect("clean up");
}

/// A leftover plaintext file must not outvote the keyring.
#[test]
fn the_keyring_is_asked_before_the_plaintext_file() {
    assert!(
        decide_paired(None, Some("a-current-token"), None),
        "the keyring alone is enough"
    );
    assert!(
        decide_paired(None, None, Some("a-token-from-a-host-without-a-keyring")),
        "the file is still the answer when the keyring holds nothing"
    );
    assert!(
        !decide_paired(None, Some("   "), Some("   ")),
        "blank is not a token in either place"
    );
    assert!(!decide_paired(None, None, None));
    assert!(decide_paired(
        Some("passed-on-the-command-line"),
        None,
        None
    ));
}

/// A long-lived bearer token must not go out over the network in the clear by default.
#[test]
fn a_non_loopback_service_needs_https_or_the_explicit_flag() {
    for address in [
        "http://127.0.0.1:8710",
        "http://[::1]:8710",
        "http://localhost:8710",
        "https://nas.example",
        "https://192.168.0.5:8710",
    ] {
        ensure_transport_is_safe(&service(address), false)
            .unwrap_or_else(|error| panic!("{address} should be accepted: {error}"));
    }

    for address in ["http://nas.example", "http://192.168.0.5:8710"] {
        let refused = ensure_transport_is_safe(&service(address), false)
            .expect_err("plain http off this machine is refused");
        assert!(
            refused.to_string().contains("--allow-insecure-service"),
            "the refusal has to name the way out: {refused}"
        );
        ensure_transport_is_safe(&service(address), true).expect("the explicit flag accepts it");
    }
}

/// A direct write truncates the destination first, so an interruption leaves a broken file
/// behind. The temporary-file-and-rename means a failed write leaves the previous one whole.
#[test]
fn a_failed_write_leaves_the_previous_configuration_intact() {
    let directory = scratch("atomic-write");
    write_public_config(&directory, &service("https://nas.example/")).expect("the first write");
    let before = std::fs::read(directory.join("capture.json")).expect("the written file");

    // The rename cannot happen, because the name the new content goes to is taken by a
    // directory. That stands in for every way the write can end early.
    std::fs::create_dir(directory.join("capture.json.new")).expect("block the temporary name");
    write_public_config(&directory, &service("https://other.example/"))
        .expect_err("the write cannot complete");

    assert_eq!(
        std::fs::read(directory.join("capture.json")).expect("the file is still there"),
        before,
        "an interrupted write must not touch the file that was already good"
    );
    assert_eq!(
        read_public_config(&directory)
            .expect("still readable")
            .expect("still there")
            .service
            .as_str(),
        "https://nas.example/"
    );
    std::fs::remove_dir_all(&directory).expect("clean up");
}

/// The silent fallback: a truncated file became `http://127.0.0.1:8710`, so an agent paired
/// against a remote host handed its bearer token to whatever was listening locally.
#[test]
fn an_unreadable_configuration_is_reported_rather_than_replaced() {
    let directory = scratch("unreadable");
    assert!(
        read_public_config(&directory)
            .expect("no file is not an error")
            .is_none(),
        "an absent file is the unpaired case, not a failure"
    );

    std::fs::write(
        directory.join("capture.json"),
        br#"{"service":"https://nas.exa"#,
    )
    .expect("write a truncated file");
    let error = read_public_config(&directory).expect_err("a truncated file is an error");
    assert!(
        error.to_string().contains("capture configuration"),
        "{error}"
    );

    // And the same through the door the HTTP client comes in by.
    let error = load_from(&directory, None, Some("t".repeat(32)))
        .err()
        .expect("load must not invent a default here");
    assert!(
        !format!("{error:?}").contains("127.0.0.1:8710"),
        "the default address must not appear as the answer: {error:?}"
    );
    std::fs::remove_dir_all(&directory).expect("clean up");
}

/// Whoever can write `capture.json` used to decide what the tray handed to the operating
/// system's "open this" and what the HTTP client sent the token to.
#[test]
fn a_foreign_scheme_reaches_neither_the_tray_nor_the_client() {
    let directory = scratch("foreign-scheme");
    std::fs::write(
        directory.join("capture.json"),
        br#"{"service":"file:///etc/passwd"}"#,
    )
    .expect("write the file");

    // The tray path.
    assert_eq!(
        stored_service_in(&directory, None),
        None,
        "an address the agent cannot speak to must not reach open::that_detached"
    );
    // The HTTP client path.
    let error = load_from(&directory, None, Some("t".repeat(32)))
        .err()
        .expect("load refuses it too");
    assert!(format!("{error:#}").contains("http or https"), "{error:#}");

    // An explicit `--service` goes through the same gate.
    assert_eq!(
        stored_service_in(&directory, Some(service("ftp://nas.example/"))),
        None
    );
    assert!(
        load_from(
            &directory,
            Some(service("ftp://nas.example/")),
            Some("t".repeat(32))
        )
        .is_err()
    );

    // A usable one passes both ways.
    write_public_config(&directory, &service("https://nas.example/")).expect("rewrite");
    assert_eq!(
        stored_service_in(&directory, None).map(|value| value.to_string()),
        Some("https://nas.example/".to_owned())
    );
    assert_eq!(
        load_from(&directory, None, Some("t".repeat(32)))
            .expect("a usable configuration loads")
            .service
            .as_str(),
        "https://nas.example/"
    );
    std::fs::remove_dir_all(&directory).expect("clean up");
}

/// RD-1190-22: `--service` at `run` used to warn about plain http and send the token anyway.
#[test]
fn an_explicit_plain_http_service_is_refused_at_start() {
    let directory = scratch("explicit-http");
    let error = load_from(
        &directory,
        Some(service("http://192.168.0.5:8710")),
        Some("t".repeat(32)),
    )
    .err()
    .expect("plain http off this machine is refused");
    assert!(
        format!("{error:#}").contains("--allow-insecure-service"),
        "{error:#}"
    );
    for address in ["http://127.0.0.1:8710", "https://192.168.0.5:8710"] {
        load_from(&directory, Some(service(address)), Some("t".repeat(32)))
            .unwrap_or_else(|error| panic!("{address} should be accepted: {error:#}"));
    }
    std::fs::remove_dir_all(&directory).expect("clean up");
}

/// RD-1190-22: the fallback file is the token in plain text; only its owner may read it, also
/// when an earlier, wider file was already there.
#[cfg(unix)]
#[test]
fn the_fallback_token_file_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let directory = scratch("fallback-mode");
    let path = directory.join("capture.token");
    std::fs::write(&path, "an-older-token").expect("write the old token");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("widen it");
    store_token(&directory, &"n".repeat(32), |_| {
        Err(anyhow::anyhow!("no secret service on this host"))
    })
    .expect("the Unix fallback writes the file");
    let mode = std::fs::metadata(&path)
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    std::fs::remove_dir_all(&directory).expect("clean up");
}
