use super::{
    CaptureAgentSettings, CaptureCommand, CaptureShortcuts, Family, RESERVED_MAC, RESERVED_PC,
    Shortcut, ShortcutProblem,
};

/// Parsing and writing back are one grammar: every spelling a person or the web interface sends
/// comes back in the one canonical form, which is what both registering libraries read.
#[test]
fn a_shortcut_is_read_in_any_spelling_and_written_in_one() {
    for (text, canonical) in [
        ("CmdOrCtrl+Alt+V", "CmdOrCtrl+Alt+V"),
        ("cmdorctrl + alt + v", "CmdOrCtrl+Alt+V"),
        ("CommandOrControl+Option+KeyV", "CmdOrCtrl+Alt+V"),
        ("Alt+Control+Shift+Digit7", "Ctrl+Alt+Shift+7"),
        ("Win+Ctrl+F13", "Ctrl+Super+F13"),
        ("Cmd+Option+ArrowUp", "Alt+Super+ArrowUp"),
        ("Ctrl+Alt+up", "Ctrl+Alt+ArrowUp"),
        ("Ctrl+Alt+esc", "Ctrl+Alt+Escape"),
        ("ctrl+alt+pageup", "Ctrl+Alt+PageUp"),
        ("Ctrl+Alt+F24", "Ctrl+Alt+F24"),
    ] {
        let shortcut = Shortcut::read(text).unwrap_or_else(|problem| panic!("{text}: {problem:?}"));
        assert_eq!(shortcut.to_string(), canonical, "{text}");
        assert_eq!(
            Shortcut::read(canonical).expect("the canonical form reads again"),
            shortcut,
            "{text}"
        );
    }
}

#[test]
fn what_is_not_a_shortcut_is_refused_as_invalid() {
    for text in [
        "",
        "Ctrl+Alt+",
        "Ctrl++Alt+K",
        "Ctrl+Alt",
        "Ctrl+Alt+K+L",
        "Ctrl+Alt+Hyper",
        "Ctrl+Alt+F0",
        "Ctrl+Alt+F25",
        "Ctrl+Alt+F05",
        "Ctrl+Ctrl+Alt+K",
        "Hyper+Alt+K",
        // On one of the two platforms that is the same key named twice.
        "CmdOrCtrl+Ctrl+K",
        "CmdOrCtrl+Super+K",
        "Ctrl+Alt+\u{e4}",
    ] {
        assert_eq!(
            Shortcut::parse(text),
            Err(ShortcutProblem::Invalid),
            "{text:?}"
        );
    }
}

/// One modifier is a program's shortcut (Ctrl+C, Cmd+T) or a typed character (Option+E on a
/// Mac); a system-wide registration would take it away in every program.
#[test]
fn a_shortcut_needs_two_of_ctrl_alt_and_super_on_both_platforms() {
    for text in [
        "Ctrl+C",
        "CmdOrCtrl+Shift+T",
        "Alt+F4",
        "Alt+Shift+K",
        "Super+L",
        "Shift+F5",
        "F13",
    ] {
        assert_eq!(
            Shortcut::parse(text),
            Err(ShortcutProblem::ModifierMissing),
            "{text}"
        );
    }
    assert!(Shortcut::parse("Ctrl+Alt+K").is_ok());
    assert!(
        Shortcut::parse("CmdOrCtrl+Super+F13").is_err(),
        "CmdOrCtrl and Super are one key on a Mac"
    );
    assert!(Shortcut::parse("Ctrl+Super+F13").is_ok());
}

#[test]
fn what_a_system_already_uses_is_refused_as_reserved() {
    for text in [
        "Ctrl+Alt+Delete",
        "CmdOrCtrl+Alt+Delete",
        "Ctrl+Alt+T",
        "Ctrl+Alt+ArrowLeft",
        // Cmd+Option+Esc is Force Quit, and CmdOrCtrl is Cmd there.
        "CmdOrCtrl+Alt+Escape",
        "CmdOrCtrl+Alt+H",
        "CmdOrCtrl+Alt+W",
        // The Office key.
        "Ctrl+Alt+Shift+Super+K",
    ] {
        assert_eq!(
            Shortcut::parse(text),
            Err(ShortcutProblem::Reserved),
            "{text}"
        );
    }
}

/// A reserved entry that does not read would quietly protect nothing.
#[test]
fn every_reserved_entry_is_a_shortcut_the_grammar_reads() {
    for entry in RESERVED_PC.iter().chain(RESERVED_MAC) {
        assert!(Shortcut::read(entry).is_ok(), "{entry}");
    }
}

#[test]
fn cmd_or_ctrl_is_ctrl_on_a_pc_and_cmd_on_a_mac() {
    let shortcut = Shortcut::read("CmdOrCtrl+Alt+V").expect("reads");
    let pc = shortcut.on(Family::Pc);
    assert!(pc.ctrl && pc.alt && !pc.super_key);
    let mac = shortcut.on(Family::Mac);
    assert!(!mac.ctrl && mac.alt && mac.super_key);
}

/// The defaults are what a fresh agent registers, so each one has to pass the very rules a
/// person's own choice is held to, and none may collide with another.
///
/// Quit has none, and neither have "Pause while gaming" (RD-1240-23), "Install server update"
/// (RD-1240-25), "Install updates automatically" (RD-1240-27), the two LinkGrabber entries,
/// "Install update" (RD-1240-24) and "Restart server" (RD-1240-32): they can be assigned, but a
/// new tray function takes no combination nobody chose.
#[test]
fn the_defaults_pass_their_own_rules_and_quit_and_the_later_commands_have_none() {
    let defaults = CaptureShortcuts::default();
    assert_eq!(defaults.validated(), Ok(defaults.clone()));
    let without = [
        CaptureCommand::Quit,
        CaptureCommand::GameMode,
        CaptureCommand::InstallServerUpdate,
        CaptureCommand::AutoInstall,
        CaptureCommand::AddAllFromLinkGrabber,
        CaptureCommand::AddAllFromLinkGrabberPaused,
        CaptureCommand::InstallUpdate,
        CaptureCommand::RestartServer,
    ];
    for command in CaptureCommand::ALL {
        assert_eq!(
            defaults.get(command).is_some(),
            !without.contains(&command),
            "{command:?}"
        );
    }
}

/// "Pause while gaming" takes a shortcut like every other command, held to the same rules.
#[test]
fn the_game_mode_switch_can_be_given_a_shortcut() {
    let mut shortcuts = CaptureShortcuts::default();
    shortcuts.set(CaptureCommand::GameMode, Some("cmdorctrl+alt+b".to_owned()));
    let accepted = shortcuts.validated().expect("valid");
    assert_eq!(
        accepted.get(CaptureCommand::GameMode),
        Some("CmdOrCtrl+Alt+B")
    );
    shortcuts.set(CaptureCommand::GameMode, Some("CmdOrCtrl+Alt+V".to_owned()));
    let refused = shortcuts.validated().expect_err("a duplicate");
    assert_eq!(refused.command, CaptureCommand::GameMode);
    assert_eq!(refused.other, Some(CaptureCommand::SendClipboard));
    let stored: CaptureShortcuts = serde_json::from_str("{}").expect("reads");
    assert_eq!(
        stored.get(CaptureCommand::GameMode),
        None,
        "none when left out"
    );
}

#[test]
fn a_shortcut_two_commands_share_on_one_platform_is_refused_for_the_later_one() {
    let mut shortcuts = CaptureShortcuts::default();
    // Not the same text, but the same keys on Windows and Linux.
    shortcuts.set(CaptureCommand::Quit, Some("Ctrl+Alt+V".to_owned()));
    let refused = shortcuts.validated().expect_err("a duplicate");
    assert_eq!(refused.command, CaptureCommand::Quit);
    assert_eq!(refused.problem, ShortcutProblem::Duplicate);
    assert_eq!(refused.other, Some(CaptureCommand::SendClipboard));
    assert_eq!(refused.problem.code(), "capture.shortcut_duplicate");

    // Removing the other one frees the combination.
    shortcuts.set(CaptureCommand::SendClipboard, None);
    let accepted = shortcuts.validated().expect("no longer a duplicate");
    assert_eq!(accepted.get(CaptureCommand::Quit), Some("Ctrl+Alt+V"));
}

#[test]
fn validation_names_the_command_and_stores_the_canonical_spelling() {
    let mut shortcuts = CaptureShortcuts::default();
    shortcuts.set(
        CaptureCommand::Open,
        Some("option+cmdorctrl+keyr".to_owned()),
    );
    let accepted = shortcuts.validated().expect("valid");
    assert_eq!(accepted.get(CaptureCommand::Open), Some("CmdOrCtrl+Alt+R"));

    shortcuts.set(CaptureCommand::PauseAll, Some("Ctrl+P".to_owned()));
    let refused = shortcuts.validated().expect_err("one modifier");
    assert_eq!(refused.command, CaptureCommand::PauseAll);
    assert_eq!(refused.problem.code(), "capture.shortcut_modifier_missing");
}

/// A field left out takes its default, an explicit `null` stays "no shortcut": somebody who
/// removed one must not get it back from the next start.
#[test]
fn a_missing_shortcut_is_the_default_and_null_is_none() {
    let settings: CaptureAgentSettings =
        serde_json::from_str(r#"{"shortcuts":{"open":null}}"#).expect("reads");
    assert!(!settings.clipboard_paused);
    assert_eq!(settings.shortcuts.get(CaptureCommand::Open), None);
    assert_eq!(
        settings.shortcuts.get(CaptureCommand::SendClipboard),
        Some("CmdOrCtrl+Alt+V")
    );
    let empty: CaptureAgentSettings = serde_json::from_str("{}").expect("reads");
    assert_eq!(empty, CaptureAgentSettings::default());
    let round = serde_json::to_string(&settings).expect("writes");
    let again: CaptureAgentSettings = serde_json::from_str(&round).expect("reads again");
    assert_eq!(again, settings);
}

#[test]
fn every_command_has_its_own_stable_name() {
    for command in CaptureCommand::ALL {
        assert_eq!(
            serde_json::to_value(command).expect("writes"),
            serde_json::Value::String(command.as_str().to_owned())
        );
    }
}

/// "Install server update" takes a shortcut like every other command (RD-1240-25), and a stored
/// set from before it reads as none.
#[test]
fn the_server_update_can_be_given_a_shortcut() {
    let mut shortcuts = CaptureShortcuts::default();
    shortcuts.set(
        CaptureCommand::InstallServerUpdate,
        // Not Alt+U: Super+Alt+U is the system's on a Mac (`RESERVED_MAC`).
        Some("cmdorctrl+alt+y".to_owned()),
    );
    let accepted = shortcuts.validated().expect("valid");
    assert_eq!(
        accepted.get(CaptureCommand::InstallServerUpdate),
        Some("CmdOrCtrl+Alt+Y")
    );
    assert_eq!(
        CaptureCommand::InstallServerUpdate.as_str(),
        "install_server_update"
    );
    let stored: CaptureShortcuts = serde_json::from_str("{}").expect("reads");
    assert_eq!(stored.get(CaptureCommand::InstallServerUpdate), None);
}

/// The tray entries that had no command before (RD-1240-24) take a shortcut like every other one,
/// with stable names, and a stored set from before them reads as none.
#[test]
fn the_linkgrabber_entries_and_the_agents_update_can_be_given_a_shortcut() {
    let mut shortcuts = CaptureShortcuts::default();
    for (command, text, name) in [
        (
            CaptureCommand::AddAllFromLinkGrabber,
            "cmdorctrl+alt+a",
            "add_all_from_linkgrabber",
        ),
        (
            CaptureCommand::AddAllFromLinkGrabberPaused,
            "cmdorctrl+alt+shift+a",
            "add_all_from_linkgrabber_paused",
        ),
        (
            CaptureCommand::InstallUpdate,
            "cmdorctrl+alt+n",
            "install_update",
        ),
    ] {
        shortcuts.set(command, Some(text.to_owned()));
        assert_eq!(command.as_str(), name);
    }
    let accepted = shortcuts.validated().expect("valid");
    assert_eq!(
        accepted.get(CaptureCommand::AddAllFromLinkGrabberPaused),
        Some("CmdOrCtrl+Alt+Shift+A")
    );
    assert_eq!(
        accepted.get(CaptureCommand::InstallUpdate),
        Some("CmdOrCtrl+Alt+N")
    );
    let stored: CaptureShortcuts = serde_json::from_str("{}").expect("reads");
    for command in [
        CaptureCommand::AddAllFromLinkGrabber,
        CaptureCommand::AddAllFromLinkGrabberPaused,
        CaptureCommand::InstallUpdate,
    ] {
        assert_eq!(stored.get(command), None, "{command:?}");
    }
}

/// "Restart server" (RD-1240-32) is a command of its own with its stable name, and takes a
/// shortcut like every other.
#[test]
fn the_server_restart_can_be_given_a_shortcut() {
    assert_eq!(CaptureCommand::RestartServer.as_str(), "restart_server");
    assert_eq!(
        serde_json::to_value(CaptureCommand::RestartServer).expect("serialize"),
        serde_json::json!("restart_server")
    );
    let mut shortcuts = CaptureShortcuts::default();
    shortcuts.set(
        CaptureCommand::RestartServer,
        Some("cmdorctrl+alt+y".to_owned()),
    );
    let accepted = shortcuts.validated().expect("valid");
    assert_eq!(
        accepted.get(CaptureCommand::RestartServer),
        Some("CmdOrCtrl+Alt+Y")
    );
}
