//! The live tray icon and its menu: built once from the state's surface, then only written.

use anyhow::{Context, Result};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{IsMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem},
};

use crate::{
    activity::{QueueEntries, QueueMenu, QueueRequest},
    tray_state::Surface,
};

/// Where the queue entries go: after the status line, "Open rDownloader" and the separator
/// below each of them, so before "Quit".
const QUEUE_POSITION: usize = 4;

/// The live tray icon together with the ids of its actionable menu entries.
pub(super) struct TrayHandle {
    pub(super) open: MenuId,
    pub(super) quit: MenuId,
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

    /// The request a queue entry stands for, or `None` for any other entry.
    pub(super) fn queue_request(&self, id: &MenuId) -> Option<QueueRequest> {
        if id == self.pause_now.id() {
            Some(QueueRequest::Pause { minutes: None })
        } else if id == self.pause_half_hour.id() {
            Some(QueueRequest::Pause { minutes: Some(30) })
        } else if id == self.pause_hour.id() {
            Some(QueueRequest::Pause { minutes: Some(60) })
        } else if id == self.start.id() {
            Some(QueueRequest::Resume)
        } else {
            None
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
        menu.append(&status_item)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&open)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit)?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_tooltip(&surface.tooltip)
            .with_icon(icon)
            .build()
            .context("create the tray icon")?;
        let handle = Self {
            open: open.id().clone(),
            quit: quit.id().clone(),
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
            tray,
        };
        handle.show_queue(surface.queue);
        Ok(handle)
    }
}
