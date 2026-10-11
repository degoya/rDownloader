//! How ffmpeg and ffprobe are paired for yt-dlp (RD-102-02, RD-1240-33).

use std::path::{Path, PathBuf};

use rd_core::{ResolvedTool, ToolSource};

use super::FfmpegTools;

fn tool(path: &str) -> Option<ResolvedTool> {
    Some(ResolvedTool {
        path: path.into(),
        source: ToolSource::Path,
    })
}

/// A lookup result as `rd_core::locate_tool_leased` returns it, without a lease.
fn found(path: PathBuf, source: ToolSource) -> Option<(ResolvedTool, Option<rd_core::ToolLease>)> {
    Some((ResolvedTool { path, source }, None))
}

/// Creates `names` as empty files in `directory` and returns their paths.
fn binaries(directory: &Path, names: &[&str]) -> Vec<PathBuf> {
    std::fs::create_dir_all(directory).expect("directory");
    names
        .iter()
        .map(|name| {
            let path = directory.join(name);
            std::fs::write(&path, b"").expect("binary");
            path
        })
        .collect()
}

#[test]
fn location_is_the_shared_directory_or_else_the_ffmpeg_binary() {
    let split = FfmpegTools {
        ffmpeg: tool("/usr/bin/ffmpeg"),
        ffprobe: tool("/usr/local/bin/ffprobe"),
        leases: Vec::new(),
    };
    assert!(split.is_complete());
    assert_eq!(
        split.location().expect("ffmpeg binary"),
        Path::new("/usr/bin/ffmpeg")
    );

    let together = FfmpegTools {
        ffmpeg: tool("/opt/vendor/ffmpeg"),
        ffprobe: tool("/opt/vendor/ffprobe"),
        leases: Vec::new(),
    };
    assert_eq!(
        together.location().expect("shared directory").as_os_str(),
        "/opt/vendor"
    );

    let missing = FfmpegTools {
        ffmpeg: tool("/usr/bin/ffmpeg"),
        ffprobe: None,
        leases: Vec::new(),
    };
    assert!(!missing.is_complete());
    assert!(missing.location().is_none());
}

/// The managed store keeps ffmpeg and ffprobe in one version folder each; with nothing
/// better around, yt-dlp is pointed at the ffmpeg binary rather than left without one.
#[test]
fn split_store_folders_hand_over_the_ffmpeg_binary() {
    let root = tempfile::tempdir().expect("tempdir");
    let ffmpeg = binaries(&root.path().join("tools/ffmpeg/9.0.1"), &["ffmpeg"]);
    let ffprobe = binaries(&root.path().join("tools/ffprobe/9.0.1"), &["ffprobe"]);
    let tools = FfmpegTools::pair(
        found(ffmpeg[0].clone(), ToolSource::Managed),
        found(ffprobe[0].clone(), ToolSource::Managed),
        Vec::new(),
    );
    assert!(tools.is_complete());
    assert_eq!(tools.location(), Some(ffmpeg[0].as_path()));
}

/// The managed pair takes precedence over a folder on `PATH` holding both (owner, 2026-10-10,
/// RD-1240-33): `/usr/bin` had won over an installed and active managed pair.
#[test]
fn the_managed_pair_beats_a_path_folder_holding_both() {
    let root = tempfile::tempdir().expect("tempdir");
    let ffmpeg = binaries(&root.path().join("tools/ffmpeg/9.0.2"), &["ffmpeg"]);
    let ffprobe = binaries(&root.path().join("tools/ffprobe/9.0.2"), &["ffprobe"]);
    let system = root.path().join("usr/bin");
    binaries(&system, &["ffmpeg", "ffprobe"]);
    let tools = FfmpegTools::pair(
        found(ffmpeg[0].clone(), ToolSource::Managed),
        found(ffprobe[0].clone(), ToolSource::Managed),
        vec![(system, ToolSource::Path)],
    );
    let resolved_ffmpeg = tools.ffmpeg.as_ref().expect("ffmpeg");
    let resolved_ffprobe = tools.ffprobe.as_ref().expect("ffprobe");
    assert_eq!(resolved_ffmpeg.path, ffmpeg[0]);
    assert_eq!(resolved_ffmpeg.source, ToolSource::Managed);
    assert_eq!(resolved_ffprobe.path, ffprobe[0]);
    assert_eq!(resolved_ffprobe.source, ToolSource::Managed);
    assert_eq!(tools.location(), Some(ffmpeg[0].as_path()));
}

/// Only an ffmpeg path the person set wins over the managed pair.
#[test]
fn an_explicit_ffmpeg_beats_the_managed_pair() {
    let root = tempfile::tempdir().expect("tempdir");
    let configured = binaries(&root.path().join("custom"), &["ffmpeg"]);
    let managed = binaries(&root.path().join("tools/ffprobe/9.0.2"), &["ffprobe"]);
    let tools = FfmpegTools::pair(
        found(configured[0].clone(), ToolSource::Explicit),
        found(managed[0].clone(), ToolSource::Managed),
        Vec::new(),
    );
    let resolved_ffmpeg = tools.ffmpeg.as_ref().expect("ffmpeg");
    assert_eq!(resolved_ffmpeg.path, configured[0]);
    assert_eq!(resolved_ffmpeg.source, ToolSource::Explicit);
}

/// A managed ffmpeg without a managed ffprobe is split as before: a directory holding both
/// beats the split pair.
#[test]
fn a_managed_ffmpeg_without_a_managed_ffprobe_is_paired_as_before() {
    let root = tempfile::tempdir().expect("tempdir");
    let ffmpeg = binaries(&root.path().join("tools/ffmpeg/9.0.1"), &["ffmpeg"]);
    let ffprobe = binaries(&root.path().join("elsewhere"), &["ffprobe"]);
    let empty = root.path().join("empty");
    std::fs::create_dir_all(&empty).expect("directory");
    let vendor = root.path().join("vendor");
    let pair = binaries(&vendor, &["ffmpeg", "ffprobe"]);
    let tools = FfmpegTools::pair(
        found(ffmpeg[0].clone(), ToolSource::Managed),
        found(ffprobe[0].clone(), ToolSource::Path),
        vec![
            (empty, ToolSource::Vendor),
            (vendor.clone(), ToolSource::Vendor),
        ],
    );
    let resolved_ffmpeg = tools.ffmpeg.as_ref().expect("ffmpeg");
    let resolved_ffprobe = tools.ffprobe.as_ref().expect("ffprobe");
    assert_eq!(resolved_ffmpeg.path, pair[0]);
    assert_eq!(resolved_ffmpeg.source, ToolSource::Vendor);
    assert_eq!(resolved_ffprobe.path, pair[1]);
    assert_eq!(tools.location(), Some(vendor.as_path()));
}

#[test]
fn a_pair_that_already_shares_a_directory_is_kept() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = binaries(
        &root.path().join("tools/ffmpeg/9.0.1"),
        &["ffmpeg", "ffprobe"],
    );
    let vendor = root.path().join("vendor");
    binaries(&vendor, &["ffmpeg", "ffprobe"]);
    let tools = FfmpegTools::pair(
        found(store[0].clone(), ToolSource::Managed),
        found(store[1].clone(), ToolSource::Managed),
        vec![(vendor, ToolSource::Vendor)],
    );
    assert_eq!(tools.ffmpeg.as_ref().expect("ffmpeg").path, store[0]);
    assert_eq!(tools.location(), store[0].parent());
}

#[test]
fn ffmpegs_own_sibling_ffprobe_wins_over_one_found_elsewhere() {
    let root = tempfile::tempdir().expect("tempdir");
    let configured = binaries(&root.path().join("custom"), &["ffmpeg", "ffprobe"]);
    let managed = binaries(&root.path().join("tools/ffprobe/9.0.1"), &["ffprobe"]);
    let tools = FfmpegTools::pair(
        found(configured[0].clone(), ToolSource::Explicit),
        found(managed[0].clone(), ToolSource::Managed),
        Vec::new(),
    );
    let resolved_ffprobe = tools.ffprobe.as_ref().expect("ffprobe");
    assert_eq!(resolved_ffprobe.path, configured[1]);
    assert_eq!(resolved_ffprobe.source, ToolSource::Explicit);
    assert_eq!(tools.location(), configured[0].parent());
}

#[test]
fn an_explicit_ffmpeg_is_not_swapped_for_a_shared_directory() {
    let root = tempfile::tempdir().expect("tempdir");
    let configured = binaries(&root.path().join("custom"), &["ffmpeg"]);
    let managed = binaries(&root.path().join("tools/ffprobe/9.0.1"), &["ffprobe"]);
    let vendor = root.path().join("vendor");
    binaries(&vendor, &["ffmpeg", "ffprobe"]);
    let tools = FfmpegTools::pair(
        found(configured[0].clone(), ToolSource::Explicit),
        found(managed[0].clone(), ToolSource::Managed),
        vec![(vendor, ToolSource::Vendor)],
    );
    assert_eq!(tools.ffmpeg.as_ref().expect("ffmpeg").path, configured[0]);
    assert_eq!(tools.location(), Some(configured[0].as_path()));
}

#[test]
fn without_ffprobe_nothing_is_paired() {
    let root = tempfile::tempdir().expect("tempdir");
    let ffmpeg = binaries(&root.path().join("tools/ffmpeg/9.0.1"), &["ffmpeg"]);
    let vendor = root.path().join("vendor");
    binaries(&vendor, &["ffmpeg", "ffprobe"]);
    let tools = FfmpegTools::pair(
        found(ffmpeg[0].clone(), ToolSource::Managed),
        None,
        vec![(vendor, ToolSource::Vendor)],
    );
    assert!(!tools.is_complete());
    assert!(tools.location().is_none());
}

/// Both binaries resolved, but nothing yt-dlp could be pointed at: no merge is offered,
/// so no selector asks for one that would leave two stream files behind.
#[tokio::test]
async fn merging_is_not_offered_without_a_location() {
    let unreachable = FfmpegTools {
        ffmpeg: tool("/"),
        ffprobe: tool("/"),
        leases: Vec::new(),
    };
    assert!(unreachable.is_complete());
    assert!(unreachable.location().is_none());
    let capabilities = unreachable.media_capabilities().await;
    assert!(!capabilities.can_merge);
    assert!(!capabilities.can_transcode_audio);
}
