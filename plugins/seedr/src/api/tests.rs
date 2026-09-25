//! What Seedr's answers mean, pinned down on the host target.
//!
//! The cases here are the four the job's cross-cutting requirement names — success, rate
//! limit, authentication expiry and error — plus the two shapes that are specific to this
//! provider: a refusal served with a 200, and a plan that does not include the API.

use plugin_common::FailureKind;
use seedr_common::reason::ErrorEnvelope;

use super::{UserRecord, failure_from, retry_after_seconds};
use crate::messages;

fn envelope(body: &str) -> ErrorEnvelope {
    ErrorEnvelope::of(body.as_bytes())
}

#[test]
fn a_plain_answer_is_not_a_refusal() {
    assert!(failure_from(200, None, &envelope(r#"{"space_max":100}"#)).is_none());
    assert!(failure_from(200, None, &envelope(r#"{"result":true}"#)).is_none());
}

/// Seedr answers a refused call with a 200 as readily as with a status, so a status-first rule
/// would read `{"result": false}` as a success and hand the caller an empty document.
#[test]
fn a_refusal_on_a_two_hundred_is_still_a_refusal() {
    let failure = failure_from(
        200,
        None,
        &envelope(r#"{"result":false,"error":"bad_request"}"#),
    )
    .expect("a refusal");
    assert_eq!(failure.code, messages::API_ERROR.0);
    assert_eq!(failure.kind, FailureKind::Permanent);
    assert_eq!(failure.reason.as_deref(), Some("bad_request"));
}

#[test]
fn a_rejected_credential_ends_the_account_rather_than_the_download() {
    for status in [401, 403] {
        let failure = failure_from(status, None, &envelope("")).expect("a refusal");
        assert_eq!(failure.code, messages::AUTH_INVALID.0);
        assert_eq!(failure.kind, FailureKind::AccountInvalid, "{status}");
    }
}

/// Seedr's own documentation makes the API a premium feature, so "your plan does not include
/// this" is something a person can act on — and it must not be filed under "some 4xx".
#[test]
fn a_plan_that_does_not_include_the_api_has_its_own_answer() {
    let by_status = failure_from(402, None, &envelope("")).expect("a refusal");
    assert_eq!(by_status.code, messages::PLAN_REQUIRED.0);
    assert_eq!(by_status.kind, FailureKind::Unsupported);

    let by_word = failure_from(
        200,
        None,
        &envelope(r#"{"result":false,"error":"premium_required"}"#),
    )
    .expect("a refusal");
    assert_eq!(by_word.code, messages::PLAN_REQUIRED.0);
    assert_eq!(by_word.kind, FailureKind::Unsupported);
}

#[test]
fn a_spent_request_budget_waits_for_as_long_as_the_header_asks() {
    let stated = failure_from(429, Some(120), &envelope("")).expect("a refusal");
    assert_eq!(stated.code, messages::RATE_LIMITED.0);
    assert_eq!(stated.kind, FailureKind::RateLimited(Some(120)));
    // No header: a minute, because a refused request counts towards the cap that refused it.
    let bare = failure_from(429, None, &envelope("")).expect("a refusal");
    assert_eq!(bare.kind, FailureKind::RateLimited(Some(60)));
}

#[test]
fn an_outage_waits_and_a_missing_file_does_not() {
    let outage = failure_from(503, None, &envelope("")).expect("a refusal");
    assert_eq!(outage.code, messages::SERVER_ERROR.0);
    assert_eq!(outage.kind, FailureKind::Transient(Some(300)));

    let gone = failure_from(404, None, &envelope("")).expect("a refusal");
    assert_eq!(gone.code, messages::FILE_NOT_FOUND.0);
    assert_eq!(gone.kind, FailureKind::Permanent);
}

/// A status nothing explains still has to arrive as something a person can read, and the number
/// itself is the only part of it that is safe to repeat.
#[test]
fn a_status_no_document_explains_carries_the_number_and_nothing_else() {
    let failure = failure_from(418, None, &envelope("<html>418</html>")).expect("a refusal");
    assert_eq!(failure.code, messages::HTTP_ERROR.0);
    assert_eq!(failure.message, "Seedr HTTP status 418");
    assert_eq!(failure.reason, None);
}

/// A wrong wait is worse than the bucket's own default, which is at least one somebody can
/// reason about — so a date-shaped header, a negative one and an absurd one are all ignored.
#[test]
fn only_a_retry_after_stated_in_plausible_seconds_is_believed() {
    assert_eq!(retry_after_seconds(Some("120")), Some(120));
    assert_eq!(retry_after_seconds(Some(" 30 ")), Some(30));
    for bad in ["Wed, 21 Oct 2026 07:28:00 GMT", "-5", "0", "999999", "soon"] {
        assert_eq!(retry_after_seconds(Some(bad)), None, "{bad}");
    }
    assert_eq!(retry_after_seconds(None), None);
}

#[test]
fn the_account_record_states_free_space_only_when_both_figures_make_sense() {
    let record =
        UserRecord::of(br#"{"username":"person@example.test","space_max":100,"space_used":40}"#)
            .expect("a record");
    assert_eq!(record.username.as_deref(), Some("person@example.test"));
    assert_eq!(record.space_free(), Some(60));

    // Two fields that do not belong to each other are not "minus three gigabytes".
    let crossed = UserRecord::of(br#"{"space_max":10,"space_used":40}"#).expect("a record");
    assert_eq!(crossed.space_free(), None);
    let silent = UserRecord::of(br#"{"username":"person@example.test"}"#).expect("a record");
    assert_eq!(silent.space_free(), None);
}

/// Serde builds a struct from a sequence in field order, so an array would otherwise become a
/// record whose user name was its first element.
#[test]
fn an_answer_that_is_not_an_object_is_not_an_account_record() {
    assert!(UserRecord::of(br#"["a","b"]"#).is_none());
    assert!(UserRecord::of(b"<html>502</html>").is_none());
}
