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
