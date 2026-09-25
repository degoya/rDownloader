//! Desktop notifications for links arriving in the LinkGrabber.
//!
//! The agent subscribes to the service's capture-scoped event stream and reports the imports on
//! it. That stream stays deliberately narrow — a capture token is meant for handing links in,
//! and the full event bus would tell it about download paths, accounts and credentials. Two
//! events go down it: this one, and a captcha signal carrying a count of waiting widgets and
//! nothing more. Since RD-109-11 the agent acts on neither the second one nor anything else on
//! the stream — widget captchas are answered in the browser extension — so this is the only
//! reader of the stream, and it ignores everything that is not its own event.
//!
//! A dropped connection no longer loses what arrived in the meantime. The reader keeps the id
//! of the last frame it saw and reconnects with it as `Last-Event-ID`; the service replays
//! what came after it, through the same narrow filter (RD-110-23, and with it RD-110-22). What
//! it cannot replay -- the buffer is in memory, so a restarted service knows no id -- it says
//! with a marker, which is a line in the log here and not a toast: the agent has no way to ask
//! what it missed, and a toast on every restart would be noise about nothing.
//!
//! Reading and announcing are two futures, not one. A toast used to be shown inline in the
//! drain loop, which meant nobody read the stream while the desktop was busy drawing it: a page
//! submitting links in a loop produced a toast per submission, each one slowing the reader
//! further, until the buffer hit the oversized-frame guard. Now the reader only hands the
//! intake on, and the announcer coalesces everything that arrives inside one window into a
//! single toast (RD-109-09).

use std::{future::Future, time::Duration};

use anyhow::{Result, bail};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    client::CaptureClient,
    sse::{Frame, MAX_PENDING_BYTES, Reconnect, find_frame_end, parse_frame},
};

/// How long intakes are collected before one toast reports them.
///
/// Long enough that a burst of submissions is one notification rather than a wall of them,
/// short enough that a single drop still feels immediate.
const TOAST_WINDOW: Duration = Duration::from_secs(3);

/// The marker the service sends when the id the agent resumed from is no longer in its buffer.
const EXPIRED_EVENT: &str = "stream.expired";

/// How many intakes may wait for the announcer before the reader starts dropping them.
///
/// Dropping is the right failure here: the announcer summarises anyway, and a full queue must
/// never be a reason to stop reading the stream.
const INTAKE_QUEUE: usize = 64;

/// What one intake event says. Everything is optional so an older or newer service that words
/// its payload differently degrades to a plain "links arrived" toast instead of no toast.
#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    payload: Intake,
}

#[derive(Clone, Copy, Default, Deserialize)]
pub(crate) struct Intake {
    #[serde(default)]
    pub(crate) candidate_count: u32,
    #[serde(default)]
    pub(crate) package_count: u32,
    /// Files inside an NZB that arrived on its own. Counted apart from `candidate_count`,
    /// which is links somebody added: the two are not the same thing and reporting files as
    /// links is how one dropped container turned into "5 links added".
    #[serde(default)]
    file_count: u32,
}

/// Everything one window's worth of intakes amounts to, as one toast.
fn merge(left: Intake, right: Intake) -> Intake {
    Intake {
        candidate_count: left.candidate_count.saturating_add(right.candidate_count),
        package_count: left.package_count.saturating_add(right.package_count),
        file_count: left.file_count.saturating_add(right.file_count),
    }
}

/// Follows the intake stream for as long as the agent runs, reconnecting with backoff.
///
/// Never fails the agent: a service that is down, too old to offer the stream, or a desktop
/// without a notification daemon costs notifications, not Click'n'Load or the clipboard.
pub async fn watch_intake(client: CaptureClient, cancellation: CancellationToken) {
    let (sender, receiver) = mpsc::channel(INTAKE_QUEUE);
    // One task, two futures. The reader drops its sender when it ends, which is what closes
    // the announcer down with it.
    tokio::join!(
        read_stream(client, cancellation, sender),
        announce(receiver, TOAST_WINDOW, |intake| async move {
            show(intake).await;
        }),
    );
}

/// Holds one connection open, reconnecting until the agent stops.
pub(crate) async fn read_stream(
    client: CaptureClient,
    cancellation: CancellationToken,
    intakes: mpsc::Sender<Intake>,
) {
    let mut reconnect = Reconnect::seeded();
    let mut announced = false;
    let mut last_id: Option<String> = None;
    while !cancellation.is_cancelled() {
        match follow(
            &client,
            &cancellation,
            &intakes,
            &mut reconnect,
            &mut last_id,
        )
        .await
        {
            Ok(()) => return,
            Err(error) => {
                // Say it once at warn level; a service that is simply not running would
                // otherwise fill the log with the same line every minute. The last id is in
                // the line because it is what the next connection resumes from.
                if announced {
                    tracing::debug!(
                        %error,
                        last_event_id = last_id.as_deref().unwrap_or("-"),
                        "intake notification stream dropped; reconnecting"
                    );
                } else {
                    tracing::warn!(%error, "intake notifications unavailable; retrying");
                    announced = true;
                }
            }
        }
        let delay = reconnect.delay();
        tokio::select! {
            () = cancellation.cancelled() => return,
            () = tokio::time::sleep(delay) => {}
        }
    }
}

/// Reads one connection to exhaustion, handing every intake to the announcer.
async fn follow(
    client: &CaptureClient,
    cancellation: &CancellationToken,
    intakes: &mpsc::Sender<Intake>,
    reconnect: &mut Reconnect,
    last_id: &mut Option<String>,
) -> Result<()> {
    let mut response = client.capture_events(last_id.as_deref()).await?;
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let chunk = tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            chunk = response.chunk() => chunk?,
        };
        let Some(chunk) = chunk else {
            bail!("the service closed the event stream");
        };
        pending.extend_from_slice(&chunk);
        // A frame ends at a blank line, which is also a safe place to decode: the boundary can
        // never fall inside a multi-byte character.
        while let Some(end) = find_frame_end(&pending) {
            let bytes = pending.drain(..end).collect::<Vec<u8>>();
            let frame = parse_frame(&String::from_utf8_lossy(&bytes));
            if let Some(interval) = frame.retry {
                reconnect.requested(interval);
            }
            if frame.id.is_some() {
                *last_id = frame.id.clone();
            }
            if frame.event.as_deref() == Some(EXPIRED_EVENT) {
                tracing::warn!(
                    last_event_id = last_id.as_deref().unwrap_or("-"),
                    "the service could not resume the event stream; links that arrived while the agent was disconnected were not announced"
                );
            }
            if let Some(intake) = intake_of(&frame) {
                // `try_send`, never `send`: a slow desktop must cost notifications, not the
                // reading of the stream.
                if intakes.try_send(intake).is_err() {
                    tracing::debug!("intake notifications are backed up; one was dropped");
                }
            }
        }
        if pending.len() > MAX_PENDING_BYTES {
            bail!("event stream sent an oversized frame");
        }
    }
}

/// Shows one toast per window, for everything that arrived in it.
///
/// Takes the display as an argument so the windowing can be tested without a notification
/// daemon. A closed channel ends the loop, which is how the reader's exit ends this one.
async fn announce<S, F>(mut intakes: mpsc::Receiver<Intake>, window: Duration, show: S)
where
    S: Fn(Intake) -> F,
    F: Future<Output = ()>,
{
    while let Some(first) = intakes.recv().await {
        let mut batch = first;
        let deadline = tokio::time::sleep(window);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                () = &mut deadline => break,
                next = intakes.recv() => match next {
                    Some(intake) => batch = merge(batch, intake),
                    // The stream reader is gone; report what is in hand and stop.
                    None => break,
                },
            }
        }
        show(batch).await;
    }
}

/// The payload of one `collector.intake` frame, or `None` for anything else — comments, the
/// keep-alive, and any event kind the stream may carry later.
fn intake_of(frame: &Frame) -> Option<Intake> {
    if frame.event.as_deref() != Some("collector.intake") || frame.data.is_empty() {
        return None;
    }
    serde_json::from_str::<Envelope>(&frame.data)
        .ok()
        .map(|envelope| envelope.payload)
}

/// Shows one toast. A desktop without a notification daemon — a remote session, a bare X
/// server — is a warning in the log, not a reason to stop watching.
async fn show(intake: Intake) {
    let body = summary(&intake);
    let result = tokio::task::spawn_blocking(move || {
        let mut notification = notify_rust::Notification::new();
        notification
            .appname("rDownloader Capture")
            .summary("rDownloader")
            .body(&body);
        // Only Windows has this setter, and only there does it matter: it decides which sender
        // the toast is attributed to. `association install` registers the identifier.
        #[cfg(windows)]
        notification.app_id(crate::os_integration::WINDOWS_APP_ID);
        notification.show().map(|_| ())
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(%error, "desktop notification could not be shown"),
        Err(error) => tracing::warn!(%error, "desktop notification task failed"),
    }
}
/// Deliberately English, like the tray menu: the agent carries no translation catalogue, and
/// the four-language rule covers `web/src/locales`.
fn summary(intake: &Intake) -> String {
    if intake.file_count > 0 && intake.candidate_count == 0 {
        return match intake.file_count {
            1 => "NZB added to the LinkGrabber, 1 file".to_owned(),
            files => format!("NZB added to the LinkGrabber, {files} files"),
        };
    }
    match (intake.candidate_count, intake.package_count) {
        (0, _) => "Links added to the LinkGrabber".to_owned(),
        (1, _) => "1 link added to the LinkGrabber".to_owned(),
        (links, packages) if packages > 1 => {
            format!("{links} links in {packages} packages added to the LinkGrabber")
        }
        (links, _) => format!("{links} links added to the LinkGrabber"),
    }
}

#[cfg(test)]
mod tests {
    use super::{Intake, TOAST_WINDOW, announce, intake_of, summary};
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    fn parse_intake(frame: &str) -> Option<Intake> {
        intake_of(&crate::sse::parse_frame(frame))
    }

    #[test]
    fn only_an_intake_frame_is_turned_into_a_notification() {
        let frame = "id: 1\nevent: collector.intake\ndata: {\"payload\":{\"candidate_count\":3,\"package_count\":2}}\n\n";
        let intake = parse_intake(frame).expect("intake");
        assert_eq!(intake.candidate_count, 3);
        assert_eq!(intake.package_count, 2);

        assert!(
            parse_intake("event: collector.changed\ndata: {}\n\n").is_none(),
            "an ordinary collector change is not an import"
        );
        assert!(
            parse_intake(": keep-alive\n\n").is_none(),
            "the keep-alive comment is not an event"
        );
    }

    #[test]
    fn a_payload_the_agent_does_not_understand_still_notifies() {
        let intake = parse_intake("event: collector.intake\ndata: {}\n\n").expect("intake");
        assert_eq!(summary(&intake), "Links added to the LinkGrabber");
    }

    #[test]
    fn the_wording_follows_the_number_of_links_and_packages() {
        let one = Intake {
            candidate_count: 1,
            package_count: 1,
            file_count: 0,
        };
        let many = Intake {
            candidate_count: 8,
            package_count: 1,
            file_count: 0,
        };
        let split = Intake {
            candidate_count: 8,
            package_count: 2,
            file_count: 0,
        };
        assert_eq!(summary(&one), "1 link added to the LinkGrabber");
        assert_eq!(summary(&many), "8 links added to the LinkGrabber");
        assert_eq!(
            summary(&split),
            "8 links in 2 packages added to the LinkGrabber"
        );
    }

    #[test]
    fn an_nzb_that_arrived_on_its_own_reports_files_not_links() {
        // The files inside one container are not links somebody added; wording them as such
        // is what made a single drop read like several separate intakes.
        assert_eq!(
            summary(&Intake {
                candidate_count: 0,
                package_count: 1,
                file_count: 5,
            }),
            "NZB added to the LinkGrabber, 5 files"
        );
        assert_eq!(
            summary(&Intake {
                candidate_count: 0,
                package_count: 1,
                file_count: 1,
            }),
            "NZB added to the LinkGrabber, 1 file"
        );
    }

    /// A burst that arrives inside one window is one toast naming the total, not one toast per
    /// submission. Dropping the sender ends the window early, so this asserts the summing
    /// without waiting on a clock.
    #[tokio::test]
    async fn a_burst_of_intakes_becomes_one_toast() {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        for count in [1_u32, 2, 3] {
            sender
                .try_send(Intake {
                    candidate_count: count,
                    package_count: 1,
                    file_count: 0,
                })
                .expect("queue the intake");
        }
        drop(sender);
        let shown = Arc::new(Mutex::new(Vec::new()));
        let collector = Arc::clone(&shown);
        announce(receiver, TOAST_WINDOW, move |intake| {
            let collector = Arc::clone(&collector);
            async move {
                collector.lock().expect("collect").push(intake);
            }
        })
        .await;
        let shown = shown.lock().expect("read");
        assert_eq!(shown.len(), 1, "three intakes produced more than one toast");
        assert_eq!(shown[0].candidate_count, 6);
        assert_eq!(shown[0].package_count, 3);
        assert_eq!(
            summary(&shown[0]),
            "6 links in 3 packages added to the LinkGrabber"
        );
    }

    /// The toast is not shown from inside the read loop and not the moment an event lands: the
    /// window has to pass first, which is what bounds how often the desktop is interrupted.
    #[tokio::test]
    async fn a_toast_waits_for_the_window_to_pass() {
        let window = Duration::from_millis(150);
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let shown = Arc::new(Mutex::new(Vec::new()));
        let collector = Arc::clone(&shown);
        let started = Instant::now();
        let announcing = tokio::spawn(announce(receiver, window, move |_| {
            let collector = Arc::clone(&collector);
            async move {
                collector.lock().expect("collect").push(started.elapsed());
            }
        }));
        sender
            .try_send(Intake::default())
            .expect("queue the intake");
        tokio::time::sleep(window * 3).await;
        drop(sender);
        announcing
            .await
            .expect("the announcer ends with its channel");
        let shown = shown.lock().expect("read");
        assert_eq!(shown.len(), 1);
        assert!(
            shown[0] >= window,
            "the toast ran before the window was up: {:?}",
            shown[0]
        );
    }
}
