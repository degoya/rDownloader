use std::{path::Path, process::Command};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use rd_files::NoConsoleWindow as _;

use super::shell::{reg_add, reg_delete_value_if_present, remove_file_if_present};
use super::{Registration, Target, windows_wrapper_path};

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

pub(super) fn install(registration: &Registration) -> Result<()> {
    let directory = wrapper_directory()?;
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("create {}", directory.display()))?;
    let cmd_path = directory.join(format!("{}.cmd", registration.target.slug()));
    let vbs_path = directory.join(format!("{}.vbs", registration.target.slug()));
    std::fs::write(&cmd_path, render_cmd(registration))
        .with_context(|| format!("write {}", cmd_path.display()))?;
    std::fs::write(&vbs_path, render_vbs(&cmd_path)?)
        .with_context(|| format!("write {}", vbs_path.display()))?;
    let value = render_registry_value(&vbs_path)?;
    reg_add(RUN_KEY, Some(registration.target.display_name()), &value)
}

pub(super) fn remove(target: Target) -> Result<()> {
    reg_delete_value_if_present(RUN_KEY, target.display_name())?;
    let directory = wrapper_directory()?;
    remove_file_if_present(&directory.join(format!("{}.cmd", target.slug())))?;
    remove_file_if_present(&directory.join(format!("{}.vbs", target.slug())))
}

fn wrapper_directory() -> Result<std::path::PathBuf> {
    ProjectDirs::from("org", "rDownloader", "rDownloader")
        .map(|paths| paths.config_dir().join("autostart"))
        .context("locate rDownloader configuration directory")
}

fn render_cmd(registration: &Registration) -> String {
    format!(
        "@echo off\r\ncd /d \"{}\"\r\n\"{}\" {} >>\"{}\" 2>>\"{}\"\r\n",
        batch_escape(&registration.working_directory),
        batch_escape(&registration.executable),
        registration.target.argument(),
        batch_escape(&registration.stdout_path()),
        batch_escape(&registration.stderr_path()),
    )
}

fn render_vbs(cmd_path: &Path) -> Result<String> {
    let path = windows_wrapper_path(cmd_path)?;
    Ok(format!(
        "Set shell = CreateObject(\"WScript.Shell\")\r\nshell.Run Chr(34) & \"{}\" & Chr(34), 0, False\r\n",
        path.replace('"', "\"\"")
    ))
}

/// The `Run` key value, which Windows hands to `wscript.exe` as a command line.
///
/// Quoted like the VBScript wrapper rather than interpolated bare: a command line is split
/// by `CommandLineToArgvW`, so an unescaped quote would end the argument and everything
/// after it would become arguments of its own. A quote cannot occur in a Windows path, but
/// the value is only safe for as long as that stays true of whoever builds the path.
fn render_registry_value(vbs_path: &Path) -> Result<String> {
    let path = windows_wrapper_path(vbs_path)?.replace('"', "\\\"");
    Ok(format!("wscript.exe //B //NoLogo \"{path}\""))
}

fn batch_escape(path: &Path) -> String {
    path.to_string_lossy().replace('%', "%%")
}

pub(super) fn is_registered(target: Target) -> bool {
    Command::new("reg.exe")
        .no_console_window()
        .args(["query", RUN_KEY, "/v", target.display_name()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Registration, Target, render_cmd, render_registry_value, render_vbs};

    #[test]
    fn wrappers_quote_portable_paths() {
        let registration = Registration {
            target: Target::Capture,
            executable: PathBuf::from(r"C:\Portable 100%\O'Reilly\rdownloader-capture.exe"),
            working_directory: PathBuf::from(r"C:\Portable 100%\O'Reilly"),
            log_directory: PathBuf::from(r"C:\Portable 100%\O'Reilly\logs"),
        };
        let cmd = render_cmd(&registration);
        assert!(cmd.contains(r#"cd /d "C:\Portable 100%%\O'Reilly""#));
        assert!(cmd.contains("rdownloader-capture.exe\" run"));
        let vbs = render_vbs(Path::new(r"C:\Config Path\capture.vbs.cmd")).expect("valid VBS");
        assert!(vbs.contains("shell.Run Chr(34)"));
        let registry = render_registry_value(Path::new(
            r"C:\Users\O'Reilly\AppData\100%\rdownloader-capture.vbs",
        ))
        .expect("valid registry value");
        assert_eq!(
            registry,
            r#"wscript.exe //B //NoLogo "C:\Users\O'Reilly\AppData\100%\rdownloader-capture.vbs""#
        );
    }
}
