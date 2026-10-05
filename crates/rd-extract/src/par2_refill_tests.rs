//! RD-107-04: a repair short of blocks fetches the postponed recovery volumes and comes back.

use rd_core::{DownloadState, PackageId, PostprocessKind, PostprocessState};
use rd_db::Database;
use zip::write::SimpleFileOptions;

use crate::{
    ExtractionTrigger,
    tests::{extraction_inner, write_zip},
};

// ---------------------------------------------------------------------------
// RD-107-04: postponing the recovery volumes and the way back into downloading
// ---------------------------------------------------------------------------

/// One PAR2 packet: the magic, the total length, the MD5 of everything after it, then that
/// payload (the recovery set id, the packet type and the type's own body).
///
/// The digest is passed in rather than computed: this crate has no MD5 implementation and is
/// not going to grow one for a fixture. A payload edited without its digest is skipped by the
/// parser, which makes the whole index unreadable and fails the test loudly rather than
/// quietly testing something else.
fn par2_packet(payload: &[u8], digest: [u8; 16]) -> Vec<u8> {
    let mut packet = Vec::from(*b"PAR2\0PKT");
    let length = 32_u64 + payload.len() as u64;
    packet.extend_from_slice(&length.to_le_bytes());
    packet.extend_from_slice(&digest);
    packet.extend_from_slice(payload);
    packet
}

/// A genuinely parseable PAR2 index describing one 12-byte file in 4-byte slices.
///
/// The file it describes, `damaged.bin`, is deliberately never written: a missing file is
/// three missing blocks, the index itself carries no recovery packets and no volume is on
/// disk, so `rust_par2` reports the one verdict this job is about — `NotEnoughBlocks` with
/// three needed and none available. Two packets are enough for that: `Main` supplies the
/// slice size the parser insists on, `FileDesc` the file and its size.
fn par2_index_bytes() -> Vec<u8> {
    const SET_ID: &[u8; 16] = b"RDTESTSET0000001";
    const FILE_ID: &[u8; 16] = b"RDTESTFILE000001";
    let mut main = Vec::new();
    main.extend_from_slice(SET_ID);
    main.extend_from_slice(b"PAR 2.0\0Main\0\0\0\0");
    main.extend_from_slice(&4_u64.to_le_bytes()); // slice size
    main.extend_from_slice(&1_u32.to_le_bytes()); // number of files
    main.extend_from_slice(FILE_ID);
    let mut description = Vec::new();
    description.extend_from_slice(SET_ID);
    description.extend_from_slice(b"PAR 2.0\0FileDesc");
    description.extend_from_slice(FILE_ID);
    description.extend_from_slice(&[0x11; 16]); // full-file MD5, never reached
    description.extend_from_slice(&[0x22; 16]); // first-16K MD5, never reached
    description.extend_from_slice(&12_u64.to_le_bytes());
    description.extend_from_slice(b"damaged.bin\0"); // padded to a multiple of four

    let mut index = par2_packet(
        &main,
        [
            0x23, 0x30, 0x8c, 0x6e, 0x64, 0x4e, 0x71, 0xfd, 0xea, 0xcb, 0xc9, 0xf9, 0xb7, 0xb4,
            0x76, 0xcb,
        ],
    );
    index.extend_from_slice(&par2_packet(
        &description,
        [
            0x11, 0x4d, 0xd6, 0x69, 0x9b, 0x09, 0x66, 0x01, 0x0b, 0x22, 0x90, 0xae, 0x77, 0x40,
            0x5f, 0x6a,
        ],
    ));
    index
}

/// A queued Usenet package whose PAR2 index reports a three-block gap.
///
/// The payload and the index arrive; the three recovery volumes stay where enqueueing put
/// them, which is `Skipped`.
async fn seed_usenet_package_short_of_blocks(
    database: &Database,
    root: &std::path::Path,
) -> (PackageId, std::path::PathBuf) {
    let file = |subject: &str| rd_db::NewNzbFile {
        subject: subject.to_owned(),
        poster: "fixture".to_owned(),
        groups: vec!["alt.binaries.test".to_owned()],
        segments: vec![rd_db::NewNzbSegment {
            number: 1,
            bytes: 128,
            message_id: format!("{subject}@example.test"),
        }],
    };
    let import = database
        .add_nzb_import(rd_db::NewNzbImport {
            name: "short.nzb".to_owned(),
            sha256: "cd".repeat(32),
            category_id: None,
            source: rd_core::IngressSource::Manual,
            priority: None,
            import_mode: rd_core::ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![
                file("payload.zip"),
                file("release.par2"),
                file("release.vol000+01.par2"),
                file("release.vol001+02.par2"),
                file("release.vol003+16.par2"),
            ],
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
    let destination = std::path::PathBuf::from(&package.destination);
    std::fs::create_dir_all(&destination).expect("destination");
    write_zip(
        &destination.join("payload.zip"),
        SimpleFileOptions::default(),
        "payload.txt",
    );
    std::fs::write(destination.join("release.par2"), par2_index_bytes()).expect("index");
    // Only what really came down: the postponed volumes stay `Skipped`.
    for download in database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|download| download.package_id == package.id)
        .filter(|download| download.state != DownloadState::Skipped)
    {
        for state in [
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
            DownloadState::Completed,
        ] {
            database
                .transition_download(download.id, state)
                .await
                .expect("transition");
        }
    }
    (package.id, destination)
}

async fn package_state(database: &Database, package_id: PackageId) -> rd_core::PackageState {
    database
        .list_packages()
        .await
        .expect("packages")
        .into_iter()
        .find(|package| package.id == package_id)
        .expect("package")
        .state
}

/// The heart of RD-107-04: a repair short of blocks re-queues what it needs and stands down.
///
/// Before this there was no way back: the scheduler considered the package downloaded, the
/// pipeline was already running, and nothing could move a package from post-processing into
/// downloading again. The proof that the way back exists is the package state — it is
/// `Downloading`, not `Failed` — together with the two volumes that left `Skipped`.
#[tokio::test]
async fn a_repair_short_of_blocks_requeues_what_it_needs_and_sends_the_package_back() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (package_id, _destination) =
        seed_usenet_package_short_of_blocks(&database, &temp.path().join("usenet")).await;
    let inner = extraction_inner(&database, temp.path());

    crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto)
        .await
        .expect("one pass");

    assert_eq!(
        package_state(&database, package_id).await,
        rd_core::PackageState::Downloading,
        "the package has to go back to downloading, which is the return path this job is about"
    );
    let states: Vec<(String, DownloadState)> = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package_id)
        .map(|file| (file.file_name, file.state))
        .collect();
    let state_of = |name: &str| {
        states
            .iter()
            .find(|(file_name, _)| file_name == name)
            .map(|(_, state)| *state)
            .unwrap_or_else(|| panic!("no row for {name}"))
    };
    // Three blocks are missing; one and two cover them, and the sixteen-block volume would
    // have cost eight times the bytes for the same repair.
    assert_eq!(
        state_of("release.vol000+01.par2"),
        DownloadState::Queued,
        "the one-block volume should have been re-queued"
    );
    assert_eq!(
        state_of("release.vol001+02.par2"),
        DownloadState::Queued,
        "the two-block volume should have been re-queued"
    );
    assert_eq!(
        state_of("release.vol003+16.par2"),
        DownloadState::Skipped,
        "nothing beyond the gap may be fetched"
    );

    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let par2 = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::Par2)
        .expect("PAR2 step");
    assert_eq!(par2.state, PostprocessState::Queued, "waiting, not failed");
    assert_eq!(
        par2.code.as_deref(),
        Some(crate::par2_refill::AWAITING_BLOCKS)
    );
}

/// While the re-queued volumes are on their way, the pipeline does not run again.
///
/// Recovery after a restart lands here: `ExtractionService::recover` re-requests every package
/// with a queued step, and without this guard that second pass would verify the same gap and
/// plan against volumes that are already coming.
#[tokio::test]
async fn a_package_waiting_for_its_volumes_does_not_run_the_pipeline_again() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (package_id, _destination) =
        seed_usenet_package_short_of_blocks(&database, &temp.path().join("usenet")).await;
    let inner = extraction_inner(&database, temp.path());
    crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto)
        .await
        .expect("first pass");

    // Exactly what a restart does.
    crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto)
        .await
        .expect("second pass");

    assert_eq!(
        package_state(&database, package_id).await,
        rd_core::PackageState::Downloading
    );
    let queued = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package_id)
        .filter(|file| rd_core::is_par2_volume(&file.file_name))
        .filter(|file| file.state == DownloadState::Queued)
        .count();
    assert_eq!(
        queued, 2,
        "the second pass must not order the same gap covered twice"
    );
}

/// With nothing left to fetch, the shortfall is a verdict — and it carries a stable code.
#[tokio::test]
async fn a_shortfall_with_nothing_left_to_fetch_fails_with_a_stable_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (package_id, _destination) =
        seed_usenet_package_short_of_blocks(&database, &temp.path().join("usenet")).await;
    // Every postponed volume is gone, as it would be for a set that really is too small.
    for download in database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package_id)
        .filter(|file| file.state == DownloadState::Skipped)
    {
        database
            .transition_download(download.id, DownloadState::Cancelled)
            .await
            .expect("transition");
    }
    let inner = extraction_inner(&database, temp.path());

    crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto)
        .await
        .expect("one pass");

    assert_eq!(
        package_state(&database, package_id).await,
        rd_core::PackageState::Failed
    );
    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let par2 = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::Par2)
        .expect("PAR2 step");
    assert_eq!(par2.state, PostprocessState::Failed);
    assert_eq!(
        par2.code.as_deref(),
        Some(crate::par2_job::NOT_ENOUGH_BLOCKS),
        "a package that cannot be repaired has to say so in a way four languages can read"
    );
    assert_eq!(par2.params.get("needed").map(String::as_str), Some("3"));
    assert_eq!(par2.params.get("available").map(String::as_str), Some("0"));
}
