//! The clipboard subsystem: the loop that watches what was copied, and the rule that decides
//! which refusal is a decision and which one deserves another try.

use std::time::Duration;

use anyhow::Result;
use arboard::{Clipboard, Error as ClipboardError};
use sha2::Digest;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::{CaptureClient, Detail, ServiceRefusal};

pub(crate) async fn watch_clipboard(
    client: CaptureClient,
    cancellation: CancellationToken,
) -> Result<()> {
    let mut state = ClipboardState::default();
    let mut clipboard = None;
    let mut unavailable_logged = false;
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            _ = ticker.tick() => {
                state.tick();
                if clipboard.is_none() {
                    match Clipboard::new() {
                        Ok(value) => {
                            clipboard = Some(value);
                            unavailable_logged = false;
                        }
                        Err(error) => {
                            if !unavailable_logged {
                                tracing::warn!(%error, "clipboard unavailable; retrying");
                                unavailable_logged = true;
                            }
                            continue;
                        }
                    }
                }
                let Some(active_clipboard) = clipboard.take() else {
                    continue;
                };
                // Off the runtime. `arboard` talks to the window server synchronously and waits
                // for whichever program owns the clipboard to answer, so a frozen browser, a
                // remote-desktop session with clipboard forwarding or a compositor under load
                // used to take a tokio worker with it -- and a desktop agent has few enough
                // workers that this could stall the event stream, the transfer poll and
                // Click'n'Load along with it. `notify.rs` does the same for its equally
                // blocking call (RD-109-08).
                //
                // The handle travels with the call and comes back: `arboard` only truly opens
                // the Windows clipboard for the length of one operation, and these operations
                // are strictly sequential -- never two at once, which is the case its
                // documentation warns about.
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
                        continue;
                    }
                };
                clipboard = Some(returned);
                let text = match text {
                    Ok(text) => text,
                    Err(ClipboardError::ContentNotAvailable | ClipboardError::ClipboardOccupied) => {
                        continue;
                    }
                    Err(error) => {
                        if !unavailable_logged {
                            tracing::warn!(%error, "clipboard read failed; retrying");
                            unavailable_logged = true;
                        }
                        clipboard = None;
                        continue;
                    }
                };
                unavailable_logged = false;
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

    fn candidate(&mut self, text: &str) -> Option<ClipboardCandidate> {
        // Before the hash and before the link scan, which are the two pieces of work that scale
        // with the length of the text.
        if text.len() > MAX_CLIPBOARD_BYTES {
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
mod tests {
    use crate::client::ServiceRefusal;

    use super::{
        ClipboardState, MAX_CLIPBOARD_BYTES, MAX_SUBMISSION_RETRY_TICKS, record_submission,
    };

    /// The refusal the service really sends, as an `anyhow::Error` the loop would see.
    fn refusal(status: u16, body: &str) -> anyhow::Error {
        ServiceRefusal::new(
            "collector",
            reqwest::StatusCode::from_u16(status).expect("a real status"),
            body,
        )
        .into()
    }

    /// A copied document of a few megabytes used to be hashed and scanned once a second for as
    /// long as it sat on the clipboard, which is a laptop fan that never stops (RD-109-08).
    #[test]
    fn an_oversized_clipboard_text_is_left_alone_and_said_once() {
        let link = "https://example.com/file.bin";
        // A newline between the two, or the padding would run onto the end of the link and
        // the collector would read one very long URL instead.
        let padding = MAX_CLIPBOARD_BYTES - link.len() - 1;

        let mut state = ClipboardState::default();
        let just_over = format!("{link}\n{}", "x".repeat(padding + 1));
        assert_eq!(just_over.len(), MAX_CLIPBOARD_BYTES + 1);
        assert!(
            state.candidate(&just_over).is_none(),
            "a text over the limit must not be hashed or scanned, link in it or not"
        );
        assert!(state.oversize_reported, "and it is mentioned");
        // Still on the clipboard on the next tick: mentioned once, not once a second.
        state.oversize_reported = false;
        assert!(state.candidate(&just_over).is_none());
        assert!(state.oversize_reported);
        let mut quiet = ClipboardState::default();
        assert!(quiet.candidate(&just_over).is_none());
        assert!(quiet.candidate(&just_over).is_none());
        assert!(
            quiet.oversize_reported,
            "the flag stays set across ticks rather than being reset and reported again"
        );

        // Just under the limit is ordinary content and is handled as before.
        let just_under = format!("{link}\n{}", "x".repeat(padding));
        assert_eq!(just_under.len(), MAX_CLIPBOARD_BYTES);
        let candidate = state
            .candidate(&just_under)
            .expect("a text at the limit is still read");
        assert_eq!(candidate.urls.len(), 1);
        assert_eq!(candidate.urls[0].as_str(), link);
        assert!(
            !state.oversize_reported,
            "the next oversized text is worth mentioning again"
        );
    }

    #[test]
    fn clipboard_state_retries_until_accepted_and_then_deduplicates() {
        let mut state = ClipboardState::default();
        let text = "Download https://example.com/file.bin";
        let first = state.candidate(text).expect("new clipboard content");
        assert_eq!(first.urls.len(), 1);
        assert_eq!(first.urls[0].as_str(), "https://example.com/file.bin");
        assert!(
            state.candidate(text).is_some(),
            "failed submission must retry"
        );
        state.accept(first.hash);
        assert!(
            state.candidate(text).is_none(),
            "accepted content is deduplicated"
        );
    }

    /// The reported defect: twelve submissions in twelve seconds and no end in sight.
    #[test]
    fn a_declined_clipboard_is_submitted_exactly_once() {
        let mut state = ClipboardState::default();
        let text = "https://blocked.example/file.bin";
        let candidate = state.candidate(text).expect("new clipboard content");
        record_submission(
            &mut state,
            candidate.hash,
            Err(refusal(
                400,
                r#"{"error":"All links were skipped by the domain blocklist","code":"collector.all_links_excluded"}"#,
            )),
        );
        assert!(
            state.candidate(text).is_none(),
            "the collector looked at these addresses and said no; the next tick must not offer \
             the same text again"
        );
    }

    /// A success still records the content, exactly as before.
    #[test]
    fn a_submitted_clipboard_is_not_submitted_twice() {
        let mut state = ClipboardState::default();
        let text = "https://example.com/file.bin";
        let candidate = state.candidate(text).expect("new clipboard content");
        record_submission(&mut state, candidate.hash, Ok(()));
        assert!(state.candidate(text).is_none(), "accepted content is kept");
    }

    /// Everything the service did not decide keeps its retry, because a failure that is
    /// silently recorded as handled is never reported again.
    #[test]
    fn a_transient_failure_is_submitted_again() {
        let text = "https://example.com/file.bin";
        let cases: Vec<(&str, anyhow::Error)> = vec![
            (
                "a network error never reached the service at all",
                anyhow::anyhow!("error sending request: connection refused"),
            ),
            (
                "a 5xx is the service's own problem, not a verdict on the links",
                refusal(500, r#"{"error":"boom","code":"internal_error"}"#),
            ),
            (
                "a client error this build has never heard of is not known to be final",
                refusal(400, r#"{"error":"nope","code":"collector.something_new"}"#),
            ),
            (
                "a client error without a code decides nothing",
                refusal(400, "Bad Request"),
            ),
            (
                "an expired token is fixed by pairing again, not by forgetting the links",
                refusal(
                    401,
                    r#"{"error":"token revoked","code":"auth.unauthorized"}"#,
                ),
            ),
        ];
        for (reason, error) in cases {
            // A fresh state per case: each of these asks what *one* undecided failure does,
            // and sharing the state would instead measure the backoff growing across five of
            // them, which is the next test's job.
            let mut state = ClipboardState::default();
            let candidate = state.candidate(text).expect("new clipboard content");
            record_submission(&mut state, candidate.hash, Err(error));
            // The first retry is one tick away, and the loop ticks once a second, so this is
            // the very next pass through it.
            state.tick();
            assert!(state.candidate(text).is_some(), "{reason}");
        }
    }

    /// The other half of RD-107-15: an error the collector never decides must not turn into an
    /// unbounded loop. An expired capture token is exactly that — it is nobody's transient
    /// blink, and the agent used to re-POST the same links and log the same warning every
    /// second for as long as the text stayed on the clipboard.
    #[test]
    fn a_failure_that_never_resolves_backs_off_to_a_cap() {
        let mut state = ClipboardState::default();
        let text = "https://example.com/file.bin";
        let hash = state.candidate(text).expect("new clipboard content").hash;

        // The first retry is still prompt, which is what the retry exists for.
        assert_eq!(state.defer(hash.clone()), 1);
        assert!(
            state.candidate(text).is_none(),
            "content that has just failed is not offered again in the same tick"
        );
        state.tick();
        assert!(state.candidate(text).is_some(), "one tick later it is");

        let waits: Vec<u32> = (0..12).map(|_| state.defer(hash.clone())).collect();
        assert!(waits[0] < waits[1] && waits[1] < waits[2], "{waits:?}");
        for wait in &waits {
            assert!(*wait <= MAX_SUBMISSION_RETRY_TICKS, "{wait}");
        }
        assert_eq!(waits[11], MAX_SUBMISSION_RETRY_TICKS);

        // Content that finally succeeds starts over with a clean slate.
        state.accept(hash.clone());
        assert_eq!(state.defer(hash), 1);
    }
}
