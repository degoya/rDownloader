//! What Put.io's answers mean, pinned down on the host target.

use putio_common::reason::ErrorEnvelope;

use super::{AccountResponse, FailureKind, FileResponse, failure_from, rate_limit_wait};
use crate::messages;

fn envelope(body: &str) -> ErrorEnvelope {
    ErrorEnvelope::of(body.as_bytes())
}

#[test]
fn a_file_is_read_with_its_name_size_and_checksum() {
    let response: FileResponse = serde_json::from_str(
        r#"{"file":{"name":"ep01.mkv","size":10,"content_type":"video/x-matroska",
            "file_type":"VIDEO","parent_id":7,"crc32":"1a2b3c4d"},"status":200}"#,
    )
    .expect("a file document");
    let file = response.file.expect("the file");
    assert_eq!(file.name.as_deref(), Some("ep01.mkv"));
    assert_eq!(file.size, Some(10));
    assert!(!file.is_folder());
    assert_eq!(
        file.checksum(),
        Some(("crc32".to_owned(), "1a2b3c4d".to_owned()))
    );
}

/// Put.io marks a folder in two fields and does not always fill both. Reading only one of them
/// would let a folder through as a file, and a folder has no bytes to download.
#[test]
fn a_folder_is_recognised_from_either_field() {
    for body in [
        r#"{"file":{"id":7,"name":"Example.Release","file_type":"FOLDER"}}"#,
        r#"{"file":{"id":7,"name":"Example.Release","content_type":"application/x-directory"}}"#,
    ] {
        let response: FileResponse = serde_json::from_str(body).expect("a file document");
        assert!(response.file.expect("the file").is_folder(), "{body}");
    }
}

/// A CRC32 field Put.io has not filled in is not a checksum. Checking against it would fail
/// every download of a file the provider simply has not hashed.
#[test]
fn a_checksum_that_is_not_one_is_not_reported() {
    for crc in ["", "00000000", "zzzzzzzz", "1a2b"] {
        let response: FileResponse =
            serde_json::from_str(&format!(r#"{{"file":{{"id":1,"crc32":"{crc}"}}}}"#))
                .expect("a file document");
        assert_eq!(response.file.expect("the file").checksum(), None, "{crc}");
    }
}

#[test]
fn the_account_is_read_with_its_name_and_free_space() {
    let response: AccountResponse = serde_json::from_str(
        r#"{"info":{"username":"someone","mail":"someone@example.invalid",
            "account_active":true,"disk":{"avail":1024,"size":2048,"used":1024}}}"#,
    )
    .expect("an account document");
    let info = response.info.expect("the info");
    assert_eq!(info.username.as_deref(), Some("someone"));
    assert_eq!(info.account_active, Some(true));
    assert_eq!(info.disk.and_then(|disk| disk.avail), Some(1024));
}

#[test]
fn a_plain_success_is_not_a_refusal() {
    assert!(failure_from(200, None, &envelope(r#"{"file":{"id":1}}"#)).is_none());
    assert!(failure_from(204, None, &envelope("")).is_none());
}

/// The four refusals a person acts on differently, and the one that is only a wait.
#[test]
fn the_refusals_are_told_apart_by_what_a_person_has_to_do() {
    let expired =
        failure_from(401, None, &envelope(r#"{"error_type":"INVALID_TOKEN"}"#)).expect("a refusal");
    assert_eq!(expired.kind, FailureKind::AccountInvalid);
    assert_eq!(expired.code, messages::AUTH_INVALID.0);
    assert_eq!(expired.reason.as_deref(), Some("INVALID_TOKEN"));

    // A 403 is two different things at Put.io, and only the word tells them apart.
    let refused =
        failure_from(403, None, &envelope(r#"{"error_type":"ACCESS_DENIED"}"#)).expect("a refusal");
    assert_eq!(refused.kind, FailureKind::Unsupported);
    assert_eq!(refused.code, messages::NOT_PERMITTED.0);
    let stale =
        failure_from(403, None, &envelope(r#"{"error_type":"INVALID_GRANT"}"#)).expect("a refusal");
    assert_eq!(stale.kind, FailureKind::AccountInvalid);

    let gone =
        failure_from(404, None, &envelope(r#"{"error_type":"NOT_FOUND"}"#)).expect("a refusal");
    assert_eq!(gone.kind, FailureKind::Permanent);
    assert_eq!(gone.code, messages::FILE_NOT_FOUND.0);

    let busy = failure_from(503, None, &envelope("")).expect("a refusal");
    assert_eq!(busy.kind, FailureKind::Transient(Some(300)));
    assert_eq!(busy.code, messages::SERVER_ERROR.0);
}

/// A rate limit is a wait, and the wait is Put.io's own figure when the two clocks agree.
#[test]
fn a_rate_limit_carries_the_window_put_io_stated() {
    let waiting = failure_from(
        429,
        Some(120),
        &envelope(r#"{"error_type":"TOO_MANY_REQUESTS"}"#),
    )
    .expect("a refusal");
    assert_eq!(waiting.kind, FailureKind::RateLimited(Some(120)));
    assert_eq!(waiting.code, messages::RATE_LIMITED.0);
    // Without a usable figure the plugin waits its own minute rather than guessing.
    let default = failure_from(429, None, &envelope("")).expect("a refusal");
    assert_eq!(default.kind, FailureKind::RateLimited(Some(60)));
}

/// `X-RateLimit-Reset` is an absolute timestamp, so it needs the clock; three shapes are
/// deliberately no answer at all rather than a guess.
#[test]
fn only_a_reset_the_clocks_agree_on_becomes_a_wait() {
    assert_eq!(rate_limit_wait(Some("1 000 060"), 1_000_000), None);
    assert_eq!(rate_limit_wait(Some("1000060"), 1_000_000), Some(60));
    // Already past, not a number, and so far away the two clocks plainly disagree.
    assert_eq!(rate_limit_wait(Some("999999"), 1_000_000), None);
    assert_eq!(rate_limit_wait(Some("soon"), 1_000_000), None);
    assert_eq!(rate_limit_wait(Some("1086400"), 1_000_000), None);
    assert_eq!(rate_limit_wait(None, 1_000_000), None);
}

/// Put.io answers some refusals with a 2xx and an error document. Believing the status would
/// hand the caller an empty record and call it success.
#[test]
fn an_error_document_decides_whatever_the_status_says() {
    let refusal = failure_from(
        200,
        None,
        &envelope(r#"{"error_type":"DATABASE_ERROR","error_message":"try again"}"#),
    )
    .expect("a refusal");
    assert_eq!(refusal.code, messages::API_ERROR.0);
    assert_eq!(refusal.reason.as_deref(), Some("DATABASE_ERROR"));
    // And Put.io's sentence is nowhere in what travels.
    assert!(!refusal.message.contains("try again"));
}

/// A status nothing else explains keeps its number and nothing else.
#[test]
fn an_unremarkable_status_travels_as_its_number() {
    let refusal = failure_from(418, None, &envelope("")).expect("a refusal");
    assert_eq!(refusal.code, messages::HTTP_ERROR.0);
    assert_eq!(refusal.message, "Put.io HTTP status 418");
    assert_eq!(refusal.reason, None);
}
