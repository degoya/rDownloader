//! The live tray icon and its menu: built once from the state's surface, then only written.

use anyhow::{Context, Result};
use rd_core::{CaptureCommand, CaptureShortcuts};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{
        CheckMenuItem, IsMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem,
        accelerator::Accelerator,
    },
};

use crate::{
    activity::{QueueEntries, QueueMenu},
    self_update::UpdateMenu,
    server_update,
    tray_menu::MenuEntry,
    tray_state::{GameModeEntry, Surface},
};

/// Where the queue entries go: after the two status lines, "Open rDownloader" and the separator
/// below each, so before the clipboard entries and "Quit".
const QUEUE_POSITION: usize = 5;

/// The live tray icon together with its actionable menu entries.
pub(super) struct TrayHandle {
    /// Kept, not just their ids: the status line is rewritten and "Open" is greyed out while
    /// the service is not answering, which needs the items themselves. What to write on them
    /// is `tray_state`'s decision; here they are only written.
    pub(super) status_item: MenuItem,
    /// The server's line under the agent's, with its version and state (RD-1240-06).
    pub(super) server_item: MenuItem,
    pub(super) open_item: MenuItem,
    /// The menu itself, for the queue entries that come and go (RD-1100-06). A handle onto the
    /// same menu the icon shows: muda's menus are shared, not copied.
    menu: Menu,
    /// "Start all"; "Pause all", for 30 minutes, for an hour; the greyed hint for an agent
    /// paired without queue control; and the separator below them. Which of them the menu holds,
    /// and which can be chosen, is `tray_state`'s decision, made from the summary.
    start: MenuItem,
    pause_now: MenuItem,
    pause_half_hour: MenuItem,
    pause_hour: MenuItem,
    /// "Add all from LinkGrabber", started and paused (RD-1240-07): shown, enabled and greyed out
    /// with the queue entries, since the same right covers them.
    add_all: MenuItem,
    add_all_paused: MenuItem,
    pair_hint: MenuItem,
    queue_separator: PredefinedMenuItem,
    /// "Pause clipboard watching", ticked while it holds (RD-1180-01), and "Hand over clipboard
    /// now" (RD-1180-03); always in the menu, before "Quit".
    clipboard_watch: CheckMenuItem,
    send_clipboard: MenuItem,
    /// "Pause while gaming" (RD-1240-23), after the clipboard entries; its label, check mark and
    /// whether it can be chosen are `tray_state`'s.
    game_mode: CheckMenuItem,
    /// "Install server update X" (RD-1240-25), "Restart server" while a restart is pending
    /// (RD-1240-32), "Install update to X" (RD-1210-03), each while there is one, "Install updates
    /// automatically" while the agent is installed without the service (RD-1240-27), and the
    /// separator below them, before "Quit". Which is shown and how is the server update's watch's
    /// and the agent update's watch's decision.
    server_update: MenuItem,
    restart_server: MenuItem,
    update: MenuItem,
    auto_install: CheckMenuItem,
    update_separator: PredefinedMenuItem,
    quit: MenuItem,
    // Dropping this removes the icon from the tray. Kept named rather than `_tray` since the
    // icon and tooltip are now changed while it lives.
    tray: TrayIcon,
}

impl TrayHandle {
    pub(super) fn set_icon(&self, icon: Icon) -> Result<()> {
        self.tray
            .set_icon(Some(icon))
            .context("replace the tray icon")
    }

    pub(super) fn set_tooltip(&self, text: &str) -> Result<()> {
        self.tray
            .set_tooltip(Some(text))
            .context("set the tray tooltip")
    }

    /// Puts the queue entries the state named into the menu, before "Quit": greyed out with the
    /// hint below them for an agent that may not control the queue, none at all before the
    /// service has said which it is.
    pub(super) fn show_queue(&self, queue: QueueMenu) {
        let every: [&dyn IsMenuItem; 8] = [
            &self.start,
            &self.pause_now,
            &self.pause_half_hour,
            &self.pause_hour,
            &self.add_all,
            &self.add_all_paused,
            &self.pair_hint,
            &self.queue_separator,
        ];
        // Whatever is there goes first. Removing an entry the menu does not hold only reports
        // that it does not, which is no fault here.
        for item in every {
            let _ = self.menu.remove(item);
        }
        let (entries, locked) = match queue {
            QueueMenu::Hidden => return,
            QueueMenu::Locked => (QueueEntries::default(), true),
            QueueMenu::Offered(entries) => (entries, false),
        };
        self.start.set_enabled(entries.start);
        self.pause_now.set_enabled(entries.pause);
        self.pause_half_hour.set_enabled(entries.timed_pause);
        self.pause_hour.set_enabled(entries.timed_pause);
        // Enabled whenever the queue may be controlled: the summary does not say whether the
        // LinkGrabber holds anything, and an empty one is answered by the notification.
        self.add_all.set_enabled(!locked);
        self.add_all_paused.set_enabled(!locked);
        let shown = if locked {
            self.menu.insert_items(&every, QUEUE_POSITION)
        } else {
            self.menu.insert_items(
                &[
                    &self.start,
                    &self.pause_now,
                    &self.pause_half_hour,
                    &self.pause_hour,
                    &self.add_all,
                    &self.add_all_paused,
                    &self.queue_separator,
                ],
                QUEUE_POSITION,
            )
        };
        if let Err(error) = shown {
            tracing::warn!(%error, "the tray menu could not show its queue entries");
        }
    }

    /// Shows the service's update entry and the agent's own entries as their watches describe
    /// them, the service's first, or takes them away. All at once, so their order never depends
    /// on which changed last.
    pub(super) fn show_updates(&self, server: &server_update::View, agent: &UpdateMenu) {
        let _ = self.menu.remove(&self.server_update);
        let _ = self.menu.remove(&self.restart_server);
        let _ = self.menu.remove(&self.update);
        let _ = self.menu.remove(&self.auto_install);
        let _ = self.menu.remove(&self.update_separator);
        let mut shown: Vec<&dyn IsMenuItem> = Vec::new();
        for (item, entry) in [
            (&self.server_update, server.entry.as_ref()),
            (&self.restart_server, server.restart.as_ref()),
            (&self.update, agent.offer.as_ref()),
        ] {
            if let Some(entry) = entry {
                item.set_text(&entry.label);
                item.set_enabled(entry.enabled);
                shown.push(item);
            }
        }
        // The switch beside the agent's own update entry, the one it is about.
        if let Some(entry) = agent.auto_install {
            self.auto_install.set_enabled(entry.enabled);
            self.auto_install.set_checked(entry.checked);
            shown.push(&self.auto_install);
        }
        if shown.is_empty() {
            return;
        }
        shown.push(&self.update_separator);
        // Before "Quit", the last entry.
        let position = self.menu.items().len().saturating_sub(1);
        if let Err(error) = self.menu.insert_items(&shown, position) {
            tracing::warn!(%error, "the tray menu could not show the updates");
        }
    }

    /// The command the clicked entry stands for, or `None` for the lines and the hint. Every
    /// entry a click can choose has one (`tray_menu`, RD-1240-24); what each does is
    /// `controls::action`, the same for a click and a shortcut.
    pub(super) fn command(&self, id: &MenuId) -> Option<CaptureCommand> {
        MenuEntry::ALL
            .into_iter()
            .find(|entry| self.item_id(*entry) == id)
            .and_then(MenuEntry::command)
    }

    /// The item of each entry: a new entry is one more arm here and in `tray_menu`.
    fn item_id(&self, entry: MenuEntry) -> &MenuId {
        match entry {
            MenuEntry::StatusLine => self.status_item.id(),
            MenuEntry::ServerLine => self.server_item.id(),
            MenuEntry::Open => self.open_item.id(),
            MenuEntry::StartAll => self.start.id(),
            MenuEntry::PauseAll => self.pause_now.id(),
            MenuEntry::PauseHalfHour => self.pause_half_hour.id(),
            MenuEntry::PauseHour => self.pause_hour.id(),
            MenuEntry::AddAll => self.add_all.id(),
            MenuEntry::AddAllPaused => self.add_all_paused.id(),
            MenuEntry::PairHint => self.pair_hint.id(),
            MenuEntry::ClipboardWatch => self.clipboard_watch.id(),
            MenuEntry::SendClipboard => self.send_clipboard.id(),
            MenuEntry::GameMode => self.game_mode.id(),
            MenuEntry::ServerUpdate => self.server_update.id(),
            MenuEntry::RestartServer => self.restart_server.id(),
            MenuEntry::Update => self.update.id(),
            MenuEntry::AutoInstall => self.auto_install.id(),
            MenuEntry::Quit => self.quit.id(),
        }
    }

    /// Ticks "Pause clipboard watching" as the settings say, also after a click: the click
    /// only asks, the agent's settings decide.
    pub(super) fn show_clipboard_paused(&self, paused: bool) {
        self.clipboard_watch.set_checked(paused);
    }

    /// Writes "Pause while gaming" as the state describes it, also after a click: the click only
    /// asks, the agent's settings decide.
    pub(super) fn show_game_mode(&self, entry: GameModeEntry) {
        self.game_mode.set_text(entry.label);
        self.game_mode.set_enabled(entry.enabled());
        self.game_mode.set_checked(entry.checked);
    }

    /// Writes every entry's shortcut beside it (RD-1180-03).
    pub(super) fn show_accelerators(&self, shortcuts: &CaptureShortcuts) {
        for command in CaptureCommand::ALL {
            let accelerator = shortcuts
                .get(command)
                .and_then(|text| text.parse::<Accelerator>().ok());
            let written = match command {
                CaptureCommand::Open => self.open_item.set_accelerator(accelerator),
                CaptureCommand::StartAll => self.start.set_accelerator(accelerator),
                CaptureCommand::PauseAll => self.pause_now.set_accelerator(accelerator),
                CaptureCommand::PauseHalfHour => self.pause_half_hour.set_accelerator(accelerator),
                CaptureCommand::PauseHour => self.pause_hour.set_accelerator(accelerator),
                CaptureCommand::ClipboardWatch => self.clipboard_watch.set_accelerator(accelerator),
                CaptureCommand::SendClipboard => self.send_clipboard.set_accelerator(accelerator),
                CaptureCommand::GameMode => self.game_mode.set_accelerator(accelerator),
                CaptureCommand::InstallServerUpdate => {
                    self.server_update.set_accelerator(accelerator)
                }
                CaptureCommand::AutoInstall => self.auto_install.set_accelerator(accelerator),
                CaptureCommand::Quit => self.quit.set_accelerator(accelerator),
                CaptureCommand::AddAllFromLinkGrabber => self.add_all.set_accelerator(accelerator),
                CaptureCommand::AddAllFromLinkGrabberPaused => {
                    self.add_all_paused.set_accelerator(accelerator)
                }
                CaptureCommand::InstallUpdate => self.update.set_accelerator(accelerator),
                CaptureCommand::RestartServer => self.restart_server.set_accelerator(accelerator),
            };
            if let Err(error) = written {
                tracing::debug!(%error, ?command, "the menu could not show a shortcut");
            }
        }
    }
}

impl TrayHandle {
    /// Builds the icon and its menu as the state describes them at this moment.
    pub(super) fn build(icon: Icon, surface: &Surface) -> Result<Self> {
        let menu = Menu::new();
        let open = MenuItem::new("Open rDownloader", surface.open_enabled, None);
        let quit = MenuItem::new("Quit", true, None);
        let status_item = MenuItem::new(
            &surface.status_line,
            MenuEntry::StatusLine.clickable(),
            None,
        );
        let server_item = MenuItem::new(
            &surface.server_line,
            MenuEntry::ServerLine.clickable(),
            None,
        );
        let clipboard_watch = CheckMenuItem::new(
            "Pause clipboard watching",
            true,
            surface.clipboard_paused,
            None,
        );
        let send_clipboard = MenuItem::new("Hand over clipboard now", true, None);
        let game_mode = CheckMenuItem::new(
            surface.game_mode.label,
            surface.game_mode.enabled(),
            surface.game_mode.checked,
            None,
        );
        menu.append(&status_item)?;
        menu.append(&server_item)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&open)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&clipboard_watch)?;
        menu.append(&send_clipboard)?;
        menu.append(&game_mode)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit)?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_tooltip(&surface.tooltip)
            .with_icon(icon)
            .build()
            .context("create the tray icon")?;
        let handle = Self {
            status_item,
            server_item,
            open_item: open,
            menu,
            // Untranslated, like the rest of the menu (RD-092-05); the labels are the web
            // interface's global control (RD-1101-06).
            start: MenuItem::new("Start all", true, None),
            pause_now: MenuItem::new("Pause all", true, None),
            pause_half_hour: MenuItem::new("Pause for 30 minutes", true, None),
            pause_hour: MenuItem::new("Pause for 1 hour", true, None),
            add_all: MenuItem::new("Add all from LinkGrabber", true, None),
            add_all_paused: MenuItem::new("Add all from LinkGrabber paused", true, None),
            pair_hint: MenuItem::new(
                "Pair again to control the queue",
                MenuEntry::PairHint.clickable(),
                None,
            ),
            queue_separator: PredefinedMenuItem::separator(),
            clipboard_watch,
            send_clipboard,
            game_mode,
            server_update: MenuItem::new("Install server update", true, None),
            restart_server: MenuItem::new("Restart server", true, None),
            update: MenuItem::new("Install update", true, None),
            auto_install: CheckMenuItem::new("Install updates automatically", false, false, None),
            update_separator: PredefinedMenuItem::separator(),
            quit,
            tray,
        };
        handle.show_queue(surface.queue);
        handle.show_accelerators(&surface.accelerators);
        Ok(handle)
    }
}
