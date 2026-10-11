//! "Pause while gaming" in the tray (RD-1240-23): what the entry shows for the settings and the
//! queue reading, and that it is redrawn only when that changes.

use rd_core::CaptureAgentSettings;
use url::Url;

use super::{GAME_MODE, GAME_MODE_UNSET, GameModeEntry, TrayState, tests::apply};
use crate::{
    activity::{Activity, QueueEntries, QueueMenu},
    game_mode::{NO_QUEUE_CONTROL, NOTHING_SET_UP, QUEUE_CONTROL_UNKNOWN},
};

fn paired() -> TrayState {
    TrayState::new(Some(
        Url::parse("http://127.0.0.1:8710").expect("valid URL"),
    ))
}

fn reading(queue: QueueMenu) -> Activity {
    Activity {
        running: false,
        detail: "1 queued".to_owned(),
        queue,
    }
}

fn controlling() -> QueueMenu {
    QueueMenu::Offered(QueueEntries::default())
}

fn watching(enabled: bool) -> CaptureAgentSettings {
    let mut settings = CaptureAgentSettings::default();
    settings.game_mode.processes = vec!["game.exe".to_owned()];
    settings.game_mode.enabled = enabled;
    settings
}

/// Nothing set up: greyed out, and the label says where to set it up.
#[test]
fn without_a_program_or_full_screen_the_entry_points_to_the_settings() {
    let mut state = paired();
    state.on_transfers(reading(controlling()));
    assert_eq!(
        state.surface().game_mode,
        GameModeEntry {
            label: GAME_MODE_UNSET,
            checked: false,
            refused: Some(NOTHING_SET_UP),
        }
    );
    let mut off = CaptureAgentSettings::default();
    off.game_mode.enabled = false;
    assert!(
        state.on_settings(&off).game_mode.is_none(),
        "switched off with nothing set up is still nothing to switch"
    );
}

/// Set up: ticked while switched on, and it follows a switch made anywhere.
#[test]
fn the_check_mark_follows_the_switch() {
    let mut state = paired();
    state.on_transfers(reading(controlling()));
    let on = state.on_settings(&watching(true));
    assert_eq!(
        on.game_mode,
        Some(GameModeEntry {
            label: GAME_MODE,
            checked: true,
            refused: None,
        })
    );
    let off = state.on_settings(&watching(false));
    assert_eq!(off.game_mode.map(|entry| entry.checked), Some(false));
    assert!(
        state.on_settings(&watching(false)).game_mode.is_none(),
        "the same again redraws nothing"
    );
}

/// The switch needs queue control: greyed out for an agent paired without it and before the
/// service said, enabled once it did.
#[test]
fn only_an_agent_with_queue_control_can_choose_it() {
    let mut state = paired();
    state.on_settings(&watching(true));
    assert_eq!(
        state.surface().game_mode.refused,
        Some(QUEUE_CONTROL_UNKNOWN),
        "not known yet"
    );
    let locked = state.on_transfers(reading(QueueMenu::Locked));
    assert_eq!(
        locked.game_mode.and_then(|entry| entry.refused),
        Some(NO_QUEUE_CONTROL),
        "still greyed out, for another reason"
    );
    assert!(state.surface().game_mode.checked, "but shows the state");
    let offered = state.on_transfers(reading(controlling()));
    assert_eq!(offered.game_mode.map(GameModeEntry::enabled), Some(true));
    assert!(
        state
            .on_transfers(reading(controlling()))
            .game_mode
            .is_none(),
        "a reading that changes nothing redraws nothing"
    );
}

/// What a tray shows after every update is what the state describes.
#[test]
fn applying_the_updates_reproduces_the_entry() {
    let mut state = paired();
    let mut shown = state.surface();
    let steps = vec![
        state.on_settings(&watching(true)),
        state.on_transfers(reading(controlling())),
        state.on_settings(&watching(false)),
        state.on_transfers(reading(QueueMenu::Locked)),
        state.on_settings(&CaptureAgentSettings::default()),
        state.on_transfers(reading(controlling())),
        state.on_settings(&watching(true)),
    ];
    for update in &steps {
        apply(&mut shown, update);
    }
    assert_eq!(shown, state.surface());
    assert!(shown.game_mode.checked && shown.game_mode.enabled());
}
