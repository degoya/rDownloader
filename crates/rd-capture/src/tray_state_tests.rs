use url::Url;

use super::{IconKind, Surface, TrayState, Update, initial_tooltip};
use crate::{
    activity::{Activity, QueueMenu, TOOLTIP_LIMIT},
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
        queue: QueueMenu::Hidden,
    }
}

fn offering(queue: QueueMenu, detail: &str) -> Activity {
    Activity {
        queue,
        ..transfers(false, detail)
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
    if let Some(queue) = update.queue {
        surface.queue = queue;
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
    assert_eq!(
        surface.queue,
        QueueMenu::Hidden,
        "no entry before the service said the agent may"
    );
}

/// The queue entries appear with the first reading that allows them, swap between pause and
/// resume as the reading does, and go when the right goes (RD-1100-06).
#[test]
fn the_queue_entries_follow_the_reading_and_are_named_only_on_a_change() {
    let mut state = paired();
    let update = state.on_transfers(offering(QueueMenu::Pause, "no transfers"));
    assert_eq!(update.queue, Some(QueueMenu::Pause));
    assert_eq!(state.surface().queue, QueueMenu::Pause);
    assert_eq!(
        state
            .on_transfers(offering(QueueMenu::Pause, "1 queued"))
            .queue,
        None,
        "the same entries again rebuild nothing"
    );
    let update = state.on_transfers(offering(QueueMenu::Resume, "paused until 18:30"));
    assert_eq!(update.queue, Some(QueueMenu::Resume));
    assert!(
        update
            .status_line
            .as_deref()
            .is_some_and(|line| line.ends_with("paused until 18:30"))
    );
    let update = state.on_transfers(offering(QueueMenu::Hidden, "paused until 18:30"));
    assert_eq!(update.queue, Some(QueueMenu::Hidden));
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
        state.on_transfers(offering(QueueMenu::Pause, "1 queued")),
        state.on_transfers(offering(QueueMenu::Resume, "paused until 18:30")),
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
