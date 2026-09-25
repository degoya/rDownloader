//! What the tray says about transfers, derived from the service's figures.
//!
//! Kept apart from `tray` so it compiles and is tested on every host, including the Linux
//! machines where the tray itself does not exist.
//!
//! The transfer rate is read, not derived. It used to be computed here from the change in
//! committed bytes between two polls, unsmoothed, while the web UI computed a smoothed one of
//! its own — two answers to the same question, and a third was only a matter of time. The
//! service keeps the rate now (RD-104-02) and both read the same figure. Nothing else about
//! this changes: the summary still carries counts and byte totals only, which is all a capture
//! token may see — no names, no paths.

/// The figures `GET /api/v1/capture/summary` answers with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct Summary {
    pub active: u32,
    pub queued: u32,
    pub failed: u32,
    #[serde(with = "byte_string")]
    pub committed_bytes: u64,
    #[serde(with = "byte_string")]
    pub total_bytes: u64,
    /// The queue's smoothed rate, as the service measured it.
    #[serde(default)]
    pub bytes_per_second: u64,
    /// How long the queue still needs at that rate, as the service estimated it, and `None`
    /// when no honest figure exists: an entry still to be fetched whose size is unknown, a
    /// rate of zero, a paused transfer (RD-104-02).
    ///
    /// `default` on top of the `Option` because the two absences are different things. A
    /// service older than RD-108-01 sends no field at all, and the agent has to render the
    /// line without a remaining time rather than throw the whole answer away; a current one
    /// sends `null` whenever it has nothing honest to say.
    #[serde(default)]
    pub eta_seconds: Option<u64>,
}

/// Byte counts cross the API as strings, because they do not fit a JSON number safely.
mod byte_string {
    use serde::{Deserialize, Deserializer, de::Error};

    /// A string that is not a number is a broken contract, and it is reported as one.
    ///
    /// It used to be `unwrap_or(0)`. A changed format -- `"1.2e9"`, a thousands separator, an
    /// empty field -- then turned into zero, and the tray showed `0%` or "no transfers" while
    /// downloads were running: a broken API contract that looks exactly like a quiet system, and
    /// nothing anywhere said the display was guessing. The caller is `watch_activity`, which
    /// logs the failure and asks again on the next tick, so reporting it costs nothing but makes
    /// it visible (RD-109-10).
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(input: D) -> Result<u64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Text(String),
            Number(u64),
        }
        Ok(match Either::deserialize(input)? {
            Either::Text(value) => value.parse().map_err(|error| {
                D::Error::custom(format!("byte count {value:?} is not a number: {error}"))
            })?,
            Either::Number(value) => value,
        })
    }
}

/// What the tray shows: whether anything is running, and the line under the icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Activity {
    pub running: bool,
    pub detail: String,
}

/// Renders the tray's transfer line: `3 active · 47% · 12.4 MB/s`.
///
/// Untranslated on purpose. The agent carries no message catalogue (RD-092-05), and the rest of
/// the menu is English for the same reason.
pub(crate) fn describe(summary: Summary) -> Activity {
    let running = summary.active > 0;
    if !running && summary.queued == 0 && summary.failed == 0 {
        return Activity {
            running: false,
            detail: "no transfers".to_owned(),
        };
    }
    let mut parts = Vec::new();
    if summary.active > 0 {
        parts.push(format!("{} active", summary.active));
    }
    if summary.queued > 0 {
        parts.push(format!("{} queued", summary.queued));
    }
    if summary.failed > 0 {
        parts.push(format!("{} failed", summary.failed));
    }
    if let Some(percent) = progress_percent(summary) {
        parts.push(format!("{percent}%"));
    }
    // Only while something is actually running: a rate next to an idle queue reads as motion,
    // and a remaining time beside one reads as motion twice over. Both stand or fall together.
    if running && summary.bytes_per_second > 0 {
        parts.push(format_rate(summary.bytes_per_second));
        if let Some(seconds) = summary.eta_seconds {
            parts.push(format!("{} left", format_duration(seconds)));
        }
    }
    Activity {
        running,
        detail: parts.join(" · "),
    }
}

/// Overall progress across everything unfinished, when the sizes are known.
fn progress_percent(summary: Summary) -> Option<u32> {
    if summary.total_bytes == 0 {
        return None;
    }
    let ratio = summary.committed_bytes.min(summary.total_bytes) as f64
        / summary.total_bytes as f64
        * 100.0;
    Some(ratio.round() as u32)
}

/// One decimal up to the gigabyte, which is what fits in a menu line.
fn format_rate(bytes_per_second: u64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut value = bytes_per_second as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", value.round() as u64, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A remaining time for a menu line: at most two units, no leading zeros, days at the top.
///
/// Not the web interface's `formatDuration`, which writes a clock (`1:12:30`). A clock in a
/// tray line sits next to a rate and a percentage with nothing to say which of them it is;
/// `1h 12m` says it in the string itself and is shorter as soon as days are involved.
fn format_duration(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    if seconds >= DAY {
        two_units(seconds / DAY, 'd', seconds % DAY / HOUR, 'h')
    } else if seconds >= HOUR {
        two_units(seconds / HOUR, 'h', seconds % HOUR / MINUTE, 'm')
    } else if seconds >= MINUTE {
        two_units(seconds / MINUTE, 'm', seconds % MINUTE, 's')
    } else {
        format!("{seconds}s")
    }
}

/// `2d 4h`, and a bare `2d` when the smaller unit is zero: a menu line has no room for a
/// segment that says nothing.
fn two_units(major: u64, major_unit: char, minor: u64, minor_unit: char) -> String {
    if minor == 0 {
        format!("{major}{major_unit}")
    } else {
        format!("{major}{major_unit} {minor}{minor_unit}")
    }
}

/// The tray's one line: the server status, and the transfers when there are any.
///
/// Composed here rather than in `tray` so its length can be measured on every host, including
/// the Linux machines where the tray does not exist.
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn status_line(status: &str, detail: &str) -> String {
    if detail.is_empty() {
        return status.to_owned();
    }
    format!("{status} \u{2014} {detail}")
}

/// What a Windows tray tooltip holds: a 128-`WCHAR` buffer, the terminating NUL included.
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) const TOOLTIP_LIMIT: usize = 127;

/// The status line, cut to what the platform can carry in a tooltip.
///
/// Only the tooltip is cut; the menu's status item has no such buffer behind it. The cut is a
/// guarantee rather than a precaution: the hostname, the three counts, the rate and now the
/// remaining time are each unbounded, and the longest line they can form runs well past the
/// limit. Characters, not bytes - the buffer counts UTF-16 code units, and every character
/// this line can contain is one of them.
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn tooltip(line: &str) -> String {
    if line.chars().count() <= TOOLTIP_LIMIT {
        return line.to_owned();
    }
    let kept: String = line.chars().take(TOOLTIP_LIMIT - 1).collect();
    format!("{kept}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::{
        Activity, Summary, TOOLTIP_LIMIT, describe, format_duration, format_rate, status_line,
        tooltip,
    };

    fn summary(active: u32, queued: u32, failed: u32, committed: u64, total: u64) -> Summary {
        Summary {
            active,
            queued,
            failed,
            committed_bytes: committed,
            total_bytes: total,
            bytes_per_second: 0,
            eta_seconds: None,
        }
    }

    fn moving(mut summary: Summary, bytes_per_second: u64) -> Summary {
        summary.bytes_per_second = bytes_per_second;
        summary
    }

    fn ending_in(mut summary: Summary, eta_seconds: u64) -> Summary {
        summary.eta_seconds = Some(eta_seconds);
        summary
    }

    #[test]
    fn nothing_running_says_so_and_leaves_the_icon_idle() {
        let activity = describe(summary(0, 0, 0, 0, 0));
        assert_eq!(
            activity,
            Activity {
                running: false,
                detail: "no transfers".to_owned()
            }
        );
    }

    #[test]
    fn a_running_transfer_names_the_count_the_progress_and_the_rate() {
        let activity = describe(moving(summary(3, 2, 0, 470, 1_000), 13_000_000));
        assert!(activity.running);
        assert_eq!(activity.detail, "3 active · 2 queued · 47% · 12.4 MB/s");
    }

    /// The point of RD-108-01: the figure the web interface shows stands in the tray too, and
    /// it stands beside the rate rather than anywhere else in the line.
    #[test]
    fn a_running_transfer_names_the_remaining_time_after_the_rate() {
        let activity = describe(ending_in(
            moving(summary(3, 2, 0, 470, 1_000), 13_000_000),
            4_350,
        ));
        assert_eq!(
            activity.detail,
            "3 active · 2 queued · 47% · 12.4 MB/s · 1h 12m left"
        );
    }

    /// A queue that is waiting is not motion, so no rate is shown beside it.
    #[test]
    fn a_waiting_queue_shows_neither_rate_nor_remaining_time() {
        let activity = describe(ending_in(moving(summary(0, 5, 0, 0, 0), 9_999_999), 600));
        assert!(!activity.running);
        assert_eq!(activity.detail, "5 queued");
    }

    /// A rate of zero is the service saying "nothing is moving", not a number to print.
    #[test]
    fn a_still_queue_shows_no_rate_and_no_remaining_time() {
        let activity = describe(ending_in(summary(1, 0, 0, 500, 1_000), 42));
        assert_eq!(activity.detail, "1 active · 50%");
    }

    /// The service says `null` for an unknown size, a rate of zero or a paused transfer. The
    /// tray then shows nothing there - no infinity sign, no "calculating" (R·-104-02).
    #[test]
    fn an_absent_estimate_leaves_the_segment_out_rather_than_filling_it() {
        let activity = describe(moving(summary(1, 0, 0, 500, 1_000), 1_048_576));
        assert_eq!(activity.detail, "1 active · 50% · 1.0 MB/s");
    }

    #[test]
    fn failures_are_named_even_with_nothing_running() {
        let activity = describe(summary(0, 0, 2, 0, 0));
        assert!(!activity.running);
        assert_eq!(activity.detail, "2 failed");
    }

    #[test]
    fn progress_is_left_out_when_no_size_is_known() {
        let activity = describe(summary(1, 0, 0, 500, 0));
        assert_eq!(activity.detail, "1 active");
    }

    #[test]
    fn rates_are_rendered_with_one_decimal_above_bytes() {
        assert_eq!(format_rate(0), "0 B/s");
        assert_eq!(format_rate(512), "512 B/s");
        assert_eq!(format_rate(1_536), "1.5 KB/s");
        assert_eq!(format_rate(13_000_000), "12.4 MB/s");
    }

    #[test]
    fn byte_counts_parse_whether_they_arrive_as_strings_or_numbers() {
        let from_strings: Summary = serde_json::from_str(
            r#"{"active":1,"queued":0,"failed":0,"committed_bytes":"25","total_bytes":"100","bytes_per_second":40}"#,
        )
        .expect("parse");
        assert_eq!(from_strings.committed_bytes, 25);
        assert_eq!(from_strings.bytes_per_second, 40);
        let from_numbers: Summary = serde_json::from_str(
            r#"{"active":1,"queued":0,"failed":0,"committed_bytes":25,"total_bytes":100,"bytes_per_second":40}"#,
        )
        .expect("parse");
        assert_eq!(from_numbers, from_strings);
    }

    /// An older service does not send the field; the tray then shows counts and progress
    /// without inventing a rate.
    #[test]
    fn a_missing_rate_is_no_rate_rather_than_a_parse_failure() {
        let parsed: Summary = serde_json::from_str(
            r#"{"active":1,"queued":0,"failed":0,"committed_bytes":"25","total_bytes":"100"}"#,
        )
        .expect("parse");
        assert_eq!(parsed.bytes_per_second, 0);
        assert_eq!(describe(parsed).detail, "1 active · 25%");
    }

    #[test]
    fn remaining_times_are_two_units_at_most_with_days_on_top() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(60), "1m");
        assert_eq!(format_duration(90), "1m 30s");
        assert_eq!(format_duration(3_600), "1h");
        assert_eq!(format_duration(4_350), "1h 12m");
        assert_eq!(format_duration(86_400), "1d");
        assert_eq!(format_duration(180_000), "2d 2h");
    }

    /// The seconds behind an hour are noise in a menu line, so two units is the whole rule:
    /// nothing below the second unit is ever printed.
    #[test]
    fn remaining_times_never_print_a_third_unit() {
        assert_eq!(format_duration(90_061), "1d 1h");
        assert_eq!(format_duration(3_661), "1h 1m");
    }

    /// A service older than RD-108-01 sends no such field. The agent renders the line without
    /// a remaining time instead of discarding the answer and going blank.
    #[test]
    fn a_service_without_the_field_still_parses_and_renders() {
        let parsed: Summary = serde_json::from_str(
            r#"{"active":1,"queued":0,"failed":0,"committed_bytes":"25","total_bytes":"100","bytes_per_second":1048576}"#,
        )
        .expect("parse");
        assert_eq!(parsed.eta_seconds, None);
        assert_eq!(describe(parsed).detail, "1 active · 25% · 1.0 MB/s");
    }

    /// A current service says `null` where it has nothing honest to say, which has to read the
    /// same way as the field being absent altogether.
    #[test]
    fn an_explicit_null_estimate_reads_as_no_estimate() {
        let parsed: Summary = serde_json::from_str(
            r#"{"active":1,"queued":0,"failed":0,"committed_bytes":"25","total_bytes":"100","bytes_per_second":1048576,"eta_seconds":null}"#,
        )
        .expect("parse");
        assert_eq!(parsed.eta_seconds, None);
        let counted: Summary = serde_json::from_str(
            r#"{"active":1,"queued":0,"failed":0,"committed_bytes":"25","total_bytes":"100","bytes_per_second":1048576,"eta_seconds":90}"#,
        )
        .expect("parse");
        assert_eq!(counted.eta_seconds, Some(90));
        assert_eq!(
            describe(counted).detail,
            "1 active · 25% · 1.0 MB/s · 1m 30s left"
        );
    }

    #[test]
    fn the_status_line_drops_the_separator_when_there_is_nothing_to_report() {
        assert_eq!(status_line("server running", ""), "server running");
        assert_eq!(
            status_line("server running", "1 active"),
            "server running — 1 active"
        );
    }

    /// A realistic line is handed to Windows whole; nothing is cut that fits.
    #[test]
    fn an_ordinary_line_reaches_the_tooltip_untouched() {
        let line = status_line(
            "rDownloader Capture v1.0.8 — 127.0.0.1:8710 — server running",
            "3 active · 2 queued · 47% · 12.4 MB/s · 1h 12m left",
        );
        assert!(line.chars().count() <= TOOLTIP_LIMIT, "{line}");
        assert_eq!(tooltip(&line), line);
    }

    /// The Windows tooltip is a 128-WCHAR buffer, NUL included, and the line just grew a fifth
    /// segment. Hostname, counts, rate and remaining time are each unbounded, so the longest
    /// line the agent can build runs far past that and the cut has to hold it.
    #[test]
    fn the_longest_line_the_agent_can_build_still_fits_the_windows_tooltip() {
        let detail = describe(Summary {
            active: u32::MAX,
            queued: u32::MAX,
            failed: u32::MAX,
            committed_bytes: u64::MAX,
            total_bytes: u64::MAX,
            bytes_per_second: u64::MAX,
            eta_seconds: Some(u64::MAX),
        })
        .detail;
        let host = "a".repeat(253);
        let line = status_line(
            &format!("rDownloader Capture v10.10.10 — {host}:65535 — server not reachable"),
            &detail,
        );
        assert!(
            line.chars().count() > TOOLTIP_LIMIT,
            "the worst case is what the cut exists for, and it measures {}",
            line.chars().count()
        );
        let tip = tooltip(&line);
        assert_eq!(tip.chars().count(), TOOLTIP_LIMIT);
        assert!(tip.ends_with('…'), "a cut line says that it was cut: {tip}");
    }

    /// A broken contract must not look like a quiet system (RD-109-10).
    #[test]
    fn a_byte_count_that_is_not_a_number_is_reported_rather_than_zero() {
        for broken in ["1.2e9", "1,200,000", "", "lots"] {
            let document = format!(
                r#"{{"active":1,"queued":0,"failed":0,"committed_bytes":"{broken}","total_bytes":"10"}}"#
            );
            let error = serde_json::from_str::<Summary>(&document)
                .expect_err("a byte count that is not a number has to fail");
            assert!(
                error.to_string().contains("is not a number"),
                "{broken:?}: {error}"
            );
        }

        // The two forms the contract really promises still work.
        let text = serde_json::from_str::<Summary>(
            r#"{"active":1,"queued":0,"failed":0,"committed_bytes":"4096","total_bytes":"8192"}"#,
        )
        .expect("a decimal string is a byte count");
        assert_eq!(text.committed_bytes, 4096);
        let number = serde_json::from_str::<Summary>(
            r#"{"active":1,"queued":0,"failed":0,"committed_bytes":4096,"total_bytes":8192}"#,
        )
        .expect("a JSON number is a byte count too");
        assert_eq!(number.total_bytes, 8192);
    }
}
