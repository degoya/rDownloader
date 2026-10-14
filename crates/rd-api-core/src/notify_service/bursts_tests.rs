//! Which bus events open a burst, and what a closed burst says (RD-1240-17), in its built-in
//! wording and to a rule's own (RD-1291-01).

use rd_core::{EventEnvelope, EventKind};
use rd_notify::{Coalescer, NotificationEvent, NotificationRule, Occurrence, Placeholder as P};

use super::{burst_text, coalescing_kind, source_label};

fn state(previous: &str, next: &str) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::DownloadState,
        serde_json::json!({ "download_id": "d", "previous": previous, "state": next }),
    )
}

#[test]
fn a_download_starts_when_it_leaves_the_queue_or_its_resolver() {
    for previous in ["queued", "resolving"] {
        assert_eq!(
            coalescing_kind(&state(previous, "downloading")),
            Some(NotificationEvent::DownloadStarted),
            "{previous}"
        );
    }
    // Going on after a pause or a retry wait is the same download, and every other state is
    // not a start at all.
    for (previous, next) in [
        ("paused", "downloading"),
        ("retry_wait", "downloading"),
        ("downloading", "completed"),
        ("queued", "paused"),
    ] {
        assert_eq!(
            coalescing_kind(&state(previous, next)),
            None,
            "{previous} -> {next}"
        );
    }
    let renamed = EventEnvelope::new(
        EventKind::DownloadState,
        serde_json::json!({ "download_id": "d", "renamed": true }),
    );
    assert_eq!(coalescing_kind(&renamed), None);
}

#[test]
fn an_intake_is_links_added_and_an_edit_is_not() {
    let intake = EventEnvelope::new(
        EventKind::CollectorIntake,
        serde_json::json!({ "batch_id": "b", "candidate_count": 3, "source": "clipboard" }),
    );
    assert_eq!(
        coalescing_kind(&intake),
        Some(NotificationEvent::LinksAdded)
    );
    let edit = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "batch_id": "b", "candidate_count": 3 }),
    );
    assert_eq!(coalescing_kind(&edit), None);
}

fn occurrence(event: NotificationEvent, id: &str, items: u64, name: &str) -> Occurrence {
    Occurrence {
        event,
        category_id: None,
        event_id: id.to_owned(),
        items,
        name: Some(name.to_owned()),
        occurred_at: chrono::Utc::now(),
        package_id: None,
    }
}

#[test]
fn a_burst_of_imports_is_one_text_with_the_sum() {
    let mut coalescer = Coalescer::default();
    let now = chrono::Utc::now();
    for index in 0..40 {
        let source = if index % 2 == 0 { "clipboard" } else { "API" };
        coalescer.push(
            occurrence(
                NotificationEvent::LinksAdded,
                &format!("e{index}"),
                5,
                source,
            ),
            now,
        );
    }
    let burst = coalescer.drain().remove(0);
    let (title, body) = burst_text(&burst);
    assert_eq!(title, "200 links added in 40 imports");
    assert_eq!(
        body,
        "200 links arrived in the LinkGrabber from: clipboard, API."
    );

    coalescer.push(
        occurrence(NotificationEvent::LinksAdded, "one", 1, "browser extension"),
        now,
    );
    let (title, body) = burst_text(&coalescer.drain().remove(0));
    assert_eq!(title, "1 link added");
    assert_eq!(
        body,
        "1 link arrived in the LinkGrabber from: browser extension."
    );
}

#[test]
fn one_start_names_its_file_and_many_are_counted() {
    let mut coalescer = Coalescer::default();
    let now = chrono::Utc::now();
    coalescer.push(
        occurrence(NotificationEvent::DownloadStarted, "a", 1, "a.mkv"),
        now,
    );
    let (title, body) = burst_text(&coalescer.drain().remove(0));
    assert_eq!(title, "Download started: a.mkv");
    assert_eq!(body, "Started: a.mkv.");

    for index in 0..50 {
        coalescer.push(
            occurrence(
                NotificationEvent::DownloadStarted,
                &format!("e{index}"),
                1,
                &format!("part{index}.rar"),
            ),
            now,
        );
    }
    let (title, body) = burst_text(&coalescer.drain().remove(0));
    assert_eq!(title, "50 downloads started");
    assert_eq!(
        body,
        "Started: part0.rar, part1.rar, part2.rar and 47 more."
    );
}

#[test]
fn an_intake_source_reads_as_words() {
    assert_eq!(source_label("browser_extension"), "browser extension");
    assert_eq!(source_label("hot_folder"), "hot folder");
    // A source added later still shows as what it is called.
    assert_eq!(source_label("something_new"), "something_new");
}

fn worded(title: &str, body: &str) -> NotificationRule {
    NotificationRule {
        id: rd_core::NotificationRuleId::new(),
        name: "own words".to_owned(),
        enabled: true,
        target_id: rd_core::NotificationTargetId::new(),
        events: vec![
            NotificationEvent::DownloadStarted,
            NotificationEvent::LinksAdded,
        ],
        category_id: None,
        min_severity: rd_notify::Severity::Info,
        title_template: Some(title.to_owned()),
        body_template: Some(body.to_owned()),
    }
}

/// The owner's repro of 1.29.0: a rule on `download_started` with its own title and text got
/// "Download started: sample.bin" / "Started: sample.bin.". Its own words now go out.
#[test]
fn one_start_goes_out_in_the_rule_s_own_words() {
    let mut coalescer = Coalescer::default();
    coalescer.push(
        occurrence(NotificationEvent::DownloadStarted, "a", 1, "sample.bin"),
        chrono::Utc::now(),
    );
    let burst = coalescer.drain().remove(0);
    let (title, body) = burst_text(&burst);
    assert_eq!(title, "Download started: sample.bin");
    let rule = worded(
        "MYTITLE {file}",
        "MYBODY {count}x {names} at {time} ({message})",
    );
    let (title, body) = rule.wording(&title, &body, &burst.values(&chrono::Utc));
    assert_eq!(title, "MYTITLE sample.bin");
    let time = burst.first_at.format("%H:%M");
    assert_eq!(
        body,
        format!("MYBODY 1x sample.bin at {time} (Started: sample.bin.)")
    );
}

#[test]
fn many_starts_count_and_name_and_leave_the_file_empty() {
    let mut coalescer = Coalescer::default();
    for index in 0..50 {
        coalescer.push(
            occurrence(
                NotificationEvent::DownloadStarted,
                &format!("e{index}"),
                1,
                &format!("part{index}.rar"),
            ),
            chrono::Utc::now(),
        );
    }
    let burst = coalescer.drain().remove(0);
    let (title, body) = burst_text(&burst);
    let rule = worded("{count} started [{file}]", "{names}");
    let values = burst.values(&chrono::Utc);
    let (own_title, own_body) = rule.wording(&title, &body, &values);
    assert_eq!(own_title, "50 started []");
    assert_eq!(own_body, "part0.rar, part1.rar, part2.rar and 47 more");
    // Filled again - a restart's replay - the burst says the same.
    assert_eq!(
        rule.wording(&title, &body, &burst.values(&chrono::Utc)),
        (own_title, own_body)
    );
    // A rule without a template keeps the built-in wording.
    let plain = NotificationRule {
        title_template: None,
        body_template: None,
        ..rule
    };
    assert_eq!(plain.wording(&title, &body, &values), (title, body));
}

/// The editor's sample bursts are what a real burst of three says (RD-1291-01).
#[test]
fn the_editor_s_sample_bursts_are_real_ones() {
    let cases = [
        (
            NotificationEvent::DownloadStarted,
            [
                (1, "IMG_2048.jpg"),
                (1, "IMG_2049.jpg"),
                (1, "IMG_2050.jpg"),
            ],
        ),
        (
            NotificationEvent::LinksAdded,
            [(4, "clipboard"), (4, "browser extension"), (4, "clipboard")],
        ),
    ];
    for (event, occurrences) in cases {
        let mut coalescer = Coalescer::default();
        for (index, (items, name)) in occurrences.into_iter().enumerate() {
            coalescer.push(
                occurrence(event, &format!("e{index}"), items, name),
                chrono::Utc::now(),
            );
        }
        let burst = coalescer.drain().remove(0);
        let (title, body) = burst_text(&burst);
        let mut values = burst.values(&chrono::Utc);
        values.set(P::Title, title);
        values.set(P::Message, body);
        for (placeholder, example) in event.examples() {
            assert_eq!(
                values.get(*placeholder),
                *example,
                "{event:?} {placeholder:?}"
            );
        }
    }
}
