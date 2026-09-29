//! The environment a program started for post-processing inherits from the service.
//!
//! Security review 2026-09-28, finding 7: user scripts, the archive tools, ffmpeg and rclone
//! inherited every variable the service was started with. The service itself reads no secret
//! from its environment, but whatever an operator put there - a token for another program, a
//! cloud credential, a variable a container runtime injected - reached every script a category
//! runs. Each of those programs now starts from an empty environment plus the variables a
//! program needs to run at all ([`KEPT`], [`KEPT_PREFIXES`]) and whatever its caller sets on
//! top: a script's `RD_*`/`SAB_*` values, the archive tools' locale. A variable of the service's
//! own environment named `RD_*` is not passed on either; only the values the service sets are.

use std::ffi::OsString;

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

/// Clears `command`'s environment and passes through only [`KEPT`], [`KEPT_PREFIXES`] and
/// `extra` from the service's own.
///
/// `extra` follows the same rule as the two lists: an entry ending in `_` is a prefix, any
/// other names one variable. Values the caller sets afterwards with `env`/`envs` are added on
/// top, as with any `Command`.
pub fn restrict_environment(command: &mut tokio::process::Command, extra: &[&str]) {
    command
        .env_clear()
        .envs(kept_variables(std::env::vars_os(), extra));
}

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

    use super::{RCLONE_VARIABLES, kept_variables};

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
}
