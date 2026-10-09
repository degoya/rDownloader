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
    self_update::OfferEntry,
    tray_state::Surface,
};

/// Where the queue entries go: after the status line, "Open rDownloader" and the separator
/// below each of them, so before the clipboard entries and "Quit".
const QUEUE_POSITION: usize = 4;

/// The live tray icon together with its actionable menu entries.
pub(super) struct TrayHandle {
    /// Kept, not just their ids: the status line is rewritten and "Open" is greyed out while
    /// the service is not answering, which needs the items themselves. What to write on them
    /// is `tray_state`'s decision; here they are only written.
    pub(super) status_item: MenuItem,
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
    pair_hint: MenuItem,
    queue_separator: PredefinedMenuItem,
    /// "Pause clipboard watching", ticked while it holds (RD-1180-01), and "Hand over clipboard
    /// now" (RD-1180-03); always in the menu, before "Quit".
    clipboard_watch: CheckMenuItem,
    send_clipboard: MenuItem,
    /// "Install update to X" and the separator below it, before "Quit", while the agent offers
    /// its own update (RD-1210-03).
    update: MenuItem,
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
        let every: [&dyn IsMenuItem; 6] = [
            &self.start,
            &self.pause_now,
            &self.pause_half_hour,
            &self.pause_hour,
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
        let shown = if locked {
            self.menu.insert_items(&every, QUEUE_POSITION)
        } else {
            self.menu.insert_items(
                &[
                    &self.start,
                    &self.pause_now,
                    &self.pause_half_hour,
                    &self.pause_hour,
                    &self.queue_separator,
                ],
                QUEUE_POSITION,
            )
        };
        if let Err(error) = shown {
            tracing::warn!(%error, "the tray menu could not show its queue entries");
        }
    }

    /// Shows the agent's own update entry as the watch describes it, or takes it away.
    pub(super) fn show_update(&self, entry: Option<&OfferEntry>) {
        let _ = self.menu.remove(&self.update);
        let _ = self.menu.remove(&self.update_separator);
        let Some(entry) = entry else {
            return;
        };
        self.update.set_text(&entry.label);
        self.update.set_enabled(entry.enabled);
        // Before "Quit", the last entry.
        let position = self.menu.items().len().saturating_sub(1);
        if let Err(error) = self
            .menu
            .insert_items(&[&self.update, &self.update_separator], position)
        {
            tracing::warn!(%error, "the tray menu could not show the agent's update");
        }
    }

    /// Whether `id` is the update entry.
    pub(super) fn is_update(&self, id: &MenuId) -> bool {
        self.update.id() == id
    }

    /// The command an entry stands for, or `None` for the status line and the hint. What each
    /// command does is `controls::action`, the same for a click and a shortcut.
    pub(super) fn command(&self, id: &MenuId) -> Option<CaptureCommand> {
        CaptureCommand::ALL
            .into_iter()
            .find(|command| self.item_id(*command) == id)
    }

    fn item_id(&self, command: CaptureCommand) -> &MenuId {
        match command {
            CaptureCommand::Open => self.open_item.id(),
            CaptureCommand::StartAll => self.start.id(),
            CaptureCommand::PauseAll => self.pause_now.id(),
            CaptureCommand::PauseHalfHour => self.pause_half_hour.id(),
            CaptureCommand::PauseHour => self.pause_hour.id(),
            CaptureCommand::ClipboardWatch => self.clipboard_watch.id(),
            CaptureCommand::SendClipboard => self.send_clipboard.id(),
            CaptureCommand::Quit => self.quit.id(),
        }
    }

    /// Ticks "Pause clipboard watching" as the settings say, also after a click: the click
    /// only asks, the agent's settings decide.
    pub(super) fn show_clipboard_paused(&self, paused: bool) {
        self.clipboard_watch.set_checked(paused);
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
                CaptureCommand::Quit => self.quit.set_accelerator(accelerator),
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
        let status_item = MenuItem::new(&surface.status_line, false, None);
        let clipboard_watch = CheckMenuItem::new(
            "Pause clipboard watching",
            true,
            surface.clipboard_paused,
            None,
        );
        let send_clipboard = MenuItem::new("Hand over clipboard now", true, None);
        menu.append(&status_item)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&open)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&clipboard_watch)?;
        menu.append(&send_clipboard)?;
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
            open_item: open,
            menu,
            // Untranslated, like the rest of the menu (RD-092-05); the labels are the web
            // interface's global control (RD-1101-06).
            start: MenuItem::new("Start all", true, None),
            pause_now: MenuItem::new("Pause all", true, None),
            pause_half_hour: MenuItem::new("Pause for 30 minutes", true, None),
            pause_hour: MenuItem::new("Pause for 1 hour", true, None),
            pair_hint: MenuItem::new("Pair again to control the queue", false, None),
            queue_separator: PredefinedMenuItem::separator(),
            clipboard_watch,
            send_clipboard,
            update: MenuItem::new("Install update", true, None),
            update_separator: PredefinedMenuItem::separator(),
            quit,
            tray,
        };
        handle.show_queue(surface.queue);
        handle.show_accelerators(&surface.accelerators);
        Ok(handle)
    }
}
