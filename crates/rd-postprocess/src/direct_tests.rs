//! RD-1100-07: direct unpack's dialogue with `unrar -vp`, against a stand-in that asks the way
//! `unrar` does (`Insert disk with …`, `[C]ontinue, [Q]uit` on stderr) and reads the answer from
//! stdin.

use std::path::Path;

use crate::{
    RarToolKind,
    direct::{asked_volume, keep_tail},
    rar_args::{RarAction, rar_arguments},
};

#[test]
fn the_question_is_recognised_once_it_has_been_printed_whole() {
    assert_eq!(
        asked_volume("\nInsert disk with /dl/Film/Film.part2.rar\n [C]ontinue, [Q]uit "),
        Some("Film.part2.rar".to_owned())
    );
    assert_eq!(
        asked_volume(
            "Extracting  a.bin  OK\nInsert disk with D:\\dl\\Film 2.part03.rar [C]ontinue"
        ),
        Some("Film 2.part03.rar".to_owned())
    );
    // Half a question is no question yet: the choice has not been printed.
    assert_eq!(
        asked_volume("\nInsert disk with /dl/Film.part2.rar\n"),
        None
    );
    assert_eq!(asked_volume("Extracting  a.bin  OK"), None);
    // The last question counts, not one that was already answered.
    assert_eq!(
        asked_volume(
            "Insert disk with a.part2.rar [C]ontinue, [Q]uit C\nInsert disk with a.part3.rar\n [C]ontinue"
        ),
        Some("a.part3.rar".to_owned())
    );
}

#[test]
fn the_kept_console_never_cuts_a_character_in_two() {
    let mut text = "\u{e4}".repeat(10);
    keep_tail(&mut text, 5);
    assert!(text.len() <= 6, "{text}");
    assert!(text.chars().all(|c| c == '\u{e4}'));
    let mut short = "abc".to_owned();
    keep_tail(&mut short, 5);
    assert_eq!(short, "abc");
}

#[test]
fn a_direct_unpack_pauses_before_every_volume_and_never_answers_for_itself() {
    let arguments = rar_arguments(
        RarToolKind::Unrar,
        RarAction::Follow {
            staging: Path::new("/dl/.rd-xdabc"),
        },
        Path::new("/dl/Film.part1.rar"),
        None,
    );
    let args: Vec<String> = arguments
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "x");
    assert!(args.contains(&"-vp".to_owned()), "{args:?}");
    // `-y` would answer the volume question before the caller could.
    assert!(!args.contains(&"-y".to_owned()), "{args:?}");
    assert!(args.contains(&"-o-".to_owned()), "{args:?}");
    assert!(args.contains(&"-p-".to_owned()), "{args:?}");
    assert!(args.contains(&"-op/dl/.rd-xdabc".to_owned()), "{args:?}");
    assert_eq!(args.last().map(String::as_str), Some("/dl/Film.part1.rar"));
}

#[cfg(unix)]
mod with_a_stand_in {
    use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

    use tokio::sync::mpsc;

    use crate::{
        ArchiveLimits, DirectRequest, ExternalRarTool, RarToolKind, adopt_direct,
        extract_rar_direct,
    };

    /// `unrar x -vp` in miniature: the first line of every volume says `n/total`, the rest is
    /// payload appended to `payload.bin`; a volume holding `CORRUPT` fails like a checksum error.
    const STAND_IN: &str = r#"#!/bin/sh
for arg in "$@"; do
  case "$arg" in -op*) out="${arg#-op}" ;; esac
  last="$arg"
done
base="${last%.part1.rar}"
pause=0
for arg in "$@"; do [ "$arg" = "-vp" ] && pause=1; done
total=$(head -n 1 "$last" | cut -d/ -f2)
: > "$out/payload.bin"
n=1
while [ "$n" -le "$total" ]; do
  volume="$base.part$n.rar"
  if [ "$n" -gt 1 ] && [ "$pause" = 1 ]; then
    printf '\nInsert disk with %s\n [C]ontinue, [Q]uit ' "$volume" >&2
    read answer || exit 255
    [ "$answer" = C ] || exit 255
  fi
  [ -f "$volume" ] || { echo "Cannot find volume $volume" >&2; exit 10; }
  if grep -q CORRUPT "$volume"; then echo "$volume : packed data checksum error" >&2; exit 3; fi
  tail -n +2 "$volume" >> "$out/payload.bin"
  n=$((n + 1))
done
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

    fn volume(directory: &Path, index: usize, total: usize, payload: &str) {
        std::fs::write(
            directory.join(format!("Film.part{index}.rar")),
            format!("{index}/{total}\n{payload}\n"),
        )
        .expect("volume");
    }

    fn staging_left(directory: &Path) -> Vec<String> {
        std::fs::read_dir(directory)
            .expect("package folder")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(crate::DIRECT_STAGING_PREFIX))
            .collect()
    }

    /// The second and third volumes are written only when the tool asks for them, which is
    /// what a download that is still running looks like from here.
    #[tokio::test]
    async fn every_volume_is_handed_over_when_it_is_asked_for_and_the_tree_is_moved_in_whole() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = stand_in(temp.path());
        let package = temp.path().join("dl");
        std::fs::create_dir_all(&package).expect("package");
        volume(&package, 1, 3, "one");
        let (asks, mut questions) = mpsc::channel::<crate::direct::VolumeAsk>(1);
        let answering = {
            let package = package.clone();
            tokio::spawn(async move {
                let mut asked = Vec::new();
                while let Some(ask) = questions.recv().await {
                    let index = asked.len() + 2;
                    volume(&package, index, 3, ["two", "three"][index - 2]);
                    asked.push(ask.volume.clone());
                    let _ = ask.reply.send(true);
                }
                asked
            })
        };
        let first = package.join("Film.part1.rar");
        let staged = extract_rar_direct(DirectRequest {
            tool: &tool,
            first_volume: &first,
            parent: &package,
            limits: ArchiveLimits::default(),
            password: None,
            asks,
        })
        .await
        .expect("direct unpack");
        assert_eq!(
            answering.await.expect("answers"),
            ["Film.part2.rar", "Film.part3.rar"]
        );
        assert_eq!(
            staged.volumes,
            ["Film.part1.rar", "Film.part2.rar", "Film.part3.rar"]
        );
        // Nothing is in the package before it is adopted.
        assert!(!package.join("payload.bin").exists());
        adopt_direct(&staged.staging, &package).expect("adopt");
        assert_eq!(
            std::fs::read_to_string(package.join("payload.bin")).expect("payload"),
            "one\ntwo\nthree\n"
        );
        assert!(
            staging_left(&package).is_empty(),
            "{:?}",
            staging_left(&package)
        );
    }

    /// A volume that will not come intact is answered with no: the tool stops, and what it had
    /// written so far goes with its staging directory.
    #[tokio::test]
    async fn a_refused_volume_stops_the_tool_and_leaves_nothing_behind() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = stand_in(temp.path());
        let package = temp.path().join("dl");
        std::fs::create_dir_all(&package).expect("package");
        volume(&package, 1, 2, "one");
        let (asks, mut questions) = mpsc::channel::<crate::direct::VolumeAsk>(1);
        tokio::spawn(async move {
            while let Some(ask) = questions.recv().await {
                let _ = ask.reply.send(false);
            }
        });
        let first = package.join("Film.part1.rar");
        let refused = extract_rar_direct(DirectRequest {
            tool: &tool,
            first_volume: &first,
            parent: &package,
            limits: ArchiveLimits::default(),
            password: None,
            asks,
        })
        .await;
        assert!(refused.is_err());
        assert!(
            staging_left(&package).is_empty(),
            "{:?}",
            staging_left(&package)
        );
        assert!(!package.join("payload.bin").exists());
    }

    /// A volume the tool cannot read is the tool's verdict, classified like an ordinary unpack's.
    #[tokio::test]
    async fn a_damaged_volume_fails_the_attempt_with_the_tools_verdict() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = stand_in(temp.path());
        let package = temp.path().join("dl");
        std::fs::create_dir_all(&package).expect("package");
        volume(&package, 1, 2, "one");
        volume(&package, 2, 2, "CORRUPT");
        let (asks, mut questions) = mpsc::channel::<crate::direct::VolumeAsk>(1);
        tokio::spawn(async move {
            while let Some(ask) = questions.recv().await {
                let _ = ask.reply.send(true);
            }
        });
        let first = package.join("Film.part1.rar");
        let damaged = extract_rar_direct(DirectRequest {
            tool: &tool,
            first_volume: &first,
            parent: &package,
            limits: ArchiveLimits::default(),
            password: None,
            asks,
        })
        .await;
        assert!(damaged.is_err());
        assert!(
            staging_left(&package).is_empty(),
            "{:?}",
            staging_left(&package)
        );
    }

    /// 7-Zip has no volume pause; it is refused before a staging directory exists.
    #[tokio::test]
    async fn seven_zip_is_refused_before_anything_starts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = ExternalRarTool {
            executable: temp.path().join("7z"),
            kind: RarToolKind::SevenZip,
            timeout: Duration::from_secs(1),
        };
        let (asks, _questions) = mpsc::channel::<crate::direct::VolumeAsk>(1);
        let first = temp.path().join("Film.part1.rar");
        let refused = extract_rar_direct(DirectRequest {
            tool: &tool,
            first_volume: &first,
            parent: temp.path(),
            limits: ArchiveLimits::default(),
            password: None,
            asks,
        })
        .await;
        assert!(matches!(
            refused,
            Err(crate::ExtractionError::Unsupported(_))
        ));
        assert!(staging_left(temp.path()).is_empty());
    }
}
