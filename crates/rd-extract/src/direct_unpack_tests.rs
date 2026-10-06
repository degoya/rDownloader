//! Direct unpack through the whole service (RD-1100-07): a Usenet package whose RAR volumes
//! complete one after another, against a stand-in for `unrar` that asks for every further volume
//! the way `unrar -vp` does and records how it was started.

use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use rd_core::{DownloadId, DownloadState, PackageId, PackageState, PostprocessKind};
use rd_db::Database;

use crate::{ExtractionConfig, ExtractionService};

/// `unrar` in miniature. The first line of every volume says `n/total`, the rest is payload,
/// appended to `payload.bin` by `x`; `t` only reads. A volume holding `CORRUPT` fails like a
/// checksum error. Every start on an archive is logged with its arguments.
fn stand_in(directory: &Path) -> (PathBuf, PathBuf) {
    let executable = directory.join("unrar");
    let log = directory.join("unrar.log");
    std::fs::write(
        &executable,
        format!(
            r#"#!/bin/sh
[ $# -eq 0 ] && {{ echo 'UNRAR 7.01 freeware      Copyright (c) 1993-2024 Alexander Roshal'; exit 0; }}
echo "$*" >> '{log}'
command="$1"
for arg in "$@"; do
  case "$arg" in -op*) out="${{arg#-op}}" ;; esac
  last="$arg"
done
base="${{last%.part1.rar}}"
pause=0
for arg in "$@"; do [ "$arg" = "-vp" ] && pause=1; done
total=$(head -n 1 "$last" | cut -d/ -f2)
[ "$command" = x ] && : > "$out/payload.bin"
n=1
while [ "$n" -le "$total" ]; do
  volume="$base.part$n.rar"
  if [ "$n" -gt 1 ] && [ "$pause" = 1 ]; then
    printf '\nInsert disk with %s\n [C]ontinue, [Q]uit ' "$volume" >&2
    read answer || exit 255
    [ "$answer" = C ] || exit 255
  fi
  [ -f "$volume" ] || {{ echo "Cannot find volume $volume" >&2; exit 10; }}
  if grep -q CORRUPT "$volume"; then echo "$volume : packed data checksum error" >&2; exit 3; fi
  [ "$command" = x ] && tail -n +2 "$volume" >> "$out/payload.bin"
  n=$((n + 1))
done
echo "All OK"
"#,
            log = log.display()
        ),
    )
    .expect("stand-in");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    (executable, log)
}

async fn use_settings(database: &Database, unrar: &Path, extra: serde_json::Value) {
    let mut settings = serde_json::json!({ "rar_tool": "unrar", "rar_executable": unrar });
    if let (Some(settings), Some(extra)) = (settings.as_object_mut(), extra.as_object()) {
        settings.extend(extra.clone());
    }
    database
        .set_setting("service.settings".to_owned(), settings)
        .await
        .expect("settings");
}

fn config(temp: &Path) -> ExtractionConfig {
    ExtractionConfig {
        default_passwords_file: temp.join("passwords.txt"),
        rar_timeout: Duration::from_secs(10),
        default_scripts_directory: temp.join("scripts"),
        hold: rd_core::PostprocessHold::new(),
        quiet_hold: rd_core::PostprocessHold::new(),
        upload_limit: None,
    }
}

/// A queued Usenet package holding `names`, nothing of it on disk yet.
async fn seed(
    database: &Database,
    root: &Path,
    label: &str,
    names: &[&str],
) -> (PackageId, PathBuf, HashMap<String, DownloadId>) {
    let file = |subject: &str| rd_db::NewNzbFile {
        subject: subject.to_owned(),
        poster: "fixture".to_owned(),
        groups: vec!["alt.binaries.test".to_owned()],
        segments: vec![rd_db::NewNzbSegment {
            number: 1,
            bytes: 64,
            message_id: format!("{label}-{subject}@example.test"),
        }],
    };
    let import = database
        .add_nzb_import(rd_db::NewNzbImport {
            name: format!("{label}.nzb"),
            sha256: format!("{:0>64}", label.len()),
            category_id: None,
            source: rd_core::IngressSource::Manual,
            priority: None,
            import_mode: rd_core::ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: names.iter().map(|name| file(name)).collect(),
        })
        .await
        .expect("NZB import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            root.to_path_buf(),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue as package");
    assert_eq!(package.kind, rd_core::DownloadKind::Usenet);
    let destination = PathBuf::from(&package.destination);
    std::fs::create_dir_all(&destination).expect("destination");
    let ids = database
        .downloads_for_package(package.id)
        .await
        .expect("downloads")
        .into_iter()
        .map(|download| (download.file_name, download.id))
        .collect();
    (package.id, destination, ids)
}

fn write_volume(destination: &Path, index: usize, total: usize, payload: &str) {
    std::fs::write(
        destination.join(format!("Film.part{index}.rar")),
        format!("{index}/{total}\n{payload}\n"),
    )
    .expect("volume");
}

/// The way a complete Usenet file arrives: through `Verifying` to `Completed`.
async fn arrive(database: &Database, id: DownloadId) {
    for state in [
        DownloadState::Resolving,
        DownloadState::Downloading,
        DownloadState::Verifying,
        DownloadState::Completed,
    ] {
        database
            .transition_download(id, state)
            .await
            .expect("transition");
    }
}

fn direct_staging(destination: &Path) -> Vec<String> {
    std::fs::read_dir(destination)
        .expect("package folder")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(rd_postprocess::STAGING_PREFIX))
        .collect()
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

async fn finished(database: &Database, package_id: PackageId) -> PackageState {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let state = database
                .get_package(package_id)
                .await
                .expect("package")
                .expect("package exists")
                .state;
            if matches!(state, PackageState::Completed | PackageState::Failed) {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the package left post-processing")
}

async fn unpack_codes(database: &Database, package_id: PackageId) -> Vec<Option<String>> {
    database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps")
        .into_iter()
        .filter(|step| step.kind == PostprocessKind::ExtractRar)
        .map(|step| step.code)
        .collect()
}

fn started(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

const VOLUMES: [&str; 3] = ["Film.part1.rar", "Film.part2.rar", "Film.part3.rar"];
const PAYLOAD: [&str; 3] = ["one", "two", "three"];

/// The set is unpacked while it downloads, one volume after the other as each arrives, and the
/// pipeline only moves it into place: no second unpack, no RAR test, and the bytes the normal
/// way writes for the same volumes.
#[tokio::test]
async fn a_set_is_unpacked_while_it_downloads_and_matches_the_normal_way() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (unrar, log) = stand_in(temp.path());
    use_settings(
        &database,
        &unrar,
        serde_json::json!({ "direct_unpack": true }),
    )
    .await;
    let service = ExtractionService::start(database.clone(), config(temp.path()));

    let (direct, directory, ids) = seed(&database, &temp.path().join("a"), "a", &VOLUMES).await;
    write_volume(&directory, 1, 3, PAYLOAD[0]);
    arrive(&database, ids[VOLUMES[0]]).await;
    eventually("the direct unpack started on the first volume", || {
        !direct_staging(&directory).is_empty()
    })
    .await;
    // Waiting for the second volume: nothing is in the package yet.
    assert!(!directory.join("payload.bin").exists());
    for (index, payload) in PAYLOAD.iter().enumerate().skip(1) {
        write_volume(&directory, index + 1, 3, payload);
        arrive(&database, ids[VOLUMES[index]]).await;
    }
    assert_eq!(finished(&database, direct).await, PackageState::Completed);
    let directly = std::fs::read(directory.join("payload.bin")).expect("payload");
    assert_eq!(directly, b"one\ntwo\nthree\n");
    assert!(
        direct_staging(&directory).is_empty(),
        "{:?}",
        direct_staging(&directory)
    );
    assert_eq!(
        unpack_codes(&database, direct).await,
        [Some("postprocess.unpack_completed_direct".to_owned())]
    );
    let runs = started(&log);
    assert_eq!(runs.len(), 1, "one start, the direct one: {runs:?}");
    assert!(runs[0].starts_with("x -vp"), "{runs:?}");

    // The same volumes the normal way.
    use_settings(
        &database,
        &unrar,
        serde_json::json!({ "direct_unpack": false }),
    )
    .await;
    let (normal, other, ids) = seed(&database, &temp.path().join("b"), "bb", &VOLUMES).await;
    for (index, payload) in PAYLOAD.iter().enumerate() {
        write_volume(&other, index + 1, 3, payload);
        arrive(&database, ids[VOLUMES[index]]).await;
    }
    assert_eq!(finished(&database, normal).await, PackageState::Completed);
    assert_eq!(
        std::fs::read(other.join("payload.bin")).expect("payload"),
        directly
    );
    assert_eq!(
        unpack_codes(&database, normal).await,
        [Some("postprocess.unpack_completed".to_owned())]
    );
    service.shutdown().await;
}

/// A volume that came without some of its articles is never handed to the tool: the attempt is
/// given up and leaves nothing behind, and once the volume is whole — the repair, here done by
/// hand — the set is unpacked the normal way.
#[tokio::test]
async fn a_volume_missing_articles_gives_the_attempt_up_and_the_normal_way_unpacks() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (unrar, log) = stand_in(temp.path());
    use_settings(
        &database,
        &unrar,
        serde_json::json!({ "direct_unpack": true }),
    )
    .await;
    let service = ExtractionService::start(database.clone(), config(temp.path()));
    let (package, directory, ids) = seed(&database, &temp.path().join("a"), "a", &VOLUMES).await;
    write_volume(&directory, 1, 3, PAYLOAD[0]);
    arrive(&database, ids[VOLUMES[0]]).await;
    eventually("the direct unpack started", || {
        !direct_staging(&directory).is_empty()
    })
    .await;

    // What the Usenet runner does with a file some articles of which no server had.
    let damaged = ids[VOLUMES[1]];
    write_volume(&directory, 2, 3, "CORRUPT");
    for state in [DownloadState::Resolving, DownloadState::Downloading] {
        database
            .transition_download(damaged, state)
            .await
            .expect("transition");
    }
    database
        .defer_par2_verdict(damaged, 2)
        .await
        .expect("missing articles");
    database
        .transition_download(damaged, DownloadState::Verifying)
        .await
        .expect("transition");
    eventually("the attempt was given up and its staging removed", || {
        direct_staging(&directory).is_empty()
    })
    .await;
    assert!(!directory.join("payload.bin").exists());

    write_volume(&directory, 2, 3, PAYLOAD[1]);
    database
        .transition_download(damaged, DownloadState::Completed)
        .await
        .expect("repaired");
    write_volume(&directory, 3, 3, PAYLOAD[2]);
    arrive(&database, ids[VOLUMES[2]]).await;
    assert_eq!(finished(&database, package).await, PackageState::Completed);
    assert_eq!(
        std::fs::read(directory.join("payload.bin")).expect("payload"),
        b"one\ntwo\nthree\n"
    );
    assert_eq!(
        unpack_codes(&database, package).await,
        [Some("postprocess.unpack_completed".to_owned())]
    );
    let runs = started(&log);
    assert!(runs.iter().any(|run| run.starts_with("x -o-")), "{runs:?}");
    assert!(direct_staging(&directory).is_empty());
    service.shutdown().await;
}

/// A pause in the middle gives the attempt up: no half file in the package, no staging left,
/// and after the resume the set is unpacked the normal way.
#[tokio::test]
async fn a_pause_gives_the_attempt_up_without_leaving_half_a_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (unrar, _log) = stand_in(temp.path());
    use_settings(
        &database,
        &unrar,
        serde_json::json!({ "direct_unpack": true }),
    )
    .await;
    let service = ExtractionService::start(database.clone(), config(temp.path()));
    let (package, directory, ids) = seed(&database, &temp.path().join("a"), "a", &VOLUMES).await;
    write_volume(&directory, 1, 3, PAYLOAD[0]);
    arrive(&database, ids[VOLUMES[0]]).await;
    eventually("the direct unpack started", || {
        !direct_staging(&directory).is_empty()
    })
    .await;

    let paused = ids[VOLUMES[1]];
    database
        .transition_download(paused, DownloadState::Paused)
        .await
        .expect("pause");
    eventually("the attempt was given up and its staging removed", || {
        direct_staging(&directory).is_empty()
    })
    .await;
    assert!(!directory.join("payload.bin").exists());

    database
        .transition_download(paused, DownloadState::Queued)
        .await
        .expect("resume");
    for (index, payload) in PAYLOAD.iter().enumerate().skip(1) {
        write_volume(&directory, index + 1, 3, payload);
        arrive(&database, ids[VOLUMES[index]]).await;
    }
    assert_eq!(finished(&database, package).await, PackageState::Completed);
    assert_eq!(
        std::fs::read(directory.join("payload.bin")).expect("payload"),
        b"one\ntwo\nthree\n"
    );
    assert_eq!(
        unpack_codes(&database, package).await,
        [Some("postprocess.unpack_completed".to_owned())]
    );
    service.shutdown().await;
}

/// A verification that fails keeps a direct unpack out, even when the package is unpacked
/// anyway: the set is unpacked again from the volumes, and the staging goes.
#[tokio::test]
async fn a_failed_verification_never_lets_a_direct_unpack_in() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (unrar, log) = stand_in(temp.path());
    use_settings(
        &database,
        &unrar,
        serde_json::json!({ "direct_unpack": true, "safe_postproc": false }),
    )
    .await;
    let service = ExtractionService::start(database.clone(), config(temp.path()));
    let mut names = VOLUMES.to_vec();
    names.push("Film.sfv");
    let (package, directory, ids) = seed(&database, &temp.path().join("a"), "a", &names).await;
    std::fs::write(directory.join("Film.sfv"), "Film.part2.rar 00000000\n").expect("index");
    arrive(&database, ids["Film.sfv"]).await;
    write_volume(&directory, 1, 3, PAYLOAD[0]);
    arrive(&database, ids[VOLUMES[0]]).await;
    eventually("the direct unpack started", || {
        !direct_staging(&directory).is_empty()
    })
    .await;
    for (index, payload) in PAYLOAD.iter().enumerate().skip(1) {
        write_volume(&directory, index + 1, 3, payload);
        arrive(&database, ids[VOLUMES[index]]).await;
    }
    finished(&database, package).await;
    assert_eq!(
        unpack_codes(&database, package).await,
        [Some("postprocess.unpack_completed".to_owned())]
    );
    let runs = started(&log);
    assert!(runs[0].starts_with("x -vp"), "{runs:?}");
    assert!(runs.iter().any(|run| run.starts_with("x -o-")), "{runs:?}");
    assert!(
        direct_staging(&directory).is_empty(),
        "{:?}",
        direct_staging(&directory)
    );
    service.shutdown().await;
}

/// `postprocess.before_direct_unpack_adopted`: the set was unpacked while the package
/// downloaded, and the pipeline stopped before it moved it into place.
///
/// The restart knows nothing of that unpack: it removes its staging directory — and the one a
/// kill inside the tool would leave, planted here — and unpacks the set the normal way.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_direct_unpack_crashed_before_it_was_moved_in_is_unpacked_again_by_the_next_start() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (unrar, log) = stand_in(temp.path());
    use_settings(
        &database,
        &unrar,
        serde_json::json!({ "direct_unpack": true }),
    )
    .await;
    let (package, directory, ids) = seed(&database, &temp.path().join("a"), "a", &VOLUMES).await;
    {
        // The pipeline without its job loop, which would record the crash as a failure.
        let inner = crate::tests::extraction_inner(&database, temp.path());
        write_volume(&directory, 1, 3, PAYLOAD[0]);
        arrive(&database, ids[VOLUMES[0]]).await;
        crate::direct_unpack::on_volume_completed(&inner, ids[VOLUMES[0]]).await;
        for (index, payload) in PAYLOAD.iter().enumerate().skip(1) {
            write_volume(&directory, index + 1, 3, payload);
            arrive(&database, ids[VOLUMES[index]]).await;
        }
        let guard =
            rd_core::failpoint::FailpointGuard::once("postprocess.before_direct_unpack_adopted");
        let result =
            crate::package_job::run_package(&inner, package, crate::ExtractionTrigger::Auto).await;
        assert!(result.is_err(), "the pipeline ran past its crash point");
        assert!(guard.fired(), "the crash point was never reached");
    }
    // What the stop left: the set in its staging directory, nothing at the destination.
    assert_eq!(direct_staging(&directory).len(), 1);
    assert!(!directory.join("payload.bin").exists());
    let killed = directory.join(format!("{}killed", rd_postprocess::DIRECT_STAGING_PREFIX));
    std::fs::create_dir_all(&killed).expect("staging");
    std::fs::write(killed.join("payload.bin"), b"on").expect("partial output");

    // What every start does before the services come up, then the restart itself.
    database.recover_interrupted().await.expect("recover rows");
    let service = ExtractionService::start(database.clone(), config(temp.path()));
    service.recover().await.expect("recover");
    assert_eq!(finished(&database, package).await, PackageState::Completed);
    crate::tests::wait_until_finished(&service, package).await;
    service.shutdown().await;

    assert_eq!(
        std::fs::read(directory.join("payload.bin")).expect("payload"),
        b"one\ntwo\nthree\n"
    );
    assert!(
        direct_staging(&directory).is_empty(),
        "{:?}",
        direct_staging(&directory)
    );
    assert_eq!(
        unpack_codes(&database, package).await,
        [Some("postprocess.unpack_completed".to_owned())]
    );
    assert!(started(&log).iter().any(|run| run.starts_with("x -o-")));
}
