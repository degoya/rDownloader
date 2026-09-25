//! The state the tray shows, without the tray.
//!
//! Which mark the icon carries, whether "Open rDownloader" can be chosen, what the status item
//! and the tooltip say: all of that is decided here, from the service state, the transfer poll
//! and the agent's notices about itself. Nothing in this module names a type from `image`, `tao`
//! or `tray-icon` -- those crates are Windows- and macOS-only, and the Linux agent must link
//! none of them -- so the rules compile and are tested on every host. `tray.rs` holds the
//! handles and applies what this module decides; `icon.rs` and `status.rs` are the same cut,
//! one step earlier than the obvious type (RD-109-13).
//!
//! The module carries the tray's gate plus `test`, like `status.rs`: the tests run here, and a
//! Linux release build compiles no code it cannot reach.
//!
//! Two rules a description of the whole surface would hide are stated as updates instead: the
//! icon is only named when it actually changes, because redrawing it on every poll flickers on
//! Windows; and a server-state change rewrites the status item but leaves the tooltip as it was,
//! which is what the tray did before its rules moved here and is not this module's to change.

use url::Url;

use crate::{
    activity::{self, Activity},
    status::{ServerStatus, status_label},
    supervision::{AgentNotice, notice_label},
};

/// Which of the two marks the tray shows.
///
/// The busy mark is the idle one with the activity badge painted over it (`icon.rs`); which of
/// the two is up is decided here, what they look like is not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum IconKind {
    Idle,
    Busy,
}

/// Everything the surface shows, as the state wants it now: what a tray built at this moment is
/// given, and what a tray that has applied every update since it was built is showing. The two
/// are the same thing, and a test holds them together.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Surface {
    pub icon: IconKind,
    pub open_enabled: bool,
    pub status_line: String,
    pub tooltip: String,
}

/// What one event changes on the surface. A field left `None` is left alone.
///
/// Not a whole [`Surface`], because which parts an event touches is itself a rule: the icon is
/// only redrawn on a real change, and "Open" only moves with the service state.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct Update {
    pub icon: Option<IconKind>,
    pub open_enabled: Option<bool>,
    pub status_line: Option<String>,
    pub tooltip: Option<String>,
}

impl Update {
    /// Nothing to apply: the event said what the tray already shows.
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The tray's state machine: what has been seen, and what the surface should show for it.
#[derive(Clone, Debug)]
pub(crate) struct TrayState {
    busy: bool,
    /// The transfer line, appended to the server line; `None` until the first reading.
    transfers: Option<String>,
    /// What the agent has to say about itself, appended after the transfers; `None` while there
    /// is nothing wrong. Only the latest is kept: the status item is one line.
    notice: Option<String>,
    /// The server line, as `status_label` renders it for the current state.
    status: String,
    /// The configured service, or `None` when the agent is not paired; only used for relabelling.
    configured: Option<Url>,
    server: ServerStatus,
    /// What the tooltip says now: the product name until the first transfer reading or notice,
    /// then the cut status line of whichever came last.
    tooltip: String,
}

/// The tooltip a freshly built tray carries, before anything has been read.
pub(crate) fn initial_tooltip() -> String {
    format!("rDownloader Capture v{}", env!("CARGO_PKG_VERSION"))
}

impl TrayState {
    /// Before anything has answered: the service is starting, nothing transfers.
    pub(crate) fn new(configured: Option<Url>) -> Self {
        let server = ServerStatus::Starting;
        Self {
            busy: false,
            transfers: None,
            notice: None,
            status: status_label(configured.as_ref(), server),
            configured,
            server,
            tooltip: initial_tooltip(),
        }
    }

    /// Everything the surface should show right now.
    pub(crate) fn surface(&self) -> Surface {
        Surface {
            icon: self.icon(),
            open_enabled: open_enabled(self.server),
            status_line: self.status_line(),
            tooltip: self.tooltip.clone(),
        }
    }

    /// Rewrites the status line and gates "Open rDownloader" on the service answering.
    ///
    /// Opening the browser while the service is still starting lands on an error page, and the
    /// user has no way to tell that from the service being down. A status the tray already
    /// holds changes nothing: the health poll sends on every tick.
    pub(crate) fn on_server_status(&mut self, status: ServerStatus) -> Update {
        if self.server == status {
            return Update::default();
        }
        self.server = status;
        self.status = status_label(self.configured.as_ref(), status);
        Update {
            open_enabled: Some(open_enabled(status)),
            status_line: Some(self.status_line()),
            ..Update::default()
        }
    }

    /// Swaps the mark when transfers start or stop, and keeps the status line current.
    ///
    /// The icon is only named on an actual change: `set_icon` redraws the tray, and doing that
    /// every five seconds makes it flicker on Windows for no reason.
    pub(crate) fn on_transfers(&mut self, activity: Activity) -> Update {
        let changed = self.busy != activity.running;
        self.busy = activity.running;
        self.transfers = Some(activity.detail);
        let line = self.status_line();
        // The menu item takes the line whole; Windows keeps a tooltip in a fixed buffer and
        // silently loses whatever runs past it, so that one is cut to fit.
        self.tooltip = activity::tooltip(&line);
        Update {
            icon: changed.then_some(self.icon()),
            open_enabled: None,
            status_line: Some(line),
            tooltip: Some(self.tooltip.clone()),
        }
    }

    /// Reports something the agent noticed about itself.
    ///
    /// The tray used to keep saying "healthy" after a background task had died or after
    /// Click'n'Load had got only half its addresses, which is precisely when the agent has
    /// stopped doing what it claims (RD-109-07). The same notice twice is nothing new.
    pub(crate) fn on_notice(&mut self, notice: &AgentNotice) -> Update {
        let label = notice_label(notice);
        if self.notice.as_deref() == Some(label.as_str()) {
            return Update::default();
        }
        self.notice = Some(label);
        let line = self.status_line();
        self.tooltip = activity::tooltip(&line);
        Update {
            status_line: Some(line),
            tooltip: Some(self.tooltip.clone()),
            ..Update::default()
        }
    }

    fn icon(&self) -> IconKind {
        if self.busy {
            IconKind::Busy
        } else {
            IconKind::Idle
        }
    }

    /// The status item's text: the server line, the transfers when there are any, and whatever
    /// the agent has to say about itself.
    ///
    /// Composed in `activity`, which compiles everywhere, so the length this can reach is
    /// measured by tests that also run on the Linux hosts where no tray exists.
    fn status_line(&self) -> String {
        let line =
            activity::status_line(&self.status, self.transfers.as_deref().unwrap_or_default());
        activity::status_line(&line, self.notice.as_deref().unwrap_or_default())
    }
}

/// "Open rDownloader" can be chosen exactly while the service answers.
fn open_enabled(server: ServerStatus) -> bool {
    server == ServerStatus::Running
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{IconKind, Surface, TrayState, Update, initial_tooltip};
    use crate::{
        activity::{Activity, TOOLTIP_LIMIT},
        status::{ServerStatus, status_label},
        supervision::AgentNotice,
    };

    fn service() -> Url {
        Url::parse("http://127.0.0.1:8710").expect("valid URL")
    }

    fn paired() -> TrayState {
        TrayState::new(Some(service()))
    }

    fn transfers(running: bool, detail: &str) -> Activity {
        Activity {
            running,
            detail: detail.to_owned(),
        }
    }

    fn task_stopped(task: &'static str) -> AgentNotice {
        AgentNotice::TaskEnded {
            task,
            reason: "it panicked".to_owned(),
        }
    }

    /// What a tray does with an update: the part of `tray.rs` that is only assignments.
    fn apply(surface: &mut Surface, update: &Update) {
        if let Some(icon) = update.icon {
            surface.icon = icon;
        }
        if let Some(enabled) = update.open_enabled {
            surface.open_enabled = enabled;
        }
        if let Some(line) = &update.status_line {
            surface.status_line = line.clone();
        }
        if let Some(tooltip) = &update.tooltip {
            surface.tooltip = tooltip.clone();
        }
    }

    /// The tray is built before anything has answered, so the first thing it shows is the
    /// starting line, the plain mark, "Open" greyed out and a tooltip that names the product.
    #[test]
    fn a_fresh_tray_shows_the_idle_mark_with_open_disabled_and_the_starting_line() {
        let surface = paired().surface();
        assert_eq!(surface.icon, IconKind::Idle);
        assert!(!surface.open_enabled, "the service has not answered yet");
        assert_eq!(
            surface.status_line,
            status_label(Some(&service()), ServerStatus::Starting)
        );
        assert_eq!(surface.tooltip, initial_tooltip());
    }

    /// An agent that is not paired has no service to name, and says so instead of guessing.
    #[test]
    fn an_unpaired_agent_says_so_on_its_status_line() {
        let state = TrayState::new(None);
        assert_eq!(
            state.surface().status_line,
            status_label(None, ServerStatus::Starting)
        );
        assert!(!state.surface().open_enabled);
    }

    /// The service answering is what turns "Open" on, and it rewrites the status line with it.
    ///
    /// The icon and the tooltip are not named: neither has anything to do with the service.
    #[test]
    fn the_service_answering_enables_open_and_rewrites_the_status_line() {
        let mut state = paired();
        let update = state.on_server_status(ServerStatus::Running);
        assert_eq!(update.open_enabled, Some(true));
        assert_eq!(
            update.status_line.as_deref(),
            Some(status_label(Some(&service()), ServerStatus::Running).as_str())
        );
        assert_eq!(
            update.icon, None,
            "the mark does not follow the server state"
        );
        assert_eq!(update.tooltip, None);
        assert!(state.surface().open_enabled);
    }

    /// A service that goes away takes "Open" with it: the browser would land on an error page.
    #[test]
    fn the_service_going_away_disables_open_again() {
        let mut state = paired();
        state.on_server_status(ServerStatus::Running);
        let update = state.on_server_status(ServerStatus::Unreachable);
        assert_eq!(update.open_enabled, Some(false));
        assert!(
            update
                .status_line
                .as_deref()
                .is_some_and(|line| line.ends_with("server not reachable"))
        );
        assert!(!state.surface().open_enabled);
    }

    /// The health poll sends on every tick; a status the tray already holds must cost nothing,
    /// or the status item would be rewritten every five seconds.
    #[test]
    fn the_same_server_status_again_changes_nothing() {
        let mut state = paired();
        assert!(
            state.on_server_status(ServerStatus::Starting).is_empty(),
            "the initial state repeated is not a change"
        );
        state.on_server_status(ServerStatus::Running);
        assert!(state.on_server_status(ServerStatus::Running).is_empty());
    }

    /// Of the three server states exactly one lets the browser be opened.
    #[test]
    fn open_is_enabled_in_exactly_one_server_state() {
        for (status, expected) in [
            (ServerStatus::Starting, false),
            (ServerStatus::Running, true),
            (ServerStatus::Unreachable, false),
        ] {
            let mut state = paired();
            state.on_server_status(status);
            assert_eq!(
                state.surface().open_enabled,
                expected,
                "Open is wrong while the server is {status:?}"
            );
        }
    }

    /// The first transfer swaps the mark for the badged one and puts the figures on the line
    /// and in the tooltip.
    #[test]
    fn transfers_starting_swap_the_mark_for_the_busy_one() {
        let mut state = paired();
        let update = state.on_transfers(transfers(true, "1 active"));
        assert_eq!(update.icon, Some(IconKind::Busy));
        assert!(
            update
                .status_line
                .as_deref()
                .is_some_and(|line| line.ends_with("1 active"))
        );
        assert_eq!(
            update.tooltip, update.status_line,
            "a short line fits whole"
        );
        assert_eq!(
            update.open_enabled, None,
            "transfers say nothing about the service"
        );
        assert_eq!(state.surface().icon, IconKind::Busy);
    }

    /// The poll reports every five seconds. While the answer stays "running", the figures move
    /// but the icon must not be redrawn: on Windows that flickers.
    #[test]
    fn a_second_busy_reading_leaves_the_icon_alone() {
        let mut state = paired();
        state.on_transfers(transfers(true, "1 active"));
        let update = state.on_transfers(transfers(true, "2 active"));
        assert_eq!(update.icon, None, "the mark was redrawn without changing");
        assert!(
            update
                .status_line
                .as_deref()
                .is_some_and(|line| line.ends_with("2 active")),
            "the figures still move"
        );
    }

    /// When the last transfer ends the plain mark comes back.
    #[test]
    fn transfers_stopping_swap_the_idle_mark_back() {
        let mut state = paired();
        state.on_transfers(transfers(true, "1 active"));
        let update = state.on_transfers(transfers(false, "3 queued"));
        assert_eq!(update.icon, Some(IconKind::Idle));
        assert_eq!(state.surface().icon, IconKind::Idle);
        assert!(
            state
                .on_transfers(transfers(false, "2 queued"))
                .icon
                .is_none(),
            "idle twice is not a change either"
        );
    }

    /// The line is server, then transfers, then notice, and each part survives the others
    /// changing.
    #[test]
    fn the_transfer_line_survives_a_server_status_change() {
        let mut state = paired();
        state.on_transfers(transfers(true, "1 active"));
        let update = state.on_server_status(ServerStatus::Running);
        let line = update.status_line.expect("the line is rewritten");
        assert!(line.contains("server running"), "{line}");
        assert!(line.ends_with("1 active"), "{line}");
    }

    /// A notice is appended after the transfers, on the line and in the tooltip.
    #[test]
    fn a_notice_is_appended_to_the_status_line_and_the_tooltip() {
        let mut state = paired();
        state.on_transfers(transfers(true, "1 active"));
        let update = state.on_notice(&task_stopped("clipboard monitoring"));
        let line = update.status_line.expect("the line is rewritten");
        assert!(
            line.ends_with("1 active \u{2014} clipboard monitoring stopped"),
            "{line}"
        );
        assert_eq!(update.tooltip.as_deref(), Some(line.as_str()));
        assert_eq!(update.icon, None);
        assert_eq!(update.open_enabled, None);
    }

    /// A notice that is already on the line is not news.
    #[test]
    fn the_same_notice_again_changes_nothing() {
        let mut state = paired();
        state.on_notice(&task_stopped("clipboard monitoring"));
        assert!(
            state
                .on_notice(&task_stopped("clipboard monitoring"))
                .is_empty()
        );
    }

    /// The status item is one line, so only the latest notice is kept.
    #[test]
    fn a_new_notice_replaces_the_old_one() {
        let mut state = paired();
        state.on_notice(&task_stopped("clipboard monitoring"));
        let update = state.on_notice(&task_stopped("intake notifications"));
        let line = update.status_line.expect("the line is rewritten");
        assert!(line.ends_with("intake notifications stopped"), "{line}");
        assert!(!line.contains("clipboard monitoring"), "{line}");
    }

    /// The tooltip follows the transfer poll and the notices, not the server state. That is
    /// what the tray did before its rules moved here; the test pins it so that changing it is a
    /// decision rather than a side effect.
    #[test]
    fn a_server_status_change_leaves_the_tooltip_as_it_was() {
        let mut state = paired();
        state.on_transfers(transfers(true, "1 active"));
        let before = state.surface().tooltip;
        let update = state.on_server_status(ServerStatus::Running);
        assert_eq!(update.tooltip, None);
        assert_eq!(state.surface().tooltip, before);
    }

    /// The menu takes the line whole; the tooltip is cut to what Windows can carry.
    #[test]
    fn the_tooltip_is_cut_to_fit_while_the_menu_takes_the_line_whole() {
        let mut state = paired();
        let update = state.on_transfers(transfers(true, &"x".repeat(2 * TOOLTIP_LIMIT)));
        let line = update.status_line.expect("the line is rewritten");
        let tooltip = update.tooltip.expect("the tooltip is rewritten");
        assert!(line.chars().count() > TOOLTIP_LIMIT);
        assert_eq!(tooltip.chars().count(), TOOLTIP_LIMIT);
        assert_eq!(state.surface().tooltip, tooltip);
    }

    /// The surface the state describes is exactly what a tray shows after applying every
    /// update in order, for a run that walks through all four kinds of event.
    #[test]
    fn applying_every_update_in_order_reproduces_the_surface() {
        let mut state = paired();
        let mut shown = state.surface();
        let steps: Vec<Update> = vec![
            state.on_server_status(ServerStatus::Starting),
            state.on_transfers(transfers(false, "")),
            state.on_server_status(ServerStatus::Running),
            state.on_transfers(transfers(true, "1 active")),
            state.on_notice(&task_stopped("the transfer poll")),
            state.on_transfers(transfers(true, "2 active")),
            state.on_server_status(ServerStatus::Unreachable),
            state.on_notice(&task_stopped("the transfer poll")),
            state.on_transfers(transfers(false, "1 failed")),
        ];
        for update in &steps {
            apply(&mut shown, update);
        }
        assert_eq!(shown, state.surface());
        assert_eq!(shown.icon, IconKind::Idle);
        assert!(!shown.open_enabled);
    }
}
