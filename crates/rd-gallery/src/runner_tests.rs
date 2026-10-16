//! gallery-dl's arguments, its folder and how its failures are classified.

use std::path::Path;

use super::{claim_folder, gallery_args, gallery_folder, map_gallery_error, timed_out};
use rd_core::{FailureKind, ProxyKind};
use rd_scheduler::{ToolNetwork, ToolProxy};
use secrecy::SecretString;

fn proxy(password: Option<&str>) -> ToolNetwork {
    let password = password.map(|password| SecretString::from(password.to_owned()));
    ToolNetwork::with_proxy(
        ToolProxy::new(
            ProxyKind::Socks5,
            "socks5://proxy.example:1080".parse().expect("endpoint"),
            password.as_ref().map(|_| "alice"),
            password.as_ref(),
        )
        .expect("proxy"),
    )
}

fn args(network: &ToolNetwork) -> Vec<String> {
    gallery_args(
        Path::new("/downloads/set"),
        "https://gallery.example/set/1",
        None,
        network,
        None,
    )
    .into_iter()
    .map(|arg| arg.to_string_lossy().into_owned())
    .collect()
}

/// RD-1240-08: the download's proxy is gallery-dl's own option, and without one there is
/// none.
#[test]
fn the_proxy_is_on_the_command_line_only_when_there_is_one() {
    assert_eq!(
        args(&proxy(None)),
        [
            "--proxy",
            "socks5://proxy.example:1080",
            "-D",
            "/downloads/set",
            "--",
            "https://gallery.example/set/1"
        ]
    );
    let direct = args(&ToolNetwork::direct());
    assert!(
        !direct.iter().any(|arg| arg.contains("proxy")),
        "{direct:?}"
    );
}

/// RD-1320-03: a cookie profile is gallery-dl's `--cookies` file, named by its path.
#[test]
fn a_cookie_file_is_handed_over_by_path() {
    let args: Vec<String> = gallery_args(
        Path::new("/downloads/set"),
        "https://gallery.example/set/1",
        None,
        &ToolNetwork::direct(),
        Some(Path::new("/tmp/rd-cookies-x.txt")),
    )
    .into_iter()
    .map(|arg| arg.to_string_lossy().into_owned())
    .collect();
    assert_eq!(
        args,
        [
            "--cookies",
            "/tmp/rd-cookies-x.txt",
            "-D",
            "/downloads/set",
            "--",
            "https://gallery.example/set/1"
        ]
    );
}

/// Credentials never reach the argument list; gallery-dl reads them from its environment.
#[test]
fn proxy_credentials_stay_off_the_command_line() {
    let args = args(&proxy(Some("pr0xy-secret")));
    assert!(!args.iter().any(|arg| arg.contains("pr0xy")), "{args:?}");
    assert!(!args.iter().any(|arg| arg == "--proxy"), "{args:?}");
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-o".to_owned(), "proxy-env=true".to_owned()]),
        "{args:?}"
    );
}

#[test]
fn a_proxy_gallery_dl_cannot_speak_fails_with_a_code() {
    let stderr = "requests.exceptions.InvalidSchema: Missing dependencies for SOCKS support.";
    let failure = proxy(None)
        .unsupported_proxy("gallery-dl", stderr)
        .expect("failure");
    assert_eq!(failure.code.as_deref(), Some("proxy.unsupported_by_tool"));
    assert!(!failure.category.is_retryable());
}

/// RA-TR-03: a deadline is a failure with a retry, not a stop.
#[test]
fn a_run_past_its_deadline_is_a_retryable_failure() {
    let failure = timed_out("");
    assert_eq!(failure.code.as_deref(), Some("gallery.tool_failed"));
    assert!(failure.category.is_retryable());
    assert!(
        failure.message.contains("time limit"),
        "{}",
        failure.message
    );
}

/// RD-1240-28: a second gallery of the same name met the first one's folder, gallery-dl
/// skipped the file there, and the download finished with nothing transferred.
#[test]
fn a_new_gallery_never_shares_another_one_s_folder() {
    let destination = tempfile::tempdir().expect("tempdir");
    let root = destination.path();
    // Nothing there yet, or only an empty folder: the download's own name.
    assert_eq!(claim_folder(root, "picture", false), "picture");
    std::fs::create_dir(root.join("picture")).expect("folder");
    assert_eq!(claim_folder(root, "picture", false), "picture");
    // Another download's file in it: the next free name.
    std::fs::write(root.join("picture").join("direct-picture.png"), b"png").expect("file");
    assert_eq!(claim_folder(root, "picture", false), "picture (1)");
    // An earlier attempt of the same download: its own files, adopted.
    assert_eq!(claim_folder(root, "picture", true), "picture");
    // The claimed name read back as the folder, so a later attempt finds it again.
    assert_eq!(gallery_folder("picture (1)"), "picture (1)");
}

#[test]
fn folder_name_is_sanitized_and_never_empty() {
    assert_eq!(gallery_folder("artworks"), "artworks");
    assert_eq!(gallery_folder("set: one?"), "set_ one_");
    assert_eq!(gallery_folder(""), "gallery");
}

#[test]
fn errors_map_to_retry_policy() {
    assert!(matches!(
        map_gallery_error("error: Unsupported URL 'https://x'").category,
        FailureKind::Unsupported
    ));
    assert!(matches!(
        map_gallery_error("error: Login required").category,
        FailureKind::Permanent
    ));
    assert!(matches!(
        map_gallery_error("HTTPError 503").category,
        FailureKind::Transient { .. }
    ));
}

/// RD-1240-28: gallery-dl's words for a proxy that refused its password, from the live test
/// of 1.24, are the proxy's failure, not a sign-in the gallery asks for.
#[test]
fn a_proxy_refusing_its_password_is_a_proxy_failure() {
    let stderr = "[downloader.http][error] ProxyError: ('Unable to connect to proxy', \
        OSError('Tunnel connection failed: 407 Proxy Authentication Required'))";
    let failure = map_gallery_error(stderr);
    assert_eq!(failure.code.as_deref(), Some("proxy.auth_failed"));
    assert!(matches!(failure.category, FailureKind::Permanent));
    assert_eq!(
        map_gallery_error("[twitter][error] AuthenticationError: login required")
            .code
            .as_deref(),
        Some("gallery.auth_required")
    );
}
