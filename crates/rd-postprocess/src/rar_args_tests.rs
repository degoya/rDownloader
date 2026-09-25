//! RD-120-56: the RAR tools' argument lists, and a model of the Windows command line they cross.
//!
//! The model has two halves, both ported from their sources rather than guessed: Rust's own
//! quoting (`append_arg` in `library/std/src/sys/args/windows.rs`, `Quote::Auto`) and `unrar`'s
//! parser (`GetCmdParam`, `strfn.cpp` of unrar 7.20). It first has to reproduce the field
//! failure word for word, which is what makes its verdict on the fix worth anything.

use std::{ffi::OsString, path::Path};

use crate::{
    RarToolKind,
    rar_args::{RarAction, rar_arguments, unrar_quote},
};

const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn text(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// Rust std, `append_arg` with `Quote::Auto`: what `Command::arg` writes onto a Windows command
/// line.
fn rust_std_quote(arg: &[u16]) -> Vec<u16> {
    let quote = arg.is_empty() || arg.iter().any(|&unit| unit == 0x20 || unit == 0x09);
    let mut out = Vec::new();
    if quote {
        out.push(QUOTE);
    }
    let mut backslashes = 0_usize;
    for &unit in arg {
        if unit == BACKSLASH {
            backslashes += 1;
        } else {
            if unit == QUOTE {
                out.extend(std::iter::repeat_n(BACKSLASH, backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(unit);
    }
    if quote {
        out.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        out.push(QUOTE);
    }
    out
}

/// unrar's `GetCmdParam`, applied until the line is used up. The first entry is the program.
fn unrar_parse(line: &[u16]) -> Vec<Vec<u16>> {
    let is_space = |unit: Option<&u16>| matches!(unit, Some(&0x20 | &0x09));
    let mut params = Vec::new();
    let mut pos = 0;
    loop {
        while is_space(line.get(pos)) {
            pos += 1;
        }
        if pos >= line.len() {
            return params;
        }
        let mut param = Vec::new();
        let mut quoted = false;
        while pos < line.len() && (quoted || !is_space(line.get(pos))) {
            if line[pos] == QUOTE {
                if line.get(pos + 1) == Some(&QUOTE) {
                    param.push(QUOTE);
                    pos += 1;
                } else {
                    quoted = !quoted;
                }
            } else {
                param.push(line[pos]);
            }
            pos += 1;
        }
        params.push(param);
    }
}

/// The command line `CreateProcessW` receives: the program always quoted, as std does, then the
/// arguments separated by one space each, rendered by `render`.
fn command_line(args: &[OsString], render: impl Fn(&[u16]) -> Vec<u16>) -> Vec<u16> {
    let mut line = units(r#""C:\Tools\rdownloader\vendor\unrar.exe""#);
    for arg in args {
        line.push(0x20);
        line.extend(render(&units(&arg.to_string_lossy())));
    }
    line
}

/// What unrar ends up with, program name dropped.
fn round_trip(args: &[OsString], render: impl Fn(&[u16]) -> Vec<u16>) -> Vec<String> {
    unrar_parse(&command_line(args, render))
        .iter()
        .skip(1)
        .map(|param| text(param))
        .collect()
}

fn strings(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

const STAGING: &str = r"\\?\D:\a b\.rd-xabc";
const ARCHIVE: &str = r"\\?\D:\a b\release.part1.rar";

/// The field report, reproduced: `cannot create \\?\d:\downloads\lust auf genuss nr 11 -
/// november 2026\.rd-xc2ec8h\\ruvalfa-....pdf`. The old argument list, through Rust's quoting and
/// unrar's parser, ends in the doubled separator; without a space nothing is quoted and it does not.
#[test]
fn the_model_reproduces_the_field_failure_of_the_old_argument_list() {
    let old = |package: &str| -> Vec<OsString> {
        ["x", "-o-", "-y", "-idc", "-p-", "--"]
            .iter()
            .map(OsString::from)
            .chain([
                OsString::from(format!(r"\\?\{package}\a.rar")),
                OsString::from(format!(r"\\?\{package}\.rd-xc2ec8h\")),
            ])
            .collect()
    };
    let spaced = round_trip(
        &old(r"D:\downloads\Lust auf Genuss Nr 11 - November 2026"),
        rust_std_quote,
    );
    assert_eq!(
        spaced.last().map(String::as_str),
        Some(r"\\?\D:\downloads\Lust auf Genuss Nr 11 - November 2026\.rd-xc2ec8h\\")
    );
    let plain = round_trip(
        &old(r"D:\downloads\serien\Adventure.Buddies.S03E06"),
        rust_std_quote,
    );
    assert_eq!(
        plain.last().map(String::as_str),
        Some(r"\\?\D:\downloads\serien\Adventure.Buddies.S03E06\.rd-xc2ec8h\")
    );
}

/// The same cause, the password: Rust escapes `"` as `\"`, unrar keeps the backslash.
#[test]
fn the_model_shows_rusts_quoting_mangling_a_password_for_unrar() {
    let args = [OsString::from(r#"-pab"cd"#)];
    assert_eq!(round_trip(&args, rust_std_quote), [r#"-pab\cd"#]);
}

#[test]
fn unrar_gets_the_destination_as_a_switch_and_no_argument_ends_in_a_separator() {
    let built = rar_arguments(
        RarToolKind::Unrar,
        RarAction::Extract {
            staging: Path::new(STAGING),
        },
        Path::new(ARCHIVE),
        None,
    );
    let args = strings(&built.args);
    assert_eq!(
        args,
        [
            "x",
            "-o-",
            "-y",
            "-idc",
            "-p-",
            &format!("-op{STAGING}"),
            "--",
            ARCHIVE
        ]
    );
    for arg in &args {
        assert!(!arg.is_empty());
        assert!(!arg.ends_with('\\') && !arg.ends_with('/'), "{arg}");
    }
    // The archive is the only thing after `--`; the destination is not a positional argument.
    let dashes = args.iter().position(|arg| arg == "--").expect("--");
    assert_eq!(args.len(), dashes + 2);
}

#[test]
fn the_test_command_and_seven_zip_keep_their_argument_lists() {
    let unrar_test = rar_arguments(
        RarToolKind::Unrar,
        RarAction::Test,
        Path::new("/pkg/a.rar"),
        Some("pw"),
    );
    assert_eq!(
        strings(&unrar_test.args),
        ["t", "-y", "-idc", "-ppw", "--", "/pkg/a.rar"]
    );
    let seven_extract = rar_arguments(
        RarToolKind::SevenZip,
        RarAction::Extract {
            staging: Path::new("/pkg/.rd-xabc"),
        },
        Path::new("/pkg/a.rar"),
        None,
    );
    assert_eq!(
        strings(&seven_extract.args),
        [
            "x",
            "-y",
            "-bsp1",
            "-bso0",
            "-p",
            "-o/pkg/.rd-xabc",
            "--",
            "/pkg/a.rar"
        ]
    );
    let seven_test = rar_arguments(
        RarToolKind::SevenZip,
        RarAction::Test,
        Path::new("/pkg/a.rar"),
        Some("pw"),
    );
    assert_eq!(
        strings(&seven_test.args),
        ["t", "-y", "-bso0", "-ppw", "--", "/pkg/a.rar"]
    );
}

/// Every argument arrives unchanged under unrar's parser, passwords included.
#[test]
fn every_unrar_argument_survives_the_windows_command_line() {
    let passwords = [
        r#"a"b"#,
        r#"a\"b"#,
        r"trailing\",
        r"trailing space\ ",
        r#"" leading quote"#,
        r#"ends in quote""#,
        r#""""#,
        " spaced out ",
        "tab\there",
        r#"all of it: \" \\"" \"#,
        "p\u{e4}ssw\u{f6}rt \u{1f511}",
    ];
    for password in passwords {
        for action in [
            RarAction::Extract {
                staging: Path::new(STAGING),
            },
            RarAction::Test,
        ] {
            let built = rar_arguments(
                RarToolKind::Unrar,
                action,
                Path::new(ARCHIVE),
                Some(password),
            );
            assert_eq!(
                round_trip(&built.args, unrar_quote),
                strings(&built.args),
                "password {password:?}, {action:?}"
            );
            assert!(strings(&built.args).contains(&format!("-p{password}")));
        }
    }
}

/// Not just the examples: every string of up to five characters over the characters that
/// matter to either parser.
#[test]
fn unrar_quoting_round_trips_every_short_string() {
    let alphabet = ['a', ' ', '\t', '"', '\\'];
    let mut pending = vec![String::new()];
    let mut checked = 0;
    while let Some(prefix) = pending.pop() {
        if !prefix.is_empty() {
            let args = [OsString::from(&prefix), OsString::from("next")];
            assert_eq!(
                round_trip(&args, unrar_quote),
                [prefix.clone(), "next".to_owned()],
                "{prefix:?}"
            );
            checked += 1;
        }
        if prefix.chars().count() < 5 {
            pending.extend(
                alphabet
                    .iter()
                    .map(|character| format!("{prefix}{character}")),
            );
        }
    }
    assert_eq!(checked, 5 + 25 + 125 + 625 + 3125);
}

#[test]
fn the_log_line_names_every_argument_but_the_password() {
    let secret = r#"canary "secret" \ value"#;
    let built = rar_arguments(
        RarToolKind::Unrar,
        RarAction::Extract {
            staging: Path::new(STAGING),
        },
        Path::new(ARCHIVE),
        Some(secret),
    );
    let line = built.redacted();
    assert_eq!(
        line,
        format!("x -o- -y -idc -p*** -op{STAGING} -- {ARCHIVE}")
    );
    assert!(!line.contains("canary"));
    let none = rar_arguments(
        RarToolKind::Unrar,
        RarAction::Test,
        Path::new(ARCHIVE),
        None,
    );
    assert_eq!(none.redacted(), format!("t -y -idc -p- -- {ARCHIVE}"));
}

/// A writer the subscriber below appends to, so the test can read what was logged.
///
/// This and `capturing_subscriber` serve only the canary below, which needs `/bin/false` and so
/// is Linux-only; they carry its gate, or Windows and macOS clippy report them as dead code.
#[cfg(target_os = "linux")]
#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

#[cfg(target_os = "linux")]
impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("log buffer")).into_owned()
    }
}

#[cfg(target_os = "linux")]
fn capturing_subscriber(sink: &Captured) -> impl tracing::Subscriber + Send + Sync {
    let sink = sink.clone();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || sink.clone())
        .finish()
}

/// The canary: both steps log the tool, the arguments and the exit code, and the password of
/// neither reaches the log. `/bin/false` stands in for the tool - it takes any argument list and
/// exits 1, so both runs reach the line with the exit code.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_log_names_tool_arguments_and_exit_code_and_never_the_password() {
    use crate::{ArchiveLimits, ExternalRarTool, rar::extract_rar_into, test_rar};

    let canary = r#"CANARY-7f3a "q" \ x"#;
    let temp = tempfile::tempdir().expect("temporary directory");
    let staging = temp.path().join("a b").join(".rd-xabc");
    std::fs::create_dir_all(&staging).expect("staging");
    let archive = temp.path().join("a b").join("a.rar");
    let tool = ExternalRarTool {
        executable: "/bin/false".into(),
        kind: RarToolKind::Unrar,
        timeout: std::time::Duration::from_secs(30),
    };
    let sink = Captured::default();
    let _guard = tracing::subscriber::set_default(capturing_subscriber(&sink));
    assert!(test_rar(&tool, &archive, Some(canary)).await.is_err());
    assert!(
        extract_rar_into(
            &tool,
            &archive,
            &staging,
            ArchiveLimits::default(),
            Some(canary),
            None
        )
        .await
        .is_err()
    );
    let log = sink.text();
    assert!(!log.contains("CANARY"), "{log}");
    assert_eq!(log.matches("RAR tool started").count(), 2, "{log}");
    assert_eq!(log.matches("RAR tool finished").count(), 2, "{log}");
    assert_eq!(log.matches("tool=/bin/false").count(), 4, "{log}");
    assert_eq!(log.matches("exit=\"1\"").count(), 2, "{log}");
    assert!(log.contains("args=t -y -idc -p*** -- "), "{log}");
    assert!(
        log.contains(&format!(
            "args=x -o- -y -idc -p*** -op{} -- ",
            staging.display()
        )),
        "{log}"
    );
}

/// `a.rar`, 80 bytes, RAR 5, stored: one member `doc.pdf` holding `hello\n`. Written by RAR 7.23
/// for the RD-120-56 diagnosis; kept here as bytes rather than as a binary file in the tree.
const HELLO_RAR: [u8; 80] = [
    0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 0x01, 0x00, 0x33, 0x92, 0xb5, 0xe5, 0x0a, 0x01, 0x05, 0x06,
    0x00, 0x05, 0x01, 0x01, 0x80, 0x80, 0x00, 0x0e, 0xc3, 0x33, 0x31, 0x26, 0x02, 0x03, 0x0b, 0x86,
    0x00, 0x04, 0x86, 0x00, 0xa4, 0x83, 0x02, 0x20, 0x30, 0x3a, 0x36, 0x80, 0x00, 0x01, 0x08, 0x2f,
    0x64, 0x6f, 0x63, 0x2e, 0x70, 0x64, 0x66, 0x0a, 0x03, 0x13, 0xa6, 0xef, 0xb4, 0x6a, 0x1e, 0x13,
    0x1b, 0x0e, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x0a, 0x1d, 0x77, 0x56, 0x51, 0x03, 0x05, 0x04, 0x00,
];

/// A real `unrar` extracts into a folder with a space through `-op`. It needs the program, which
/// the build machines do not carry: `RD_TEST_UNRAR` names it (the vendor `unrar` of a package
/// build), otherwise `unrar` on `PATH`; without either the test says so and passes.
#[tokio::test]
async fn a_real_unrar_extracts_into_a_folder_with_a_space() {
    use crate::{ArchiveKind, ArchiveLimits, ArchiveSet, ExternalRarTool, ExtractRequest};

    let explicit = std::env::var("RD_TEST_UNRAR").ok();
    let Some(unrar) = rd_core::locate_tool(explicit.as_deref(), None, "unrar") else {
        eprintln!("skipped: no unrar (set RD_TEST_UNRAR)");
        return;
    };
    let temp = tempfile::tempdir().expect("temporary directory");
    let package = temp.path().join("Lust auf Genuss Nr 11 - November 2026");
    std::fs::create_dir_all(&package).expect("package");
    let archive = package.join("a.rar");
    std::fs::write(&archive, HELLO_RAR).expect("archive");
    let destination = package.join("out dir");
    let set = ArchiveSet {
        kind: ArchiveKind::Rar,
        base: "a".to_owned(),
        volumes: vec![archive],
    };
    let request = ExtractRequest {
        set: &set,
        destination: destination.clone(),
        limits: ArchiveLimits::default(),
        rar_tool: Some(ExternalRarTool {
            executable: unrar.path.clone(),
            kind: RarToolKind::Unrar,
            timeout: std::time::Duration::from_secs(60),
        }),
        merge: false,
        progress: None,
    };
    let (report, password) = crate::extract_with_passwords(request, &[None])
        .await
        .expect("extraction");
    eprintln!("extracted with {}", unrar.path.display());
    assert_eq!((report.files, password), (1, None));
    assert_eq!(
        std::fs::read(destination.join("doc.pdf")).expect("extracted member"),
        b"hello\n"
    );
}
