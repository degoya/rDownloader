//! Each case of the analysis, counted (RD-1260-04): how many notifications a run of events
//! makes, decided on the payloads alone.

use rd_notify::NotificationEvent;
use serde_json::json;

use super::{
    captcha_notice, package_failed_body, package_outcome, power_notice, queue_moment, queue_notice,
    storage_blocked, storage_notice,
};

fn package(previous: Option<&str>, state: &str) -> serde_json::Value {
    json!({ "package_id": "p", "state": state, "previous": previous })
}

/// A package of fifty files finishing: the post-processing stages, the final write, a reorder
/// and a rename afterwards. One "Package finished".
#[test]
fn a_finished_package_is_one_notification_however_often_it_is_written() {
    let mut events = vec![package(Some("downloading"), "postprocessing")];
    events.extend((0..6).map(|_| package(Some("postprocessing"), "postprocessing")));
    events.push(package(Some("postprocessing"), "completed"));
    // The same state written again, and the events that name the package without a state.
    events.push(package(Some("completed"), "completed"));
    events.push(json!({ "package_id": "p", "reordered_downloads": 50 }));
    events.push(json!({ "renamed_packages": 1 }));
    let outcomes: Vec<_> = events.iter().filter_map(package_outcome).collect();
    assert_eq!(outcomes, vec![NotificationEvent::PackageCompleted]);
}

/// A package whose fifty files all failed: one "Package failed" naming the count, where it used
/// to stay `queued` and push nothing.
#[test]
fn a_package_of_failed_files_is_one_failure_with_the_count() {
    let failed = json!({
        "package_id": "p", "state": "failed", "previous": "queued", "files": 50, "failed_files": 50
    });
    assert_eq!(
        package_outcome(&failed),
        Some(NotificationEvent::PackageFailed)
    );
    assert_eq!(
        package_failed_body(&failed, "Holiday"),
        "Holiday: 50 of 50 files failed."
    );
    let again = json!({ "package_id": "p", "state": "failed", "previous": "failed" });
    assert_eq!(package_outcome(&again), None);
    // A post-processing failure has no failed file to count.
    let unpack = json!({ "state": "failed", "files": 3, "failed_files": 0 });
    assert_eq!(package_failed_body(&unpack, "Holiday"), "Holiday (failed)");
}

fn captcha(change: &str, waiting: usize) -> serde_json::Value {
    let pending: Vec<_> = (0..waiting)
        .map(|index| json!({ "id": format!("c{index}"), "host": "files.example" }))
        .collect();
    json!({ "pending": pending, "change": change })
}

/// Five captchas arriving one after another, two answered, three running out: one "waiting"
/// and one "not answered in time", where every change used to be one more "waiting".
#[test]
fn a_wave_of_captchas_is_one_start_and_one_end() {
    let events = [
        captcha("added", 1),
        captcha("added", 2),
        captcha("added", 3),
        captcha("added", 4),
        captcha("added", 5),
        captcha("settled", 4),
        captcha("settled", 3),
        captcha("expired", 2),
        captcha("expired", 1),
        captcha("expired", 0),
    ];
    let notices: Vec<_> = events.iter().filter_map(captcha_notice).collect();
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert_eq!(notices[0].0, "A captcha is waiting");
    assert!(notices[0].1.contains("files.example"), "{}", notices[0].1);
    assert_eq!(notices[1].0, "Captcha not answered in time");
}

#[test]
fn a_captcha_answered_or_withdrawn_says_nothing() {
    for change in ["settled", "withdrawn"] {
        assert_eq!(captcha_notice(&captcha(change, 0)), None, "{change}");
    }
    // An event that names no change -- what every one looked like before -- is no notice.
    assert_eq!(captcha_notice(&json!({ "pending": [] })), None);
}

/// A sixty-second countdown is one "pending" and one end; the action running is not notified.
#[test]
fn a_countdown_is_one_start_and_its_end_is_named() {
    let now = chrono::Utc::now();
    let runs_at = now + chrono::Duration::seconds(60);
    let (title, body) = power_notice(
        &json!({ "state": "pending", "pending": "shutdown", "runs_at": runs_at }),
        now,
    )
    .expect("a countdown starting is news");
    assert_eq!(title, "Shutdown pending");
    assert!(body.contains("in 60 seconds"), "{body}");
    let (title, _) = power_notice(
        &json!({ "state": "cancelled", "action": "standby", "reason": "cancelled" }),
        now,
    )
    .expect("a cancelled countdown is news");
    assert_eq!(title, "Standby cancelled");
    let (title, _) = power_notice(
        &json!({ "state": "cancelled", "action": "shutdown", "reason": "new_work" }),
        now,
    )
    .expect("a countdown called off is news");
    assert_eq!(title, "Shutdown called off");
    assert_eq!(
        power_notice(&json!({ "state": "running", "action": "shutdown" }), now),
        None
    );
    // The tick payload of before, without a state, is no notice.
    assert_eq!(
        power_notice(&json!({ "pending": "shutdown", "runs_at": runs_at }), now),
        None
    );
}

#[test]
fn a_storage_root_released_is_no_out_of_space() {
    assert!(storage_blocked(
        &json!({ "blocked": 1, "change": "blocked" })
    ));
    assert!(!storage_blocked(
        &json!({ "blocked": 0, "change": "released" })
    ));
    assert!(!storage_blocked(&json!({ "blocked": 1 })));
}

/// RD-1270-18: "Storage root out of space" names the root, as the event does.
#[test]
fn out_of_space_names_its_storage_root() {
    let (title, body) = storage_notice(&json!({
        "blocked": 1, "change": "blocked", "target": "fallback",
        "name": "Downloads", "path": "/srv/downloads"
    }))
    .expect("a block is notified");
    assert_eq!(title, "Storage root out of space: Downloads");
    assert_eq!(
        body,
        "Downloads (/srv/downloads) fell below its free-space threshold and takes no new work."
    );
    // A root no longer configured has no name: the general wording.
    let (title, _) =
        storage_notice(&json!({ "blocked": 1, "change": "blocked", "target": "x", "name": null }))
            .expect("still notified");
    assert_eq!(title, "Storage root out of space");
    assert!(
        storage_notice(&json!({ "blocked": 0, "change": "released", "name": "Downloads" }))
            .is_none()
    );
}

/// RD-1290-04: each `queue.state` is its moment, a pause says who set it.
#[test]
fn the_queue_s_moments_say_what_happened() {
    let (event, title, body) =
        queue_notice(&json!({ "state": "paused", "reason": "schedule" })).expect("a pause");
    assert_eq!(event, NotificationEvent::QueuePaused);
    assert_eq!(title, "Queue paused");
    assert_eq!(body, "The bandwidth schedule pauses the queue.");
    let (_, _, body) =
        queue_notice(&json!({ "state": "paused", "reason": "manual" })).expect("a pause");
    assert_eq!(body, "The queue was paused by hand.");
    assert_eq!(
        queue_moment(&json!({ "state": "resumed" })),
        Some(NotificationEvent::QueueResumed)
    );
    let (event, title, _) = queue_notice(&json!({ "state": "finished" })).expect("ran empty");
    assert_eq!(
        (event, title.as_str()),
        (NotificationEvent::QueueFinished, "Queue finished")
    );
    assert!(queue_notice(&json!({ "state": "busy" })).is_none());
    assert!(queue_notice(&json!({})).is_none());
}
