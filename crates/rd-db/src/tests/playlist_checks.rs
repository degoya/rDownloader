//! What the check of a playlist leaves in the LinkGrabber (RD-1250-03): packages named after the
//! playlists, and a failure's message with the parameters it is translated with.

use rd_core::{IngressSource, LinkCandidate, MediaCandidate, MediaCandidateState, MediaInfo};

use crate::Database;

/// The media metadata the probe answers for one entry, from a playlist or from none.
fn entry(video: &str, playlist: Option<&str>) -> MediaInfo {
    serde_json::from_value(serde_json::json!({
        "title": format!("Video {video}"),
        "page_url": format!("https://www.youtube.com/watch?v={video}"),
        "variants": [],
        "selected": "best",
        "playlist_title": playlist,
    }))
    .expect("media info")
}

/// What `check_media` does with a probe answer: the first entry onto the link, the rest beside it.
async fn record_listing(database: &Database, candidate: &LinkCandidate, entries: Vec<MediaInfo>) {
    let mut entries = entries.into_iter();
    let first = entries.next().expect("one entry");
    database
        .record_candidate_check(
            candidate.id,
            Some(rd_core::LinkCheckResult {
                url: candidate.url.clone(),
                status: rd_core::LinkStatus::Online,
                file_name: Some(crate::media_file_name(&first)),
                size: None,
                media: Some(first),
            }),
            None,
            false,
            None,
        )
        .await
        .expect("record");
    let rest: Vec<MediaCandidate> = entries
        .map(|info| MediaCandidate {
            info,
            state: MediaCandidateState::default(),
        })
        .collect();
    if !rest.is_empty() {
        database
            .add_media_candidates(candidate.package_id.expect("package"), rest)
            .await
            .expect("entries");
    }
}

/// Two playlists and a single video pasted together arrive as one package named after the
/// host; the regroup after the check gives each playlist its own package under its title,
/// still auto-named so the package-name rules apply, and leaves the single video where it is.
#[tokio::test]
async fn each_playlist_becomes_a_package_named_after_its_title() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("playlists.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = [
        "https://www.youtube.com/playlist?list=PLfirst",
        "https://www.youtube.com/playlist?list=PLsecond",
        "https://www.youtube.com/watch?v=single",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (batch, packages, candidates) = database
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
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 1, "one package for the host at intake");

    let first = Some("Blender Fundamentals 2.8");
    let second = Some("Second List");
    record_listing(
        &database,
        &candidates[0],
        vec![entry("a1", first), entry("a2", first), entry("a3", first)],
    )
    .await;
    record_listing(
        &database,
        &candidates[1],
        vec![entry("b1", second), entry("b2", second)],
    )
    .await;
    record_listing(&database, &candidates[2], vec![entry("single", None)]).await;

    database
        .regroup_collector_batches(vec![batch.id])
        .await
        .expect("regroup");

    let regrouped = database.list_collector_packages().await.expect("packages");
    let listed = database.list_candidates().await.expect("candidates");
    let members = |name: &str| {
        let package = regrouped
            .iter()
            .find(|package| package.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "no package {name:?}, got {:?}",
                    regrouped
                        .iter()
                        .map(|package| package.name.as_str())
                        .collect::<Vec<_>>()
                )
            });
        assert!(package.auto_named, "{name}: a derived name, rules apply");
        listed
            .iter()
            .filter(|candidate| candidate.package_id == Some(package.id))
            .count()
    };
    assert_eq!(
        members("Blender Fundamentals 2.8"),
        3,
        "the playlist link and two entries"
    );
    assert_eq!(members("Second List"), 2);
    assert_eq!(
        regrouped.len(),
        3,
        "the single video keeps a package of its own"
    );
    let single = listed
        .iter()
        .find(|candidate| candidate.id == candidates[2].id)
        .expect("single video");
    let single_package = regrouped
        .iter()
        .find(|package| Some(package.id) == single.package_id)
        .expect("its package");
    assert!(
        !matches!(
            single_package.name.as_str(),
            "Blender Fundamentals 2.8" | "Second List"
        ),
        "a video from no playlist joins none"
    );
}

/// A failed check keeps the parameters its message is translated with (RD-1250-03), redacted,
/// and the next conclusive answer clears them with the message.
///
/// Before the column the row held code and English text only, and the interface either left
/// `{detail}` empty, a sentence that ends in its colon, or fell back to the English text.
#[tokio::test]
async fn a_failed_check_keeps_its_message_parameters() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("check-params.sqlite"))
        .await
        .expect("database");
    let (_batch, _packages, candidates) = database
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
            providers: vec![None],
            urls: vec![
                "https://www.youtube.com/playlist?list=PLgone"
                    .parse()
                    .expect("url"),
            ],
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    let id = candidates[0].id;
    let detail = "ERROR: [youtube] gjTzGhcQ21E: Video unavailable";
    let failure = rd_core::Failure::coded(
        rd_core::FailureKind::Permanent,
        "media.ytdlp_failed",
        format!("yt-dlp failed: {detail}"),
    )
    .with_param("detail", detail)
    .with_param("proxy", "socks5h://alice:pr0xy@proxy.example.com:1080");
    database
        .record_candidate_check(id, None, Some((&failure).into()), false, None)
        .await
        .expect("record");
    let listed = database.list_candidates().await.expect("candidates");
    let row = listed.iter().find(|c| c.id == id).expect("row");
    assert_eq!(row.error_code.as_deref(), Some("media.ytdlp_failed"));
    assert_eq!(
        row.error_params.get("detail").map(String::as_str),
        Some(detail)
    );
    let proxy = row.error_params.get("proxy").expect("proxy param");
    assert!(
        !proxy.contains("pr0xy"),
        "a parameter is redacted like the text: {proxy}"
    );

    database
        .claim_candidates_for_check(vec![id])
        .await
        .expect("claim");
    database
        .record_candidate_check(
            id,
            Some(rd_core::LinkCheckResult {
                url: candidates[0].url.clone(),
                status: rd_core::LinkStatus::Online,
                file_name: Some("Video.mp4".to_owned()),
                size: None,
                media: None,
            }),
            None,
            false,
            None,
        )
        .await
        .expect("record");
    let listed = database.list_candidates().await.expect("candidates");
    let row = listed.iter().find(|c| c.id == id).expect("row");
    assert_eq!(row.error_code, None);
    assert!(
        row.error_params.is_empty(),
        "the parameters go with the message"
    );
}
