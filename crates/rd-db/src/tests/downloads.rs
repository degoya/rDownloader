//! Download rows: creation, resolver refresh and pins, completion, kinds and media rows.

use chrono::{Duration, Utc};
use rd_core::{AuthProfileSelection, DownloadId, IngressSource, PackageId};

use super::{SELECTION, probe_url};
use crate::{Database, NewDownload, NewPackage};

#[tokio::test]
async fn resolver_refresh_can_only_be_claimed_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("refresh.sqlite"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "refresh".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: "https://example.test/file".parse().expect("URL"),
            file_name: "file".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    assert!(
        database
            .claim_resolver_refresh(download.id)
            .await
            .expect("first claim")
    );
    assert!(
        !database
            .claim_resolver_refresh(download.id)
            .await
            .expect("second claim")
    );
}

/// The pre-resume refresh is transient on purpose (RD-108-17).
///
/// A second resume of an expired capture claims a second slot from the windowed budget and
/// starts again from the address the person consented to — there is no write-back that could
/// hand it a renewed URL of unknown lifetime. This test fails the moment one is added.
#[tokio::test]
async fn a_second_resume_re_refreshes_instead_of_reusing_a_stored_url() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("replay-refresh.sqlite"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "replay".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let captured = rd_core::CapturedRequest {
        effective_url: Some(
            "https://cdn.example.net/f.bin?Expires=1000000000"
                .parse()
                .expect("URL"),
        ),
        method: "GET".to_owned(),
        expires_at: Some(Utc::now() - Duration::hours(2)),
        approved_origins: vec!["https://cdn.example.net".to_owned()],
        ..rd_core::CapturedRequest::default()
    };
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: "https://hoster.example/dl/1".parse().expect("URL"),
            file_name: "f.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: Some(Box::new(crate::NewReplayTemplate {
                request: captured.clone(),
                consent: rd_core::ReplayConsent {
                    granted_at: Utc::now(),
                    template_hash: "hash".to_owned(),
                    approved_origins: vec!["https://cdn.example.net".to_owned()],
                },
                body_ref: None,
                candidate_id: None,
            })),
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    // First resume: the expired capture is refreshed, which costs one slot.
    assert!(
        database
            .claim_replay_refresh(download.id)
            .await
            .expect("first claim")
    );
    let after_first = database
        .request_template(download.id)
        .await
        .expect("template")
        .expect("a template");
    assert_eq!(
        after_first.request, captured,
        "a refresh must not rewrite the consented capture"
    );

    // Second resume: nothing stored a renewed address, so the same decision is taken again
    // against the same consented capture, and a second slot is spent knowingly.
    assert!(
        database
            .claim_replay_refresh(download.id)
            .await
            .expect("second claim")
    );
    let after_second = database
        .request_template(download.id)
        .await
        .expect("template")
        .expect("a template");
    assert_eq!(after_second.request, captured);
    assert_eq!(
        after_second.request.effective_url, captured.effective_url,
        "the stored address stays the captured one across refreshes"
    );
    assert_eq!(after_second.consent.template_hash, "hash");
}

#[tokio::test]
async fn download_can_be_created_paused_without_entering_the_runnable_queue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("paused.sqlite"))
        .await
        .expect("database");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "paused".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");

    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id: package.id,
            source: "https://example.test/file".parse().expect("URL"),
            file_name: "file".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Paused,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    assert_eq!(download.state, rd_core::DownloadState::Paused);
    assert_eq!(
        database
            .get_download(download.id)
            .await
            .expect("read download")
            .expect("stored download")
            .state,
        rd_core::DownloadState::Paused
    );
}

#[tokio::test]
async fn first_resolver_version_pin_wins_and_survives_reads() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("pin.sqlite"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "pin".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: "https://example.test/file".parse().expect("URL"),
            file_name: "file".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    let first = rd_core::ResolverPin {
        plugin_id: rd_core::PluginId::new(),
        version: "1.0.0".to_owned(),
    };
    let second = rd_core::ResolverPin {
        plugin_id: rd_core::PluginId::new(),
        version: "2.0.0".to_owned(),
    };

    assert_eq!(
        database
            .claim_resolver_pin(download.id, first.clone())
            .await
            .expect("first claim"),
        first
    );
    assert_eq!(
        database
            .claim_resolver_pin(download.id, second)
            .await
            .expect("second claim"),
        first
    );
    assert_eq!(
        database.resolver_pin(download.id).await.expect("read pin"),
        Some(first.clone())
    );

    // A version this build still has keeps its pin: the whole point of pinning is that a
    // plugin upgrade cannot move a job that is already running.
    let freed = database
        .clear_unsatisfiable_resolver_pins(vec![(
            first.plugin_id.to_string(),
            first.version.clone(),
        )])
        .await
        .expect("reconcile pins");
    assert_eq!(freed, 0);
    assert_eq!(
        database.resolver_pin(download.id).await.expect("read pin"),
        Some(first.clone())
    );

    // A version that is gone would make the job unresolvable for ever, so the pin goes and
    // the job resolves through the current build of the same plugin instead.
    let freed = database
        .clear_unsatisfiable_resolver_pins(vec![(first.plugin_id.to_string(), "2.0.0".to_owned())])
        .await
        .expect("reconcile pins");
    assert_eq!(freed, 1);
    assert_eq!(
        database.resolver_pin(download.id).await.expect("read pin"),
        None
    );
}

/// Every kind writes and reads back as itself.
///
/// The read path used to be a hand-written match, so a variant added to the enum was stored
/// correctly and loaded as `Http`. Walking the whole enum means a new kind cannot be added
/// without this test seeing it.
#[test]
fn every_download_kind_survives_a_round_trip_through_the_column() {
    for kind in [
        rd_core::DownloadKind::Http,
        rd_core::DownloadKind::Usenet,
        rd_core::DownloadKind::Media,
        rd_core::DownloadKind::Gallery,
        rd_core::DownloadKind::Record,
        rd_core::DownloadKind::Torrent,
        rd_core::DownloadKind::Ftp,
        rd_core::DownloadKind::Sftp,
        rd_core::DownloadKind::Plugin,
        rd_core::DownloadKind::ObjectStorage,
    ] {
        let stored = serde_json::to_string(&kind).expect("serialize");
        let column = stored.trim_matches('"');
        assert_eq!(crate::models::parse_kind(column), kind, "{column}");
    }
    assert_eq!(
        crate::models::parse_kind("something-a-later-version-invented"),
        rd_core::DownloadKind::Http
    );
}

#[tokio::test]
async fn captured_request_metadata_survives_a_reopen_of_the_database() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("capture.sqlite");
    let request = rd_core::CapturedRequest {
        effective_url: Some("https://cdn.example.com/a/report.pdf".parse().expect("URL")),
        method: "GET".to_owned(),
        referrer: Some("https://example.com/downloads".to_owned()),
        user_agent: Some("Mozilla/5.0".to_owned()),
        content_disposition: Some("attachment; filename=\"report.pdf\"".to_owned()),
        headers: vec![rd_core::CapturedHeader {
            name: "accept".to_owned(),
            value: "*/*".to_owned(),
        }],
        ..rd_core::CapturedRequest::default()
    };
    let candidate_id = {
        let database = Database::open(path.clone()).await.expect("database");
        let (_, _, candidates) = database
            .add_collector_batch(crate::NewCollectorBatch {
                package_hints: Vec::new(),
                mirror_hints: Vec::new(),
                source: IngressSource::BrowserDownload,
                source_label: Some("Chrome".to_owned()),
                package_name: None,
                password: None,
                passwords: Vec::new(),
                category_id: None,
                priority: None,
                urls: vec!["https://files.example.com/report.pdf".parse().expect("URL")],
                providers: vec![None],
                file_names: vec![Some("report.pdf".to_owned())],
                sizes: Vec::new(),
                requests: vec![Some(request.clone())],
                body_refs: vec![None],
                auto_check: false,
                source_attributes: Vec::new(),
            })
            .await
            .expect("batch");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].request.as_ref(), Some(&request));
        candidates[0].id
    };
    // Reopening proves the metadata lives in the database, not in memory.
    let database = Database::open(path).await.expect("reopen");
    let candidates = database.list_candidates().await.expect("candidates");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, candidate_id);
    assert_eq!(candidates[0].request.as_ref(), Some(&request));
    assert_eq!(candidates[0].file_name.as_deref(), Some("report.pdf"));
}

/// External runners report progress in throttled samples and may finish without a final one,
/// which used to leave a completed download stuck at a partial percentage in the UI.
#[tokio::test]
async fn completing_a_download_reports_it_as_fully_transferred() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("complete.sqlite"))
        .await
        .expect("database");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "media".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id: package.id,
            source: "https://example.test/clip".parse().expect("URL"),
            file_name: "clip.mp4".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Media,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    // The last sample yt-dlp produced before it finished.
    database
        .set_download_progress(download.id, 28_839_279, Some(40_391_148))
        .await
        .expect("progress");
    for next in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
    ] {
        database
            .transition_download(download.id, next)
            .await
            .expect("transition");
    }
    let completed = database
        .complete_download(download.id, "clip.mp4".to_owned(), None)
        .await
        .expect("complete");

    assert_eq!(completed.state, rd_core::DownloadState::Completed);
    assert_eq!(
        completed.committed_bytes.get(),
        completed.total_bytes.expect("total").get(),
        "a completed download must report 100 %"
    );
    assert_eq!(completed.committed_bytes.get(), 40_391_148);
}

/// A stalled sample can overshoot the announced total; the completed row must not claim more
/// bytes than it reports as the size.
#[tokio::test]
async fn completing_a_download_absorbs_an_overshooting_sample() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("overshoot.sqlite"))
        .await
        .expect("database");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "http".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id: package.id,
            source: "https://example.test/file.bin".parse().expect("URL"),
            file_name: "file.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    database
        .set_download_progress(download.id, 90_331_545, Some(87_450_419))
        .await
        .expect("progress");
    for next in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
    ] {
        database
            .transition_download(download.id, next)
            .await
            .expect("transition");
    }

    let completed = database
        .complete_download(download.id, "file.bin".to_owned(), None)
        .await
        .expect("complete");
    assert_eq!(
        completed.committed_bytes.get(),
        completed.total_bytes.expect("total").get()
    );
    assert_eq!(completed.committed_bytes.get(), 90_331_545);
}

/// Migration 0034 adds the media format inventory column. Rows written before it — a
/// download whose selection is a bare preset, and a candidate with no inventory at all —
/// must survive untouched, because the alternative is rewriting rows that may be
/// mid-download.
#[tokio::test]
async fn media_rows_written_before_the_format_selector_still_load() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("media-legacy.sqlite"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "media".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");

    // Shaped exactly like a 0.6 row: no contract version, no criteria.
    let legacy = rd_core::MediaSelection {
        page_url: "https://www.youtube.com/watch?v=abc".parse().expect("url"),
        variant_id: "1080p".to_owned(),
        format: "bv*[height<=1080]+ba/b[height<=1080]".to_owned(),
        kind: rd_core::MediaKind::Video,
        ext: "mp4".to_owned(),
        title: "clip".to_owned(),
        contract_version: 0,
        criteria: None,
        resolved: None,
    };
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: "https://www.youtube.com/watch?v=abc".parse().expect("URL"),
            file_name: "clip.mp4".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: SELECTION,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Media,
            media: Some(legacy.clone()),
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    let reloaded = database
        .get_download(download.id)
        .await
        .expect("load")
        .expect("download")
        .media
        .expect("media selection");
    assert_eq!(reloaded, legacy, "the stored blob round-trips unchanged");
    assert_eq!(reloaded.contract_version, 0);
    assert_eq!(
        reloaded
            .effective_criteria()
            .expect("a preset resolves")
            .max_height,
        Some(1080),
        "the preset bridge is what keeps old rows downloadable"
    );

    // A candidate that never had an inventory reads back as "nothing stored", not as an
    // error, so the endpoint can fall back to the bounded variant list.
    let (_, _, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            urls: vec!["https://example.test/file.bin".parse().expect("url")],
            providers: vec![None],
            file_names: vec![None],
            sizes: vec![None],
            requests: vec![None],
            body_refs: vec![None],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    let candidate = candidates.first().expect("one candidate");
    assert_eq!(
        database
            .candidate_media_state(candidate.id)
            .await
            .expect("state loads"),
        None
    );
}

/// Which records hold a plugin version, and which ones only remember it (RD-108-10).
///
/// Installing a plugin never removes the older version, because a job already under way keeps
/// the version that started it. Clearing the leftovers by hand therefore needs one honest
/// answer to "is anything still bound to this version", and the two records that name a
/// version are the resolver pin and the transfer checkpoint.
#[tokio::test]
async fn only_unfinished_work_holds_a_plugin_version() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("plugin-versions.sqlite"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "versions".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let new_download = |name: &str| NewDownload {
        id: DownloadId::new(),
        package_id,
        source: probe_url(),
        file_name: name.to_owned(),
        total_bytes: None,
        expected_checksum: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: SELECTION,
        initial_state: rd_core::DownloadState::Queued,
        kind: rd_core::DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        enrichment: Vec::new(),
        secret_fragment: None,
    };
    let pinned = database
        .create_download(new_download("pinned.bin"))
        .await
        .expect("pinned download");
    let checkpointed = database
        .create_download(new_download("checkpointed.bin"))
        .await
        .expect("checkpointed download");

    let plugin = rd_core::PluginId::new();
    let id = plugin.to_string();
    database
        .claim_resolver_pin(
            pinned.id,
            rd_core::ResolverPin {
                plugin_id: plugin,
                version: "1.0.0".to_owned(),
            },
        )
        .await
        .expect("pin");
    database
        .save_plugin_transfer(
            checkpointed.id,
            id.clone(),
            "1.0.0".to_owned(),
            Some(b"resume".to_vec()),
        )
        .await
        .expect("checkpoint");

    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage"),
        2,
        "the pin and the checkpoint each hold the version they name"
    );
    // The version an upgrade installed beside it holds nothing on its own.
    assert_eq!(
        database
            .plugin_version_usage(&id, "2.0.0")
            .await
            .expect("usage of the newer version"),
        0
    );

    // The blockers are named, oldest first, so a refusal can point at the job in the way.
    assert_eq!(
        database
            .plugin_version_blockers(&id, "1.0.0", 2)
            .await
            .expect("blockers"),
        vec!["pinned.bin".to_owned(), "checkpointed.bin".to_owned()]
    );

    // The one exception the query makes, and the one the documentation states as the rule: a
    // completed download never runs again, so it releases the version the moment it finishes.
    let finished = database
        .create_download(new_download("finished.bin"))
        .await
        .expect("finished download");
    database
        .claim_resolver_pin(
            finished.id,
            rd_core::ResolverPin {
                plugin_id: plugin,
                version: "1.0.0".to_owned(),
            },
        )
        .await
        .expect("pin the third download");
    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage while all three are unfinished"),
        3
    );
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
        rd_core::DownloadState::Completed,
    ] {
        database
            .transition_download(finished.id, state)
            .await
            .expect("transition towards completion");
    }
    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage after one of them completed"),
        2,
        "a completed download releases the version it pinned"
    );

    // Cancelling releases nothing. `ProgressControl::cancel` keeps the partial data and
    // `resume` puts the job back in the queue, so the version it would resume with is still
    // spoken for.
    database
        .transition_download(pinned.id, rd_core::DownloadState::Cancelled)
        .await
        .expect("cancel");
    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage after cancelling"),
        2,
        "a cancelled download can be resumed, so it keeps its pin"
    );

    // Deleting it does, and takes the pin with it.
    database
        .delete_download(pinned.id)
        .await
        .expect("delete the cancelled download");
    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage after deleting"),
        1
    );

    database
        .clear_plugin_transfer(checkpointed.id)
        .await
        .expect("clear checkpoint");
    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage after the transfer finished"),
        0
    );
    assert!(
        database
            .plugin_version_blockers(&id, "1.0.0", 2)
            .await
            .expect("blockers")
            .is_empty()
    );
}

/// The per-package read is what keeps a completion check off the whole `downloads` table, so
/// it has to return exactly that package's files, in queue order, and nothing else.
#[tokio::test]
async fn downloads_for_package_returns_only_that_package_in_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("per-package.sqlite"))
        .await
        .expect("database");
    let mut packages = Vec::new();
    for (name, files) in [
        ("first", ["a.bin", "b.bin"]),
        ("second", ["c.bin", "d.bin"]),
    ] {
        let package_id = PackageId::new();
        database
            .create_package(NewPackage {
                id: package_id,
                name: name.to_owned(),
                destination: directory.path().to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        for file_name in files {
            database
                .create_download(NewDownload {
                    id: DownloadId::new(),
                    package_id,
                    source: format!("https://example.test/{file_name}")
                        .parse()
                        .expect("URL"),
                    file_name: file_name.to_owned(),
                    total_bytes: None,
                    expected_checksum: None,
                    account_id: None,
                    proxy_profile_id: None,
                    auth_profile: SELECTION,
                    initial_state: rd_core::DownloadState::Queued,
                    kind: rd_core::DownloadKind::Http,
                    media: None,
                    remote_credential_id: None,
                    replay: None,
                    mirror_group: None,
                    enrichment: Vec::new(),
                    secret_fragment: None,
                })
                .await
                .expect("download");
        }
        packages.push(package_id);
    }

    let names = |files: Vec<rd_core::DownloadFile>| {
        files
            .into_iter()
            .map(|file| file.file_name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(
            database
                .downloads_for_package(packages[0])
                .await
                .expect("first package")
        ),
        ["a.bin", "b.bin"]
    );
    assert_eq!(
        names(
            database
                .downloads_for_package(packages[1])
                .await
                .expect("second package")
        ),
        ["c.bin", "d.bin"]
    );
    assert!(
        database
            .downloads_for_package(PackageId::new())
            .await
            .expect("unknown package")
            .is_empty(),
        "a package that does not exist has no files, not everybody else's"
    );
}
