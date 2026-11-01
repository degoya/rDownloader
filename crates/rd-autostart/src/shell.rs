//! The process, file and registry helpers the autostart registration and the capture agent's
//! OS integration (`rd-capture`) both need. The registry is written through its API (`winreg`),
//! never through a `reg.exe` child (RD-1400-07).
//!
//! The capture agent already depends on this crate, yet carried its own copies of `reg_add`
//! and `run` (audit 1.9.1, INTAKE-13); two copies of the code that writes `HKCU` drift apart
//! the first time one of them learns something, such as hiding the console window.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Runs `command` to its end; fails, naming `operation`, when it cannot start or exits
/// unsuccessfully.
///
/// # Errors
///
/// When the command cannot be started or exits with a failure status.
pub fn run(command: &mut Command, operation: &str) -> Result<()> {
    let status = command.status().with_context(|| operation.to_owned())?;
    if !status.success() {
        bail!("{operation} failed with {status}");
    }
    Ok(())
}

/// Removes `path`; a file that is already gone counts as removed.
///
/// # Errors
///
/// When the file exists and cannot be removed.
pub fn remove_file_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

/// The path below `HKEY_CURRENT_USER` that `key` names, which is written `HKCU\…` or
/// `HKEY_CURRENT_USER\…`; every key rDownloader writes is the user's own, so any other hive is
/// refused. Not gated on Windows, so it is tested on every host.
#[cfg(any(windows, test))]
fn user_subkey(key: &str) -> Result<&str> {
    ["HKCU\\", "HKEY_CURRENT_USER\\"]
        .iter()
        .find_map(|hive| {
            key.get(..hive.len())
                .filter(|prefix| prefix.eq_ignore_ascii_case(hive))
                .map(|_| &key[hive.len()..])
        })
        .filter(|path| !path.is_empty())
        .with_context(|| format!("not a key below HKEY_CURRENT_USER: {key}"))
}

/// Writes a `REG_SZ` value through the registry API: the value `name`, or the key's default
/// value for `None`, created along with the key when either is missing. No `reg.exe` child: a
/// program that writes the registry through another one is what a malware heuristic looks for
/// (RD-1400-07).
///
/// # Errors
///
/// When the key is not the user's or the registry refuses the write.
#[cfg(windows)]
pub fn reg_add(key: &str, name: Option<&str>, value: &str) -> Result<()> {
    let (opened, _) = winreg::HKCU
        .create_subkey(user_subkey(key)?)
        .with_context(|| format!("open Windows registry key {key}"))?;
    opened
        .set_value(name.unwrap_or(""), &value)
        .with_context(|| format!("write Windows registry value under {key}"))
}

/// Deletes the value `name` under `key`; a value or key that is not there counts as deleted.
///
/// # Errors
///
/// When the key is not the user's, or the value exists and cannot be deleted.
#[cfg(windows)]
pub fn reg_delete_value_if_present(key: &str, name: &str) -> Result<()> {
    let opened = match winreg::HKCU
        .open_subkey_with_flags(user_subkey(key)?, winreg::enums::KEY_SET_VALUE)
    {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("open Windows registry key {key}"));
        }
    };
    match opened.delete_value(name) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("remove Windows registry value {name} under {key}"))
        }
        _ => Ok(()),
    }
}

/// Deletes `key` with everything under it; a key that is not there counts as deleted.
///
/// # Errors
///
/// When the key is not the user's, or it exists and cannot be deleted.
#[cfg(windows)]
pub fn reg_delete_key_if_present(key: &str) -> Result<()> {
    match winreg::HKCU.delete_subkey_all(user_subkey(key)?) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("remove Windows registry key {key}"))
        }
        _ => Ok(()),
    }
}

/// Whether the value `name` exists under `key`; a key that cannot be read counts as absent.
#[cfg(windows)]
#[must_use]
pub fn reg_value_exists(key: &str, name: &str) -> bool {
    user_subkey(key)
        .ok()
        .and_then(|path| winreg::HKCU.open_subkey(path).ok())
        .is_some_and(|opened| opened.get_raw_value(name).is_ok())
}

#[cfg(test)]
mod tests {
    use super::{remove_file_if_present, run, user_subkey};

    #[test]
    fn only_keys_of_the_current_user_are_written() {
        assert_eq!(
            user_subkey(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run").expect("HKCU"),
            r"Software\Microsoft\Windows\CurrentVersion\Run"
        );
        assert_eq!(
            user_subkey(r"hkey_current_user\Software\Classes\.nzb").expect("long form"),
            r"Software\Classes\.nzb"
        );
        for refused in [
            r"HKLM\Software\Classes",
            "HKCU",
            r"HKCU\",
            r"Software\Classes",
            "HKCU/x",
        ] {
            assert!(user_subkey(refused).is_err(), "accepted {refused:?}");
        }
    }

    #[test]
    fn removing_a_missing_file_is_not_an_error() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("gone.cmd");
        remove_file_if_present(&path).expect("a missing file counts as removed");
        std::fs::write(&path, "x").expect("write file");
        remove_file_if_present(&path).expect("remove file");
        assert!(!path.exists());
    }

    #[test]
    fn a_command_that_cannot_start_names_the_operation() {
        let error = run(
            &mut std::process::Command::new("rd-autostart-no-such-program"),
            "start the impossible",
        )
        .expect_err("a missing program fails");
        assert!(
            format!("{error:#}").contains("start the impossible"),
            "{error:#}"
        );
    }
}
