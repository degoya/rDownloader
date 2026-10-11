//! "Add all from LinkGrabber" from the tray, started or paused (RD-1240-07).
//!
//! The tray names the request, the agent's transfer poll makes it with the agent's token
//! (`POST /api/v1/capture/linkgrabber/enqueue`, behind `capture:queue` like the queue entries),
//! and the answer is a desktop notification. What the notification says is decided here, on
//! every host; the answer carries counts and a code only, like the rest of the capture surface.

use anyhow::Error;

use crate::client::{CaptureClient, ServiceRefusal};

/// The refusal of an agent paired without queue control.
const SCOPE_INSUFFICIENT: &str = "auth.scope_insufficient";

/// What the service queued from the LinkGrabber.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct Enqueued {
    /// Links of the packages that went to the queue.
    #[serde(default)]
    pub links: u32,
    /// NZB imports that went to the queue.
    #[serde(default)]
    pub nzbs: u32,
    /// Packages and NZB imports left in the LinkGrabber for holding something already added.
    #[serde(default)]
    pub duplicates: u32,
    /// Packages and NZB imports that could not be queued.
    #[serde(default)]
    pub failed: u32,
    /// The stable code of the first failure.
    #[serde(default)]
    pub first_error: Option<String>,
}

/// Makes the request and shows its outcome. English, like the menu (RD-092-05).
pub(crate) async fn add_all(client: &CaptureClient, paused: bool) {
    let outcome = client.enqueue_linkgrabber(paused).await;
    if let Err(error) = &outcome {
        tracing::warn!(%error, paused, "the tray's LinkGrabber request was not carried out");
    }
    crate::notify::toast(notice(&outcome, paused)).await;
}

/// The notification for one outcome.
pub(crate) fn notice(outcome: &Result<Enqueued, Error>, paused: bool) -> String {
    match outcome {
        Ok(answer) => answered(answer, paused),
        Err(error) => refused(error),
    }
}

/// "3 links and 1 NZB added to the downloads, paused; 1 entry with links already added left in
/// the LinkGrabber; 1 failed (collector.package_busy)".
fn answered(answer: &Enqueued, paused: bool) -> String {
    let mut added = Vec::new();
    if answer.links > 0 {
        added.push(counted(answer.links, "link", "links"));
    }
    if answer.nzbs > 0 {
        added.push(counted(answer.nzbs, "NZB", "NZBs"));
    }
    let mut text = if !added.is_empty() {
        let how = if paused { ", paused" } else { "" };
        format!("{} added to the downloads{how}", added.join(" and "))
    } else if answer.duplicates == 0 && answer.failed == 0 {
        return "The LinkGrabber has nothing to add".to_owned();
    } else {
        "Nothing added from the LinkGrabber".to_owned()
    };
    if answer.duplicates > 0 {
        text.push_str(&format!(
            "; {} with links already added left in the LinkGrabber",
            counted(answer.duplicates, "entry", "entries")
        ));
    }
    if answer.failed > 0 {
        text.push_str(&format!("; {} failed", answer.failed));
        if let Some(code) = &answer.first_error {
            text.push_str(&format!(" ({code})"));
        }
    }
    text
}

/// A refused or failed request, by its code.
fn refused(error: &Error) -> String {
    let Some(refusal) = error.downcast_ref::<ServiceRefusal>() else {
        return "Nothing added from the LinkGrabber: rDownloader did not answer".to_owned();
    };
    match refusal.code() {
        Some(SCOPE_INSUFFICIENT) => "Pair the agent again to add from the LinkGrabber".to_owned(),
        Some(code) => format!("Nothing added from the LinkGrabber ({code})"),
        None => format!(
            "Nothing added from the LinkGrabber (HTTP {})",
            refusal.status().as_u16()
        ),
    }
}

fn counted(count: u32, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

#[cfg(test)]
#[path = "linkgrabber_tests.rs"]
mod tests;
