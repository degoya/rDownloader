//! Free-text settings (RD-1250-01): what a manifest may declare, and what a target may hold.

use super::{MAX_SETTING_LINE_CHARS, MAX_SETTING_TEXT_CHARS, validate_settings};
use crate::SettingManifest;

fn text(name: &str, multiline: bool) -> SettingManifest {
    SettingManifest {
        name: name.to_owned(),
        choices: Vec::new(),
        multiline,
        default: None,
    }
}

#[test]
fn a_setting_without_choices_is_free_text() {
    let section = text("section", false);
    assert!(section.is_free_text());
    validate_settings(&[section, text("path_mapping", true)]).expect("free text is declarable");

    let parsed: SettingManifest =
        toml::from_str("name = \"path_mapping\"\nmultiline = true").expect("parses");
    assert_eq!(parsed, text("path_mapping", true));
}

#[test]
fn one_line_holds_no_line_break_and_no_control_character() {
    let section = text("section", false);
    assert!(section.accepts("12"));
    assert!(section.accepts("Filme & Serien"));
    assert!(section.accepts(&"x".repeat(MAX_SETTING_LINE_CHARS)));
    assert!(!section.accepts(&"x".repeat(MAX_SETTING_LINE_CHARS + 1)));
    for refused in ["1\n2", "1\r2", "tab\there", "nul\0", "escape\u{1b}[0m"] {
        assert!(!section.accepts(refused), "{refused:?}");
    }
}

#[test]
fn several_lines_hold_line_breaks_and_nothing_else_of_the_kind() {
    let mapping = text("path_mapping", true);
    assert!(mapping.accepts("/srv/downloads=/data/media\n/mnt/b=/b"));
    assert!(mapping.accepts("/srv/downloads=/data/media\r\n/mnt/b=/b\r\n"));
    assert!(mapping.accepts(&"x".repeat(MAX_SETTING_TEXT_CHARS)));
    assert!(!mapping.accepts(&"x".repeat(MAX_SETTING_TEXT_CHARS + 1)));
    assert!(!mapping.accepts("/a=/b\t"));
    assert!(!mapping.accepts("/a=/b\u{7f}"));
}

#[test]
fn a_choice_accepts_its_values_only() {
    let priority = SettingManifest {
        name: "priority".to_owned(),
        choices: vec!["1".to_owned(), "2".to_owned()],
        multiline: false,
        default: Some("2".to_owned()),
    };
    assert!(!priority.is_free_text());
    assert!(priority.accepts("1"));
    assert!(!priority.accepts("3"));
    assert!(!priority.accepts(""));
    validate_settings(std::slice::from_ref(&priority)).expect("a choice");
    let lines = SettingManifest {
        multiline: true,
        ..priority
    };
    validate_settings(&[lines]).expect_err("a choice has no lines");
}

#[test]
fn a_free_text_default_is_text_the_setting_would_take() {
    let defaulted = |default: &str, multiline: bool| SettingManifest {
        default: Some(default.to_owned()),
        ..text("section", multiline)
    };
    validate_settings(&[defaulted("all", false)]).expect("a plain default");
    validate_settings(&[defaulted("/a=/b\n/c=/d", true)]).expect("several lines");
    for (setting, why) in [
        (defaulted("", false), "an empty default"),
        (defaulted("/a=/b\n/c=/d", false), "two lines in one"),
        (
            defaulted(&"x".repeat(MAX_SETTING_LINE_CHARS + 1), false),
            "a default too long",
        ),
    ] {
        validate_settings(&[setting]).expect_err(why);
    }
}
