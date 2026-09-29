//! 7-Zip's Windows command line (security review 2026-09-28, finding 2), on the model of
//! `rar_args_tests.rs`: a port of 7-Zip's parser (`SplitCommandLine`,
//! `CPP/Common/CommandLineParser.cpp`) that first reproduces the injection through Rust's quoting,
//! then checks that [`seven_zip_quote`] round-trips every argument it accepts and refuses the one
//! kind it cannot express.

use std::{ffi::OsString, path::Path};

use crate::{
    ExtractionError, RarToolKind,
    rar_args::{RarAction, rar_arguments, seven_zip_quote},
    rar_args_tests::{ARCHIVE, QUOTE, STAGING, command_line, rust_std_quote, strings, text, units},
};

/// 7-Zip's `SplitCommandLine`, both halves: the line is trimmed once, then one argument is cut off
/// at a time. Space and tab separate outside quotes, every `"` toggles quoting and is dropped, and
/// nothing escapes anything. A piece is kept when the separator was not its first character
/// (`return i != 0`), which is how runs of spaces vanish and how `""` still yields an empty
/// argument. The first entry is the program.
fn seven_zip_parse(line: &[u16]) -> Vec<Vec<u16>> {
    let blank = |unit: &u16| matches!(*unit, 0x20 | 0x09 | 0x0a);
    let start = line
        .iter()
        .position(|unit| !blank(unit))
        .unwrap_or(line.len());
    let end = line
        .iter()
        .rposition(|unit| !blank(unit))
        .map_or(start, |last| last + 1);
    let mut rest = &line[start..end];
    let mut parts = Vec::new();
    loop {
        let mut part = Vec::new();
        let mut quoted = false;
        let mut index = 0;
        while index < rest.len() {
            let unit = rest[index];
            if (unit == 0x20 || unit == 0x09) && !quoted {
                break;
            }
            if unit == QUOTE {
                quoted = !quoted;
            } else {
                part.push(unit);
            }
            index += 1;
        }
        if index != 0 {
            parts.push(part);
        }
        if index + 1 >= rest.len() {
            return parts;
        }
        rest = &rest[index + 1..];
    }
}

/// What 7-Zip ends up with for `args` rendered by `render`, program name dropped.
fn round_trip(args: &[OsString], render: impl Fn(&[u16]) -> Vec<u16>) -> Vec<String> {
    seven_zip_parse(&command_line(args, render))
        .iter()
        .skip(1)
        .map(|param| text(param))
        .collect()
}

/// What 7-Zip ends up with for the pieces `windows_raw_args` hands to `raw_arg`.
fn raw_round_trip(raw: &[Vec<u16>]) -> Vec<String> {
    let mut line = units(r#""C:\Program Files\7-Zip\7z.exe""#);
    for arg in raw {
        line.push(0x20);
        line.extend(arg);
    }
    seven_zip_parse(&line)
        .iter()
        .skip(1)
        .map(|param| text(param))
        .collect()
}

fn extract() -> RarAction<'static> {
    RarAction::Extract {
        staging: Path::new(STAGING),
    }
}

/// The finding, reproduced: through Rust's quoting the password `x" -spf -w"` becomes the switches
/// `-px\`, `-spf` and `-w\`, all in front of `--`.
#[test]
fn the_model_reproduces_the_switch_injection_through_rusts_quoting() {
    let built = rar_arguments(
        RarToolKind::SevenZip,
        extract(),
        Path::new(ARCHIVE),
        Some(r#"x" -spf -w""#),
    );
    let parsed = round_trip(&built.args, rust_std_quote);
    assert_eq!(parsed[4..7], [r"-px\", "-spf", r"-w\"]);
    let dashes = parsed.iter().position(|arg| arg == "--").expect("--");
    let injected = parsed.iter().position(|arg| arg == "-spf").expect("-spf");
    assert!(injected < dashes, "{parsed:?}");
}

/// Every argument of both actions arrives unchanged, a password without a `"` included - spaces,
/// tabs and backslashes, a trailing one too.
#[test]
fn every_quote_free_seven_zip_argument_survives_the_windows_command_line() {
    let passwords = [
        "plain",
        r"trailing\",
        r"trailing space\ ",
        r"\\double\\",
        " spaced out ",
        "tab\there",
        r"-spf -snld \ -w\ ",
        "p\u{e4}ssw\u{f6}rt \u{1f511}",
    ];
    for password in passwords {
        for action in [extract(), RarAction::Test] {
            let built = rar_arguments(
                RarToolKind::SevenZip,
                action,
                Path::new(ARCHIVE),
                Some(password),
            );
            let raw = built
                .windows_raw_args(RarToolKind::SevenZip)
                .expect("quote-free password");
            assert_eq!(
                raw_round_trip(&raw),
                strings(&built.args),
                "password {password:?}, {action:?}"
            );
            assert!(strings(&built.args).contains(&format!("-p{password}")));
        }
    }
    // No password: the bare `-p` that keeps 7-Zip off the console.
    let built = rar_arguments(RarToolKind::SevenZip, extract(), Path::new(ARCHIVE), None);
    let raw = built
        .windows_raw_args(RarToolKind::SevenZip)
        .expect("no password");
    assert_eq!(raw_round_trip(&raw), strings(&built.args));
}

/// A password with a `"` never reaches 7-Zip's command line: the attempt is refused with its own
/// code, and it counts as a password problem, so the next candidate is still tried. `unrar` can
/// express the same password and is not affected.
#[test]
fn a_seven_zip_password_with_a_quote_is_refused_before_the_tool_starts() {
    for password in [r#"x" -spf -w""#, r#"a"b"#, r#"""#, r#"ends in quote""#] {
        for action in [extract(), RarAction::Test] {
            let seven = rar_arguments(
                RarToolKind::SevenZip,
                action,
                Path::new(ARCHIVE),
                Some(password),
            );
            let refused = seven.windows_raw_args(RarToolKind::SevenZip);
            assert!(
                matches!(refused, Err(ExtractionError::PasswordHasQuote)),
                "password {password:?}, {action:?}: {refused:?}"
            );
            let unrar = rar_arguments(
                RarToolKind::Unrar,
                action,
                Path::new(ARCHIVE),
                Some(password),
            );
            assert!(unrar.windows_raw_args(RarToolKind::Unrar).is_ok());
        }
    }
    let error = ExtractionError::PasswordHasQuote;
    assert_eq!(error.code(), "extract.password_has_quote");
    assert!(error.is_password_problem());
    assert_eq!(error.detail(), None);
}

/// Not just the examples: every string of up to five characters over the characters that matter
/// to the parser. The quote-free ones round-trip, every one with a `"` is refused.
#[test]
fn seven_zip_quoting_round_trips_or_refuses_every_short_string() {
    let alphabet = ['a', ' ', '\t', '"', '\\'];
    let mut pending = vec![String::new()];
    let (mut kept, mut refused) = (0, 0);
    while let Some(prefix) = pending.pop() {
        let quoted = seven_zip_quote(&units(&prefix));
        if prefix.contains('"') {
            assert_eq!(quoted, None, "{prefix:?}");
            refused += 1;
        } else {
            let args = [OsString::from(&prefix), OsString::from("next")];
            assert_eq!(
                round_trip(&args, |arg| seven_zip_quote(arg).expect("quote-free")),
                [prefix.clone(), "next".to_owned()],
                "{prefix:?}"
            );
            kept += 1;
        }
        if prefix.chars().count() < 5 {
            pending.extend(
                alphabet
                    .iter()
                    .map(|character| format!("{prefix}{character}")),
            );
        }
    }
    // The empty string included: `""` is an empty argument to 7-Zip.
    assert_eq!(kept, 1 + 4 + 16 + 64 + 256 + 1024);
    assert_eq!(
        refused,
        (5 + 25 + 125 + 625 + 3125) - (4 + 16 + 64 + 256 + 1024)
    );
}

/// The whole path under Windows: `test_rar` refuses before anything is spawned. The executable
/// only has to exist - the test binary itself - because it is never started.
#[cfg(windows)]
#[tokio::test]
async fn test_rar_refuses_a_seven_zip_password_with_a_quote_on_windows() {
    let tool = crate::ExternalRarTool {
        executable: std::env::current_exe().expect("test binary"),
        kind: RarToolKind::SevenZip,
        timeout: std::time::Duration::from_secs(30),
    };
    let result = crate::test_rar(&tool, Path::new(ARCHIVE), Some(r#"x" -spf -w""#)).await;
    assert!(
        matches!(result, Err(ExtractionError::PasswordHasQuote)),
        "{result:?}"
    );
}
