//! System-wide shortcuts for the tray commands (RD-1180-03), registered with `global-hotkey`.
//!
//! The shortcuts come from the service with the rest of the agent's settings, so a change made in
//! the web interface or over MCP is registered here within one poll: [`Hotkeys::apply`] releases
//! what is no longer wanted and claims what is new, and nothing is restarted.
//!
//! Where they arrive differs. On Windows and macOS the registration belongs to the thread that
//! runs the tray's event loop, which is also what delivers the key presses, so the tray owns a
//! [`Hotkeys`] and calls it from there; without the tray (`--no-tray`) there is no such loop and
//! the agent says so. On Linux there is no tray, and `global-hotkey` runs an X11 connection on a
//! thread of its own, so [`run_headless`] owns it in a task. Wayland gives no program a global
//! shortcut, which the agent reports instead of registering keys that would only fire while one
//! of its X11 windows has the focus.
//!
//! A combination the system refuses -- another program holds it -- is reported with the others to
//! the service, so the settings page can say which one; the agent keeps running either way.

use std::collections::BTreeMap;

use global_hotkey::{GlobalHotKeyManager, hotkey::HotKey};
use rd_core::{
    CaptureCommand, CapturePlatform, CaptureShortcutReport, CaptureShortcuts, ShortcutsUnavailable,
};

/// The operating system this build runs on, as the report names it.
pub(crate) fn platform() -> CapturePlatform {
    if cfg!(windows) {
        CapturePlatform::Windows
    } else if cfg!(target_os = "macos") {
        CapturePlatform::Macos
    } else {
        CapturePlatform::Linux
    }
}

/// Whether this session can take a shortcut at all.
pub(crate) fn availability() -> Result<(), ShortcutsUnavailable> {
    #[cfg(target_os = "linux")]
    {
        let variable = |name: &str| std::env::var(name).ok();
        linux_availability(
            variable("WAYLAND_DISPLAY").as_deref(),
            variable("DISPLAY").as_deref(),
            variable("XDG_SESSION_TYPE").as_deref(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

/// The Linux rule, with the session's variables passed in so it can be tested.
///
/// `global-hotkey` would not say it on its own: its X11 thread ends quietly when it cannot
/// connect, and every registration after that reports success.
#[cfg(any(target_os = "linux", test))]
fn linux_availability(
    wayland_display: Option<&str>,
    display: Option<&str>,
    session_type: Option<&str>,
) -> Result<(), ShortcutsUnavailable> {
    let set = |value: Option<&str>| value.is_some_and(|value| !value.trim().is_empty());
    if set(wayland_display) || session_type.is_some_and(|kind| kind.eq_ignore_ascii_case("wayland"))
    {
        return Err(ShortcutsUnavailable::Wayland);
    }
    if !set(display) {
        return Err(ShortcutsUnavailable::NoDisplay);
    }
    Ok(())
}

/// The registration for one stored shortcut.
///
/// Read through the shared grammar first, so what is registered is exactly what the service
/// accepted, then handed over in the canonical spelling `global-hotkey` reads as it stands.
pub(crate) fn hotkey_of(text: &str) -> Option<HotKey> {
    let shortcut = rd_core::Shortcut::parse(text).ok()?;
    shortcut.to_string().parse().ok()
}

/// What to release and what to claim to get from the held shortcuts to the wanted ones.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) release: Vec<CaptureCommand>,
    pub(crate) claim: Vec<(CaptureCommand, String)>,
}

/// The change from `held` to `wanted`, command by command. Releasing comes first, so two
/// commands can swap their shortcuts in one change.
pub(crate) fn plan(held: &BTreeMap<CaptureCommand, String>, wanted: &CaptureShortcuts) -> Plan {
    let mut plan = Plan::default();
    for command in CaptureCommand::ALL {
        let want = wanted.get(command);
        match held.get(&command) {
            Some(holding) if Some(holding.as_str()) == want => {}
            Some(_) => {
                plan.release.push(command);
                if let Some(want) = want {
                    plan.claim.push((command, want.to_owned()));
                }
            }
            // Also a shortcut the system refused last time: it is tried again.
            None => {
                if let Some(want) = want {
                    plan.claim.push((command, want.to_owned()));
                }
            }
        }
    }
    plan
}

/// The report for an agent that registers no shortcut at all.
pub(crate) fn unavailable(reason: ShortcutsUnavailable) -> CaptureShortcutReport {
    CaptureShortcutReport {
        platform: platform(),
        refused: Vec::new(),
        unavailable: Some(reason),
        reported_at: None,
    }
}

/// The registered shortcuts.
pub(crate) struct Hotkeys {
    manager: GlobalHotKeyManager,
    held: BTreeMap<CaptureCommand, (String, HotKey)>,
    reported: Option<CaptureShortcutReport>,
}

impl Hotkeys {
    /// Sets the registration up; on Windows and macOS on the tray's thread.
    pub(crate) fn new() -> Result<Self, ShortcutsUnavailable> {
        availability()?;
        let manager = GlobalHotKeyManager::new().map_err(|error| {
            tracing::warn!(%error, "system-wide shortcuts cannot be registered");
            ShortcutsUnavailable::Failed
        })?;
        Ok(Self {
            manager,
            held: BTreeMap::new(),
            reported: None,
        })
    }

    /// Registers the shortcuts `wanted` names. Returns the report when it says something the
    /// last one did not.
    pub(crate) fn apply(&mut self, wanted: &CaptureShortcuts) -> Option<CaptureShortcutReport> {
        let holding: BTreeMap<CaptureCommand, String> = self
            .held
            .iter()
            .map(|(command, (text, _))| (*command, text.clone()))
            .collect();
        let plan = plan(&holding, wanted);
        for command in plan.release {
            if let Some((_, hotkey)) = self.held.remove(&command)
                && let Err(error) = self.manager.unregister(hotkey)
            {
                tracing::debug!(%error, ?command, "a shortcut could not be released");
            }
        }
        for (command, text) in plan.claim {
            let Some(hotkey) = hotkey_of(&text) else {
                tracing::warn!(?command, shortcut = %text, "the shortcut does not read; not registered");
                continue;
            };
            match self.manager.register(hotkey) {
                Ok(()) => {
                    tracing::info!(?command, shortcut = %text, "shortcut registered");
                    self.held.insert(command, (text, hotkey));
                }
                Err(error) => {
                    tracing::warn!(%error, ?command, shortcut = %text, "the system refused the shortcut");
                }
            }
        }
        let report = CaptureShortcutReport {
            platform: platform(),
            refused: CaptureCommand::ALL
                .into_iter()
                .filter(|command| {
                    wanted.get(*command).is_some() && !self.held.contains_key(command)
                })
                .collect(),
            unavailable: None,
            reported_at: None,
        };
        if self.reported.as_ref() == Some(&report) {
            return None;
        }
        self.reported = Some(report.clone());
        Some(report)
    }

    /// The command a key press belongs to.
    pub(crate) fn command(&self, id: u32) -> Option<CaptureCommand> {
        self.held
            .iter()
            .find(|(_, (_, hotkey))| hotkey.id() == id)
            .map(|(command, _)| *command)
    }
}

/// The shortcuts of a run without a tray (RD-1180-03): on Linux they are listened for here; on
/// Windows and macOS the tray's event loop is what receives them, so `--no-tray` has none, and
/// the settings page is told so.
pub(crate) fn spawn_headless(
    background: &mut tokio::task::JoinSet<()>,
    controls: crate::controls::Controls,
    settings: tokio::sync::watch::Receiver<rd_core::CaptureAgentSettings>,
    service: url::Url,
    cancellation: &tokio_util::sync::CancellationToken,
) {
    #[cfg(target_os = "linux")]
    background.spawn(run_headless(
        controls,
        settings,
        service,
        cancellation.clone(),
    ));
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (background, settings, service, cancellation);
        let _ = controls
            .settings
            .send(crate::agent_settings::SettingsRequest::Report(unavailable(
                rd_core::ShortcutsUnavailable::NoTray,
            )));
    }
}

/// Listens for the shortcuts on Linux, where no tray exists, and passes each command on.
#[cfg(target_os = "linux")]
async fn run_headless(
    controls: crate::controls::Controls,
    mut settings: tokio::sync::watch::Receiver<rd_core::CaptureAgentSettings>,
    service: url::Url,
    cancellation: tokio_util::sync::CancellationToken,
) {
    use crate::{agent_settings::SettingsRequest, controls::Action};
    use global_hotkey::{GlobalHotKeyEvent, HotKeyState};

    let report = |report: CaptureShortcutReport| {
        let _ = controls.settings.send(SettingsRequest::Report(report));
    };
    let mut hotkeys = match Hotkeys::new() {
        Ok(hotkeys) => hotkeys,
        Err(reason) => {
            tracing::info!(?reason, "no system-wide shortcuts in this session");
            report(unavailable(reason));
            return;
        }
    };
    let (presses, mut pressed) = tokio::sync::mpsc::unbounded_channel();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state == HotKeyState::Pressed {
            let _ = presses.send(event.id);
        }
    }));
    loop {
        let wanted = settings.borrow_and_update().shortcuts.clone();
        if let Some(sent) = hotkeys.apply(&wanted) {
            report(sent);
        }
        tokio::select! {
            () = cancellation.cancelled() => return,
            changed = settings.changed() => if changed.is_err() { return },
            Some(id) = pressed.recv() => {
                let Some(command) = hotkeys.command(id) else { continue };
                tracing::info!(?command, "shortcut pressed");
                match crate::controls::action(command) {
                    Action::Open => open_in_browser(&service),
                    Action::Quit => cancellation.cancel(),
                    // As the tray's entry: nothing to switch while nothing is set up. Queue
                    // control is the service's to refuse, which the settings task logs.
                    Action::ToggleGameMode if !settings.borrow().game_mode.watches() => {
                        tracing::info!(reason = crate::game_mode::NOTHING_SET_UP, "not switching game mode");
                    }
                    other => controls.pass(other),
                }
            }
        }
    }
}

/// "Open rDownloader" without a tray: the desktop's own opener. A process rather than a crate,
/// because the headless build links nothing that opens a desktop application.
#[cfg(target_os = "linux")]
fn open_in_browser(service: &url::Url) {
    if let Err(error) = std::process::Command::new("xdg-open")
        .arg(service.as_str())
        .spawn()
    {
        tracing::warn!(%error, "could not open the rDownloader web interface");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rd_core::{CaptureCommand, CaptureShortcuts, ShortcutsUnavailable};

    use super::{hotkey_of, linux_availability, plan};

    /// Every default reads in the registering library as it stands, so a fresh agent registers
    /// all of them.
    #[test]
    fn every_default_shortcut_becomes_a_registration() {
        let defaults = CaptureShortcuts::default();
        for command in CaptureCommand::ALL {
            if let Some(text) = defaults.get(command) {
                assert!(hotkey_of(text).is_some(), "{command:?}: {text}");
            }
        }
        // Two commands, two registrations: Shift makes a different one.
        assert_ne!(
            hotkey_of("CmdOrCtrl+Alt+K").map(|hotkey| hotkey.id()),
            hotkey_of("CmdOrCtrl+Alt+Shift+K").map(|hotkey| hotkey.id())
        );
        assert!(
            hotkey_of("Ctrl+C").is_none(),
            "the shared rules hold here too"
        );
        assert!(hotkey_of("not a shortcut").is_none());
    }

    /// A change made in the settings is applied without a restart: what changed is released and
    /// claimed, what stayed is left alone (RD-1180-03).
    #[test]
    fn a_changed_setting_releases_and_claims_only_what_changed() {
        let defaults = CaptureShortcuts::default();
        let fresh = plan(&BTreeMap::new(), &defaults);
        assert!(fresh.release.is_empty());
        assert_eq!(fresh.claim.len(), 7, "every default, quit has none");

        let held: BTreeMap<CaptureCommand, String> = fresh.claim.into_iter().collect();
        assert_eq!(
            plan(&held, &defaults),
            super::Plan::default(),
            "nothing changed"
        );

        let mut changed = defaults.clone();
        changed.set(CaptureCommand::Open, Some("CmdOrCtrl+Alt+R".to_owned()));
        changed.set(CaptureCommand::PauseAll, None);
        changed.set(CaptureCommand::Quit, Some("CmdOrCtrl+Alt+F12".to_owned()));
        let step = plan(&held, &changed);
        assert_eq!(
            step.release,
            vec![CaptureCommand::Open, CaptureCommand::PauseAll]
        );
        assert_eq!(
            step.claim,
            vec![
                (CaptureCommand::Open, "CmdOrCtrl+Alt+R".to_owned()),
                (CaptureCommand::Quit, "CmdOrCtrl+Alt+F12".to_owned()),
            ]
        );
    }

    #[test]
    fn two_commands_swap_their_shortcuts_in_one_change() {
        let defaults = CaptureShortcuts::default();
        let held: BTreeMap<CaptureCommand, String> = CaptureCommand::ALL
            .into_iter()
            .filter_map(|command| Some((command, defaults.get(command)?.to_owned())))
            .collect();
        let mut swapped = defaults.clone();
        swapped.set(
            CaptureCommand::Open,
            defaults
                .get(CaptureCommand::SendClipboard)
                .map(str::to_owned),
        );
        swapped.set(
            CaptureCommand::SendClipboard,
            defaults.get(CaptureCommand::Open).map(str::to_owned),
        );
        let step = plan(&held, &swapped);
        assert_eq!(
            step.release,
            vec![CaptureCommand::Open, CaptureCommand::SendClipboard],
            "both are released before either is claimed"
        );
        assert_eq!(step.claim.len(), 2);
    }

    /// Wayland has no global shortcut for a program to claim, and without a display there is
    /// nothing to register with; both are said, not registered blindly.
    #[test]
    fn linux_registers_under_x11_only() {
        assert_eq!(linux_availability(None, Some(":0"), Some("x11")), Ok(()));
        assert_eq!(linux_availability(None, Some(":0"), None), Ok(()));
        assert_eq!(
            linux_availability(Some("wayland-0"), Some(":0"), Some("wayland")),
            Err(ShortcutsUnavailable::Wayland),
            "XWayland's DISPLAY does not make it an X11 session"
        );
        assert_eq!(
            linux_availability(None, Some(":0"), Some("wayland")),
            Err(ShortcutsUnavailable::Wayland)
        );
        assert_eq!(
            linux_availability(None, None, None),
            Err(ShortcutsUnavailable::NoDisplay)
        );
        assert_eq!(
            linux_availability(Some(""), Some(""), Some("tty")),
            Err(ShortcutsUnavailable::NoDisplay)
        );
    }
}
