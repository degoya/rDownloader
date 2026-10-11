use std::collections::HashSet;

use chrono::{Duration, Utc};

use super::{
    CaptureGameMode, GameModeAction, GameModeProblem, InForce, MAX_GAME_MODE_PROCESSES, process_key,
};
use crate::BandwidthProfileId;

fn watching(names: &[&str]) -> CaptureGameMode {
    CaptureGameMode {
        processes: names.iter().map(|name| (*name).to_owned()).collect(),
        ..CaptureGameMode::default()
    }
}

/// Windows lists `game` for `game.exe`, macOS a path inside `Game.app`; a person writes either.
#[test]
fn a_process_name_is_compared_as_its_bare_file_name() {
    for (name, key) in [
        ("Game.exe", "game"),
        ("game", "game"),
        ("  Game.EXE ", "game"),
        (
            "/Applications/Steam.app/Contents/MacOS/steam_osx",
            "steam_osx",
        ),
        ("Steam.app", "steam"),
        (r"C:\Games\cs2.exe", "cs2"),
        (".exe", ".exe"),
    ] {
        assert_eq!(process_key(name), key, "{name}");
    }
}

#[test]
fn the_settings_are_stored_trimmed_and_each_name_once() {
    let stored = watching(&[" Game.exe", "", "game", "obs64.exe "])
        .validated()
        .expect("valid");
    assert_eq!(stored.processes, ["Game.exe", "obs64.exe"]);
    assert!(stored.active());
    assert!(!CaptureGameMode::default().active(), "off by default");
    assert!(
        !watching(&["  "]).validated().expect("valid").active(),
        "blank names watch nothing"
    );
}

/// The switch (RD-1240-23): on unless somebody switched it off, also for settings stored before it
/// existed; off, the triggers stay and nothing is watched.
#[test]
fn game_mode_is_switched_on_unless_switched_off() {
    let before: CaptureGameMode =
        serde_json::from_value(serde_json::json!({ "full_screen": true })).expect("reads");
    assert!(before.enabled, "stored before the switch: on");
    assert!(before.active());
    assert!(CaptureGameMode::default().enabled);

    let off = CaptureGameMode {
        enabled: false,
        ..watching(&["game.exe"])
    };
    assert!(off.watches(), "the triggers stay");
    assert!(!off.active());
    let stored = off.validated().expect("valid");
    assert!(!stored.enabled, "validating keeps the switch");
    let round: CaptureGameMode =
        serde_json::from_value(serde_json::to_value(&stored).expect("writes")).expect("reads");
    assert_eq!(round, stored);
}

#[test]
fn names_with_a_path_too_many_names_and_a_profile_action_without_profile_are_refused() {
    assert_eq!(
        watching(&[r"C:\Games\game.exe"]).validated(),
        Err(GameModeProblem::ProcessInvalid)
    );
    assert_eq!(
        watching(&["game\u{7}"]).validated(),
        Err(GameModeProblem::ProcessInvalid)
    );
    let many: Vec<String> = (0..=MAX_GAME_MODE_PROCESSES)
        .map(|index| format!("game{index}"))
        .collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(
        watching(&many).validated(),
        Err(GameModeProblem::TooManyProcesses)
    );
    let profile = CaptureGameMode {
        full_screen: true,
        action: GameModeAction::Profile,
        ..CaptureGameMode::default()
    };
    assert_eq!(profile.validated(), Err(GameModeProblem::ProfileMissing));
    let chosen = CaptureGameMode {
        profile_id: Some(BandwidthProfileId::new()),
        ..profile
    };
    assert!(chosen.validated().is_ok());
    assert_eq!(
        GameModeProblem::ProfileMissing.code(),
        "capture.game_mode_profile_missing"
    );
}

#[test]
fn a_running_process_is_found_by_its_key() {
    let mode = watching(&["Game.exe", "obs64"]);
    let running: HashSet<String> = ["explorer", "obs64"].map(str::to_owned).into();
    assert_eq!(mode.running_process(&running), Some("obs64"));
    assert_eq!(mode.running_process(&HashSet::new()), None);
}

/// The service lets the agent hold only over nothing or over its own hold, and lift only its own:
/// a pause somebody set, extended, ended or replaced is theirs.
#[test]
fn only_the_agents_own_hold_is_renewed_or_lifted() {
    let own = Utc::now() + Duration::minutes(10);
    let other = own + Duration::minutes(5);
    assert!(InForce::Nothing.may_hold(None), "a fresh hold");
    assert!(InForce::Until(own).may_hold(Some(own)), "renewing its own");
    assert!(
        !InForce::Until(other).may_hold(None),
        "somebody's timed pause"
    );
    assert!(!InForce::Open.may_hold(None), "somebody's open pause");
    assert!(
        !InForce::Until(other).may_hold(Some(own)),
        "somebody changed it meanwhile"
    );
    assert!(
        !InForce::Nothing.may_hold(Some(own)),
        "somebody ended it: it stays ended until the game does"
    );
    assert!(InForce::Until(own).is_own(own));
    assert!(!InForce::Until(other).is_own(own));
    assert!(!InForce::Open.is_own(own));
    assert!(!InForce::Nothing.is_own(own));
}
