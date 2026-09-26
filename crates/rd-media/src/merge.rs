//! Recognises a download whose video and audio streams yt-dlp fetched but did not merge.
//!
//! yt-dlp only merges when it can run ffmpeg. When it cannot, it says so on stderr and, with
//! errors ignored or in older versions, still exits successfully: the target file is never
//! written, and the separate `<stem>.f<format>.<ext>` stream files are left beside it. The
//! download selection already avoids merge formats when ffmpeg is known to be missing, so this
//! is the case where ffmpeg was configured but yt-dlp could not reach it. Counting that as a
//! completed download hands post-processing two half files under names nobody chose.

use std::path::Path;

use rd_core::{Failure, FailureKind};

/// What yt-dlp prints when a merge was requested and ffmpeg could not be run, both as the
/// warning ("The formats won't be merged") and as the error ("Aborting due to
/// --abort-on-error").
const MERGE_SKIPPED: &str = "requested merging of multiple formats but ffmpeg is not installed";

/// Whether yt-dlp's stderr reports a merge it skipped for lack of ffmpeg.
pub(crate) fn merge_skipped(stderr: &str) -> bool {
    stderr
        .lines()
        .any(|line| line.to_ascii_lowercase().contains(MERGE_SKIPPED))
}

/// The failure for a download whose streams were not merged.
///
/// `Unsupported`, not transient: another attempt reaches the same missing ffmpeg, and what
/// helps is fixing the tool configuration.
pub(crate) fn merge_failure() -> Failure {
    Failure::coded(
        FailureKind::Unsupported,
        "media.merge_ffmpeg_unreachable",
        "yt-dlp could not reach ffmpeg, so video and audio were downloaded as separate files \
         and not merged",
    )
}

/// Whether `name` is one of the per-format stream files yt-dlp writes for `target`, e.g.
/// `clip.f137.mp4` for `clip.mp4`. Unfinished fragments (`.part`, `.ytdl`) do not count.
pub(crate) fn is_stream_file(target: &Path, name: &str) -> bool {
    let Some(stem) = target.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    let Some(rest) = name
        .strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix(".f"))
    else {
        return false;
    };
    let Some((format, extension)) = rest.split_once('.') else {
        return false;
    };
    let format_ok = !format.is_empty()
        && format
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let extension_ok =
        !extension.is_empty() && extension.chars().all(|c| c.is_ascii_alphanumeric());
    format_ok && extension_ok && !matches!(extension, "part" | "ytdl")
}

/// Whether yt-dlp reported `target` as the result but left only separate stream files.
///
/// A target that exists was merged, whatever else is lying next to it.
pub(crate) async fn left_unmerged(target: &Path) -> bool {
    if tokio::fs::try_exists(target).await.unwrap_or(true) {
        return false;
    }
    let Some(directory) = target.parent() else {
        return false;
    };
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return false;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| is_stream_file(target, name))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{is_stream_file, left_unmerged, merge_failure, merge_skipped};

    #[test]
    fn the_merge_warning_and_the_merge_error_are_both_recognised() {
        assert!(merge_skipped(
            "WARNING: You have requested merging of multiple formats but ffmpeg is not \
             installed. The formats won't be merged"
        ));
        assert!(merge_skipped(
            "[youtube] abc: Downloading webpage\n\
             ERROR: You have requested merging of multiple formats but ffmpeg is not \
             installed. Aborting due to --abort-on-error"
        ));
    }

    #[test]
    fn other_ffmpeg_warnings_are_not_a_skipped_merge() {
        assert!(!merge_skipped(""));
        assert!(!merge_skipped(
            "WARNING: ffmpeg not found. The downloaded format may not be the best available."
        ));
        assert!(!merge_skipped("[Merger] Merging formats into \"clip.mp4\""));
    }

    #[test]
    fn the_failure_carries_its_stable_code() {
        let failure = merge_failure();
        assert_eq!(
            failure.code.as_deref(),
            Some("media.merge_ffmpeg_unreachable")
        );
        assert_eq!(failure.category, rd_core::FailureKind::Unsupported);
    }

    #[test]
    fn only_finished_per_format_files_of_the_target_are_stream_files() {
        let target = Path::new("/downloads/pkg/clip.mp4");
        assert!(is_stream_file(target, "clip.f137.mp4"));
        assert!(is_stream_file(target, "clip.f251.webm"));
        assert!(is_stream_file(target, "clip.fhls-720p.mp4"));
        assert!(!is_stream_file(target, "clip.mp4"));
        assert!(!is_stream_file(target, "clip.f137.mp4.part"));
        assert!(!is_stream_file(target, "clip.f137.part"));
        assert!(!is_stream_file(target, "other.f137.mp4"));
        assert!(!is_stream_file(target, "clip.f.mp4"));
        assert!(!is_stream_file(target, "clip.f137"));
    }

    #[tokio::test]
    async fn a_missing_target_with_stream_files_beside_it_is_left_unmerged() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("clip.mp4");
        std::fs::write(temp.path().join("clip.f137.mp4"), b"video").expect("video");
        std::fs::write(temp.path().join("clip.f251.webm"), b"audio").expect("audio");
        assert!(left_unmerged(&target).await);

        // Once the merged file exists the leftovers do not matter.
        std::fs::write(&target, b"merged").expect("merged");
        assert!(!left_unmerged(&target).await);
    }

    #[tokio::test]
    async fn a_missing_target_without_stream_files_is_not_a_skipped_merge() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert!(!left_unmerged(&temp.path().join("clip.mp4")).await);
    }
}
