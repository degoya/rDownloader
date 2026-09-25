//! What a Seedr answer means, pinned down on the host target.
//!
//! The state machine is the part worth testing here rather than in the contract suite: the
//! whole of it turns on the one thing Seedr does that the other four providers of this world do
//! not — a finished transfer stops being a transfer.

use seedr_common::{folder::Listing, reason::ErrorEnvelope};

use super::{
    ErrorKind, Stage, bounded_name, created_title, created_transfer_id, failure_from, find_folder,
    magnet_display_name, place, retry_after_seconds, stage_of,
};
use crate::messages;

fn listing(body: &str) -> Listing {
    Listing::of(body.as_bytes()).expect("a listing")
}

fn envelope(body: &str) -> ErrorEnvelope {
    ErrorEnvelope::of(body.as_bytes())
}

/// A transfer that is running is `working`, and one Seedr has not started on is `preparing` —
/// not `working` at zero, which is a progress bar that does not move for an hour.
#[test]
fn a_running_transfer_reports_its_progress_and_a_fresh_one_waits() {
    let running = listing(r#"{"torrents":[{"id":11,"name":"Example","progress":42.5}]}"#);
    assert_eq!(stage_of(&running, 11, "Example"), Stage::Working(Some(425)));

    let fresh = listing(r#"{"torrents":[{"id":11,"name":"Example","progress":0}]}"#);
    assert_eq!(stage_of(&fresh, 11, "Example"), Stage::Preparing(15));

    let silent = listing(r#"{"torrents":[{"id":11,"name":"Example"}]}"#);
    assert_eq!(stage_of(&silent, 11, "Example"), Stage::Preparing(15));
}

/// The one thing about Seedr the whole design turns on: a finished transfer is not a transfer
/// with a finished flag, it is a folder. Its own document warns that even 100 % is not the end
/// — 101 % means the folder exists — so the transfer leaving the list is what finished means.
#[test]
fn a_transfer_that_became_a_folder_is_ready() {
    let done = listing(
        r#"{"torrents":[],"folders":[{"id":5,"name":"Example"},{"id":6,"name":"Something else"}]}"#,
    );
    assert_eq!(stage_of(&done, 11, "Example"), Stage::Ready(5));

    // At 100 % it is still a transfer, so it is still running.
    let nearly = listing(
        r#"{"torrents":[{"id":11,"name":"Example","progress":100}],
            "folders":[{"id":5,"name":"Example"}]}"#,
    );
    assert_eq!(stage_of(&nearly, 11, "Example"), Stage::Working(Some(1000)));
}

/// A transfer that is gone with no folder to show for it is what a transfer somebody removed in
/// Seedr's own interface looks like. Waiting for it would keep a row polling an account for
/// ever.
#[test]
fn a_transfer_that_is_gone_without_a_folder_ends_the_job() {
    let empty = listing(r#"{"torrents":[],"folders":[]}"#);
    assert_eq!(
        stage_of(&empty, 11, "Example"),
        Stage::Failed(messages::TRANSFER_GONE)
    );
    // And a job that never had a name cannot be matched to any folder, so it ends the same way
    // rather than adopting the first folder in the account.
    let named = listing(r#"{"torrents":[],"folders":[{"id":5,"name":"Example"}]}"#);
    assert_eq!(
        stage_of(&named, 11, ""),
        Stage::Failed(messages::TRANSFER_GONE)
    );
    assert_eq!(find_folder(&named, "  "), None);
    assert_eq!(find_folder(&named, " Example "), Some(5));
}

#[test]
fn the_created_transfer_is_read_out_of_whichever_spelling_arrived() {
    for body in [
        br#"{"result":true,"user_torrent_id":11,"title":"Example"}"#.as_slice(),
        br#"{"result":true,"torrent_id":"11","title":"Example"}"#,
        br#"{"result":true,"id":11,"name":"Example"}"#,
    ] {
        assert_eq!(created_transfer_id(body), Some(11), "{body:?}");
        assert_eq!(created_title(body).as_deref(), Some("Example"));
    }
    // An answer that named nothing is not a transfer this installation can poll.
    assert_eq!(created_transfer_id(br#"{"result":true}"#), None);
    assert_eq!(created_transfer_id(br#"["11"]"#), None);
    assert_eq!(created_title(br#"{"result":true}"#), None);
}

/// A transfer with no name at all could never be matched to the folder it becomes, so the
/// magnet's own display name is worth recovering.
#[test]
fn a_magnet_display_name_is_the_fallback_for_a_transfer_seedr_did_not_name() {
    assert_eq!(
        magnet_display_name("magnet:?xt=urn:btih:da39&dn=Example.Release.2026").as_deref(),
        Some("Example.Release.2026")
    );
    assert_eq!(
        magnet_display_name("magnet:?xt=urn:btih:da39&dn=Example%20Release").as_deref(),
        Some("Example Release")
    );
    assert_eq!(
        magnet_display_name("magnet:?xt=urn:btih:da39&dn=Example+Release").as_deref(),
        Some("Example Release")
    );
    assert_eq!(magnet_display_name("magnet:?xt=urn:btih:da39"), None);
    assert_eq!(magnet_display_name("https://example.invalid/x"), None);
}

/// Seedr's own text, stored on a handle and compared against a folder name later, so it is
/// bounded and stripped rather than trusted.
#[test]
fn a_name_is_bounded_and_stripped_of_controls() {
    assert_eq!(bounded_name("  Example\u{7}  "), "Example");
    assert_eq!(bounded_name(&"x".repeat(400)).len(), 255);
    // Cut at a whole character, never inside one. Written as an escape rather than as the
    // character, because a Rust source in this repository is ASCII (`no_german.rs`).
    let wide = bounded_name(&"\u{e4}".repeat(200));
    assert!(wide.len() <= 255);
    assert!(wide.chars().all(|character| character == '\u{e4}'));
}

/// The transfer's own name is the root of every package hint, and it is not repeated when the
/// walk's first segment is the same name — otherwise every package lands in `Name/Name`.
#[test]
fn a_package_hint_is_the_transfer_name_and_the_folders_below_it() {
    assert_eq!(place("Example", &[]).as_deref(), Some("Example"));
    assert_eq!(
        place("Example", &["Season 1".to_owned()]).as_deref(),
        Some("Example/Season 1")
    );
    assert_eq!(
        place("Example", &["Example".to_owned(), "Extras".to_owned()]).as_deref(),
        Some("Example/Extras")
    );
    // Nothing that could leave the package survives, and a transfer with no name at all still
    // gets the folders it had.
    assert_eq!(
        place(
            "Example",
            &["..".to_owned(), "a/b".to_owned(), String::new()]
        )
        .as_deref(),
        Some("Example")
    );
    assert_eq!(place("", &[]), None);
}

#[test]
fn a_plain_answer_is_not_a_refusal_and_a_two_hundred_that_says_no_is() {
    assert!(failure_from(200, None, &envelope(r#"{"result":true}"#)).is_none());
    let refusal = failure_from(
        200,
        None,
        &envelope(r#"{"result":false,"error":"bad_magnet"}"#),
    )
    .expect("a refusal");
    assert_eq!(refusal.code, messages::API_ERROR.0);
    assert_eq!(refusal.kind, ErrorKind::Permanent);
    assert_eq!(
        refusal.params,
        vec![("reason", "bad_magnet".to_owned())],
        "the word travels and the sentence does not"
    );
}

#[test]
fn a_rejected_credential_and_a_plan_that_does_not_reach_are_told_apart() {
    let rejected = failure_from(401, None, &envelope("")).expect("a refusal");
    assert_eq!(rejected.code, messages::AUTH_INVALID.0);
    assert_eq!(rejected.kind, ErrorKind::AccountInvalid);

    let plan = failure_from(402, None, &envelope("")).expect("a refusal");
    assert_eq!(plan.code, messages::PLAN_REQUIRED.0);
    assert_eq!(plan.kind, ErrorKind::Unsupported);
}

/// Seedr's own answer for a transfer that did not fit says in its own name that the content was
/// remembered, so a person who frees space has a job that can still run.
#[test]
fn a_full_account_waits_rather_than_ending_the_job() {
    let full = failure_from(
        200,
        None,
        &envelope(r#"{"result":"not_enough_space_added_to_wishlist","code":400}"#),
    )
    .expect("a refusal");
    assert_eq!(full.code, messages::OUT_OF_SPACE.0);
    assert_eq!(full.kind, ErrorKind::Transient(Some(300)));
}

#[test]
fn a_rate_limit_waits_for_as_long_as_the_header_asks() {
    let stated = failure_from(429, Some(120), &envelope("")).expect("a refusal");
    assert_eq!(stated.code, messages::RATE_LIMITED.0);
    assert_eq!(stated.kind, ErrorKind::RateLimited(Some(120)));
    let bare = failure_from(429, None, &envelope("")).expect("a refusal");
    assert_eq!(bare.kind, ErrorKind::RateLimited(Some(60)));

    for bad in ["Wed, 21 Oct 2026 07:28:00 GMT", "-5", "0", "999999"] {
        assert_eq!(retry_after_seconds(Some(bad)), None, "{bad}");
    }
    assert_eq!(retry_after_seconds(Some("120")), Some(120));
}

/// A missing transfer is `offline` rather than permanent, which is what lets `discard` treat it
/// as already done: there is nothing left to remove.
#[test]
fn a_missing_transfer_is_offline_and_an_outage_waits() {
    let gone = failure_from(404, None, &envelope("")).expect("a refusal");
    assert_eq!(gone.code, messages::TRANSFER_GONE.0);
    assert_eq!(gone.kind, ErrorKind::Offline);

    let outage = failure_from(503, None, &envelope("")).expect("a refusal");
    assert_eq!(outage.code, messages::SERVER_ERROR.0);
    assert_eq!(outage.kind, ErrorKind::Transient(Some(300)));
}

#[test]
fn a_status_no_document_explains_carries_the_number_and_nothing_else() {
    let failure = failure_from(418, None, &envelope("<html>418</html>")).expect("a refusal");
    assert_eq!(failure.code, messages::HTTP_ERROR.0);
    assert_eq!(failure.message, "Seedr HTTP status 418");
    assert_eq!(failure.params, vec![("status", "418".to_owned())]);
}
