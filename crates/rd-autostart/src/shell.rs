//! The process, file and registry helpers the autostart registration and the capture agent's
//! OS integration (`rd-capture`) both need.
//!
//! The capture agent already depends on this crate, yet carried its own copies of `reg_add`
//! and `run` (audit 1.9.1, INTAKE-13); two copies of the code that writes `HKCU` drift apart
//! the first time one of them learns something, such as hiding the console window.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
#[cfg(windows)]
use rd_files::NoConsoleWindow as _;

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

/// Writes a `REG_SZ` value with `reg.exe`: the value `name`, or the key's default value for
/// `None`, created along with the key when either is missing.
///
/// # Errors
///
/// When `reg.exe` cannot be started or refuses the write.
#[cfg(windows)]
pub fn reg_add(key: &str, name: Option<&str>, value: &str) -> Result<()> {
    let mut command = Command::new("reg.exe");
    command.no_console_window().args(["add", key]);
    if let Some(name) = name {
        command.args(["/v", name]);
    } else {
        command.arg("/ve");
    }
    command.args(["/t", "REG_SZ", "/d", value, "/f"]);
    run(&mut command, "write Windows registry")
}

/// Deletes the value `name` under `key`; a value that is not there counts as deleted.
///
/// # Errors
///
/// When `reg.exe` cannot be started or the value exists and cannot be deleted.
#[cfg(windows)]
pub fn reg_delete_value_if_present(key: &str, name: &str) -> Result<()> {
    let present = reg_query(&[key, "/v", name]).context("query Windows registry")?;
    if !present {
        return Ok(());
    }
    run(
        Command::new("reg.exe")
            .no_console_window()
            .args(["delete", key, "/v", name, "/f"]),
        "remove Windows registry value",
    )
}

/// Deletes `key` with everything under it; a key that is not there counts as deleted.
///
/// # Errors
///
/// When `reg.exe` cannot be started or the key exists and cannot be deleted.
#[cfg(windows)]
pub fn reg_delete_key_if_present(key: &str) -> Result<()> {
    let present = reg_query(&[key]).with_context(|| format!("query Windows registry key {key}"))?;
    if !present {
        return Ok(());
    }
    run(
        Command::new("reg.exe")
            .no_console_window()
            .args(["delete", key, "/f"]),
        "remove Windows registry key",
    )
}

/// Whether `reg.exe query` finds what `arguments` ask for, without showing its output.
#[cfg(windows)]
fn reg_query(arguments: &[&str]) -> std::io::Result<bool> {
    Command::new("reg.exe")
        .no_console_window()
        .arg("query")
        .args(arguments)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::{remove_file_if_present, run};

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
