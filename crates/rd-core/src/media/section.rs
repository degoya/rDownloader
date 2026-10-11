//! A part of a video instead of the whole, and the pauses a download keeps between its
//! requests (RD-1240-15).
//!
//! Both are handed to yt-dlp as they are — `--download-sections`, `--sleep-requests`,
//! `--sleep-interval` — and nothing here cuts or waits on its own. They live beside the
//! criteria because they are per-job choices stored with them, not format filters.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::criteria::CriteriaError;

/// Latest position a section may name, in seconds: a week, past any video a site serves.
pub const MAX_SECTION_SECONDS: u32 = 7 * 24 * 60 * 60;
/// Longest pause accepted, in seconds. Ten minutes between two requests is already a crawl;
/// more reads as a typo, not as courtesy.
pub const MAX_PAUSE_SECONDS: u32 = 600;

/// The part of a video to download, by its start and end in seconds.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct MediaSection {
    /// Where the part begins; `None` is the beginning of the video.
    pub start_seconds: Option<u32>,
    /// Where the part ends; `None` is the end of the video.
    pub end_seconds: Option<u32>,
}

impl MediaSection {
    /// The section as stored: `None` when it names the whole video, refused when it ends
    /// before it begins or lies past [`MAX_SECTION_SECONDS`].
    pub fn sanitized(self) -> Result<Option<Self>, CriteriaError> {
        let start = self.start_seconds.filter(|seconds| *seconds > 0);
        let end = self.end_seconds;
        if start.is_none() && end.is_none() {
            return Ok(None);
        }
        if [start, end]
            .into_iter()
            .flatten()
            .any(|seconds| seconds > MAX_SECTION_SECONDS)
        {
            return Err(CriteriaError::Value { field: "section" });
        }
        if let Some(end) = end
            && start.unwrap_or(0) >= end
        {
            return Err(CriteriaError::Range { field: "section" });
        }
        Ok(Some(Self {
            start_seconds: start,
            end_seconds: end,
        }))
    }

    /// The value of yt-dlp's `--download-sections`: `*START-END` in seconds, `inf` for an
    /// open end. The leading `*` makes it a time range rather than a chapter-title regex.
    #[must_use]
    pub fn download_sections(&self) -> String {
        let start = self.start_seconds.unwrap_or(0);
        match self.end_seconds {
            Some(end) => format!("*{start}-{end}"),
            None => format!("*{start}-inf"),
        }
    }
}

/// Pauses yt-dlp keeps so a site is not asked too fast; `0` is no pause.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct MediaPauses {
    /// Seconds between the requests made while a page is read (`--sleep-requests`).
    pub sleep_requests_seconds: u32,
    /// Seconds before each download starts (`--sleep-interval`).
    pub sleep_interval_seconds: u32,
}

impl MediaPauses {
    /// No pause at all, the default.
    pub const NONE: Self = Self {
        sleep_requests_seconds: 0,
        sleep_interval_seconds: 0,
    };

    /// The pauses as stored, refused above [`MAX_PAUSE_SECONDS`].
    pub const fn sanitized(self) -> Result<Self, CriteriaError> {
        if self.sleep_requests_seconds > MAX_PAUSE_SECONDS
            || self.sleep_interval_seconds > MAX_PAUSE_SECONDS
        {
            return Err(CriteriaError::Value { field: "pauses" });
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{CriteriaError, MAX_PAUSE_SECONDS, MAX_SECTION_SECONDS, MediaPauses, MediaSection};

    fn section(start: Option<u32>, end: Option<u32>) -> MediaSection {
        MediaSection {
            start_seconds: start,
            end_seconds: end,
        }
    }

    #[test]
    fn a_section_naming_the_whole_video_is_none() {
        assert_eq!(section(None, None).sanitized(), Ok(None));
        assert_eq!(section(Some(0), None).sanitized(), Ok(None));
    }

    #[test]
    fn a_section_is_written_as_a_time_range() {
        let both = section(Some(90), Some(150)).sanitized().expect("valid");
        assert_eq!(
            both.map(|s| s.download_sections()).as_deref(),
            Some("*90-150")
        );
        let open_end = section(Some(90), None).sanitized().expect("valid");
        assert_eq!(
            open_end.map(|s| s.download_sections()).as_deref(),
            Some("*90-inf")
        );
        let from_start = section(Some(0), Some(30)).sanitized().expect("valid");
        assert_eq!(
            from_start,
            Some(section(None, Some(30))),
            "a zero start is the beginning"
        );
        assert_eq!(
            from_start.map(|s| s.download_sections()).as_deref(),
            Some("*0-30")
        );
    }

    #[test]
    fn a_section_ending_before_it_begins_or_past_the_limit_is_refused() {
        let range = Err(CriteriaError::Range { field: "section" });
        assert_eq!(section(Some(60), Some(60)).sanitized(), range);
        assert_eq!(section(Some(61), Some(60)).sanitized(), range);
        assert_eq!(section(None, Some(0)).sanitized(), range);
        let value = Err(CriteriaError::Value { field: "section" });
        assert_eq!(
            section(Some(MAX_SECTION_SECONDS + 1), None).sanitized(),
            value
        );
        assert_eq!(
            section(None, Some(MAX_SECTION_SECONDS + 1)).sanitized(),
            value
        );
    }

    #[test]
    fn pauses_above_the_limit_are_refused() {
        let pauses = MediaPauses {
            sleep_requests_seconds: MAX_PAUSE_SECONDS,
            sleep_interval_seconds: 5,
        };
        assert_eq!(pauses.sanitized(), Ok(pauses));
        assert_eq!(
            MediaPauses {
                sleep_interval_seconds: MAX_PAUSE_SECONDS + 1,
                ..MediaPauses::NONE
            }
            .sanitized(),
            Err(CriteriaError::Value { field: "pauses" })
        );
    }
}
