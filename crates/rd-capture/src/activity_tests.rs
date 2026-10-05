use chrono::{DateTime, TimeZone, Utc};

use super::{
    Activity, QueueMenu, Summary, TOOLTIP_LIMIT, describe, format_duration, format_rate, pause_end,
    status_line, tooltip,
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
        paused: 0,
        paused_until: None,
        queue_control: false,
    }
}

fn controlling(mut summary: Summary) -> Summary {
    summary.queue_control = true;
    summary
}

fn at(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 4, hour, minute, 0)
        .single()
        .expect("a valid time")
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
            detail: "no transfers".to_owned(),
            queue: QueueMenu::Hidden,
        }
    );
}

/// An agent paired without queue control is offered nothing, whatever the queue does
/// (RD-1100-06).
#[test]
fn without_queue_control_the_menu_offers_no_queue_entry() {
    for figures in [
        summary(0, 0, 0, 0, 0),
        summary(3, 2, 0, 0, 0),
        Summary {
            paused: 4,
            paused_until: Some(at(18, 30)),
            ..summary(0, 0, 0, 0, 0)
        },
    ] {
        assert_eq!(describe(figures).queue, QueueMenu::Hidden, "{figures:?}");
    }
}

/// Pause while anything is waiting or moving, and on an empty queue; resume while a timed
/// pause holds or everything left is paused.
#[test]
fn the_queue_entries_follow_what_there_is_to_pause_or_resume() {
    assert_eq!(
        describe(controlling(summary(0, 0, 0, 0, 0))).queue,
        QueueMenu::Pause
    );
    assert_eq!(
        describe(controlling(summary(1, 2, 0, 0, 0))).queue,
        QueueMenu::Pause
    );
    let mut all_paused = controlling(summary(0, 0, 1, 0, 0));
    all_paused.paused = 3;
    assert_eq!(describe(all_paused).queue, QueueMenu::Resume);
    let mut some_paused = controlling(summary(0, 2, 0, 0, 0));
    some_paused.paused = 3;
    assert_eq!(
        describe(some_paused).queue,
        QueueMenu::Pause,
        "something still waits"
    );
    let mut timed = controlling(summary(0, 0, 0, 0, 0));
    timed.paused_until = Some(at(18, 30));
    assert_eq!(describe(timed).queue, QueueMenu::Resume);
}

/// A timed pause leads the line, and paused files are counted like the others.
#[test]
fn a_timed_pause_leads_the_line_and_paused_files_are_counted() {
    let mut figures = summary(0, 0, 0, 0, 0);
    figures.paused = 2;
    assert_eq!(describe(figures).detail, "2 paused");
    figures.paused_until = Some(at(18, 30));
    let detail = describe(figures).detail;
    assert!(detail.starts_with("paused until "), "{detail}");
    assert!(detail.ends_with(" · 2 paused"), "{detail}");
}

/// The end reads as a clock today and carries the day otherwise, in the zone of `now`.
#[test]
fn a_pause_end_is_a_clock_today_and_a_date_beyond() {
    let now = at(17, 0);
    assert_eq!(pause_end(at(18, 30), &now), "18:30");
    let tomorrow = at(18, 30) + chrono::Duration::days(1);
    assert_eq!(pause_end(tomorrow, &now), "Oct 5 18:30");
    let berlin = chrono::FixedOffset::east_opt(2 * 3600).expect("an offset");
    assert_eq!(
        pause_end(at(18, 30), &now.with_timezone(&berlin)),
        "20:30",
        "the clock of the zone `now` is in"
    );
}

/// A service without the pause fields still parses, and offers no queue entry.
#[test]
fn a_service_without_the_pause_fields_offers_no_queue_entry() {
    let parsed: Summary = serde_json::from_str(
        r#"{"active":1,"queued":0,"failed":0,"committed_bytes":"25","total_bytes":"100"}"#,
    )
    .expect("parse");
    assert_eq!(parsed.paused, 0);
    assert_eq!(parsed.paused_until, None);
    assert_eq!(describe(parsed).queue, QueueMenu::Hidden);
    let current: Summary = serde_json::from_str(
        r#"{"active":0,"queued":0,"failed":0,"committed_bytes":"0","total_bytes":"0","paused":1,"paused_until":"2026-10-04T18:30:00Z","queue_control":true}"#,
    )
    .expect("parse");
    assert_eq!(current.paused_until, Some(at(18, 30)));
    assert_eq!(describe(current).queue, QueueMenu::Resume);
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
        paused: u32::MAX,
        paused_until: Some(at(23, 59) + chrono::Duration::days(20)),
        queue_control: true,
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
