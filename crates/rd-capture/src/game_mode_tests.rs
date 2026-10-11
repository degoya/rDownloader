use std::collections::HashSet;

use chrono::{Duration, Utc};
use rd_core::{CaptureGameMode, GameModeAction};

use super::{
    Guard, Held, Seen, Step, Trigger,
    detect::{parse_ps, parse_windows_line},
    trigger,
};

fn running(names: &[&str]) -> Seen {
    Seen {
        full_screen: false,
        processes: names.iter().map(|name| (*name).to_owned()).collect(),
    }
}

/// A full-screen program counts only while that trigger is on; a process by its bare name.
#[test]
fn the_desktop_triggers_what_the_settings_watch_for() {
    let mode = CaptureGameMode {
        processes: vec!["Game.exe".to_owned()],
        ..CaptureGameMode::default()
    };
    let full = Seen {
        full_screen: true,
        processes: HashSet::new(),
    };
    assert_eq!(trigger(&mode, &full), None, "full screen is not watched");
    assert_eq!(
        trigger(&mode, &running(&["explorer", "game"])),
        Some(Trigger::Process("Game.exe".to_owned()))
    );
    let both = CaptureGameMode {
        full_screen: true,
        ..mode
    };
    assert_eq!(trigger(&both, &full), Some(Trigger::FullScreen));
    assert_eq!(trigger(&both, &Seen::default()), None);
}

/// A hold while the game runs, renewed before it runs out, lifted once the game is over.
#[test]
fn the_agent_holds_renews_and_lifts_its_own_hold() {
    let now = Utc::now();
    let mut guard = Guard::default();
    assert_eq!(guard.step(false, GameModeAction::Pause, now), Step::Wait);

    let step = guard.step(true, GameModeAction::Pause, now);
    assert_eq!(step, Step::Hold { renews: None });
    let until = now + Duration::minutes(15);
    guard.held(step, Held::Until(until), GameModeAction::Pause);
    assert_eq!(
        guard.step(true, GameModeAction::Pause, now + Duration::minutes(1)),
        Step::Wait,
        "plenty of time left"
    );
    let step = guard.step(true, GameModeAction::Pause, now + Duration::minutes(5));
    assert_eq!(
        step,
        Step::Hold {
            renews: Some(until)
        },
        "renewed with ten minutes left"
    );
    let later = until + Duration::minutes(5);
    guard.held(step, Held::Until(later), GameModeAction::Pause);

    let step = guard.step(false, GameModeAction::Pause, now + Duration::minutes(6));
    assert_eq!(step, Step::Release { until: later }, "the game is over");
    guard.released();
    assert_eq!(guard, Guard::Idle);
}

/// Switched off from the tray while the agent holds (RD-1240-23): the game still runs, but
/// nothing triggers any more, so the next step lifts the hold.
#[test]
fn switching_off_lifts_the_agents_hold_at_once() {
    let now = Utc::now();
    let mut mode = CaptureGameMode {
        full_screen: true,
        ..CaptureGameMode::default()
    };
    let full = Seen {
        full_screen: true,
        processes: HashSet::new(),
    };
    let mut guard = Guard::default();
    let step = guard.step(trigger(&mode, &full).is_some(), mode.action, now);
    assert_eq!(step, Step::Hold { renews: None });
    let until = now + Duration::minutes(15);
    guard.held(step, Held::Until(until), mode.action);

    mode.enabled = false;
    assert_eq!(
        trigger(&mode, &full),
        None,
        "switched off: nothing triggers"
    );
    assert_eq!(
        guard.step(
            trigger(&mode, &full).is_some(),
            mode.action,
            now + Duration::seconds(5)
        ),
        Step::Release { until },
        "lifted, not left to run out"
    );
}

/// Somebody else's pause is never taken over; asked again on the next look, so the game is
/// stepped aside for once theirs ends.
#[test]
fn somebody_elses_hold_is_waited_out() {
    let now = Utc::now();
    let mut guard = Guard::default();
    let step = guard.step(true, GameModeAction::Pause, now);
    guard.held(step, Held::Refused, GameModeAction::Pause);
    assert_eq!(guard, Guard::Idle);
    assert_eq!(
        guard.step(true, GameModeAction::Pause, now),
        Step::Hold { renews: None }
    );
}

/// The agent's pause, resumed by somebody while the game runs, stays resumed until the game is
/// over; the next game is stepped aside for again.
#[test]
fn a_hold_somebody_ended_stays_ended_while_the_game_runs() {
    let now = Utc::now();
    let until = now + Duration::minutes(15);
    let mut guard = Guard::Holding {
        until,
        action: GameModeAction::Pause,
    };
    let step = guard.step(true, GameModeAction::Pause, now + Duration::minutes(6));
    guard.held(step, Held::Refused, GameModeAction::Pause);
    assert_eq!(guard, Guard::Yielded);
    assert_eq!(
        guard.step(true, GameModeAction::Pause, now + Duration::minutes(7)),
        Step::Wait,
        "neither renewed nor lifted"
    );
    assert_eq!(
        guard.step(false, GameModeAction::Pause, now + Duration::minutes(8)),
        Step::Wait
    );
    assert_eq!(guard, Guard::Idle, "the game is over");
    assert_eq!(
        guard.step(true, GameModeAction::Pause, now + Duration::minutes(9)),
        Step::Hold { renews: None }
    );
}

/// The settings page switched from pausing to a profile while the agent held: the old hold is
/// lifted first, then the new one is set.
#[test]
fn a_changed_action_lifts_the_old_hold_first() {
    let now = Utc::now();
    let until = now + Duration::minutes(15);
    let mut guard = Guard::Holding {
        until,
        action: GameModeAction::Pause,
    };
    assert_eq!(
        guard.step(true, GameModeAction::Profile, now),
        Step::Release { until }
    );
    guard.released();
    assert_eq!(
        guard.step(true, GameModeAction::Profile, now),
        Step::Hold { renews: None }
    );
}

/// A failed call changes nothing: the step is made again on the next look.
#[test]
fn a_failed_call_is_made_again() {
    let now = Utc::now();
    let until = now + Duration::minutes(15);
    let mut guard = Guard::Holding {
        until,
        action: GameModeAction::Pause,
    };
    let release = guard.step(false, GameModeAction::Pause, now);
    assert_eq!(guard.step(false, GameModeAction::Pause, now), release);
}

/// The Windows helper's line: the notification state, then the process names.
#[test]
fn the_windows_helper_line_reads() {
    let seen = parse_windows_line("3\tSystem|explorer|cs2|Game\r\n").expect("a line");
    assert!(seen.full_screen, "Direct3D full screen");
    assert!(seen.processes.contains("cs2"));
    assert!(seen.processes.contains("game"), "compared lower case");
    for (state, full) in [
        (1, false),
        (2, true),
        (4, true),
        (5, false),
        (6, false),
        (7, false),
    ] {
        let seen = parse_windows_line(&format!("{state}\t")).expect("a line");
        assert_eq!(seen.full_screen, full, "state {state}");
        assert!(seen.processes.is_empty());
    }
    assert_eq!(parse_windows_line("Add-Type failed"), None);
    assert_eq!(parse_windows_line("x\tgame"), None);
}

#[test]
fn the_macos_process_list_reads_as_bare_names() {
    let names = parse_ps(
        "/sbin/launchd\n/Applications/Steam.app/Contents/MacOS/steam_osx\n  \n/Applications/Game.app/Contents/MacOS/Game Engine\n",
    );
    assert!(names.contains("launchd"));
    assert!(names.contains("steam_osx"));
    assert!(names.contains("game engine"), "names may have spaces");
    assert_eq!(names.len(), 3);
}
