use std::{path::Path, process::Command};

use anyhow::{Context, Result, bail};
use directories::BaseDirs;

use super::shell::remove_file_if_present;
use super::{Registration, Target};

pub(super) fn install(registration: &Registration) -> Result<()> {
    ensure_systemctl()?;
    let directory = unit_directory()?;
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("create {}", directory.display()))?;
    let path = directory.join(unit_name(registration.target));
    std::fs::write(&path, render_unit(registration)?)
        .with_context(|| format!("write {}", path.display()))?;
    systemctl(&["daemon-reload"], "reload systemd user units")?;
    systemctl(
        &["enable", unit_name(registration.target)],
        "enable systemd user unit",
    )
}

pub(super) fn remove(target: Target) -> Result<()> {
    let directory = unit_directory()?;
    let path = directory.join(unit_name(target));
    let _ = Command::new("systemctl")
        .args(["--user", "disable", unit_name(target)])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    remove_file_if_present(
        &directory
            .join(format!("{}.wants", target.wanted_by()))
            .join(unit_name(target)),
    )?;
    remove_file_if_present(&path)?;
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(())
}

fn unit_directory() -> Result<std::path::PathBuf> {
    BaseDirs::new()
        .map(|paths| paths.config_dir().join("systemd/user"))
        .context("locate systemd user-unit directory")
}

fn unit_name(target: Target) -> &'static str {
    match target {
        Target::Server => "rdownloader.service",
        Target::Capture => "rdownloader-capture.service",
    }
}

fn render_unit(registration: &Registration) -> Result<String> {
    let after = match registration.target {
        Target::Server => "network-online.target",
        Target::Capture => "graphical-session.target network-online.target",
    };
    Ok(format!(
        "[Unit]\nDescription={}\nAfter={after}\nWants=network-online.target\n\n[Service]\nType=simple\nWorkingDirectory={}\nExecStart={} {}\nRestart=no\nStandardOutput={}\nStandardError={}\n\n[Install]\nWantedBy={}\n",
        registration.target.display_name(),
        systemd_quote(&registration.working_directory)?,
        systemd_quote(&registration.executable)?,
        registration.target.argument(),
        systemd_value(&format!("append:{}", registration.stdout_path().display()))?,
        systemd_value(&format!("append:{}", registration.stderr_path().display()))?,
        registration.target.wanted_by(),
    ))
}

fn systemd_quote(path: &Path) -> Result<String> {
    systemd_value(path.to_str().context("autostart path is not Unicode")?)
}

fn systemd_value(value: &str) -> Result<String> {
    if value.contains(['\n', '\r', '\0']) {
        bail!("autostart path cannot be represented in a systemd unit");
    }
    Ok(format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    ))
}

fn ensure_systemctl() -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("systemd user services are unavailable; use the portable start script")?;
    if !status.success() {
        bail!("systemd user services are unavailable; use the portable start script");
    }
    Ok(())
}

fn systemctl(arguments: &[&str], operation: &str) -> Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .status()
        .with_context(|| operation.to_owned())?;
    if !status.success() {
        bail!(
            "{operation} failed with {status}; systemd user services may be unavailable, use the portable start script"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Registration, Target, render_unit};

    #[test]
    fn unit_quotes_spaces_apostrophes_and_percent() {
        let root = PathBuf::from("/tmp/Portable 100%/O'Reilly");
        let registration = Registration {
            target: Target::Capture,
            executable: root.join("rdownloader-capture"),
            working_directory: root.clone(),
            log_directory: root.join("logs"),
        };
        let unit = render_unit(&registration).expect("valid unit");
        assert!(unit.contains("Portable 100%%/O'Reilly"));
        assert!(unit.contains("WantedBy=graphical-session.target"));
        assert!(
            unit.contains("ExecStart=\"/tmp/Portable 100%%/O'Reilly/rdownloader-capture\" run")
        );
    }
}
