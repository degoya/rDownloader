use crate::client::{ServiceRefusal, Submitted};

use super::{
    ClipboardState, HandOver, MAX_CLIPBOARD_BYTES, MAX_SUBMISSION_RETRY_TICKS, links_to_hand_over,
    record_submission,
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
    record_submission(&mut state, candidate.hash, Ok(Submitted::Added));
    assert!(state.candidate(text).is_none(), "accepted content is kept");
}

/// A copied series page lands on the pick board once (RD-1190-17): repeated, every hand-over
/// listed the page again, and the drawer's list vanished under a new id.
#[test]
fn a_page_waiting_for_a_choice_is_not_handed_over_again() {
    let mut state = ClipboardState::default();
    let text = "https://series.example/serie/show/";
    let candidate = state.candidate(text).expect("new clipboard content");
    record_submission(&mut state, candidate.hash, Ok(Submitted::PickWaiting(30)));
    state.tick();
    assert!(state.candidate(text).is_none(), "the list is on the board");
    assert_eq!(
        HandOver::Listed(Submitted::PickWaiting(1)).message(),
        "A page lists 1 release; choose it in rDownloader's LinkGrabber"
    );
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

/// Paused, the loop does not read the clipboard at all, and a submission that was waiting for
/// its retry is dropped rather than delivered while the person has asked for nothing (RD-1180-01).
#[test]
fn while_paused_nothing_is_read_and_nothing_waiting_is_delivered() {
    let mut state = ClipboardState::default();
    let text = "https://example.com/before-the-pause.bin";
    let candidate = state.candidate(text).expect("new clipboard content");
    record_submission(
        &mut state,
        candidate.hash,
        Err(anyhow::anyhow!("connection refused")),
    );
    assert!(
        state.pending.is_some(),
        "a failed submission waits for its retry"
    );

    for _ in 0..5 {
        state.tick();
        assert!(!state.watching(true), "paused: the tick reads nothing");
    }
    assert!(
        state.pending.is_none(),
        "the retry is dropped with the pause"
    );
}

/// What was copied during the pause is on the clipboard when watching resumes. It is taken as
/// seen, not delivered; the next thing copied is delivered as usual.
#[test]
fn after_resuming_nothing_from_the_pause_is_delivered_but_the_next_copy_is() {
    let mut state = ClipboardState::default();
    assert!(state.watching(false));
    let before = state
        .candidate("https://example.com/watched.bin")
        .expect("watching delivers");
    state.accept(before.hash);

    assert!(!state.watching(true));
    // Copied during the pause; the loop does not see it until it resumes.
    let during = "https://example.com/copied-while-paused.bin";
    assert!(state.watching(false), "resumed");
    assert!(
        state.candidate(during).is_none(),
        "the link copied during the pause is not delivered afterwards"
    );
    assert!(
        state.candidate(during).is_none(),
        "nor on any later tick while it stays on the clipboard"
    );

    let after = state
        .candidate("https://example.com/copied-after-resuming.bin")
        .expect("the next copy is delivered");
    assert_eq!(
        after.urls[0].as_str(),
        "https://example.com/copied-after-resuming.bin"
    );
}

/// An empty clipboard at the moment watching resumes leaves nothing to take as seen, so the link
/// copied right after is delivered rather than swallowed as a leftover.
#[test]
fn an_empty_clipboard_at_resuming_does_not_swallow_the_next_copy() {
    let mut state = ClipboardState::default();
    assert!(!state.watching(true));
    assert!(state.watching(false));
    state.saw_nothing();
    assert!(
        state
            .candidate("https://example.com/copied-after-resuming.bin")
            .is_some()
    );

    // The same for a text too long to read: it is never delivered, and it is no leftover.
    assert!(!state.watching(true));
    assert!(state.watching(false));
    let oversized = "x".repeat(MAX_CLIPBOARD_BYTES + 1);
    assert!(state.candidate(&oversized).is_none());
    assert!(
        state
            .candidate("https://example.com/another-copy.bin")
            .is_some()
    );
}

/// "Hand over clipboard now" (RD-1180-03): the links of one text, or why there is nothing to hand
/// over, in the words the notification uses.
#[test]
fn handing_over_says_how_many_links_or_that_there_was_none() {
    let candidate =
        links_to_hand_over("see https://example.com/a.bin and https://example.com/b.bin")
            .expect("two links");
    assert_eq!(candidate.urls.len(), 2);
    assert_eq!(
        HandOver::Delivered(candidate.urls.len()).message(),
        "2 links from the clipboard handed over"
    );
    assert_eq!(
        HandOver::Delivered(1).message(),
        "1 link from the clipboard handed over"
    );

    let none = links_to_hand_over("just words").expect_err("no link");
    assert_eq!(none, HandOver::NoLinks);
    assert_eq!(none.message(), "No link found on the clipboard");
    assert_eq!(
        links_to_hand_over(&"x".repeat(MAX_CLIPBOARD_BYTES + 1)).expect_err("too long"),
        HandOver::TooLong
    );
}

/// A hand-over while watching runs records the text, so the watcher does not deliver it again.
#[test]
fn a_handed_over_text_is_not_delivered_again_by_the_watcher() {
    let mut state = ClipboardState::default();
    let text = "https://example.com/handed-over.bin";
    let candidate = links_to_hand_over(text).expect("a link");
    state.accept(candidate.hash);
    assert!(state.watching(false));
    assert!(state.candidate(text).is_none());
}
