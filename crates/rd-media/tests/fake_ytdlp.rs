//! End-to-end tests against the `fake-yt-dlp.sh` fixture (no real yt-dlp needed).

#![cfg(unix)]

use std::{path::PathBuf, sync::Arc};

use rd_core::{DownloadKind, DownloadState, MediaKind, MediaSettings, PackageId};
use rd_db::{Database, NewDownload, NewPackage};
use rd_media::{MediaProbe, MediaRunner, YtDlpProbe};
use rd_scheduler::{ExternalRunner, RunOutcome};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// One request at a time and no rate limit — the runner's behaviour under limits is
/// covered by the bandwidth tests, not by the yt-dlp fixtures.
fn test_limits() -> rd_scheduler::RunLimits {
    rd_scheduler::RunLimits {
        max_parallel_requests: 1,
        bandwidth: rd_limits::ScopedLimiter::unlimited(),
    }
}

/// A vault in the test's own temp directory. The media runner needs one to resolve a
/// cookie profile; these tests select none, so it stays empty.
async fn secrets(temp: &std::path::Path) -> rd_secrets::SecretStore {
    rd_secrets::SecretStore::open(temp.join("secrets"))
        .await
        .expect("secret store")
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{name}.sh"))
}

fn settings_for(script: &str) -> Arc<RwLock<MediaSettings>> {
    Arc::new(RwLock::new(MediaSettings {
        media_ytdlp_executable: Some(fixture(script).to_string_lossy().into_owned()),
        media_ffmpeg_executable: Some(fixture("fake-yt-dlp").to_string_lossy().into_owned()),
        media_default_variant: "720p".to_owned(),
        ..MediaSettings::default()
    }))
}

fn settings() -> Arc<RwLock<MediaSettings>> {
    settings_for("fake-yt-dlp")
}

#[tokio::test]
async fn probe_returns_variants_for_a_single_page_and_fans_out_playlists() {
    let probe = YtDlpProbe::new(settings());
    let single = probe
        .probe(&"https://www.youtube.com/watch?v=abc".parse().expect("url"))
        .await
        .expect("probe");
    assert_eq!(single.len(), 1);
    let info = &single[0].info;
    assert_eq!(info.title, "Idle Immortal / Trailer");
    assert_eq!(info.duration_seconds, Some(95));
    assert_eq!(info.selected, "720p");
    let ids: Vec<&str> = info.variants.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, vec!["best", "1080p", "720p", "audio_mp3"]);
    assert_eq!(rd_db::media_file_name(info), "Idle Immortal _ Trailer.mp4");

    let playlist = probe
        .probe(
            &"https://www.youtube.com/playlist?list=PL1"
                .parse()
                .expect("url"),
        )
        .await
        .expect("playlist");
    assert_eq!(playlist.len(), 2);
    assert_eq!(
        playlist[1].info.page_url.as_str(),
        "https://www.youtube.com/watch?v=two"
    );
    // A flat listing carries no formats, and nothing probes an entry again before it is
    // queued: without a selection here every entry reached the runner as
    // `media.selection_missing` (RD-120-50).
    for entry in &playlist {
        let selection = entry
            .info
            .selection()
            .unwrap_or_else(|| panic!("{} has no selection", entry.info.page_url));
        assert_eq!(selection.variant_id, "best");
        assert!(
            selection.format == "bv*+ba/b" || selection.format == "b",
            "the fallback hands the choice to yt-dlp: {}",
            selection.format
        );
    }
}

#[tokio::test]
async fn probe_maps_tool_errors_to_failures() {
    let probe = YtDlpProbe::new(settings_for("fake-yt-dlp-fail"));
    let failure = probe
        .probe(&"https://www.youtube.com/watch?v=abc".parse().expect("url"))
        .await
        .expect_err("failure");
    assert_eq!(failure.code.as_deref(), Some("media.ytdlp_failed"));
    assert_eq!(failure.category, rd_core::FailureKind::Permanent);
}

/// The cookie profile reaches yt-dlp as a file, and the file does not outlive the download.
///
/// The fake records its own argv, which is the only way to assert what actually reached the
/// process rather than what the plan builder intended.
#[tokio::test]
async fn runner_hands_yt_dlp_a_scoped_cookie_file_and_removes_it_afterwards() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("media.sqlite"))
        .await
        .expect("database");
    let secrets = secrets(temp.path()).await;
    let secret_ref = secrets
        .put(secrecy::SecretString::from(
            "# Netscape HTTP Cookie File\n\
             .youtube.com\tTRUE\t/\tTRUE\t0\tSID\ttop-secret-session\n\
             .tracker.invalid\tTRUE\t/\tTRUE\t0\ttrack\tfollow-me\n"
                .to_owned(),
        ))
        .await
        .expect("store cookies");
    let profile = database
        .create_auth_profile(rd_db::NewAuthProfile {
            name: "youtube".to_owned(),
            scope: rd_core::AuthScope::parse("youtube.com", true).expect("scope"),
            method: rd_core::AuthMethod::Cookies,
            origin: rd_core::AuthOrigin::Manual,
            enabled: true,
            expires_at: None,
            username: None,
            secret_ref: Some(secret_ref),
            certificate_ref: None,
        })
        .await
        .expect("profile");

    let destination = temp.path().join("pkg");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "clip".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let selection = rd_core::MediaSelection {
        page_url: "https://www.youtube.com/watch?v=abc".parse().expect("url"),
        variant_id: "best".to_owned(),
        format: "b".to_owned(),
        kind: MediaKind::Video,
        ext: "mp4".to_owned(),
        title: "clip".to_owned(),
        contract_version: rd_core::MEDIA_CONTRACT_VERSION,
        criteria: None,
        resolved: None,
    };
    let file = database
        .create_download(NewDownload {
            id: rd_core::DownloadId::new(),
            package_id: package.id,
            source: selection.page_url.clone(),
            file_name: "clip.mp4".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Pinned(profile.id),
            initial_state: DownloadState::Queued,
            kind: DownloadKind::Media,
            media: Some(selection),
            replay: None,
            remote_credential_id: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    let runner = MediaRunner::new(database.clone(), secrets, settings());
    let outcome = runner
        .run(&file, &package, CancellationToken::new(), test_limits())
        .await
        .expect("run");
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "unexpected outcome {outcome:?}"
    );

    let argv = std::fs::read_to_string(destination.join("ytdlp-args.txt")).expect("argv");
    let args: Vec<&str> = argv.split_whitespace().collect();
    let index = args
        .iter()
        .position(|arg| *arg == "--cookies")
        .expect("--cookies was not passed");
    let cookie_path = std::path::PathBuf::from(args[index + 1]);

    // The credential travelled as a path, never as an argument value.
    assert!(
        !argv.contains("top-secret-session"),
        "cookies leaked into argv"
    );

    // What the process actually received, captured by the fake while it still existed.
    let handed_over =
        std::fs::read_to_string(destination.join("ytdlp-cookies.txt")).expect("cookie copy");
    assert!(handed_over.contains("top-secret-session"));
    // The other domain sharing the browser jar must not have come along.
    assert!(
        !handed_over.contains("follow-me"),
        "a foreign domain's cookie was handed to the extractor:\n{handed_over}"
    );

    // And the file is gone now that the run has returned.
    assert!(
        !cookie_path.exists(),
        "cookie file {} outlived the download",
        cookie_path.display()
    );
}

#[tokio::test]
async fn runner_downloads_audio_reports_progress_and_final_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("media.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("pkg");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "clip".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let selection = rd_core::MediaSelection {
        page_url: "https://www.youtube.com/watch?v=abc".parse().expect("url"),
        variant_id: "audio_mp3".to_owned(),
        format: "ba/b".to_owned(),
        kind: MediaKind::Audio,
        ext: "mp3".to_owned(),
        title: "clip".to_owned(),
        contract_version: rd_core::MEDIA_CONTRACT_VERSION,
        criteria: rd_core::MediaFormatCriteria::preset("audio_mp3").map(Box::new),
        resolved: None,
    };
    let file = database
        .create_download(NewDownload {
            id: rd_core::DownloadId::new(),
            package_id: package.id,
            source: selection.page_url.clone(),
            file_name: "clip.mp3".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: DownloadState::Queued,
            kind: DownloadKind::Media,
            media: Some(selection),
            replay: None,
            remote_credential_id: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    assert_eq!(file.kind, DownloadKind::Media);
    let runner = MediaRunner::new(database.clone(), secrets(temp.path()).await, settings());
    let outcome = runner
        .run(&file, &package, CancellationToken::new(), test_limits())
        .await
        .expect("run");
    match outcome {
        RunOutcome::Completed { final_name } => assert_eq!(final_name, "clip.mp3"),
        other => panic!("unexpected outcome {other:?}"),
    }
    assert!(destination.join("clip.mp3").is_file());
    let stored = database
        .get_download(file.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(stored.committed_bytes.get(), 102_400);
    assert_eq!(stored.total_bytes.map(|v| v.get()), Some(102_400));
    // `--print` puts yt-dlp into quiet mode, which also hides the progress lines the runner
    // parses; without `--progress` a media download shows no progress at all.
    let args = std::fs::read_to_string(destination.join("ytdlp-args.txt")).expect("args");
    assert!(args.contains("--progress"), "missing --progress in {args}");
}

#[tokio::test]
async fn runner_stops_when_cancelled() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("media.sqlite"))
        .await
        .expect("database");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "slow".to_owned(),
            destination: temp.path().join("slow").to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let selection = rd_core::MediaSelection {
        page_url: "https://www.youtube.com/watch?v=slow".parse().expect("url"),
        variant_id: "best".to_owned(),
        format: "bv*+ba/b".to_owned(),
        kind: MediaKind::Video,
        ext: "mp4".to_owned(),
        title: "slow".to_owned(),
        // Shaped exactly like a row written before the format selector existed.
        contract_version: 0,
        criteria: None,
        resolved: None,
    };
    let file = database
        .create_download(NewDownload {
            id: rd_core::DownloadId::new(),
            package_id: package.id,
            source: selection.page_url.clone(),
            file_name: "slow.mp4".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: DownloadState::Queued,
            kind: DownloadKind::Media,
            media: Some(selection),
            replay: None,
            remote_credential_id: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    let runner = MediaRunner::new(
        database,
        secrets(temp.path()).await,
        settings_for("fake-yt-dlp-slow"),
    );
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        cancel.cancel();
    });
    let started = std::time::Instant::now();
    let outcome = runner
        .run(&file, &package, token, test_limits())
        .await
        .expect("run");
    assert!(matches!(outcome, RunOutcome::Stopped));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

/// A merged video arrives as two streams that each count from 0 to 100 %. The reported
/// progress has to keep growing across the switch and end at the merged file's size.
#[tokio::test]
async fn runner_merges_two_streams_and_reports_the_full_size() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("media.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("clip");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "clip".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let selection = rd_core::MediaSelection {
        page_url: "https://www.youtube.com/watch?v=abc".parse().expect("url"),
        variant_id: "1080p".to_owned(),
        format: "bv*[height<=1080]+ba/b[height<=1080]".to_owned(),
        kind: MediaKind::Video,
        ext: "mp4".to_owned(),
        title: "clip".to_owned(),
        // A legacy preset row: no criteria stored, resolved through the preset bridge.
        contract_version: 0,
        criteria: None,
        resolved: None,
    };
    let file = database
        .create_download(NewDownload {
            id: rd_core::DownloadId::new(),
            package_id: package.id,
            source: selection.page_url.clone(),
            file_name: "clip.mp4".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: DownloadState::Queued,
            kind: DownloadKind::Media,
            media: Some(selection),
            replay: None,
            remote_credential_id: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    let runner = MediaRunner::new(database.clone(), secrets(temp.path()).await, settings());
    let outcome = runner
        .run(&file, &package, CancellationToken::new(), test_limits())
        .await
        .expect("run");
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );

    let stored = database
        .get_download(file.id)
        .await
        .expect("get")
        .expect("exists");
    // 300 KiB merged output, not the 200 KiB of the last stream alone.
    assert_eq!(stored.committed_bytes.get(), 307_200);
    assert_eq!(stored.total_bytes.map(|value| value.get()), Some(307_200));
}

/// RD-102-03: a yt-dlp below the floor this build supports stops media probing, names the
/// capability it stopped, and stops nothing else.
///
/// The fixture answers every other call exactly as the working one does, so reaching a
/// failure here can only be the version gate.
#[tokio::test]
async fn a_yt_dlp_below_the_supported_floor_blocks_only_media_probing() {
    let probe = YtDlpProbe::new(settings_for("fake-yt-dlp-ancient"));
    let failure = probe
        .probe(&"https://www.youtube.com/watch?v=abc".parse().expect("url"))
        .await
        .expect_err("an unsupported yt-dlp must not probe");
    assert_eq!(failure.code.as_deref(), Some("media.tool_incompatible"));
    assert_eq!(
        failure.params.get("capability").map(String::as_str),
        Some("media_download"),
        "the failure has to say which capability it took away"
    );
    assert_eq!(
        failure.params.get("tool").map(String::as_str),
        Some("yt-dlp")
    );
    assert_eq!(
        failure.params.get("version").map(String::as_str),
        Some("2019.01.01")
    );
    assert!(
        failure.params.contains_key("min_version"),
        "the upgrade path has to be part of the failure"
    );

    // Nothing global was switched off: the same probe against a supported version still runs.
    let working = YtDlpProbe::new(settings());
    assert_eq!(
        working
            .probe(&"https://www.youtube.com/watch?v=abc".parse().expect("url"))
            .await
            .expect("a supported yt-dlp still probes")
            .len(),
        1
    );
}
