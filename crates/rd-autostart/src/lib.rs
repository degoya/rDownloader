//! Per-user autostart registration for the rDownloader binaries, and where an installed build
//! keeps its data.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

mod installed;
pub mod shell;
mod stable_path;

pub use installed::{INSTALL_KIND_FILE, install_kind, installed_home};
pub use stable_path::stable_executable_path;

/// One independently installable rDownloader background process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    Server,
    Capture,
}

impl Target {
    fn slug(self) -> &'static str {
        match self {
            Self::Server => "rdownloader",
            Self::Capture => "rdownloader-capture",
        }
    }

    /// The name shown in the Windows Run key and the systemd unit's `Description`; a launchd
    /// agent has no such field, so macOS does without it.
    #[cfg(any(windows, target_os = "linux"))]
    fn display_name(self) -> &'static str {
        match self {
            Self::Server => "rDownloader Service",
            Self::Capture => "rDownloader Capture",
        }
    }

    fn argument(self) -> &'static str {
        match self {
            Self::Server => "serve",
            Self::Capture => "run",
        }
    }

    #[cfg(target_os = "macos")]
    fn launchd_label(self) -> &'static str {
        match self {
            Self::Server => "org.rdownloader.service",
            Self::Capture => "org.rdownloader.capture",
        }
    }

    #[cfg(target_os = "linux")]
    fn wanted_by(self) -> &'static str {
        match self {
            Self::Server => "default.target",
            Self::Capture => "graphical-session.target",
        }
    }
}

struct Registration {
    target: Target,
    executable: PathBuf,
    working_directory: PathBuf,
    log_directory: PathBuf,
}

impl Registration {
    fn new(target: Target, executable: &Path) -> Result<Self> {
        if !executable.is_absolute() {
            bail!("autostart executable must be an absolute path");
        }
        // An installed build starts in the user's data folder, as its service would move there
        // anyway; its logs must not land in a program folder it may not write (RD-180-05).
        let working_directory = match installed_home(executable)? {
            Some(home) => home,
            None => executable
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .context("autostart executable has no parent directory")?
                .to_owned(),
        };
        let log_directory = working_directory.join("logs");
        std::fs::create_dir_all(&log_directory)
            .with_context(|| format!("create {}", log_directory.display()))?;
        Ok(Self {
            target,
            executable: executable.to_owned(),
            working_directory,
            log_directory,
        })
    }

    fn stdout_path(&self) -> PathBuf {
        self.log_directory
            .join(format!("{}.log", self.target.slug()))
    }

    fn stderr_path(&self) -> PathBuf {
        self.log_directory
            .join(format!("{}.err.log", self.target.slug()))
    }
}

/// Registers `executable` to start for the current user at the next login.
///
/// Registered under [`stable_executable_path`], so a package manager's update does not leave the
/// login entry pointing at the previous version's folder.
pub fn install(target: Target, executable: &Path) -> Result<()> {
    let registration = Registration::new(target, &stable_executable_path(executable))?;
    platform::install(&registration)
}

/// Removes the current user's registration without stopping a running process.
pub fn remove(target: Target) -> Result<()> {
    platform::remove(target)
}

/// Whether `target` has its `Run` entry for the next login. The updater asks before it takes a
/// failed MSI update back (RD-180-02): removing the newer package removes the entry too, and the
/// reinstalled previous one only gets it back when it is registered again.
#[cfg(windows)]
#[must_use]
pub fn is_registered(target: Target) -> bool {
    platform::is_registered(target)
}

/// The text of a Windows autostart wrapper path, refused when it carries a control character.
///
/// A line break in the path would close the `shell.Run` line of the generated VBScript and turn
/// everything after it into a statement of its own, and the registry value is a command line
/// that `wscript.exe` is handed verbatim. The path is derived from the install location today,
/// so nothing reaches this from outside — but the systemd renderer rejects the same characters
/// and the property-list renderer rejects `\0`, and a renderer that trusts its input is the one
/// that stops being safe when somebody changes where the wrapper lives.
///
/// Ungated so it compiles and is tested on every platform; only the renderers that use it are
/// Windows-only.
#[cfg(any(windows, test))]
fn windows_wrapper_path(path: &Path) -> Result<&str> {
    let value = path
        .to_str()
        .context("autostart wrapper path is not Unicode")?;
    if value.contains(['\n', '\r', '\0']) {
        bail!("autostart path cannot be represented in a Windows autostart wrapper");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::windows_wrapper_path;

    #[test]
    fn a_wrapper_path_with_a_control_character_is_refused() {
        // A line break would end the `shell.Run` statement and make the rest of the path a
        // VBScript statement of its own.
        for hostile in [
            "C:\\Tools\\rdownloader\nshell.Run Chr(34) & \"calc.exe\" & Chr(34), 0, False",
            "C:\\Tools\\rdownloader\r\nevil.cmd",
            "C:\\Tools\\rdownloader\0.cmd",
        ] {
            assert!(
                windows_wrapper_path(Path::new(hostile)).is_err(),
                "accepted {hostile:?}"
            );
        }
        assert_eq!(
            windows_wrapper_path(Path::new(r"C:\Portable 100%\O'Reilly\capture.vbs"))
                .expect("an ordinary path"),
            r"C:\Portable 100%\O'Reilly\capture.vbs"
        );
    }
}

#[cfg(windows)]
mod platform {
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
}

#[cfg(target_os = "linux")]
mod platform {
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
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::Path;

    use anyhow::{Context, Result, bail};
    use directories::BaseDirs;

    use super::{Registration, Target};

    pub(super) fn install(registration: &Registration) -> Result<()> {
        let directory = launch_agents_directory()?;
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("create {}", directory.display()))?;
        let path = directory.join(plist_name(registration.target));
        std::fs::write(&path, render_plist(registration)?)
            .with_context(|| format!("write {}", path.display()))
    }

    pub(super) fn remove(target: Target) -> Result<()> {
        let path = launch_agents_directory()?.join(plist_name(target));
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
        Ok(())
    }

    fn launch_agents_directory() -> Result<std::path::PathBuf> {
        BaseDirs::new()
            .map(|paths| paths.home_dir().join("Library/LaunchAgents"))
            .context("locate macOS LaunchAgents directory")
    }

    fn plist_name(target: Target) -> String {
        format!("{}.plist", target.launchd_label())
    }

    fn render_plist(registration: &Registration) -> Result<String> {
        Ok(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>{}</string>\n  </array>\n  <key>WorkingDirectory</key>\n  <string>{}</string>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>StandardOutPath</key>\n  <string>{}</string>\n  <key>StandardErrorPath</key>\n  <string>{}</string>\n</dict>\n</plist>\n",
            registration.target.launchd_label(),
            plist_path(&registration.executable)?,
            registration.target.argument(),
            plist_path(&registration.working_directory)?,
            plist_path(&registration.stdout_path())?,
            plist_path(&registration.stderr_path())?,
        ))
    }

    fn plist_path(path: &Path) -> Result<String> {
        let value = path.to_str().context("autostart path is not Unicode")?;
        if value.contains(['\0']) {
            bail!("autostart path cannot be represented in a property list");
        }
        Ok(xml_escape(value))
    }

    fn xml_escape(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }

    #[cfg(test)]
    mod tests {
        use std::path::PathBuf;

        use super::{Registration, Target, render_plist};

        #[test]
        fn plist_escapes_portable_paths() {
            let root = PathBuf::from("/tmp/Portable 100% & O'Reilly");
            let registration = Registration {
                target: Target::Server,
                executable: root.join("rdownloader"),
                working_directory: root.clone(),
                log_directory: root.join("logs"),
            };
            let plist = render_plist(&registration).expect("valid plist");
            assert!(plist.contains("Portable 100% &amp; O&apos;Reilly"));
            assert!(plist.contains("org.rdownloader.service"));
        }
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    use anyhow::{Result, bail};

    use super::{Registration, Target};

    pub(super) fn install(_registration: &Registration) -> Result<()> {
        bail!("autostart is not supported on this operating system")
    }

    pub(super) fn remove(_target: Target) -> Result<()> {
        bail!("autostart is not supported on this operating system")
    }
}
