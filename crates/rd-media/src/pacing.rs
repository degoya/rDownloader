//! A part of the video and the pauses between requests, as yt-dlp flags (RD-1240-15).
//!
//! Kept apart from `crate::args` so the plan's other flags read as they did: this is one call
//! in `DownloadPlan::build`, emitted after the embed flags and before the `--` that ends the
//! options.

use std::ffi::OsString;

use rd_core::{MediaFormatCriteria, MediaPauses, MediaSettings};

use crate::args::DownloadPlan;

impl DownloadPlan<'_> {
    /// `--download-sections` for a section, `--sleep-requests`/`--sleep-interval` for a
    /// pause; nothing for a whole video without pauses.
    ///
    /// The section is cut where yt-dlp and ffmpeg cut it, at the nearest keyframe: no
    /// `--force-keyframes-at-cuts`, which would re-encode the whole part.
    pub(crate) fn push_pacing_args(&self, args: &mut Vec<OsString>) {
        let mut push = |value: String| args.push(OsString::from(value));
        if let Some(section) = self.section {
            push("--download-sections".to_owned());
            push(section.download_sections());
        }
        if self.pauses.sleep_requests_seconds > 0 {
            push("--sleep-requests".to_owned());
            push(self.pauses.sleep_requests_seconds.to_string());
        }
        if self.pauses.sleep_interval_seconds > 0 {
            push("--sleep-interval".to_owned());
            push(self.pauses.sleep_interval_seconds.to_string());
        }
    }
}

/// The pauses one download keeps: its own when the job has them, the configured ones
/// otherwise.
#[must_use]
pub(crate) fn job_pauses(
    criteria: Option<&MediaFormatCriteria>,
    settings: &MediaSettings,
) -> MediaPauses {
    criteria
        .and_then(|criteria| criteria.pauses)
        .unwrap_or_else(|| settings.pauses())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rd_core::{
        MediaEmbedPolicy, MediaFormatCriteria, MediaOutput, MediaPauses, MediaSection,
        MediaSettings, TrackSelection,
    };

    use super::job_pauses;
    use crate::args::DownloadPlan;

    fn args(section: Option<MediaSection>, pauses: MediaPauses) -> Vec<String> {
        DownloadPlan {
            format: "b",
            output: Path::new("/downloads/pkg/clip.%(ext)s"),
            ffmpeg_location: None,
            cookies: None,
            limit_rate: None,
            output_mode: &MediaOutput::Passthrough,
            tracks: &TrackSelection::default(),
            embed: &MediaEmbedPolicy::default(),
            section,
            pauses,
            page_url: "https://example.test/clip",
        }
        .build()
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect()
    }

    fn pair(args: &[String], flag: &str) -> Option<String> {
        args.windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1].clone())
    }

    #[test]
    fn a_whole_video_without_pauses_adds_no_flag() {
        let args = args(None, MediaPauses::NONE);
        for flag in [
            "--download-sections",
            "--sleep-requests",
            "--sleep-interval",
        ] {
            assert!(!args.iter().any(|arg| arg == flag), "{flag} was emitted");
        }
    }

    #[test]
    fn a_section_and_pauses_come_before_the_end_of_the_options() {
        let section = MediaSection {
            start_seconds: Some(90),
            end_seconds: Some(150),
        };
        let pauses = MediaPauses {
            sleep_requests_seconds: 2,
            sleep_interval_seconds: 5,
        };
        let args = args(Some(section), pauses);
        assert_eq!(
            pair(&args, "--download-sections").as_deref(),
            Some("*90-150")
        );
        assert_eq!(pair(&args, "--sleep-requests").as_deref(), Some("2"));
        assert_eq!(pair(&args, "--sleep-interval").as_deref(), Some("5"));
        let end = args
            .iter()
            .position(|arg| arg == "--")
            .expect("options end");
        let sections = args
            .iter()
            .position(|arg| arg == "--download-sections")
            .expect("section flag");
        assert!(sections < end, "a flag after `--` would be read as a URL");
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://example.test/clip")
        );
    }

    #[test]
    fn only_the_pause_that_is_set_is_emitted() {
        let args = args(
            None,
            MediaPauses {
                sleep_requests_seconds: 0,
                sleep_interval_seconds: 3,
            },
        );
        assert_eq!(pair(&args, "--sleep-interval").as_deref(), Some("3"));
        assert!(!args.iter().any(|arg| arg == "--sleep-requests"));
    }

    #[test]
    fn a_jobs_own_pauses_win_over_the_configured_ones() {
        let settings = MediaSettings {
            media_sleep_requests_seconds: 4,
            media_sleep_interval_seconds: 8,
            ..MediaSettings::default()
        };
        let configured = MediaPauses {
            sleep_requests_seconds: 4,
            sleep_interval_seconds: 8,
        };
        assert_eq!(job_pauses(None, &settings), configured);
        let inherits = MediaFormatCriteria::default();
        assert_eq!(job_pauses(Some(&inherits), &settings), configured);
        let own = MediaFormatCriteria {
            pauses: Some(MediaPauses::NONE),
            ..MediaFormatCriteria::default()
        };
        assert_eq!(
            job_pauses(Some(&own), &settings),
            MediaPauses::NONE,
            "a job may switch the configured pauses off"
        );
    }
}
