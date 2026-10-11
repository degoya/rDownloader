//! Which bus events open a burst, and what a closed burst says (RD-1240-17).

use rd_core::{EventEnvelope, EventKind};
use rd_notify::{Coalescer, NotificationEvent, Occurrence};

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
