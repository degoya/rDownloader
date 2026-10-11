//! A playlist address becomes one candidate per video (RD-1240-37).

use crate::select::MediaCapabilities;

/// `yt-dlp -J --flat-playlist --no-playlist --playlist-end 200` against a YouTube playlist,
/// 2026.08.19, trimmed to four entries; the two stand-ins are from another list's listing.
const FLAT_PLAYLIST: &str = r#"{
  "_type": "playlist",
  "id": "PLa1F2ddGya_-UvuAqHAksYnB0qL9yWDO6",
  "title": "Blender Fundamentals 2.8",
  "uploader": "Blender",
  "webpage_url": "https://www.youtube.com/playlist?list=PLa1F2ddGya_-UvuAqHAksYnB0qL9yWDO6",
  "extractor": "youtube:tab",
  "playlist_count": 43,
  "entries": [
    {"_type": "url", "ie_key": "Youtube", "id": "MF1qEhBSfq4",
     "url": "https://www.youtube.com/watch?v=MF1qEhBSfq4",
     "title": "First Steps - Blender 2.80 Fundamentals", "duration": 59, "uploader": "Blender",
     "thumbnails": [{"url": "https://i.ytimg.com/vi/MF1qEhBSfq4/hqdefault.jpg", "height": 94, "width": 168}]},
    {"_type": "url", "ie_key": "Youtube", "id": "Xh_SGzJgauM",
     "url": "https://www.youtube.com/watch?v=Xh_SGzJgauM",
     "title": "[Private video]", "duration": null, "uploader": null,
     "thumbnails": [{"url": "https://i.ytimg.com/img/no_thumbnail.jpg", "height": 90, "width": 120}]},
    {"_type": "url", "ie_key": "Youtube", "id": "ILqOWe3zAbk",
     "url": "https://www.youtube.com/watch?v=ILqOWe3zAbk",
     "title": "Viewport Navigation - Blender 2.80 Fundamentals", "duration": 222, "uploader": "Blender",
     "thumbnails": [{"url": "https://i.ytimg.com/vi/ILqOWe3zAbk/hqdefault.jpg", "height": 94, "width": 168}]},
    {"_type": "url", "ie_key": "Youtube", "id": "jQU_UhBWZFk",
     "url": "https://www.youtube.com/watch?v=jQU_UhBWZFk",
     "title": "[Deleted video]", "duration": null, "uploader": null,
     "thumbnails": [{"url": "https://i.ytimg.com/img/no_thumbnail.jpg", "height": 90, "width": 120}]}
  ]
}"#;

fn probed() -> url::Url {
    "https://www.youtube.com/playlist?list=PLa1F2ddGya_-UvuAqHAksYnB0qL9yWDO6"
        .parse()
        .expect("url")
}

#[test]
fn a_flat_youtube_listing_becomes_one_candidate_per_available_video() {
    let metadata: super::Metadata = serde_json::from_str(FLAT_PLAYLIST).expect("metadata");
    let candidates = super::candidates(metadata, &probed(), "best", MediaCapabilities::complete())
        .expect("candidates");
    let pages: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.info.page_url.as_str())
        .collect();
    assert_eq!(
        pages,
        [
            "https://www.youtube.com/watch?v=MF1qEhBSfq4",
            "https://www.youtube.com/watch?v=ILqOWe3zAbk",
        ],
        "the private and the deleted stand-in are never offered"
    );
    let first = &candidates[0].info;
    assert_eq!(first.title, "First Steps - Blender 2.80 Fundamentals");
    assert_eq!(first.duration_seconds, Some(59));
    assert_eq!(first.video_id.as_deref(), Some("MF1qEhBSfq4"));
    assert_eq!(
        first.thumbnail.as_deref(),
        Some("https://i.ytimg.com/vi/MF1qEhBSfq4/hqdefault.jpg")
    );
    assert!(
        first.selection().is_some(),
        "a flat entry still carries a selection (RD-120-50)"
    );
}

#[test]
fn a_listing_of_stand_ins_only_is_an_empty_playlist() {
    let metadata: super::Metadata = serde_json::from_value(serde_json::json!({
        "_type": "playlist",
        "title": "Gone",
        "entries": [
            {"_type": "url", "id": "Xh_SGzJgauM", "url": "https://www.youtube.com/watch?v=Xh_SGzJgauM",
             "title": "[Private video]", "duration": null},
        ],
    }))
    .expect("metadata");
    let failure = super::candidates(metadata, &probed(), "best", MediaCapabilities::complete())
        .expect_err("nothing to offer");
    assert_eq!(failure.code.as_deref(), Some("media.playlist_empty"));
}

/// A video that happens to be titled like a stand-in but has a length is a real video.
#[test]
fn only_a_stand_in_without_a_length_is_skipped() {
    let metadata: super::Metadata = serde_json::from_value(serde_json::json!({
        "_type": "playlist",
        "entries": [
            {"id": "a", "url": "https://www.youtube.com/watch?v=a", "title": "[Private video]", "duration": 12},
        ],
    }))
    .expect("metadata");
    let candidates = super::candidates(metadata, &probed(), "best", MediaCapabilities::complete())
        .expect("candidates");
    assert_eq!(candidates.len(), 1);
}

#[test]
fn a_single_page_stays_one_candidate() {
    let metadata: super::Metadata = serde_json::from_value(serde_json::json!({
        "_type": "video",
        "id": "MF1qEhBSfq4",
        "title": "First Steps - Blender 2.80 Fundamentals",
        "webpage_url": "https://www.youtube.com/watch?v=MF1qEhBSfq4",
        "formats": [],
    }))
    .expect("metadata");
    let probed: url::Url = "https://www.youtube.com/watch?v=MF1qEhBSfq4"
        .parse()
        .expect("url");
    let candidates = super::candidates(metadata, &probed, "best", MediaCapabilities::complete())
        .expect("candidates");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].info.page_url, probed);
}
