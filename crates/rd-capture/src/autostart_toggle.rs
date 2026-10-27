//! "Start at login" in the tray (RD-1390-03): the agent's own login entry, switched without a
//! shell.
//!
//! The entry is ticked by the real registration (`rd_autostart::is_registered`) and switches it
//! with the `install` and `remove` that `rdownloader-capture autostart install|remove` uses. Beside
//! the service it is hidden, as the agent's own update switch is (`AgentSetup::WithService`):
//! there the service's installer decides what starts at login. A switch that fails leaves the
//! registration, and with it the tick, as it was, and says why in a notification.
//!
//! Apart from `tray.rs`, which compiles on Windows and macOS only: the rules are tested on every
//! host, and the Linux agent's shortcut goes the same way.

use std::path::Path;

use anyhow::{Context, Result};
use rd_autostart::Target;
use rd_update::agent::AgentSetup;

/// The entry's label; English like the rest of the menu (RD-092-05).
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) const START_AT_LOGIN: &str = "Start at login";

/// The entry as the tray shows it: `None` beside the service, where it is hidden, otherwise its
/// tick, which is the registration.
pub(crate) fn autostart_entry(setup: AgentSetup, registered: bool) -> Option<bool> {
    (setup != AgentSetup::WithService).then_some(registered)
}

/// Switches the registration: `remove` while there is one, `install` for `executable` otherwise.
/// Returns whether the agent is registered now; on an error it is as it was.
pub(crate) fn toggle_autostart(
    registered: bool,
    executable: &Path,
    install: impl FnOnce(&Path) -> Result<()>,
    remove: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    set_autostart(!registered, registered, executable, install, remove)
}

/// Brings the registration to `wanted`: `install` for `executable` when it is wanted and missing,
/// `remove` when it is there and not wanted, nothing when it already is as wanted. A click names
/// what it wants, so a registration changed outside the tray is never turned the other way.
/// Returns whether the agent is registered now; on an error it is as it was.
pub(crate) fn set_autostart(
    wanted: bool,
    registered: bool,
    executable: &Path,
    install: impl FnOnce(&Path) -> Result<()>,
    remove: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    match (wanted, registered) {
        (true, false) => install(executable)?,
        (false, true) => remove()?,
        _ => {}
    }
    Ok(wanted)
}

/// The tick after a switch: the new registration, or the old one when the switch failed.
fn tick_after(registered: bool, outcome: &Result<bool>) -> bool {
    outcome.as_ref().map_or(registered, |now| *now)
}

/// The entry for this agent as the system has it now: the tray is built with it, and the health
/// poll reads it again. Beside the service the registration is not even asked for.
pub(crate) fn current() -> Option<bool> {
    let setup = AgentSetup::detect();
    autostart_entry(
        setup,
        setup != AgentSetup::WithService && rd_autostart::is_registered(Target::Capture),
    )
}

/// "Start at login", chosen in the tray (`wanted`: the tick the click left, which is what the
/// person saw and meant) or by its shortcut (`None`: the opposite of the registration the system
/// has now, since a key press shows no tick). Returns the tick that follows; `None` beside the
/// service, where the entry is hidden and its shortcut switches nothing. A failure is a warning
/// in the log and a notification, and the tick stays the old one.
pub(crate) async fn requested(wanted: Option<bool>) -> Option<bool> {
    let (registered, outcome) = match tokio::task::spawn_blocking(move || switch(wanted)).await {
        Ok(Some(switched)) => switched,
        Ok(None) => {
            tracing::info!("installed with the service, whose installer decides; not switching");
            return None;
        }
        Err(error) => {
            tracing::warn!(%error, "switching start at login stopped");
            return None;
        }
    };
    let tick = tick_after(registered, &outcome);
    match outcome {
        Ok(registered) => tracing::info!(registered, "start at login switched"),
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "start at login could not be switched");
            crate::notify::toast(format!("Start at login could not be switched: {error:#}")).await;
        }
    }
    Some(tick)
}

/// The blocking half of [`requested`]: `reg.exe`, `systemctl` and the files they write. Returns
/// the registration before the switch and the switch's outcome.
fn switch(wanted: Option<bool>) -> Option<(bool, Result<bool>)> {
    let registered = current()?;
    let install = |path: &Path| rd_autostart::install(Target::Capture, path);
    let remove = || rd_autostart::remove(Target::Capture);
    let outcome = std::env::current_exe()
        .context("locate capture executable")
        .and_then(|executable| match wanted {
            // A click: what its tick says.
            Some(wanted) => set_autostart(wanted, registered, &executable, install, remove),
            // The shortcut: the other way from the registration the system has.
            None => toggle_autostart(registered, &executable, install, remove),
        });
    Some((registered, outcome))
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, path::Path};

    use anyhow::bail;
    use rd_update::{InstallKind, agent::AgentSetup};

    use super::{autostart_entry, set_autostart, tick_after, toggle_autostart};

    const EXECUTABLE: &str = "/opt/rdownloader/rdownloader-capture";

    #[test]
    fn beside_the_service_the_entry_is_hidden() {
        for registered in [false, true] {
            assert_eq!(autostart_entry(AgentSetup::WithService, registered), None);
        }
    }

    #[test]
    fn alone_the_entry_shows_the_registration() {
        for kind in [
            InstallKind::Portable,
            InstallKind::Msi,
            InstallKind::Homebrew,
        ] {
            let setup = AgentSetup::Alone(kind);
            assert_eq!(autostart_entry(setup, true), Some(true), "{kind:?}");
            assert_eq!(autostart_entry(setup, false), Some(false), "{kind:?}");
        }
    }

    #[test]
    fn toggling_installs_then_removes() {
        let calls = RefCell::new(Vec::new());
        let install = |path: &Path| {
            calls
                .borrow_mut()
                .push(format!("install {}", path.display()));
            Ok(())
        };
        let remove = || {
            calls.borrow_mut().push("remove".to_owned());
            Ok(())
        };
        let executable = Path::new(EXECUTABLE);
        let now = toggle_autostart(false, executable, install, remove).expect("installed");
        assert!(now);
        let now = toggle_autostart(now, executable, install, remove).expect("removed");
        assert!(!now);
        assert_eq!(
            calls.into_inner(),
            [format!("install {EXECUTABLE}"), "remove".to_owned()]
        );
    }

    /// A click names what it wants: after `rdownloader-capture autostart remove` the stale tick,
    /// clicked off, removes nothing and installs nothing.
    #[test]
    fn setting_what_already_is_does_nothing() {
        let executable = Path::new(EXECUTABLE);
        let now = set_autostart(
            false,
            false,
            executable,
            |_| panic!("an unwanted registration is not installed"),
            || panic!("nothing to remove"),
        )
        .expect("nothing to do");
        assert!(!now);
        let now = set_autostart(
            true,
            true,
            executable,
            |_| panic!("already registered"),
            || panic!("a wanted registration is not removed"),
        )
        .expect("nothing to do");
        assert!(now);
    }

    #[test]
    fn a_failed_toggle_keeps_the_old_tick() {
        let executable = Path::new(EXECUTABLE);
        let refused_remove = toggle_autostart(
            true,
            executable,
            |_| panic!("a registered agent is not installed again"),
            || bail!("access denied"),
        );
        assert!(refused_remove.is_err());
        assert!(tick_after(true, &refused_remove));
        let refused_install = toggle_autostart(
            false,
            executable,
            |_| bail!("systemd user services are unavailable"),
            || panic!("an unregistered agent has nothing to remove"),
        );
        assert!(refused_install.is_err());
        assert!(!tick_after(false, &refused_install));
        assert!(tick_after(false, &Ok(true)));
        assert!(!tick_after(true, &Ok(false)));
    }
}
