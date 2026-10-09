//! The check against a served stable manifest and release list: where each channel looks,
//! what a floor does, and what a bad document is reported as.

use super::*;
use crate::{
    MemoryFetcher,
    manifest::tests::{at, key, manifest, signed, trust_for},
};

const PREFIX: &str = "https://github.com/degoya/rDownloader/releases/download/";

fn beta_url(tag: &str) -> String {
    format!("{PREFIX}{tag}/{}", Channel::Beta.file_name())
}

fn release_list(entries: &[(&str, bool)]) -> Vec<u8> {
    let releases: Vec<serde_json::Value> = entries
        .iter()
        .map(|(tag, with_manifest)| {
            let mut assets = vec![serde_json::json!({
                "name": "rdownloader-linux-x86_64.tar.gz",
                "browser_download_url": format!("{PREFIX}{tag}/rdownloader-linux-x86_64.tar.gz"),
            })];
            if *with_manifest {
                assets.push(serde_json::json!({
                    "name": Channel::Beta.file_name(),
                    "browser_download_url": beta_url(tag),
                }));
            }
            serde_json::json!({ "tag_name": tag, "draft": false, "prerelease": true, "assets": assets })
        })
        .collect();
    serde_json::to_vec(&releases).expect("encode")
}

fn served() -> (MemoryFetcher, Sources) {
    let sources = Sources::official();
    let fetcher = MemoryFetcher::new();
    fetcher.serve(
        sources.stable.as_str(),
        signed(&manifest(Channel::Stable, "1.8.0", 10)),
    );
    (fetcher, sources)
}

#[tokio::test]
async fn the_stable_channel_reads_the_stable_manifest_only() {
    let (fetcher, sources) = served();
    let report = check(
        &fetcher,
        &sources,
        &trust_for(&key()),
        Channel::Stable,
        Floors::default(),
        at(1),
    )
    .await;
    assert!(report.problem.is_none(), "{:?}", report.problem);
    assert_eq!(report.manifests.len(), 1);
    assert_eq!(report.floors.stable, Some(10));
    assert_eq!(fetcher.requests(), vec![sources.stable.to_string()]);
}

#[tokio::test]
async fn the_beta_channel_finds_the_newest_beta_manifest_in_the_release_list() {
    let (fetcher, sources) = served();
    fetcher.serve(
        sources.releases.as_str(),
        release_list(&[
            ("v1.8.0-beta.1", true),
            ("v1.8.1-beta.2", true),
            ("v1.8.1-beta.3", false),
        ]),
    );
    fetcher.serve(
        &beta_url("v1.8.1-beta.2"),
        signed(&manifest(Channel::Beta, "1.8.1-beta.2", 20)),
    );
    let report = check(
        &fetcher,
        &sources,
        &trust_for(&key()),
        Channel::Beta,
        Floors::default(),
        at(1),
    )
    .await;
    assert!(report.problem.is_none(), "{:?}", report.problem);
    let versions: Vec<&str> = report
        .manifests
        .iter()
        .map(|m| m.version.as_str())
        .collect();
    assert_eq!(versions, vec!["1.8.0", "1.8.1-beta.2"]);
    assert_eq!(
        report.floors,
        Floors {
            stable: Some(10),
            beta: Some(20)
        }
    );
}

/// Before the first stable release with a manifest, a beta installation is still served.
#[tokio::test]
async fn a_missing_stable_manifest_beside_a_verified_beta_is_no_problem() {
    let sources = Sources::official();
    let fetcher = MemoryFetcher::new();
    fetcher.serve(
        sources.releases.as_str(),
        release_list(&[("v1.8.0-beta.1", true)]),
    );
    fetcher.serve(
        &beta_url("v1.8.0-beta.1"),
        signed(&manifest(Channel::Beta, "1.8.0-beta.1", 5)),
    );
    let report = check(
        &fetcher,
        &sources,
        &trust_for(&key()),
        Channel::Beta,
        Floors::default(),
        at(1),
    )
    .await;
    assert!(report.problem.is_none(), "{:?}", report.problem);
    assert_eq!(report.manifests.len(), 1);
    // On the stable channel the same missing manifest is the whole answer.
    let report = check(
        &fetcher,
        &sources,
        &trust_for(&key()),
        Channel::Stable,
        Floors::default(),
        at(1),
    )
    .await;
    assert_eq!(
        report.problem.map(|problem| problem.code()),
        Some("update.not_published")
    );
}

/// A replayed manifest is refused and reported even when nothing else went wrong.
#[tokio::test]
async fn a_manifest_below_its_floor_is_reported_and_the_floor_stays() {
    let (fetcher, sources) = served();
    let floors = Floors {
        stable: Some(11),
        beta: None,
    };
    let report = check(
        &fetcher,
        &sources,
        &trust_for(&key()),
        Channel::Stable,
        floors,
        at(1),
    )
    .await;
    assert!(report.manifests.is_empty());
    assert_eq!(report.floors, floors);
    assert_eq!(
        report.problem.map(|problem| problem.code()),
        Some("update.stale")
    );
}

/// Tampering on one channel is reported although the other verified.
#[tokio::test]
async fn a_forged_beta_manifest_is_reported_beside_a_good_stable_one() {
    let (fetcher, sources) = served();
    fetcher.serve(
        sources.releases.as_str(),
        release_list(&[("v1.9.0-beta.1", true)]),
    );
    let forged = crate::manifest::sign(
        "rdownloader-update-v1",
        &rd_sign::SigningKey::from_bytes(&[1; 32]),
        &manifest(Channel::Beta, "1.9.0-beta.1", 30),
    )
    .expect("sign");
    fetcher.serve(&beta_url("v1.9.0-beta.1"), forged);
    let report = check(
        &fetcher,
        &sources,
        &trust_for(&key()),
        Channel::Beta,
        Floors::default(),
        at(1),
    )
    .await;
    assert_eq!(report.manifests.len(), 1);
    assert_eq!(report.floors.beta, None);
    assert_eq!(
        report.problem.map(|problem| problem.code()),
        Some("update.bad_signature")
    );
}

#[test]
fn the_release_list_only_locates_assets_under_the_repository() {
    let list = serde_json::to_vec(&serde_json::json!([
        {
            "tag_name": "v9.0.0-beta.1",
            "assets": [{ "name": Channel::Beta.file_name(), "browser_download_url": "https://evil.example/rdownloader-update-beta.json" }]
        },
        {
            "tag_name": "v1.8.0-beta.1",
            "draft": true,
            "assets": [{ "name": Channel::Beta.file_name(), "browser_download_url": beta_url("v1.8.0-beta.1") }]
        }
    ]))
    .expect("encode");
    assert_eq!(beta_manifest_url(&list, PREFIX).expect("parse"), None);
    assert!(beta_manifest_url(b"not json", PREFIX).is_err());
}

/// Finding 9 of the 2026-09-30 review: a signed manifest may name only the repository's own
/// release downloads, and a refused one raises no floor.
#[tokio::test]
async fn an_artifact_outside_the_release_downloads_refuses_the_manifest() {
    for url in [
        "https://evil.example/rdownloader-linux-x86_64.tar.gz",
        "https://github.com/degoya/rDownloader/releases/download/../../../evil/a.tar.gz",
        "https://github.com/degoya/rDownloader/releases/download/%2e%2e/%2E%2E/%2e%2e/evil/a.tar.gz",
        "https://github.com/degoya/rDownloader-fork/releases/download/v1.8.0/a.tar.gz",
    ] {
        let sources = Sources::official();
        let fetcher = MemoryFetcher::new();
        let mut release = manifest(Channel::Stable, "1.8.0", 10);
        release.artifacts[0].url = url.to_owned();
        fetcher.serve(sources.stable.as_str(), signed(&release));
        let report = check(
            &fetcher,
            &sources,
            &trust_for(&key()),
            Channel::Stable,
            Floors::default(),
            at(1),
        )
        .await;
        assert!(report.manifests.is_empty(), "{url}");
        assert_eq!(report.floors.stable, None, "{url}");
        assert_eq!(
            report.problem.map(|problem| problem.code()),
            Some("update.invalid"),
            "{url}"
        );
    }
    let release = manifest(Channel::Stable, "1.8.0", 10);
    assert!(pinned(release, PREFIX).is_ok());
}

/// The capture agent's own archives (RD-1210-03) are pinned like the application's: one outside
/// the repository's release downloads refuses the whole manifest.
#[test]
fn an_agent_archive_outside_the_release_downloads_refuses_the_manifest() {
    let mut release = manifest(Channel::Stable, "1.8.0", 10);
    release.agent_artifacts = vec![crate::manifest::tests::agent_artifact("windows", "x86_64")];
    assert!(pinned(release.clone(), PREFIX).is_ok());
    release.agent_artifacts[0].url = "https://evil.example/rdownloader-capture.zip".to_owned();
    let error = pinned(release, PREFIX).expect_err("outside the downloads");
    assert_eq!(error.code(), "update.invalid");
}

#[test]
fn the_beta_channel_reads_sixty_releases() {
    // Two releases per version since 1.9.1 (the application's and `plugins-vX.Y.Z`): sixty keep
    // thirty versions in view (RD-191-09 RA-TOOL-06). That they fit the cap is a const assertion.
    let sources = Sources::official();
    assert_eq!(sources.releases.query(), Some("per_page=60"));
}

#[test]
fn floors_only_rise() {
    let mut floors = Floors::default();
    floors.raise(Channel::Beta, 5);
    floors.raise(Channel::Beta, 3);
    assert_eq!(
        floors,
        Floors {
            stable: None,
            beta: Some(5)
        }
    );
}
