//! The Windows half: one `REG_SZ` value under `HKCU\…\Run`, written through the registry API,
//! that starts the capture agent directly (RD-1400-07; the value itself: `run_value`).

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;

use super::shell::{
    reg_add, reg_delete_value_if_present, reg_value_exists, remove_file_if_present,
};
use super::{Registration, Target, run_value};

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

pub(super) fn install(registration: &Registration) -> Result<()> {
    if registration.target == Target::Server {
        let capture = run_value::capture_beside(&registration.executable);
        if !capture.is_file() {
            bail!(
                "the service's autostart starts it through {} beside it, which is missing; \
                 every Windows package ships both",
                capture.display()
            );
        }
    }
    let value = run_value::render(registration.target, &registration.executable)?;
    reg_add(RUN_KEY, Some(registration.target.display_name()), &value)?;
    remove_old_wrappers(registration.target)
}

pub(super) fn remove(target: Target) -> Result<()> {
    reg_delete_value_if_present(RUN_KEY, target.display_name())?;
    remove_old_wrappers(target)
}

pub(super) fn is_registered(target: Target) -> bool {
    reg_value_exists(RUN_KEY, target.display_name())
}

/// Deletes the `<slug>.cmd` and `<slug>.vbs` the entry started through before RD-1400-07, and
/// their folder once it is empty. Nothing else is migrated; there is no installed base.
fn remove_old_wrappers(target: Target) -> Result<()> {
    let directory = ProjectDirs::from("org", "rDownloader", "rDownloader")
        .map(|paths| paths.config_dir().join("autostart"))
        .context("locate rDownloader configuration directory")?;
    for extension in ["cmd", "vbs"] {
        remove_file_if_present(&directory.join(format!("{}.{extension}", target.slug())))?;
    }
    // Fails while the other program's wrappers are still there, or when there is no folder.
    let _ = std::fs::remove_dir(&directory);
    Ok(())
}
