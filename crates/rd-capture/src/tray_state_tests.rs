use rd_core::{CaptureAgentSettings, CaptureCommand};
use url::Url;

use super::{CLIPBOARD_PAUSED, IconKind, Surface, TrayState, Update, initial_tooltip};
use crate::{
    activity::{Activity, QueueEntries, QueueMenu, TOOLTIP_LIMIT},
    status::{ServerStatus, agent_label, status_label},
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
        queue: QueueMenu::Hidden,
    }
}

fn offering(queue: QueueMenu, detail: &str) -> Activity {
    Activity {
        queue,
        ..transfers(false, detail)
    }
}

/// The entries for a queue with something waiting and nothing paused.
fn pausable() -> QueueMenu {
    QueueMenu::Offered(QueueEntries {
        start: false,
        pause: true,
        timed_pause: true,
    })
}

/// The entries while a timed pause holds.
fn timed_pause_holds() -> QueueMenu {
    QueueMenu::Offered(QueueEntries {
        start: true,
        pause: false,
        timed_pause: false,
    })
}

fn task_stopped(task: &'static str) -> AgentNotice {
    AgentNotice::TaskEnded {
        task,
        reason: "it panicked".to_owned(),
    }
}

/// What a tray does with an update: the part of `tray.rs` that is only assignments.
pub(super) fn apply(surface: &mut Surface, update: &Update) {
    if let Some(icon) = update.icon {
        surface.icon = icon;
    }
    if let Some(enabled) = update.open_enabled {
        surface.open_enabled = enabled;
    }
    if let Some(queue) = update.queue {
        surface.queue = queue;
    }
    if let Some(line) = &update.status_line {
        surface.status_line = line.clone();
    }
    if let Some(line) = &update.server_line {
        surface.server_line = line.clone();
    }
    if let Some(tooltip) = &update.tooltip {
        surface.tooltip = tooltip.clone();
    }
    if let Some(paused) = update.clipboard_paused {
        surface.clipboard_paused = paused;
    }
    if let Some(accelerators) = &update.accelerators {
        surface.accelerators = accelerators.clone();
    }
    if let Some(entry) = update.game_mode {
        surface.game_mode = entry;
    }
}

fn clipboard_paused(paused: bool) -> CaptureAgentSettings {
    CaptureAgentSettings {
        clipboard_paused: paused,
        ..CaptureAgentSettings::default()
    }
}

/// The tray is built before anything has answered, so the first thing it shows is the
/// starting line, the plain mark, "Open" greyed out and a tooltip that names the product.
#[test]
fn a_fresh_tray_shows_the_idle_mark_with_open_disabled_and_the_starting_line() {
    let surface = paired().surface();
    assert_eq!(surface.icon, IconKind::Idle);
    assert!(!surface.open_enabled, "the service has not answered yet");
    assert_eq!(surface.status_line, agent_label(Some(&service())));
    assert_eq!(surface.server_line, "Server \u{2014} starting");
    assert_eq!(surface.tooltip, initial_tooltip());
    assert_eq!(
        surface.queue,
        QueueMenu::Hidden,
        "no entry before the service said the agent may"
    );
}

/// The queue entries appear with the first reading, change their enabled state as the reading
/// does, and turn greyed out with the pairing hint when the right goes (RD-1100-06, RD-1101-06).
#[test]
fn the_queue_entries_follow_the_reading_and_are_named_only_on_a_change() {
    let mut state = paired();
    let update = state.on_transfers(offering(pausable(), "no transfers"));
    assert_eq!(update.queue, Some(pausable()));
    assert_eq!(state.surface().queue, pausable());
    assert_eq!(
        state.on_transfers(offering(pausable(), "1 queued")).queue,
        None,
        "the same entries again rebuild nothing"
    );
    let update = state.on_transfers(offering(timed_pause_holds(), "paused until 18:30"));
    assert_eq!(update.queue, Some(timed_pause_holds()));
    assert!(
        update
            .status_line
            .as_deref()
            .is_some_and(|line| line.ends_with("paused until 18:30"))
    );
    let update = state.on_transfers(offering(QueueMenu::Locked, "paused until 18:30"));
    assert_eq!(update.queue, Some(QueueMenu::Locked));
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

/// The service answering is what turns "Open" on, and it rewrites the server line with it.
///
/// The icon, the agent's line and the tooltip are not named: none of them follows the service
/// (RD-1240-06).
#[test]
fn the_service_answering_enables_open_and_rewrites_the_server_line() {
    let mut state = paired();
    let update = state.on_server_status(ServerStatus::Running, None);
    assert_eq!(update.open_enabled, Some(true));
    assert_eq!(
        update.server_line.as_deref(),
        Some("Server \u{2014} running")
    );
    assert_eq!(update.status_line, None);
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
    state.on_server_status(ServerStatus::Running, None);
    let update = state.on_server_status(ServerStatus::Unreachable, None);
    assert_eq!(update.open_enabled, Some(false));
    assert_eq!(
        update.server_line.as_deref(),
        Some("Server \u{2014} not reachable")
    );
    assert!(!state.surface().open_enabled);
}

/// The health poll sends on every tick; a status the tray already holds must cost nothing,
/// or the status item would be rewritten every five seconds.
#[test]
fn the_same_server_status_again_changes_nothing() {
    let mut state = paired();
    assert!(
        state
            .on_server_status(ServerStatus::Starting, None)
            .is_empty(),
        "the initial state repeated is not a change"
    );
    state.on_server_status(ServerStatus::Running, None);
    assert!(
        state
            .on_server_status(ServerStatus::Running, None)
            .is_empty()
    );
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
        state.on_server_status(status, None);
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
        update.tooltip,
        Some(format!(
            "{} \u{2014} 1 active",
            status_label(Some(&service()), ServerStatus::Starting)
        )),
        "a short line fits whole, and the tooltip still names the server"
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

/// The agent's line is the agent, then transfers, then notice; a server change leaves it
/// alone and only rewrites the server's line (RD-1240-06).
#[test]
fn the_transfer_line_survives_a_server_status_change() {
    let mut state = paired();
    state.on_transfers(transfers(true, "1 active"));
    let update = state.on_server_status(ServerStatus::Running, None);
    assert_eq!(update.status_line, None);
    let line = state.surface().status_line;
    assert!(!line.contains("server"), "{line}");
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
    let tooltip = update.tooltip.expect("the tooltip is rewritten");
    assert!(tooltip.contains("server starting"), "{tooltip}");
    assert!(
        tooltip.ends_with("clipboard monitoring stopped"),
        "{tooltip}"
    );
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
    let update = state.on_server_status(ServerStatus::Running, None);
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
        state.on_server_status(ServerStatus::Starting, None),
        state.on_transfers(transfers(false, "")),
        state.on_server_status(ServerStatus::Running, Some("1.24.0".to_owned())),
        state.on_transfers(transfers(true, "1 active")),
        state.on_notice(&task_stopped("the transfer poll")),
        state.on_transfers(transfers(true, "2 active")),
        state.on_transfers(offering(pausable(), "1 queued")),
        state.on_transfers(offering(timed_pause_holds(), "paused until 18:30")),
        state.on_transfers(offering(QueueMenu::Locked, "1 queued")),
        state.on_server_status(ServerStatus::Unreachable, None),
        state.on_notice(&task_stopped("the transfer poll")),
        state.on_settings(&clipboard_paused(true)),
        state.on_transfers(transfers(false, "1 failed")),
        state.on_settings(&clipboard_paused(false)),
    ];
    for update in &steps {
        apply(&mut shown, update);
    }
    assert_eq!(shown, state.surface());
    assert_eq!(shown.icon, IconKind::Idle);
    assert!(!shown.open_enabled);
}

/// Pausing ticks the entry, greys the mark and says so on the line and in the tooltip;
/// resuming takes all of it back (RD-1180-01).
#[test]
fn a_paused_clipboard_shows_on_the_mark_the_entry_the_line_and_the_tooltip() {
    let mut state = paired();
    state.on_transfers(transfers(false, "no transfers"));
    assert!(!state.surface().clipboard_paused);

    let update = state.on_settings(&clipboard_paused(true));
    assert_eq!(update.clipboard_paused, Some(true));
    assert_eq!(update.icon, Some(IconKind::PausedIdle));
    assert!(
        update
            .status_line
            .as_deref()
            .is_some_and(|line| line.ends_with(CLIPBOARD_PAUSED)),
        "{update:?}"
    );
    assert!(
        update
            .tooltip
            .as_deref()
            .is_some_and(|tooltip| tooltip.contains(CLIPBOARD_PAUSED))
    );
    assert_eq!(update.accelerators, None, "the shortcuts did not change");

    // Transfers starting while paused keep the grey mark and add the badge.
    let busy = state.on_transfers(transfers(true, "1 active"));
    assert_eq!(busy.icon, Some(IconKind::PausedBusy));
    assert!(
        busy.status_line
            .as_deref()
            .is_some_and(|line| line.contains("1 active") && line.ends_with(CLIPBOARD_PAUSED))
    );

    assert!(
        state.on_settings(&clipboard_paused(true)).is_empty(),
        "the same settings again change nothing"
    );
    let resumed = state.on_settings(&clipboard_paused(false));
    assert_eq!(resumed.icon, Some(IconKind::Busy));
    assert_eq!(resumed.clipboard_paused, Some(false));
    assert!(
        !resumed
            .status_line
            .as_deref()
            .unwrap_or_default()
            .contains(CLIPBOARD_PAUSED)
    );
}

/// The menu's accelerators follow a change made in the settings without a restart, and only a
/// change rewrites them (RD-1180-03).
#[test]
fn changed_shortcuts_rewrite_the_accelerators_and_nothing_else() {
    let mut state = paired();
    assert_eq!(
        state
            .surface()
            .accelerators
            .get(CaptureCommand::SendClipboard),
        Some("CmdOrCtrl+Alt+V"),
        "the defaults until the first reading"
    );
    let mut settings = CaptureAgentSettings::default();
    settings.shortcuts.set(
        CaptureCommand::SendClipboard,
        Some("CmdOrCtrl+Alt+B".to_owned()),
    );
    let update = state.on_settings(&settings);
    assert_eq!(
        update
            .accelerators
            .as_ref()
            .and_then(|shortcuts| shortcuts.get(CaptureCommand::SendClipboard)),
        Some("CmdOrCtrl+Alt+B")
    );
    assert_eq!(update.icon, None);
    assert_eq!(update.status_line, None);
    assert!(state.on_settings(&settings).is_empty());
}
