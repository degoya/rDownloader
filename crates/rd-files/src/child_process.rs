//! What a program started by the service inherits from it: its environment and the room its
//! output takes.
//!
//! Security review 2026-09-28, finding 7: user scripts, the archive tools, ffmpeg and rclone
//! inherited every variable the service was started with. The service itself reads no secret
//! from its environment, but whatever an operator put there - a token for another program, a
//! cloud credential, a variable a container runtime injected - reached every script a category
//! runs. Each of those programs now starts from an empty environment plus the variables a
//! program needs to run at all ([`KEPT`], [`KEPT_PREFIXES`]) and whatever its caller sets on
//! top: a script's `RD_*`/`SAB_*` values, the archive tools' locale. A variable of the service's
//! own environment named `RD_*` is not passed on either; only the values the service sets are.
//! The 1.8 engine audit found the downloaders and apprise still inheriting everything; they
//! follow the same rule with [`TOOL_VARIABLES`] on top.
//!
//! Output: a program can print without end, and a buffer that keeps all of it grows the service
//! with it. [`read_tail`] keeps a bounded end of a stream and reads the rest away, so the child
//! never blocks on a full pipe either.
//!
//! Window: on Windows a console program started by a process without a console opens a console
//! window of its own. [`NoConsoleWindow`] is the one way every spawn in the workspace avoids it.

use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
};

use tokio::io::{AsyncRead, AsyncReadExt};

/// Variables passed through by name.
///
/// The search path and the home directory everywhere; the temporary directory and time zone;
/// and on Windows the handful a process cannot start or find its own files without -
/// `SystemRoot` (Python's random source and Winsock need it), `ComSpec` and `PATHEXT` (a batch
/// file and the program lookup), the profile and application-data folders. None of them is a
/// credential.
pub const KEPT: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "TZ",
    "LANG",
    "USERPROFILE",
    "SYSTEMROOT",
    "WINDIR",
    "SYSTEMDRIVE",
    "COMSPEC",
    "PATHEXT",
    "TEMP",
    "TMP",
    "APPDATA",
    "LOCALAPPDATA",
];

/// Variables passed through by prefix: the locale categories (`LC_ALL`, `LC_CTYPE`, ...).
pub const KEPT_PREFIXES: &[&str] = &["LC_"];

/// What rclone reads besides [`KEPT`]: its own `RCLONE_*` settings - the configuration file,
/// the configuration password, any flag - the configuration directory it derives from
/// `XDG_CONFIG_HOME`, and the proxy it is told to use. An upload that works from a shell has to
/// keep working from the service, and every one of these is meant for rclone.
pub const RCLONE_VARIABLES: &[&str] = &[
    "RCLONE_",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
];

/// What the downloaders (yt-dlp, gallery-dl, streamlink) and apprise read besides [`KEPT`]:
/// the proxy they are told to use, in either spelling (the rule ignores ASCII case), the CA
/// bundle a TLS-intercepting network hands out, and the configuration and cache directories
/// their own config files live in. A download that works from a shell keeps working from the
/// service; none of these is a credential of the service's.
pub const TOOL_VARIABLES: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
];

/// Clears `command`'s environment and passes through only [`KEPT`], [`KEPT_PREFIXES`] and
/// `extra` from the service's own.
///
/// `extra` follows the same rule as the two lists: an entry ending in `_` is a prefix, any
/// other names one variable. Values the caller set with `env`/`envs` before - or sets
/// afterwards - stay on top, so this can run where a command is spawned, after whoever built it.
pub fn restrict_environment(command: &mut tokio::process::Command, extra: &[&str]) {
    let explicit = command
        .as_std()
        .get_envs()
        .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
        .collect::<Vec<_>>();
    command
        .env_clear()
        .envs(kept_variables(std::env::vars_os(), extra));
    for (name, value) in explicit {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
}

/// `CREATE_NO_WINDOW`: a console program started with it gets a console of its own that is
/// never shown, and the console programs it starts in turn share that hidden console.
///
/// Public for the one caller that combines it with other creation flags (the updater, which
/// starts the service); every other spawn calls [`NoConsoleWindow::no_console_window`], because
/// `creation_flags` replaces the flags set before rather than adding to them.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Starts a program without a console window on Windows; nothing changes elsewhere.
///
/// 1.8.0 live finding: after a self-update the updater had started the service as a detached
/// process, which has no console at all, so every unrar, 7z, ffmpeg or `icacls` it ran opened a
/// visible console window of its own, one after another. A spawn site that does not ask for
/// this depends on how the service itself was started, which it cannot know; so every spawn in
/// the workspace asks, and `crates/rdownloader/tests/no_console_window.rs` fails on one that
/// does not.
pub trait NoConsoleWindow {
    /// Sets [`CREATE_NO_WINDOW`] as the creation flags on Windows; a no-op elsewhere.
    fn no_console_window(&mut self) -> &mut Self;
}

impl NoConsoleWindow for tokio::process::Command {
    fn no_console_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        self.creation_flags(CREATE_NO_WINDOW);
        self
    }
}

impl NoConsoleWindow for std::process::Command {
    fn no_console_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(self, CREATE_NO_WINDOW);
        self
    }
}

/// Reads `reader` to its end and keeps the last `limit` bytes of it.
///
/// Everything before is read and dropped rather than left in the pipe, so a chatty program
/// never blocks on a full buffer, and memory stays at `limit` however much it prints. A read
/// error ends the stream: the pipe is gone, and what was kept is still the best account.
pub async fn read_tail<R: AsyncRead + Unpin>(mut reader: R, limit: usize) -> Vec<u8> {
    let mut kept = VecDeque::with_capacity(limit.min(READ_CHUNK));
    let mut chunk = vec![0_u8; READ_CHUNK];
    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        kept.extend(&chunk[..read]);
        let excess = kept.len().saturating_sub(limit);
        kept.drain(..excess);
    }
    kept.into()
}

const READ_CHUNK: usize = 8 * 1024;

/// The variables of `environment` a started program keeps.
///
/// Split out from the spawn so the rule can be tested with any environment, without touching
/// this process's own.
pub fn kept_variables(
    environment: impl IntoIterator<Item = (OsString, OsString)>,
    extra: &[&str],
) -> Vec<(OsString, OsString)> {
    environment
        .into_iter()
        .filter(|(name, _)| {
            name.to_str().is_some_and(|name| {
                KEPT.iter()
                    .chain(KEPT_PREFIXES)
                    .chain(extra)
                    .any(|rule| matches_rule(name, rule))
            })
        })
        .collect()
}

/// Whether `name` is the variable `rule` names, or starts with it when `rule` ends in `_`.
///
/// ASCII case is ignored: Windows' names are case-insensitive (`Path` is `PATH`), and elsewhere
/// a lower-case twin of a kept name (`http_proxy`, which curl-style tools prefer) carries
/// nothing the kept one would not.
fn matches_rule(name: &str, rule: &str) -> bool {
    if rule.ends_with('_') {
        name.len() > rule.len()
            && name
                .get(..rule.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(rule))
    } else {
        name.eq_ignore_ascii_case(rule)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{RCLONE_VARIABLES, TOOL_VARIABLES, kept_variables, read_tail};

    fn environment(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    fn names(kept: &[(OsString, OsString)]) -> Vec<String> {
        kept.iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect()
    }

    /// Security review 2026-09-28, finding 7: a secret in the service's environment stays
    /// there, and so does an `RD_*` variable the service did not set itself.
    #[test]
    fn only_the_allowlist_survives_the_service_environment() {
        let service = environment(&[
            ("PATH", "/usr/bin"),
            ("Path", "C:\\Windows"),
            ("HOME", "/home/rd"),
            ("LANG", "de_DE.UTF-8"),
            ("LC_TIME", "de_DE.UTF-8"),
            ("TZ", "Europe/Berlin"),
            ("SystemRoot", "C:\\Windows"),
            ("RD_TEST_SECRET_CANARY", "canary-4b1f"),
            ("RD_FINAL_DIR", "/inherited"),
            ("RDOWNLOADER_TOKEN", "rd_token"),
            ("AWS_SECRET_ACCESS_KEY", "aws"),
            ("RCLONE_CONFIG_PASS", "rclone"),
            ("LC_", "a prefix alone names nothing"),
            ("PATHOLOGICAL", "a longer name is another name"),
        ]);
        assert_eq!(
            names(&kept_variables(service.clone(), &[])),
            [
                "PATH",
                "Path",
                "HOME",
                "LANG",
                "LC_TIME",
                "TZ",
                "SystemRoot"
            ]
        );
        let rclone = names(&kept_variables(service, RCLONE_VARIABLES));
        assert!(
            rclone.contains(&"RCLONE_CONFIG_PASS".to_owned()),
            "{rclone:?}"
        );
        for secret in [
            "RD_TEST_SECRET_CANARY",
            "RDOWNLOADER_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
        ] {
            assert!(
                !rclone.contains(&secret.to_owned()),
                "{secret} reached rclone"
            );
        }
    }

    /// The downloaders keep their proxy, CA bundle and config directories, in either spelling,
    /// and nothing that is the service's own.
    #[test]
    fn the_downloaders_keep_their_network_settings_and_nothing_else() {
        let service = environment(&[
            ("PATH", "/usr/bin"),
            ("https_proxy", "http://proxy:3128"),
            ("HTTPS_PROXY", "http://proxy:3128"),
            ("no_proxy", "localhost"),
            ("SSL_CERT_FILE", "/etc/ssl/corp.pem"),
            ("REQUESTS_CA_BUNDLE", "/etc/ssl/corp.pem"),
            ("XDG_CONFIG_HOME", "/home/rd/.config"),
            ("RD_TEST_SECRET_CANARY", "canary-4b1f"),
            ("AWS_SECRET_ACCESS_KEY", "aws"),
            ("RCLONE_CONFIG_PASS", "rclone"),
        ]);
        assert_eq!(
            names(&kept_variables(service, TOOL_VARIABLES)),
            [
                "PATH",
                "https_proxy",
                "HTTPS_PROXY",
                "no_proxy",
                "SSL_CERT_FILE",
                "REQUESTS_CA_BUNDLE",
                "XDG_CONFIG_HOME"
            ]
        );
    }

    /// A value the caller set before the restriction survives it; a secret of this process
    /// does not reach the child. The child's whole environment is checked against the rule,
    /// because this process's own environment cannot be changed from a test.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_child_sees_the_allowlist_and_what_its_caller_set() {
        let mut command = tokio::process::Command::new("/usr/bin/env");
        command.env("RD_SET_BY_CALLER", "kept");
        super::restrict_environment(&mut command, TOOL_VARIABLES);
        let output = command.output().await.expect("run env");
        let text = String::from_utf8_lossy(&output.stdout);
        let seen = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect::<Vec<_>>();
        assert!(
            text.lines().any(|line| line == "RD_SET_BY_CALLER=kept"),
            "{text}"
        );
        let allowed = kept_variables(seen.clone(), TOOL_VARIABLES);
        let extra = seen
            .iter()
            .filter(|pair| !allowed.contains(pair) && pair.0 != "RD_SET_BY_CALLER")
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(extra.is_empty(), "inherited past the allowlist: {extra:?}");
    }

    /// A program that prints far more than the limit: the end is kept, memory is not.
    #[tokio::test]
    async fn only_the_tail_of_a_long_stream_is_kept() {
        let mut stream = vec![b'x'; 1024 * 1024];
        stream.extend_from_slice(b"the error at the end");
        let tail = read_tail(stream.as_slice(), 20).await;
        assert_eq!(tail, b"the error at the end");
        assert_eq!(read_tail(&b"short"[..], 20).await, b"short");
        assert!(read_tail(&b"anything"[..], 0).await.is_empty());
    }
}
