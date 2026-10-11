//! The body that sets a package's or a category's download window (RD-1240-30), and its one
//! check, shared by both routes so a window means the same on either.

use rd_core::{DownloadWindow, MAX_DOWNLOAD_WINDOW_SPANS};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::ApiError;

/// Minutes in a day; an end of this value means "up to midnight".
const MINUTES_PER_DAY: u16 = 24 * 60;
/// The weekdays a span's bitmask may name, Monday to Sunday.
const EVERY_DAY: u8 = 0b0111_1111;

/// Sets or removes a download window.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default)]
pub struct DownloadWindowRequest {
    /// The window; `null` removes it — a package then follows its category's, and a category
    /// leaves its packages to the bandwidth schedule alone. Its spans are local times in the
    /// bandwidth schedule's timezone; `ignore_schedule_pause` lets the packages download while
    /// the schedule's profile pauses downloads, never faster than any rate limit allows.
    pub download_window: Option<DownloadWindow>,
}

/// The window as it will be stored: every span on at least one weekday and a non-empty part of
/// the day, at most [`MAX_DOWNLOAD_WINDOW_SPANS`] of them; the day mask keeps its seven bits.
pub fn validated_download_window(
    window: Option<DownloadWindow>,
) -> Result<Option<DownloadWindow>, ApiError> {
    let Some(mut window) = window else {
        return Ok(None);
    };
    if window.windows.len() > MAX_DOWNLOAD_WINDOW_SPANS {
        return Err(ApiError::bad_request(
            "download_window.too_many_windows",
            "The download window has too many time spans",
        )
        .with_param("max", MAX_DOWNLOAD_WINDOW_SPANS));
    }
    for span in &mut window.windows {
        span.days &= EVERY_DAY;
        if span.days == 0 {
            return Err(ApiError::bad_request(
                "download_window.window_invalid",
                "A time span must apply to at least one weekday",
            ));
        }
        if span.start_minute >= MINUTES_PER_DAY
            || span.end_minute > MINUTES_PER_DAY
            || span.start_minute == span.end_minute
        {
            return Err(ApiError::bad_request(
                "download_window.window_invalid",
                "A time span must cover a non-empty part of the day",
            ));
        }
    }
    Ok(Some(window))
}

#[cfg(test)]
mod tests {
    use rd_core::{DownloadWindow, WeeklyWindow};

    use super::validated_download_window;

    fn span(days: u8, start_minute: u16, end_minute: u16) -> DownloadWindow {
        DownloadWindow {
            windows: vec![WeeklyWindow {
                days,
                start_minute,
                end_minute,
            }],
            ignore_schedule_pause: false,
        }
    }

    #[test]
    fn a_window_that_wraps_midnight_is_kept_and_its_day_mask_trimmed() {
        let stored = validated_download_window(Some(span(0xFF, 22 * 60, 6 * 60)))
            .expect("valid")
            .expect("set");
        assert_eq!(stored.windows[0].days, 0b0111_1111);
        assert_eq!(validated_download_window(None).expect("none"), None);
    }

    #[test]
    fn an_empty_or_impossible_span_is_refused_with_its_code() {
        for window in [
            span(0, 0, 60),
            span(1, 60, 60),
            span(1, 24 * 60, 60),
            span(1, 0, 24 * 60 + 1),
        ] {
            let error = validated_download_window(Some(window)).expect_err("refused");
            assert_eq!(error.code(), "download_window.window_invalid");
        }
        let many = DownloadWindow {
            windows: vec![span(1, 0, 60).windows[0]; rd_core::MAX_DOWNLOAD_WINDOW_SPANS + 1],
            ignore_schedule_pause: false,
        };
        assert_eq!(
            validated_download_window(Some(many))
                .expect_err("too many")
                .code(),
            "download_window.too_many_windows"
        );
    }
}
