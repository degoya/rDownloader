//! RD-1420-03 (audit L-12): `unrar` reads the archive password from its standard input, so it
//! is not on the command line that every process of the machine can read through `/proc`.
//! 7-Zip has no such reader and keeps `-p<secret>`.

use std::path::Path;

use crate::{
    RarToolKind,
    rar_args::{RarAction, rar_arguments_for, reads_password_from_stdin},
    rar_args_tests::strings,
};

const STAGING: &str = "/pkg/.rd-xabc";
const ARCHIVE: &str = "/pkg/a.part1.rar";
const SECRET: &str = "p\u{e4}ss \"w\\ord";

fn actions() -> [RarAction<'static>; 3] {
    [
        RarAction::Extract {
            staging: Path::new(STAGING),
        },
        RarAction::Follow {
            staging: Path::new(STAGING),
        },
        RarAction::Test,
    ]
}

#[test]
fn unrar_arguments_carry_no_password() {
    for action in actions() {
        for password in [SECRET, "plain", "x\" -spf -w\""] {
            let built = rar_arguments_for(
                RarToolKind::Unrar,
                action,
                Path::new(ARCHIVE),
                Some(password),
                false,
            );
            let args = strings(&built.args);
            assert!(built.password_on_stdin, "{action:?}");
            assert!(args.contains(&"-p".to_owned()), "{args:?}");
            assert!(
                args.iter().all(|arg| !arg.contains(password)),
                "{action:?}: {args:?}"
            );
            assert!(built.redacted().contains(" -p<stdin> "), "{action:?}");
        }
    }
}

/// Under Windows `unrar` decodes its input through the ANSI code page: an ASCII password goes
/// there all the same, a non-ASCII one stays on the command line, where it arrives intact.
#[test]
fn under_windows_only_an_ascii_password_goes_to_stdin() {
    let ascii = rar_arguments_for(
        RarToolKind::Unrar,
        RarAction::Test,
        Path::new(ARCHIVE),
        Some("x\" -spf -w\""),
        true,
    );
    assert!(ascii.password_on_stdin);
    assert_eq!(
        strings(&ascii.args),
        ["t", "-y", "-idc", "-p", "--", ARCHIVE]
    );
    let umlaut = rar_arguments_for(
        RarToolKind::Unrar,
        RarAction::Test,
        Path::new(ARCHIVE),
        Some(SECRET),
        true,
    );
    assert!(!umlaut.password_on_stdin);
    assert!(strings(&umlaut.args).contains(&format!("-p{SECRET}")));
    assert!(umlaut.redacted().contains(" -p*** "));
}

#[test]
fn a_password_one_line_cannot_carry_stays_on_the_command_line() {
    let long = "a".repeat(501);
    for password in [
        "",
        "two\nlines",
        "carriage\rreturn",
        "nul\0byte",
        long.as_str(),
    ] {
        assert!(
            !reads_password_from_stdin(RarToolKind::Unrar, password, false),
            "{password:?}"
        );
    }
    assert!(reads_password_from_stdin(
        RarToolKind::Unrar,
        &"a".repeat(500),
        false
    ));
    // 7-Zip's bare `-p` is the empty password, not a question.
    for windows in [false, true] {
        assert!(!reads_password_from_stdin(
            RarToolKind::SevenZip,
            "plain",
            windows
        ));
        let seven = rar_arguments_for(
            RarToolKind::SevenZip,
            RarAction::Test,
            Path::new(ARCHIVE),
            Some("plain"),
            windows,
        );
        assert!(!seven.password_on_stdin);
        assert!(strings(&seven.args).contains(&"-pplain".to_owned()));
    }
}

/// Against a real process: a stand-in `unrar` records its arguments and what arrives on its
/// standard input, for the unpack, the integrity test and direct unpack.
#[cfg(unix)]
mod with_a_stand_in {
    use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

    use tokio::sync::mpsc;

    use super::SECRET;
    use crate::{
        ArchiveLimits, DirectRequest, ExternalRarTool, RarToolKind, extract_rar_direct,
        rar::extract_rar_into, test_rar,
    };

    /// Writes its arguments to `args-<command>` and the first input line to `stdin-<command>`
    /// beside itself; anything after that line lands in `extra-<command>`. With `-vp` it asks for
    /// a second volume once, the way `unrar` does, and wants `C` for it.
    const STAND_IN: &str = r#"#!/bin/sh
dir="${0%/*}"
printf '%s\n' "$@" > "$dir/args-$1"
IFS= read -r password || exit 255
printf '%s' "$password" > "$dir/stdin-$1"
follow=0
for arg in "$@"; do
  case "$arg" in -op*) out="${arg#-op}" ;; -vp) follow=1 ;; esac
done
if [ "$follow" = 1 ]; then
  printf '\nInsert disk with %s\n [C]ontinue, [Q]uit ' "a.part2.rar" >&2
  read answer || exit 255
  [ "$answer" = C ] || exit 255
elif IFS= read -r extra; then
  printf '%s' "$extra" > "$dir/extra-$1"
fi
[ -n "$out" ] && echo hello > "$out/doc.txt"
echo "All OK"
"#;

    fn stand_in(directory: &Path) -> ExternalRarTool {
        let executable = directory.join("unrar");
        std::fs::write(&executable, STAND_IN).expect("stand-in");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
        ExternalRarTool {
            executable,
            kind: RarToolKind::Unrar,
            timeout: Duration::from_secs(10),
        }
    }

    fn seen(directory: &Path, name: &str) -> String {
        std::fs::read_to_string(directory.join(name)).unwrap_or_default()
    }

    #[tokio::test]
    async fn the_password_arrives_on_stdin_and_never_as_an_argument() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = stand_in(temp.path());
        let archive = temp.path().join("a.part1.rar");
        std::fs::write(&archive, b"stand-in").expect("archive");
        let staging = temp.path().join(".rd-xabc");
        std::fs::create_dir_all(&staging).expect("staging");

        test_rar(&tool, &archive, Some(SECRET))
            .await
            .expect("integrity test");
        extract_rar_into(
            &tool,
            &archive,
            &staging,
            ArchiveLimits::default(),
            Some(SECRET),
            None,
        )
        .await
        .expect("unpack");

        for command in ["t", "x"] {
            assert_eq!(seen(temp.path(), &format!("stdin-{command}")), SECRET);
            let args = seen(temp.path(), &format!("args-{command}"));
            assert!(args.lines().any(|arg| arg == "-p"), "{args}");
            assert!(!args.contains("ss \"w"), "{command}: {args}");
            // One line and the end of input: nothing else is written there.
            assert_eq!(seen(temp.path(), &format!("extra-{command}")), "");
        }
        assert_eq!(seen(&staging, "doc.txt"), "hello\n");
    }

    /// Direct unpack keeps the pipe for the volume questions: the password is its first line,
    /// the answer to the question the next one.
    #[tokio::test]
    async fn direct_unpack_sends_the_password_before_any_answer() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = stand_in(temp.path());
        let package = temp.path().join("dl");
        std::fs::create_dir_all(&package).expect("package");
        let first = package.join("a.part1.rar");
        std::fs::write(&first, b"stand-in").expect("archive");
        let (asks, mut questions) = mpsc::channel::<crate::direct::VolumeAsk>(1);
        tokio::spawn(async move {
            while let Some(ask) = questions.recv().await {
                let _ = ask.reply.send(true);
            }
        });
        let staged = extract_rar_direct(DirectRequest {
            tool: &tool,
            first_volume: &first,
            parent: &package,
            limits: ArchiveLimits::default(),
            password: Some(SECRET),
            asks,
        })
        .await
        .expect("direct unpack");
        assert_eq!(staged.volumes, ["a.part1.rar", "a.part2.rar"]);
        assert_eq!(seen(temp.path(), "stdin-x"), SECRET);
        let args = seen(temp.path(), "args-x");
        assert!(args.lines().any(|arg| arg == "-vp"), "{args}");
        assert!(!args.contains("ss \"w"), "{args}");
        assert_eq!(seen(&staged.staging, "doc.txt"), "hello\n");
    }
}
