//! The clipboard subsystem: the loop that watches what was copied, and the rule that decides
//! which refusal is a decision and which one deserves another try.
//!
//! Watching can be paused from the tray, a shortcut, the settings page or MCP (RD-1180-01). Paused,
//! the loop does not read the clipboard at all, and when it resumes, what was on the clipboard at
//! that moment counts as seen: a link copied during the pause is never delivered later. "Hand
//! over clipboard now" (RD-1180-03) reads it once on request, paused or not.

use std::time::Duration;

use anyhow::Result;
use arboard::{Clipboard, Error as ClipboardError};
use rd_core::CaptureAgentSettings;
use sha2::Digest;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::{CaptureClient, Detail, ServiceRefusal};

pub(crate) async fn watch_clipboard(
    client: CaptureClient,
    cancellation: CancellationToken,
    settings: watch::Receiver<CaptureAgentSettings>,
    mut hand_overs: mpsc::UnboundedReceiver<()>,
) -> Result<()> {
    let mut state = ClipboardState::default();
    let mut clipboard = None;
    let mut unavailable_logged = false;
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            // "Hand over clipboard now" (RD-1180-03): one read, paused or not, through the same
            // state, so the watcher does not deliver the same text a second time.
            Some(()) = hand_overs.recv() => {
                let outcome =
                    hand_over(&client, &mut clipboard, &mut unavailable_logged, &mut state).await;
                tracing::info!(?outcome, "clipboard handed over on request");
                crate::notify::toast(outcome.message()).await;
            }
            _ = ticker.tick() => {
                state.tick();
                // Not read at all while paused (RD-1180-01): the person said leave it alone.
                if !state.watching(settings.borrow().clipboard_paused) {
                    continue;
                }
                let text = match read_text(&mut clipboard, &mut unavailable_logged).await {
                    Read::Text(text) => text,
                    Read::Nothing => {
                        state.saw_nothing();
                        continue;
                    }
                    Read::Failed => continue,
                };
                if let Some(candidate) = state.candidate(&text) {
                    if candidate.urls.is_empty() {
                        state.accept(candidate.hash);
                    } else {
                        let result = client
                            .submit_links(candidate.urls, "clipboard", None, None)
                            .await;
                        record_submission(&mut state, candidate.hash, result);
                    }
                }
            }
        }
    }
}

/// What one look at the clipboard found.
enum Read {
    Text(String),
    /// The clipboard holds no text: empty, or a picture.
    Nothing,
    /// It could not be read this time; the next tick tries again.
    Failed,
}

/// Reads the clipboard once, opening it first when it is not open yet.
async fn read_text(clipboard: &mut Option<Clipboard>, unavailable_logged: &mut bool) -> Read {
    if clipboard.is_none() {
        match Clipboard::new() {
            Ok(value) => {
                *clipboard = Some(value);
                *unavailable_logged = false;
            }
            Err(error) => {
                if !*unavailable_logged {
                    tracing::warn!(%error, "clipboard unavailable; retrying");
                    *unavailable_logged = true;
                }
                return Read::Failed;
            }
        }
    }
    let Some(active_clipboard) = clipboard.take() else {
        return Read::Failed;
    };
    // Off the runtime. `arboard` talks to the window server synchronously and waits for
    // whichever program owns the clipboard to answer, so a frozen browser, a remote-desktop
    // session with clipboard forwarding or a compositor under load used to take a tokio worker
    // with it -- and a desktop agent has few enough workers that this could stall the event
    // stream, the transfer poll and Click'n'Load along with it. `notify.rs` does the same for
    // its equally blocking call (RD-109-08).
    //
    // The handle travels with the call and comes back: `arboard` only truly opens the Windows
    // clipboard for the length of one operation, and these operations are strictly sequential --
    // never two at once, which is the case its documentation warns about.
    let read = tokio::task::spawn_blocking(move || {
        let mut active_clipboard = active_clipboard;
        let text = active_clipboard.get_text();
        (active_clipboard, text)
    })
    .await;
    let (returned, text) = match read {
        Ok(pair) => pair,
        Err(error) => {
            tracing::warn!(%error, "clipboard read task ended; reopening");
            return Read::Failed;
        }
    };
    *clipboard = Some(returned);
    match text {
        Ok(text) => {
            *unavailable_logged = false;
            Read::Text(text)
        }
        Err(ClipboardError::ContentNotAvailable) => Read::Nothing,
        Err(ClipboardError::ClipboardOccupied) => Read::Failed,
        Err(error) => {
            if !*unavailable_logged {
                tracing::warn!(%error, "clipboard read failed; retrying");
                *unavailable_logged = true;
            }
            *clipboard = None;
            Read::Failed
        }
    }
}

/// What "Hand over clipboard now" came to, as the notification says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HandOver {
    Delivered(usize),
    /// Nothing on the clipboard is a link.
    NoLinks,
    /// More text than the agent reads ([`MAX_CLIPBOARD_BYTES`]).
    TooLong,
    /// The collector looked at the links and took none of them; carries its code.
    Declined(String),
    /// The clipboard could not be read.
    Unreadable,
    /// The service did not answer, or failed.
    Failed,
}

impl HandOver {
    /// The notification's text. English, like the rest of the agent (RD-092-05).
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Delivered(1) => "1 link from the clipboard handed over".to_owned(),
            Self::Delivered(count) => format!("{count} links from the clipboard handed over"),
            Self::NoLinks => "No link found on the clipboard".to_owned(),
            Self::TooLong => "The clipboard holds more text than the agent reads".to_owned(),
            Self::Declined(_) => "rDownloader took none of the links on the clipboard".to_owned(),
            Self::Unreadable => "The clipboard could not be read".to_owned(),
            Self::Failed => {
                "The links could not be handed over; is rDownloader running?".to_owned()
            }
        }
    }
}

/// The links of one clipboard text and its hash, or why there is nothing to hand over.
fn links_to_hand_over(text: &str) -> Result<ClipboardCandidate, HandOver> {
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err(HandOver::TooLong);
    }
    let urls = rd_collector::extract_urls(text);
    if urls.is_empty() {
        return Err(HandOver::NoLinks);
    }
    Ok(ClipboardCandidate {
        hash: sha2::Sha256::digest(text.as_bytes()).to_vec(),
        urls,
    })
}

/// Reads the clipboard once and delivers its links, whether watching is paused or not.
async fn hand_over(
    client: &CaptureClient,
    clipboard: &mut Option<Clipboard>,
    unavailable_logged: &mut bool,
    state: &mut ClipboardState,
) -> HandOver {
    let text = match read_text(clipboard, unavailable_logged).await {
        Read::Text(text) => text,
        Read::Nothing => return HandOver::NoLinks,
        Read::Failed => return HandOver::Unreadable,
    };
    let candidate = match links_to_hand_over(&text) {
        Ok(candidate) => candidate,
        Err(outcome) => return outcome,
    };
    let count = candidate.urls.len();
    match client
        .submit_links(candidate.urls, "clipboard", None, None)
        .await
    {
        Ok(()) => {
            state.accept(candidate.hash);
            HandOver::Delivered(count)
        }
        Err(error) => match decided_submission_code(&error) {
            Some(code) => {
                state.accept(candidate.hash);
                HandOver::Declined(code.to_owned())
            }
            None => {
                tracing::warn!(%error, "the clipboard could not be handed over");
                HandOver::Failed
            }
        },
    }
}

/// `rdownloader-capture send-clipboard`: the same hand-over from a process of its own.
pub(crate) async fn hand_over_once(client: &CaptureClient) -> HandOver {
    let mut clipboard = None;
    let mut unavailable_logged = false;
    hand_over(
        client,
        &mut clipboard,
        &mut unavailable_logged,
        &mut ClipboardState::default(),
    )
    .await
}

/// Codes with which the service said "looked at it, no".
///
/// Each one is a decision the collector reached by inspecting the very text that was submitted:
/// the blocklist matched every host, nothing in the text was a link at all, or every link needs
/// a transfer service that is switched off. None of those answers can change while the same
/// text sits on the clipboard, so repeating the request once a second only fills the log.
///
/// The list is deliberately short, explicit and closed. Anything not named here keeps being
/// retried — a network error, a 5xx, an expired token, a code this build has never heard of —
/// because a real failure that is quietly recorded as handled is the worse mistake: nothing
/// would ever report it again.
const DECIDED_SUBMISSION_CODES: [&str; 3] = [
    "collector.all_links_excluded",
    "collector.all_links_disabled",
    "collector.no_links_found",
];

/// Names the code when the service refused on its own judgement rather than failing.
///
/// Two conditions, both required: the status must be a client error — a 5xx is the service's
/// problem and may well pass on the next tick — and the stable code must be one this agent
/// knows to be final. The decision is made on the code, never on the text of the message,
/// which is prose the service is free to reword or translate.
fn decided_submission_code(error: &anyhow::Error) -> Option<&str> {
    let refusal = error.downcast_ref::<ServiceRefusal>()?;
    if !refusal.status().is_client_error() {
        return None;
    }
    let code = refusal.code()?;
    DECIDED_SUBMISSION_CODES.contains(&code).then_some(code)
}

/// Records what became of one clipboard submission.
///
/// A success and an explicit refusal are both "handled": the entry is remembered, so the next
/// tick leaves that clipboard content alone. Everything else is deferred rather than recorded,
/// so the same text is offered again — a service that is still starting or a network that
/// blinked deserve exactly that (RD-107-15) — but with a growing gap in front of it.
fn record_submission(state: &mut ClipboardState, hash: Vec<u8>, result: Result<()>) {
    match result {
        Ok(()) => {
            state.accept(hash);
            tracing::info!("clipboard links submitted");
        }
        Err(error) => match decided_submission_code(&error) {
            Some(code) => {
                state.accept(hash);
                tracing::info!(
                    %code,
                    %error,
                    "collector declined the clipboard links; not resubmitting them"
                );
            }
            None => {
                let wait = state.defer(hash);
                // A body that broke off on the way reads exactly like a correct, detail-free
                // refusal unless it is named. It is a connection problem, not a verdict on the
                // links, and saying so is the difference between looking at the network and
                // looking at the service (RD-109-10).
                if matches!(
                    error
                        .downcast_ref::<ServiceRefusal>()
                        .map(ServiceRefusal::detail),
                    Some(Detail::Unreadable(_))
                ) {
                    tracing::warn!(
                        %error,
                        retry_in_seconds = wait,
                        "the service answered but the answer broke off on the way"
                    );
                } else {
                    tracing::warn!(%error, retry_in_seconds = wait, "clipboard submission failed");
                }
            }
        },
    }
}

/// Longest gap between two attempts at the same clipboard content, in ticks of one second.
///
/// Five minutes. The failures that are never "decided" include the ones that do not pass on
/// their own — an expired capture token, a 403, a service that answers 500 for hours — and
/// without a ceiling the agent re-POSTed the identical links and wrote an identical `warn!`
/// every second for as long as that text sat on the clipboard.
const MAX_SUBMISSION_RETRY_TICKS: u32 = 300;

/// Longest clipboard text the agent looks at, in bytes.
///
/// A mebibyte. Every other input of this crate has a bound -- `MAX_URL_CHARS` and `MAX_LINKS` in
/// `scheme.rs`, the Click'n'Load limits, `MAX_PENDING_BYTES` in `sse.rs` -- and the clipboard did
/// not, so a copied log, CSV or page of a few megabytes was hashed with SHA-256 and scanned for
/// links once a second for as long as it sat there. The deduplication happens *after* the hash,
/// so it saved the request and not the work.
///
/// Discarded, not truncated: half a text hashes to something that belongs to nothing, and the
/// cut can land inside a URL, which the collector would then take as a link of its own
/// (RD-109-08).
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct ClipboardState {
    last_hash: Option<Vec<u8>>,
    pending: Option<PendingSubmission>,
    /// Whether the oversized clipboard content has already been mentioned. A big document that
    /// is simply left on the clipboard must not write a line every second.
    oversize_reported: bool,
    /// Watching is paused (RD-1180-01).
    paused: bool,
    /// Watching has just resumed: what the clipboard holds now was copied during the pause and
    /// is taken as seen rather than delivered.
    resumed: bool,
}

/// Clipboard content that failed and is waiting to be offered again.
///
/// Only one is kept: the loop looks at whatever is on the clipboard now, so a second failing
/// text replaces the first rather than joining it.
struct PendingSubmission {
    hash: Vec<u8>,
    failures: u32,
    ticks_remaining: u32,
}

#[derive(Debug)]
struct ClipboardCandidate {
    hash: Vec<u8>,
    urls: Vec<Url>,
}

impl ClipboardState {
    /// Advances the backoff by one tick of the clipboard loop.
    ///
    /// Counted in ticks rather than measured against a clock because the loop ticks once a
    /// second either way, and a counter is what a test can drive.
    fn tick(&mut self) {
        if let Some(pending) = self.pending.as_mut() {
            pending.ticks_remaining = pending.ticks_remaining.saturating_sub(1);
        }
    }

    /// Whether this tick reads the clipboard, given whether watching is paused.
    ///
    /// Paused, nothing is read and a submission waiting for its retry is dropped: it was copied
    /// before the pause, but the person has asked for nothing more to be delivered.
    fn watching(&mut self, paused: bool) -> bool {
        if paused {
            self.paused = true;
            self.pending = None;
            return false;
        }
        if self.paused {
            self.paused = false;
            self.resumed = true;
        }
        true
    }

    /// The first read after a pause found no text, so nothing from the pause is left over.
    fn saw_nothing(&mut self) {
        self.resumed = false;
    }

    fn candidate(&mut self, text: &str) -> Option<ClipboardCandidate> {
        // Before the hash and before the link scan, which are the two pieces of work that scale
        // with the length of the text.
        if text.len() > MAX_CLIPBOARD_BYTES {
            // Never delivered either way, so nothing of the pause is left to take as seen.
            self.resumed = false;
            if !self.oversize_reported {
                tracing::debug!(
                    bytes = text.len(),
                    limit = MAX_CLIPBOARD_BYTES,
                    "clipboard content is longer than the agent reads; leaving it alone"
                );
                self.oversize_reported = true;
            }
            return None;
        }
        self.oversize_reported = false;
        let hash = sha2::Sha256::digest(text.as_bytes()).to_vec();
        if self.resumed {
            self.resumed = false;
            self.accept(hash);
            return None;
        }
        if self.last_hash.as_ref() == Some(&hash) {
            return None;
        }
        if let Some(pending) = self.pending.as_ref()
            && pending.hash == hash
            && pending.ticks_remaining > 0
        {
            return None;
        }
        Some(ClipboardCandidate {
            hash,
            urls: rd_collector::extract_urls(text),
        })
    }

    fn accept(&mut self, hash: Vec<u8>) {
        self.last_hash = Some(hash);
        self.pending = None;
    }

    /// Holds content back after a failure and reports how many seconds until the next try.
    ///
    /// The first retry is still one tick away, so the momentary blink the retry exists for is
    /// picked up as promptly as before; only a failure that keeps repeating is slowed down.
    fn defer(&mut self, hash: Vec<u8>) -> u32 {
        let failures = match self.pending.as_ref() {
            Some(pending) if pending.hash == hash => pending.failures.saturating_add(1),
            _ => 1,
        };
        // `failures - 1` so the first wait is one tick; `min(16)` keeps the shift far away
        // from overflowing before the cap does its work.
        let ticks = 1_u32
            .checked_shl(failures.saturating_sub(1).min(16))
            .unwrap_or(MAX_SUBMISSION_RETRY_TICKS)
            .min(MAX_SUBMISSION_RETRY_TICKS);
        self.pending = Some(PendingSubmission {
            hash,
            failures,
            ticks_remaining: ticks,
        });
        ticks
    }
}

#[cfg(test)]
#[path = "clipboard_tests.rs"]
mod tests;
