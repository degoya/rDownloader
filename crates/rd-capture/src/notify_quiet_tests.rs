//! Quiet hours hold the tray's intake toasts back and end in one (RD-1260-04).

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use super::{Intake, announce, summary};

/// Two intakes during quiet hours, the second after the toast window: no toast while they
/// last, one when they end, counting both.
#[tokio::test]
async fn intakes_during_quiet_hours_are_one_toast_when_they_end() {
    let quiet = Duration::from_millis(400);
    let window = Duration::from_millis(50);
    let started = Instant::now();
    let until = chrono::Utc::now() + chrono::Duration::from_std(quiet).expect("duration");
    let (sender, receiver) = tokio::sync::mpsc::channel(8);
    let shown = Arc::new(Mutex::new(Vec::new()));
    let collector = Arc::clone(&shown);
    let announcing = tokio::spawn(announce(receiver, window, move |intake| {
        let collector = Arc::clone(&collector);
        async move {
            collector
                .lock()
                .expect("collect")
                .push((intake, started.elapsed()));
        }
    }));
    let held = Intake {
        candidate_count: 2,
        package_count: 1,
        quiet_until: Some(until),
        ..Intake::default()
    };
    sender.try_send(held).expect("queue the intake");
    tokio::time::sleep(window * 3).await;
    sender.try_send(held).expect("queue the intake");
    tokio::time::sleep(quiet * 2).await;
    drop(sender);
    announcing
        .await
        .expect("the announcer ends with its channel");
    let shown = shown.lock().expect("read");
    assert_eq!(shown.len(), 1, "quiet hours produced more than one toast");
    let (intake, at) = shown[0];
    assert!(
        at >= quiet - window,
        "the toast came before quiet hours ended: {at:?}"
    );
    assert_eq!(intake.candidate_count, 4);
    assert_eq!(
        summary(&intake),
        "While it was quiet: 4 links in 2 packages added to the LinkGrabber"
    );
}
