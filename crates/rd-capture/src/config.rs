use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use url::Url;

const KEYRING_SERVICE: &str = "rDownloader Capture";
const KEYRING_USER: &str = "capture-token";

/// Service URL assumed when nothing has been paired yet.
pub const DEFAULT_SERVICE: &str = "http://127.0.0.1:8710";

/// How often the agent asks the service how it is doing.
///
/// One constant for both polls — the tray's health probe and the transfer summary. They stay
/// two loops on purpose, because the health check works before the agent is paired while the
/// summary needs the capture token, and the tray deliberately never reads the keyring itself.
/// The cadence, though, is shared: the icon and the line under it are read together, and two
/// constants in two files that "must match" is a promise nothing enforces (RD-109-09).
pub const STATUS_POLL_INTERVAL: Duration = Duration::from_secs(5);

pub struct Connection {
    pub service: Url,
    pub token: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PublicConfig {
    service: Url,
}

pub fn save(service: &Url, token: &str, allow_insecure: bool) -> Result<()> {
    if token.trim().len() < 32 {
        bail!("capture token is unexpectedly short");
    }
    // Checked here rather than at the first request, so the answer arrives while somebody is
    // still looking at a terminal and can decide (RD-109-04).
    let service = checked_service(service.clone())?;
    ensure_transport_is_safe(&service, allow_insecure)?;
    let directory = config_directory()?;
    std::fs::create_dir_all(&directory)?;
    write_public_config(&directory, &service)?;
    store_token(&directory, token, |value| {
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
            .and_then(|entry| entry.set_password(value))
            .map_err(anyhow::Error::new)
    })
}

/// Refuses a service address that would carry the capture token in the clear.
///
/// The token is a long-lived bearer credential: it rides on every five-second poll and for the
/// whole life of the event stream. Over loopback nothing but this machine sees it, and https
/// protects it anywhere else -- but plain http to a host on the network hands it to everything
/// between, and nothing used to say so (RD-109-04).
///
/// `allow_insecure` is the named way out, for a network somebody vouches for. It is not silent:
/// it writes a warning at pairing time and the agent writes one again on every start.
pub fn ensure_transport_is_safe(service: &Url, allow_insecure: bool) -> Result<()> {
    if service.scheme() == "https" || is_loopback(service) {
        return Ok(());
    }
    if allow_insecure {
        tracing::warn!(
            %service,
            "the capture token is sent unencrypted to this address; \
             --allow-insecure-service was given"
        );
        return Ok(());
    }
    bail!(
        "{service} is neither loopback nor https, so the capture token would travel the network \
         unencrypted on every request. Use an https address, or pass \
         --allow-insecure-service to accept that for a network you trust."
    )
}

/// Whether the address is this machine talking to itself.
fn is_loopback(service: &Url) -> bool {
    match service.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        // `localhost` resolves to a loopback address by specification, and a resolver that says
        // otherwise is a problem of its own rather than one this check can decide.
        Some(url::Host::Domain(name)) => {
            name.eq_ignore_ascii_case("localhost") || name.eq_ignore_ascii_case("localhost.")
        }
        None => false,
    }
}

/// Puts the token in one place and leaves no older copy of it anywhere else.
///
/// The keyring write used to leave a `capture.token` from an earlier pairing untouched, and
/// `load` falls back to exactly that file whenever the keyring cannot be read -- a locked
/// session, a swapped backend, a different login path. A revoked token therefore kept being
/// presented for as long as the file sat there (RD-109-04).
///
/// The keyring write is a parameter so both outcomes can be driven in a test without a keyring.
fn store_token(
    directory: &std::path::Path,
    token: &str,
    into_keyring: impl FnOnce(&str) -> Result<()>,
) -> Result<()> {
    match into_keyring(token) {
        Ok(()) => discard_fallback_token(directory),
        Err(error) => save_fallback_token(directory, token)
            .with_context(|| format!("keyring unavailable ({error}); fallback failed")),
    }
}

/// Removes the plaintext token file, if one is there.
fn discard_fallback_token(directory: &std::path::Path) -> Result<()> {
    let path = directory.join("capture.token");
    match std::fs::remove_file(&path) {
        Ok(()) => {
            tracing::info!("removed the plaintext capture token; the keyring holds it now");
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::Error::new(error))
            .with_context(|| format!("remove the superseded {}", path.display())),
    }
}

/// Exit code for "started correctly, but there is nothing to connect to yet".
///
/// Distinct from a failure so a launcher can say "not paired yet" instead of "could not be
/// started": on a fresh install the agent cannot run, because pairing happens in the web
/// interface after the server is up, and reporting that as a crash sends people looking for a
/// fault that is not there.
pub const EXIT_NOT_PAIRED: u8 = 10;

/// Exit code for "the Click'n'Load port is already taken".
///
/// Same reasoning as [`EXIT_NOT_PAIRED`], for the other state that is not a fault: nothing but
/// a capture agent binds 9666, so a busy port means a Click'n'Load listener is already there —
/// a second agent from autostart, or JDownloader. Reporting that as a failed start sends people
/// to the logs for a crash that never happened.
pub const EXIT_PORT_BUSY: u8 = 11;

/// Whether a capture token is available at all, without reading one out of the keyring twice.
#[must_use]
pub fn is_paired(token: Option<&str>) -> bool {
    let fallback = config_directory()
        .ok()
        .and_then(|directory| std::fs::read_to_string(directory.join("capture.token")).ok());
    decide_paired(token, load_keyring_token().as_deref(), fallback.as_deref())
}

/// The rule behind [`is_paired`], with its three sources passed in so it can be tested.
///
/// The order is the point. The keyring is where `save` puts the token, so it is asked before
/// the plaintext file; the file used to come first, and a leftover from a machine where the
/// keyring had once been unavailable then outvoted whatever the keyring actually held. The file
/// is still consulted when the keyring holds nothing, because that is the case it exists for --
/// a pairing on a host with no working keyring (RD-109-04).
fn decide_paired(explicit: Option<&str>, keyring: Option<&str>, fallback: Option<&str>) -> bool {
    let present = |value: Option<&str>| value.is_some_and(|value| !value.trim().is_empty());
    present(explicit) || present(keyring) || present(fallback)
}

pub fn load(service: Option<Url>, token: Option<String>) -> Result<Connection> {
    load_from(&config_directory()?, service, token)
}

/// The body of [`load`], with the directory passed in so it can be driven in a test.
fn load_from(
    directory: &std::path::Path,
    service: Option<Url>,
    token: Option<String>,
) -> Result<Connection> {
    let service = match service {
        Some(explicit) => checked_service(explicit)?,
        None => match read_public_config(directory)? {
            Some(config) => config.service,
            None => default_service(),
        },
    };
    // An agent paired before RD-109-04, or one paired with --allow-insecure-service, keeps
    // working -- the rule is enforced at pairing time and nothing existing is broken by it.
    // It says so on every start rather than never, which is the whole difference.
    if ensure_transport_is_safe(&service, false).is_err() {
        tracing::warn!(
            %service,
            "the capture token is sent to this address unencrypted on every request; \
             pair again with an https address to stop that"
        );
    }
    let token = token
        .filter(|value| !value.trim().is_empty())
        .or_else(load_keyring_token)
        .or_else(|| std::fs::read_to_string(directory.join("capture.token")).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .context("capture token missing; run `rdownloader-capture configure --token ...`")?;
    Ok(Connection { service, token })
}

/// Parses [`DEFAULT_SERVICE`]; the literal is a compile-time constant, so this
/// cannot fail.
pub fn default_service() -> Url {
    Url::parse(DEFAULT_SERVICE).expect("static service URL")
}

/// The paired service URL, or `None` while the agent is unconfigured.
///
/// Unlike [`load`] this never touches the keyring: the tray only needs the host
/// for its status line, and a second lookup would risk a second macOS Keychain
/// prompt on top of the one the agent itself triggers.
#[cfg(any(windows, target_os = "macos", test))]
// Compiled on every host so `stored_service_in` below is tested here; only the tray calls it,
// and the tray does not exist on Linux.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub fn stored_service(explicit: Option<Url>) -> Option<Url> {
    let directory = config_directory().ok()?;
    stored_service_in(&directory, explicit)
}

/// The body of [`stored_service`], with the directory passed in so it can be driven in a test.
///
/// An address that cannot be used is `None` with a log line, not a value passed on: the tray
/// hands this straight to `open::that_detached`, which is the operating system's "open whatever
/// this is" (RD-109-05).
#[cfg(any(windows, target_os = "macos", test))]
fn stored_service_in(directory: &std::path::Path, explicit: Option<Url>) -> Option<Url> {
    let read = || -> Result<Option<Url>> {
        match explicit {
            Some(explicit) => Ok(Some(checked_service(explicit)?)),
            None => Ok(read_public_config(directory)?.map(|config| config.service)),
        }
    };
    match read() {
        Ok(service) => service,
        Err(error) => {
            tracing::warn!(%error, "the stored service address cannot be used");
            None
        }
    }
}

fn config_directory() -> Result<PathBuf> {
    project_dirs().map(|paths| paths.config_dir().to_owned())
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("org", "rDownloader", "Capture").context("locate capture directories")
}

/// The one place a service address is checked for being one this agent can speak to.
///
/// Three callers trusted `capture.json` to different depths and none of them checked this, which
/// is exactly why it happened nowhere: `save` wrote whatever it was given, `load` handed it to
/// `bearer_auth`, and the tray passed it to `open::that_detached` -- so whoever could write that
/// file decided what the operating system opened when somebody clicked **Open rDownloader**.
/// Everything that produces a service address now goes through here (RD-109-05).
fn checked_service(service: Url) -> Result<Url> {
    if !matches!(service.scheme(), "http" | "https") {
        bail!(
            "service address must be http or https, not {}: {service}",
            service.scheme()
        );
    }
    Ok(service)
}

/// Writes `capture.json` so a reader sees either the old content or the new one.
///
/// A direct `std::fs::write` truncates the file first, so an interruption -- a killed process, a
/// full disk, a power cut -- left a half-written `capture.json` behind. That file then failed to
/// parse, and the silent fallback in `load` quietly aimed the agent at whatever was listening
/// on the default loopback port (RD-109-05).
fn write_public_config(directory: &std::path::Path, service: &Url) -> Result<()> {
    use std::io::Write;

    let path = directory.join("capture.json");
    let temporary = directory.join("capture.json.new");
    let body = serde_json::to_vec_pretty(&PublicConfig {
        service: service.clone(),
    })?;
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&body)?;
        // Before the rename, so the rename cannot publish a name whose content is still in a
        // buffer somewhere.
        file.sync_all()
    };
    write().with_context(|| format!("write {}", temporary.display()))?;
    std::fs::rename(&temporary, &path).with_context(|| format!("replace {}", path.display()))?;
    // Best effort: without it the rename itself may not have reached the disk yet. A file
    // system that refuses to open a directory is not a reason to fail a pairing that worked.
    if let Ok(handle) = std::fs::File::open(directory) {
        let _ = handle.sync_all();
    }
    Ok(())
}

/// Reads `capture.json`, telling "there is none" apart from "it cannot be read".
///
/// `Ok(None)` means only that no file exists, which is the unpaired case. Everything else --
/// truncated JSON, an address that is not a URL, a scheme this agent cannot use, an unreadable
/// file -- is an error and is reported as one. It used to be `.ok()`, which turned every one of
/// those into the default loopback address: an agent paired against a remote host then sent its
/// bearer token to whatever happened to be listening locally on 8710, without a word.
fn read_public_config(directory: &std::path::Path) -> Result<Option<PublicConfig>> {
    let path = directory.join("capture.json");
    let content = match std::fs::read(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(anyhow::Error::new(error))
                .with_context(|| format!("read {}", path.display()));
        }
    };
    let config: PublicConfig = serde_json::from_slice(&content)
        .with_context(|| format!("{} is not a readable capture configuration", path.display()))?;
    Ok(Some(PublicConfig {
        service: checked_service(config.service)
            .with_context(|| format!("in {}", path.display()))?,
    }))
}

fn load_keyring_token() -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .ok()?
        .get_password()
        .ok()
}

#[cfg(unix)]
fn save_fallback_token(directory: &std::path::Path, token: &str) -> Result<()> {
    use std::{
        fs::OpenOptions,
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };

    let path = directory.join("capture.token");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(token.as_bytes())?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn save_fallback_token(_directory: &std::path::Path, _token: &str) -> Result<()> {
    bail!("operating-system keyring is unavailable")
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{
        decide_paired, discard_fallback_token, ensure_transport_is_safe, load_from,
        read_public_config, store_token, stored_service_in, write_public_config,
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
            ensure_transport_is_safe(&service(address), true)
                .expect("the explicit flag accepts it");
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
}
