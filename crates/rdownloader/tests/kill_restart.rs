//! Axis B of the recovery matrix (RD-180-12): a real `rdownloader serve`, stopped with
//! `SIGKILL` in the middle of its work and started again on the same data.
//!
//! Axis A stops at a named point by returning an error, which drops everything the process
//! held but leaves one question open: whether the operating system's writes had landed. Only a
//! real kill answers it, and a kill costs a spawned service per case, so these cases are
//! `#[ignore]` and run in CI (`.github/workflows/recovery.yml`), never on a development machine:
//!
//! ```bash
//! cargo nextest run -p rdownloader --test kill_restart --run-ignored only
//! ```
//!
//! The service is the binary Cargo built for this test, or the one `RD_AXIS_B_BINARY` names.
//! The files a download fetches come from an origin in this process whose gate holds every
//! connection still once a given number of bytes has gone out, so the kill lands at a known
//! point rather than at whatever moment a timer happened to pick.

#![cfg(unix)]

#[path = "kill_restart/origin.rs"]
mod origin;
#[path = "kill_restart/service.rs"]
mod service;

use std::time::Duration;

use origin::{Origin, payload};
use service::{Service, digest, file_named, leftovers, wait_for};

const MIB: u64 = 1024 * 1024;

/// A download killed with a recorded checkpoint behind it and more bytes in flight: the row
/// never claims more than the origin sent, the part file holds exactly the source's bytes up
/// to the checkpoint, and the restart resumes from there to the same file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Axis B spawns and kills a real service; CI runs it (crates/rd-core/recovery-matrix.md)"]
async fn a_download_killed_mid_transfer_resumes_to_the_same_bytes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let expected = payload(32 * MIB);
    let origin = Origin::start(expected.clone(), 20 * MIB).await;
    // One connection, so the engine's 8 MiB checkpoints fall at known offsets: two of them are
    // behind the transfer when the gate holds it at 20 MiB.
    let service = Service::prepare(
        directory.path(),
        serde_json::json!({ "max_chunks_per_file": 1 }),
    )
    .await;
    let mut first = service.start(0);
    service.ready(&mut first).await;
    let created = service
        .send(
            reqwest::Method::POST,
            "/api/v1/downloads",
            serde_json::json!({ "url": origin.url() }),
        )
        .await;
    let id = created["id"].as_str().expect("download id").to_owned();

    let (service_ref, origin_ref, id_ref) = (&service, &origin, id.as_str());
    wait_for(
        "the transfer never reached the gate",
        Duration::from_secs(90),
        || async move { (origin_ref.sent(0) >= 20 * MIB).then_some(()) },
    )
    .await;
    wait_for(
        "no checkpoint was recorded before the gate",
        Duration::from_secs(30),
        || async move {
            let row = service_ref.row("downloads", id_ref).await;
            // A decimal string on the wire, for JavaScript's sake.
            let committed = row["committed_bytes"]
                .as_str()
                .and_then(|bytes| bytes.parse::<u64>().ok())
                .unwrap_or_default();
            (committed >= 8 * MIB).then_some(())
        },
    )
    .await;
    drop(first);

    // What the kill left, read while nothing runs.
    let database = rd_db::Database::open(service.database_path())
        .await
        .expect("database");
    let row = database
        .get_download(id.parse().expect("download id"))
        .await
        .expect("row")
        .expect("the row survives the kill");
    drop(database);
    let committed = row.committed_bytes.get();
    let sent = origin.sent(0);
    assert!(
        committed <= sent,
        "the row claims {committed} bytes and the origin sent {sent}"
    );
    let part = file_named(&service.downloads(), &format!("{id}.part"))
        .expect("the part file survives the kill");
    let on_disk = std::fs::read(&part).expect("part file");
    let confirmed = usize::try_from(committed).expect("size");
    assert!(
        on_disk.len() >= confirmed,
        "the part file holds {} bytes, the row confirms {confirmed}",
        on_disk.len()
    );
    assert!(
        on_disk[..confirmed] == expected[..confirmed],
        "a confirmed byte on disk is not the source's"
    );

    origin.release();
    let mut second = service.start(1);
    service.ready(&mut second).await;
    wait_for(
        "the download did not complete after the restart",
        Duration::from_secs(180),
        || async move {
            (service_ref.row("downloads", id_ref).await["state"] == "completed").then_some(())
        },
    )
    .await;
    drop(second);

    let finished = file_named(&service.downloads(), "payload.bin").expect("the finished file");
    let bytes = std::fs::read(&finished).expect("finished file");
    assert_eq!(bytes.len(), expected.len());
    assert_eq!(
        digest(&bytes),
        digest(&expected),
        "the resumed file differs"
    );
    assert!(
        leftovers(&service.downloads()).is_empty(),
        "{:?}",
        leftovers(&service.downloads())
    );
    let fetched_again = origin.sent(1);
    assert!(
        fetched_again + 8 * MIB <= expected.len() as u64,
        "the restart fetched {fetched_again} of {} bytes: it did not resume",
        expected.len()
    );
}

/// A service killed while a package's post-processing runs: the package does not stay
/// `postprocessing`, the step that was interrupted runs again, and the payload is untouched.
///
/// The step is a user script that waits, which is what holds the pipeline still at a known
/// point; an unpack is over before a poll could see it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Axis B spawns and kills a real service; CI runs it (crates/rd-core/recovery-matrix.md)"]
async fn a_service_killed_during_post_processing_finishes_it_after_the_restart() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("tempdir");
    let expected = payload(MIB);
    // Held at the first byte until the package has its script.
    let origin = Origin::start(expected.clone(), 0).await;
    let service = Service::prepare(directory.path(), serde_json::json!({})).await;
    let first_run = directory.path().join("script-first-run");
    let second_run = directory.path().join("script-second-run");
    let script = service.scripts().join("hold.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ -e '{first}' ]; then\n    : > '{second}'\n    exit 0\nfi\n\
             : > '{first}'\nexec sleep 30\n",
            first = first_run.display(),
            second = second_run.display(),
        ),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let mut first = service.start(0);
    service.ready(&mut first).await;
    let created = service
        .send(
            reqwest::Method::POST,
            "/api/v1/downloads",
            serde_json::json!({ "url": origin.url() }),
        )
        .await;
    let id = created["id"].as_str().expect("download id").to_owned();
    let package = created["package_id"]
        .as_str()
        .expect("package id")
        .to_owned();
    service
        .send(
            reqwest::Method::PATCH,
            &format!("/api/v1/packages/{package}"),
            serde_json::json!({ "script": "hold.sh" }),
        )
        .await;
    origin.release();

    let (service_ref, package_ref, first_ref) = (&service, package.as_str(), &first_run);
    wait_for(
        "the script never started",
        Duration::from_secs(90),
        || async move { first_ref.exists().then_some(()) },
    )
    .await;
    assert_eq!(
        service.row("packages", &package).await["state"],
        "postprocessing"
    );
    drop(first);

    let mut second = service.start(1);
    service.ready(&mut second).await;
    wait_for(
        "the package never finished post-processing after the restart",
        Duration::from_secs(90),
        || async move {
            (service_ref.row("packages", package_ref).await["state"] == "completed").then_some(())
        },
    )
    .await;
    assert_eq!(service.row("downloads", &id).await["state"], "completed");
    drop(second);

    assert!(
        second_run.exists(),
        "the interrupted step did not run again"
    );
    let finished = file_named(&service.downloads(), "payload.bin").expect("the downloaded file");
    assert_eq!(
        digest(&std::fs::read(&finished).expect("downloaded file")),
        digest(&expected)
    );
    assert!(
        leftovers(&service.downloads()).is_empty(),
        "{:?}",
        leftovers(&service.downloads())
    );
}
