//! The pure half: what is claimed, what a refusal means, and what a quota answer says.
//!
//! Every fixture body here is sanitised -- the identifiers are invented, the digests are
//! repeated hex, and no address, key or account of a real person appears. Nothing opens a
//! socket.

use super::{
    ErrorKind, FileInfo, RateLimits, availability_failure, availability_is_offline, checksum,
    classify_value, download_url, ensure_http_status, error_envelope, failure_from, file_id,
    file_name, info_url, is_download_target, quota_failure, retry_after_seconds, sanitize_value,
};
use crate::messages;

fn info(json: &str) -> FileInfo {
    serde_json::from_str(json).expect("file info")
}

#[test]
fn the_three_claimed_address_shapes_all_name_one_file() {
    for url in [
        "https://pixeldrain.com/u/Ab3xY9Zq",
        "https://www.pixeldrain.com/u/Ab3xY9Zq",
        "https://pixeldrain.com/api/file/Ab3xY9Zq",
        "https://pixeldrain.com/api/file/Ab3xY9Zq/info",
    ] {
        assert_eq!(file_id(url).as_deref(), Some("Ab3xY9Zq"), "{url}");
    }
    // Nothing is claimed on hearsay: a shape nobody measured would turn a plain HTTP download
    // into a hoster error the first time the guess was wrong.
    assert_eq!(file_id("https://pixeldrain.com/d/Ab3xY9Zq"), None);
}

#[test]
fn a_list_address_is_left_to_the_crawler_and_a_foreign_host_is_not_claimed() {
    // `/l/` names several files and a resolver answers with exactly one, so it belongs to
    // `plugins/pixeldrain-crawler/`. Claiming it here would refuse every album.
    assert_eq!(file_id("https://pixeldrain.com/l/Ab3xY9Zq"), None);
    assert_eq!(file_id("https://pixeldrain.com/api/list/Ab3xY9Zq"), None);
    // A host that merely ends in the name is a different site.
    assert_eq!(file_id("https://notpixeldrain.com/u/Ab3xY9Zq"), None);
    assert_eq!(file_id("https://pixeldrain.com.evil.test/u/Ab3"), None);
    assert_eq!(file_id("https://pixeldrain.com/"), None);
    // An identifier that is not one: punctuation, traversal, or far too long.
    assert_eq!(file_id("https://pixeldrain.com/u/../../etc/passwd"), None);
    assert_eq!(file_id("https://pixeldrain.com/u/ab-cd"), None);
    assert_eq!(
        file_id(&format!("https://pixeldrain.com/u/{}", "a".repeat(33))),
        None
    );
    assert_eq!(file_id("not an address at all"), None);
}

#[test]
fn the_download_address_carries_no_signature_and_no_deadline() {
    // The whole reason a Pixeldrain job may wait an hour in the queue and still work: the
    // address is the identifier. If this ever grows a token, the expiry problem arrives with
    // it and the plugin has to re-resolve per attempt instead.
    let url = download_url("Ab3xY9Zq");
    assert_eq!(url, "https://pixeldrain.com/api/file/Ab3xY9Zq?download");
    assert!(is_download_target(&url));
    assert_eq!(
        info_url("Ab3xY9Zq"),
        "https://pixeldrain.com/api/file/Ab3xY9Zq/info"
    );
}

#[test]
fn only_the_providers_own_hosts_are_download_targets() {
    assert!(is_download_target("https://pixeldrain.com/api/file/a"));
    assert!(is_download_target("https://cdn.pixeldrain.com/api/file/a"));
    assert!(!is_download_target("https://pixeldrain.com.evil.test/a"));
    assert!(!is_download_target("http://pixeldrain.com/api/file/a"));
    assert!(!is_download_target("file:///etc/passwd"));
}

#[test]
fn a_missing_file_is_the_providers_stable_value_and_not_its_sentence() {
    // The measured shape: `/api/file/aaaaaaaa/info` answers 404 with this document. `value` is
    // the code, `message` is prose and never travels.
    let body = br#"{"success":false,"value":"not_found","message":"The requested file does not exist, it may have been deleted."}"#;
    let envelope = error_envelope(body);
    let failure = failure_from(404, None, &envelope).expect("refused");
    assert_eq!(failure.kind, ErrorKind::Offline);
    assert_eq!(failure.code, messages::FILE_NOT_FOUND.0);
    assert!(
        !failure.message.contains("may have been deleted"),
        "the provider's sentence must not be forwarded"
    );
}

#[test]
fn an_authentication_refusal_is_told_apart_from_a_missing_file() {
    let body = br#"{"success":false,"value":"authentication_required","message":"x"}"#;
    let failure = failure_from(401, None, &error_envelope(body)).expect("refused");
    assert_eq!(failure.kind, ErrorKind::AuthRequired);
    assert_eq!(failure.code, messages::ACCOUNT_REQUIRED.0);
}

#[test]
fn the_three_rate_limits_keep_three_separate_codes() {
    // They ask different things of the person, so collapsing them into one "try later" would
    // lose the only part of the answer that is actionable.
    let ip = classify_value("ip_rate_limit_reached");
    assert_eq!(ip.code, messages::IP_RATE_LIMITED.0);
    assert_eq!(ip.kind, ErrorKind::IpBlocked(Some(3600)));

    let transfer = classify_value("transfer_limit_exceeded");
    assert_eq!(transfer.code, messages::TRANSFER_LIMIT.0);
    assert_eq!(transfer.kind, ErrorKind::RateLimited(Some(3600)));

    let concurrent = classify_value("max_concurrent_downloads");
    assert_eq!(concurrent.code, messages::TOO_MANY_DOWNLOADS.0);
    assert_eq!(concurrent.kind, ErrorKind::RateLimited(Some(300)));
}

#[test]
fn a_429_without_a_document_uses_the_retry_after_the_service_asked_for() {
    let failure = ensure_http_status(429, retry_after_seconds(Some("120"))).expect_err("refused");
    assert_eq!(failure.kind, ErrorKind::IpBlocked(Some(120)));
    assert_eq!(failure.code, messages::IP_RATE_LIMITED.0);
    // A date-shaped header is ignored rather than guessed at, so the bucket's default stands.
    let failure = ensure_http_status(
        429,
        retry_after_seconds(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
    )
    .expect_err("refused");
    assert_eq!(failure.kind, ErrorKind::IpBlocked(Some(3600)));
}

#[test]
fn a_captcha_is_reported_rather_than_attempted() {
    // This plugin declares no `captcha` capability: the flow behind it was never measured, and
    // a guess would be an anti-bot workaround rather than an integration.
    let failure = classify_value("file_rate_limited_captcha_required");
    assert_eq!(failure.kind, ErrorKind::Unsupported);
    assert_eq!(failure.code, messages::CAPTCHA_REQUIRED.0);
}

#[test]
fn an_unknown_token_travels_as_a_parameter_and_prose_does_not() {
    let failure = classify_value("some_new_state");
    assert_eq!(failure.code, messages::API_ERROR.0);
    assert_eq!(
        failure.params,
        vec![("api_code", "some_new_state".to_owned())]
    );

    // Not code-shaped: nothing to branch on and nothing safe to show, so the whole value goes.
    assert_eq!(
        sanitize_value("Slow down, https://pixeldrain.com/u/x"),
        None
    );
    let failure = classify_value("Slow down, https://pixeldrain.com/u/x");
    assert_eq!(failure.code, messages::INVALID_RESPONSE.0);
    assert!(failure.params.is_empty());
}

#[test]
fn an_array_answer_is_never_mistaken_for_a_refusal() {
    // Serde reads a struct out of a sequence in field order, so an unguarded envelope would
    // invent `success` and `value` out of the first two elements of a perfectly good answer.
    let envelope = error_envelope(br#"["https://pixeldrain.com/u/a","b"]"#);
    assert!(envelope.value.is_none());
    assert!(failure_from(200, None, &envelope).is_none());
}

#[test]
fn availability_collapses_deliberately_and_only_a_block_counts_as_gone() {
    // RD-120-36: `link-status` has `online | offline | unknown` and nothing else. A file behind
    // a captcha or a spent allowance exists, so it stays online and the download attempt is
    // what carries the obstacle.
    let blocked = info(r#"{"name":"a.bin","availability":"virus_detected_abuse"}"#);
    assert!(availability_is_offline(&blocked));
    assert_eq!(
        availability_failure(&blocked).expect("refused").code,
        messages::FILE_BLOCKED.0
    );

    let limited = info(
        r#"{"id":"Ab3xY9Zq","name":"a.bin","availability":"file_rate_limited_captcha_required"}"#,
    );
    assert!(!availability_is_offline(&limited));
    assert!(availability_failure(&limited).is_some());

    let fine = info(r#"{"name":"a.bin","availability":""}"#);
    assert!(availability_failure(&fine).is_none());
    assert!(!availability_is_offline(&fine));
}

#[test]
fn a_quota_answer_is_read_before_a_download_starts() {
    let spent = RateLimits {
        download_limit: 10_000,
        download_limit_used: 10_000,
        ..RateLimits::default()
    };
    let failure = quota_failure(&spent).expect("refused");
    assert_eq!(failure.code, messages::IP_RATE_LIMITED.0);

    let transfer = RateLimits {
        transfer_limit: 500,
        transfer_limit_used: 900,
        ..RateLimits::default()
    };
    assert_eq!(
        quota_failure(&transfer).expect("refused").code,
        messages::TRANSFER_LIMIT.0
    );

    let overloaded = RateLimits {
        server_overload: true,
        ..RateLimits::default()
    };
    assert_eq!(
        quota_failure(&overloaded).expect("refused").code,
        messages::SERVER_OVERLOADED.0
    );

    // A zero limit states no limit and must not be read as one that is already reached, or
    // every download would be refused on a service that stated nothing.
    assert!(quota_failure(&RateLimits::default()).is_none());
    let room = RateLimits {
        download_limit: 10_000,
        download_limit_used: 1,
        ..RateLimits::default()
    };
    assert!(quota_failure(&room).is_none());
}

#[test]
fn a_name_is_never_invented_and_a_digest_is_held_to_its_shape() {
    let named = info(r#"{"name":"  release.rar  ","hash_sha256":"AB12"}"#);
    assert_eq!(file_name(&named).as_deref(), Some("release.rar"));
    // Four characters is not a SHA-256, and a checksum that cannot verify fails every download
    // of a perfectly good file.
    assert_eq!(checksum(&named), None);

    let digest = "ab".repeat(32);
    let hashed = info(&format!(r#"{{"name":"a.bin","hash_sha256":"{digest}"}}"#));
    assert_eq!(
        checksum(&hashed),
        Some(("sha256".to_owned(), digest.clone()))
    );

    let blank = info(r#"{"name":"   "}"#);
    assert_eq!(file_name(&blank), None);
    assert_eq!(file_name(&FileInfo::default()), None);
}
