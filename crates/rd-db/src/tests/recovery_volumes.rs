//! PAR2 recovery volumes an NZB postpones until the index asks for them.

use rd_core::{ImportMode, IngressSource};

use super::nzb_file;
use crate::{Database, NewNzbImport};

/// A release with a payload, a main PAR2 index and three recovery volumes.
fn par2_release(digest: &str) -> NewNzbImport {
    NewNzbImport {
        name: "release.nzb".to_owned(),
        sha256: digest.repeat(32),
        category_id: None,
        priority: None,
        import_mode: ImportMode::Enqueue,
        source: IngressSource::Manual,
        source_path: None,
        password: None,
        announce_arrival: false,
        files: vec![
            nzb_file("payload.zip"),
            nzb_file("release.par2"),
            nzb_file("release.vol000+01.par2"),
            nzb_file("release.vol001+02.par2"),
            nzb_file("release.vol003+16.par2"),
        ],
    }
}

/// RD-107-04: the recovery volumes wait, the main index does not.
///
/// SABnzbd's `postpone_pars`. The index is what answers whether anything is damaged at all,
/// so it comes down with the payload; the volumes are the repair material and a package that
/// arrives intact should never pay for them.
#[tokio::test]
async fn queuing_an_nzb_postpones_its_recovery_volumes_but_not_its_index() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("par2.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(par2_release("c1"))
        .await
        .expect("import");

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let states: Vec<(String, rd_core::DownloadState)> = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .map(|file| (file.file_name, file.state))
        .collect();
    let state_of = |name: &str| {
        states
            .iter()
            .find(|(file_name, _)| file_name == name)
            .map(|(_, state)| *state)
            .unwrap_or_else(|| panic!("no row for {name}"))
    };
    assert_eq!(state_of("payload.zip"), rd_core::DownloadState::Queued);
    assert_eq!(state_of("release.par2"), rd_core::DownloadState::Queued);
    for volume in [
        "release.vol000+01.par2",
        "release.vol001+02.par2",
        "release.vol003+16.par2",
    ] {
        assert_eq!(
            state_of(volume),
            rd_core::DownloadState::Skipped,
            "{volume} should have been postponed"
        );
    }
}

/// RD-120-16: a postponed volume is not a mirror, and the queue has to be able to say so.
///
/// `Skipped` carries both meanings, and the one thing that separates them is the group key:
/// a mirror is only ever written as skipped together with the group it stands down for
/// (`rd_scheduler::control::stand_down_siblings_of` tests exactly that), while a postponed
/// recovery volume has no second source at all. The interface derives its wording from this,
/// so a group key appearing here would make it call a volume a mirror again.
#[tokio::test]
async fn a_postponed_recovery_volume_carries_no_mirror_group() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("par2-no-mirror.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(par2_release("c9"))
        .await
        .expect("import");

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let postponed: Vec<rd_core::DownloadFile> = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .filter(|file| file.state == rd_core::DownloadState::Skipped)
        .collect();
    assert_eq!(postponed.len(), 3, "the three volumes of the set");
    for file in postponed {
        assert!(
            file.mirror_group.is_none(),
            "{} waits for the repair, not for another link",
            file.file_name
        );
        assert!(file.recovery, "{} is repair data", file.file_name);
    }
}

/// RD-108-23: the subject form of the live finding - the release name quoted first, the file
/// name second. Every row used to be named after its whole subject line, so no row was PAR2,
/// nothing was postponed, and a lost volume counted as a package error.
#[tokio::test]
async fn queuing_an_nzb_whose_subjects_quote_the_release_first_names_and_postpones_its_rows() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("release-first.sqlite"))
        .await
        .expect("database");
    let subject = |index: usize, name: &str| {
        format!(
            "\"Starfight.1984.German.AC3.DL.1080p.BluRay.x265-FuN\" - [{index:02}/50] - \"{name}\" yEnc (1/13)"
        )
    };
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "Starfight.1984.German.AC3.DL.1080p.BluRay.x265-FuN.nzb".to_owned(),
            sha256: "c4".repeat(32),
            category_id: None,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source: IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![
                nzb_file(&subject(
                    1,
                    "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.part01.rar",
                )),
                nzb_file(&subject(43, "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.par2")),
                nzb_file(&subject(
                    44,
                    "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.vol03+04.par2",
                )),
                nzb_file(&subject(
                    45,
                    "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.vol07+08.par2",
                )),
            ],
        })
        .await
        .expect("import");

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let mut rows: Vec<(String, bool, rd_core::DownloadState)> = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .map(|file| (file.file_name, file.recovery, file.state))
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        rows,
        [
            (
                "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.par2".to_owned(),
                true,
                rd_core::DownloadState::Queued
            ),
            (
                "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.part01.rar".to_owned(),
                false,
                rd_core::DownloadState::Queued
            ),
            (
                "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.vol03+04.par2".to_owned(),
                true,
                rd_core::DownloadState::Skipped
            ),
            (
                "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.vol07+08.par2".to_owned(),
                true,
                rd_core::DownloadState::Skipped
            ),
        ]
    );
}

/// RD-108-23: the PAR2 decision is taken again when the real name arrives.
///
/// The index's subject announces no usable name, so at enqueue time nothing is postponed
/// (there is no index to verify with) and the row is named after its subject. Once the
/// assembled file settles the name, the row is marked, the set's waiting volumes are
/// postponed - and the volume that is already downloading is not touched, nor the payload.
#[tokio::test]
async fn settling_the_index_name_marks_the_row_and_postpones_the_waiting_volumes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("settle.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "late-index.nzb".to_owned(),
            sha256: "c5".repeat(32),
            category_id: None,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source: IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![
                nzb_file("payload.zip"),
                nzb_file("Release index without a name (1/3)"),
                nzb_file("release.vol000+01.par2"),
                nzb_file("release.vol001+02.par2"),
                nzb_file("release.vol003+16.par2"),
                nzb_file("other.vol000+01.par2"),
            ],
        })
        .await
        .expect("import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    let rows = || async {
        database
            .list_downloads()
            .await
            .expect("downloads")
            .into_iter()
            .filter(|file| file.package_id == package.id)
            .collect::<Vec<_>>()
    };
    let row_named = |rows: &[rd_core::DownloadFile], name: &str| {
        rows.iter()
            .find(|file| file.file_name == name)
            .cloned()
            .unwrap_or_else(|| panic!("no row for {name}"))
    };
    let queued = rows().await;
    assert!(
        queued
            .iter()
            .all(|file| file.state == rd_core::DownloadState::Queued),
        "without a recognisable index nothing is postponed at enqueue time"
    );
    let index = row_named(&queued, "Release index without a name (1_3)");
    assert!(!index.recovery, "the subject line says nothing about PAR2");
    let running = row_named(&queued, "release.vol001+02.par2");
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
    ] {
        database
            .transition_download(running.id, state)
            .await
            .expect("volume on its way");
    }

    let postponed = database
        .settle_nzb_recovery(index.id, "release.par2".to_owned(), false)
        .await
        .expect("settle");

    assert_eq!(postponed, 2, "the two waiting volumes of the set");
    let settled = rows().await;
    let index = row_named(&settled, "release.par2");
    assert!(index.recovery, "the settled name says PAR2");
    let state_of = |name: &str| row_named(&settled, name).state;
    assert_eq!(
        state_of("release.vol000+01.par2"),
        rd_core::DownloadState::Skipped
    );
    assert_eq!(
        state_of("release.vol003+16.par2"),
        rd_core::DownloadState::Skipped
    );
    assert_eq!(
        state_of("release.vol001+02.par2"),
        rd_core::DownloadState::Downloading,
        "a volume already on its way is never postponed retroactively"
    );
    assert_eq!(state_of("payload.zip"), rd_core::DownloadState::Queued);
    assert_eq!(
        state_of("other.vol000+01.par2"),
        rd_core::DownloadState::Queued,
        "a volume of another set is not this index's business"
    );
}

/// RD-108-23: content outranks the name, and the marking follows a rename.
#[tokio::test]
async fn settling_marks_par2_content_under_any_name_and_a_rename_decides_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("settle-content.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "obfuscated.nzb".to_owned(),
            sha256: "c6".repeat(32),
            category_id: None,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source: IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![nzb_file("a1b2c3.bin"), nzb_file("d4e5f6.bin")],
        })
        .await
        .expect("import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    let rows: Vec<rd_core::DownloadFile> = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .collect();
    let by_content = rows
        .iter()
        .find(|file| file.file_name == "a1b2c3.bin")
        .expect("first row");
    let by_rename = rows
        .iter()
        .find(|file| file.file_name == "d4e5f6.bin")
        .expect("second row");

    database
        .settle_nzb_recovery(by_content.id, "a1b2c3.bin".to_owned(), true)
        .await
        .expect("settle");
    let settled = database
        .get_download(by_content.id)
        .await
        .expect("row")
        .expect("row exists");
    assert!(
        settled.recovery,
        "a PAR2 header marks the row whatever its name"
    );

    let renamed = database
        .rename_download(by_rename.id, "d4e5f6.par2".to_owned())
        .await
        .expect("rename");
    assert!(renamed.recovery, "the marking follows the new name");
    let renamed = database
        .rename_download(by_rename.id, "d4e5f6.rar".to_owned())
        .await
        .expect("rename back");
    assert!(!renamed.recovery, "and follows it back");
}

/// RD-107-04: `enable_all_par` restores the behaviour of fetching every volume.
#[tokio::test]
async fn enable_all_par_queues_every_recovery_volume_with_the_payload() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("all-par.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "enable_all_par": true }),
        )
        .await
        .expect("settings");
    let import = database
        .add_nzb_import(par2_release("c2"))
        .await
        .expect("import");

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let postponed = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .filter(|file| file.state == rd_core::DownloadState::Skipped)
        .count();
    assert_eq!(postponed, 0, "nothing may be held back with the switch on");
}

/// A set whose recovery data is volumes only has nothing to verify with if they all wait.
#[tokio::test]
async fn an_nzb_without_a_main_index_postpones_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("no-index.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "volumes-only.nzb".to_owned(),
            sha256: "c3".repeat(32),
            category_id: None,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source: IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![nzb_file("payload.zip"), nzb_file("release.vol000+01.par2")],
        })
        .await
        .expect("import");

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let postponed = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .filter(|file| file.state == rd_core::DownloadState::Skipped)
        .count();
    assert_eq!(
        postponed, 0,
        "with no index to verify against, holding the volumes back leaves nothing to repair from"
    );
}
