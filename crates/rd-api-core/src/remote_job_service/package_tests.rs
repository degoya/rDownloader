//! One remote job, one package, named after what the person added (owner report 2026-09-27).
//!
//! Two NZBs sent to Premiumize came back into the LinkGrabber as one package called
//! `source.nzb` holding the files of both. The plugin uploaded every container under that one
//! name, so both transfers -- and the cloud folder each finished into -- were called the same;
//! that half is fixed and pinned in `plugins/premiumize-common/src/container.rs`. These tests
//! pin the host half: a job's files are a package of their own whatever the provider calls
//! them, and the package carries the name the container was added under.

use std::sync::Arc;

use chrono::Utc;
use rd_core::{CollectorPackage, RemoteJobState};
use rd_plugin_host::extension::{RemoteJobArtifact, RemoteJobProgress, RemoteJobSource};

use super::{
    SubmitOutcome,
    tests::{MockProvider, handle, harness, later},
};

/// Two containers the mock claims (it reads a leading `d` as a torrent), different bytes and
/// therefore two jobs.
const FIRST: &[u8] = b"d4:infod4:name5:firstee";
const SECOND: &[u8] = b"d4:infod4:name6:secondee";

const FIRST_FILES: [&str; 2] = ["explanation.txt", "test-100MB.bin"];
const SECOND_FILES: [&str; 2] = [
    "ACES.Der.Club.der.Tennisgiganten.S01E01.mkv",
    "ACES.Der.Club.der.Tennisgiganten.S01E02.mkv",
];

/// A finished transfer as a provider that names every upload alike reports it: each file
/// hinted into a package called `source.nzb`.
fn finished(files: &[&str]) -> RemoteJobProgress {
    RemoteJobProgress::Ready {
        artifacts: files
            .iter()
            .map(|file| RemoteJobArtifact {
                url: format!("https://example.invalid/dl/{file}"),
                file_name: Some((*file).to_owned()),
                size: Some(10),
                package_hint: Some("source.nzb".to_owned()),
            })
            .collect(),
    }
}

/// Runs two container jobs to the end, each submitted under `names[i]`, and answers the
/// package each job points at together with the file names it holds, sorted.
async fn two_finished_jobs(names: [Option<&str>; 2]) -> Vec<(CollectorPackage, Vec<String>)> {
    let provider = Arc::new(MockProvider::default());
    provider.will_submit(Ok(handle("PM01")));
    provider.will_submit(Ok(handle("PM02")));
    // Due rows are driven in the order they were created, so the first answer is the first job's.
    provider.will_answer(finished(&FIRST_FILES));
    provider.will_answer(finished(&SECOND_FILES));
    let harness = harness(&provider).await;
    let mut jobs = Vec::new();
    for (bytes, name) in [FIRST, SECOND].into_iter().zip(names) {
        let SubmitOutcome::Started(job) = harness
            .service
            .submit_named(
                harness.account,
                RemoteJobSource::Container(bytes.to_vec()),
                name.map(str::to_owned),
            )
            .await
            .expect("submit")
        else {
            panic!("a container the mock claims starts a job");
        };
        assert_eq!(job.source_name.as_deref(), name, "the row keeps the name");
        jobs.push(job.id);
    }

    let now = Utc::now();
    harness.sweep(now).await;
    // The plugin is handed the name with the bytes, so a provider that names its job after
    // the upload can use the person's own name (`job-context`).
    assert_eq!(
        provider.names(),
        names
            .iter()
            .map(|name| name.map(str::to_owned))
            .collect::<Vec<_>>()
    );
    harness.sweep(later(now, 3_600)).await;

    let packages = harness
        .database
        .list_collector_packages()
        .await
        .expect("packages");
    assert_eq!(packages.len(), 2, "one package per job: {packages:?}");
    let candidates = harness
        .database
        .list_candidates()
        .await
        .expect("candidates");
    let mut outcome = Vec::new();
    for id in jobs {
        let job = harness.job(id).await;
        assert_eq!(job.state, RemoteJobState::Ready);
        let package_id = job.package_id.expect("the job points at its package");
        let package = packages
            .iter()
            .find(|package| package.id == package_id)
            .expect("the package is listed")
            .clone();
        let mut files: Vec<String> = candidates
            .iter()
            .filter(|candidate| candidate.package_id == Some(package_id))
            .filter_map(|candidate| candidate.file_name.clone())
            .collect();
        files.sort();
        outcome.push((package, files));
    }
    outcome
}

fn owned(files: [&str; 2]) -> Vec<String> {
    files.iter().map(|file| (*file).to_owned()).collect()
}

/// The regression: two jobs whose provider named both transfers `source.nzb` are two
/// packages, and neither holds a file of the other.
#[tokio::test]
async fn two_jobs_with_one_provider_name_are_two_packages() {
    let packages = two_finished_jobs([None, None]).await;
    assert_ne!(packages[0].0.id, packages[1].0.id);
    assert_eq!(packages[0].1, owned(FIRST_FILES));
    assert_eq!(packages[1].1, owned(SECOND_FILES));
}

/// The name: a container's file name, not what the provider called its transfer, and stated
/// rather than guessed, so the regroup after the online check does not rename it.
#[tokio::test]
async fn a_job_package_is_named_after_the_container_it_was_added_as() {
    let packages = two_finished_jobs([
        Some("Speedtest.nzb"),
        Some("ACES.Der.Club.der.Tennisgiganten.S01.GERMAN.1080p.nzb"),
    ])
    .await;
    assert_eq!(packages[0].0.name, "Speedtest");
    assert_eq!(
        packages[1].0.name,
        "ACES.Der.Club.der.Tennisgiganten.S01.GERMAN.1080p"
    );
    assert!(packages.iter().all(|(package, _)| !package.auto_named));
    assert_eq!(packages[0].1, owned(FIRST_FILES));
    assert_eq!(packages[1].1, owned(SECOND_FILES));
}
