//! What `api.rs` decides, checked without a host.

use super::*;

fn envelope(body: &str) -> ErrorEnvelope {
    serde_json::from_str(body).expect("the failure envelope parses")
}

#[test]
fn matches_accepts_only_http_and_https() {
    assert!(matches("http"));
    assert!(matches("https"));
    assert!(!matches("ftp"));
    // The one that matters: a magnet is Real-Debrid's business but not this plugin's, because
    // a torrent has no address until it has been uploaded, waited for and chosen from.
    assert!(!matches("magnet"));
}

#[test]
fn link_body_encodes_the_form_field() {
    assert_eq!(
        link_body("https://example.test/f/abc"),
        b"link=https%3A%2F%2Fexample.test%2Ff%2Fabc"
    );
}

#[test]
fn parse_download_url_accepts_a_well_formed_url() {
    let parsed =
        parse_download_url("https://34.download.real-debrid.com/d/TOKEN/release.rar").expect("URL");
    assert_eq!(
        parsed.as_str(),
        "https://34.download.real-debrid.com/d/TOKEN/release.rar"
    );
}

#[test]
fn parse_download_url_rejects_a_malformed_url() {
    let failure = parse_download_url("not a url").expect_err("malformed URL");
    assert!(matches!(failure.kind, ErrorKind::Permanent));
    assert_eq!(failure.code, messages::INVALID_URL);
    assert!(failure.params.iter().any(|(name, _)| *name == "error"));
}

#[test]
fn a_successful_answer_describes_no_failure() {
    assert!(failure_from(200, None, &envelope(r#"{"download":"https://x/y"}"#)).is_none());
}

/// A 2xx carrying an `error_code` is still a refusal. Real-Debrid answers some of them with
/// 200, and reading only the status would queue a download that does not exist.
#[test]
fn an_error_code_outranks_a_successful_status() {
    let failure = failure_from(
        200,
        None,
        &envelope(r#"{"error":"bad token","error_code":8}"#),
    )
    .expect("a refusal");
    assert!(matches!(failure.kind, ErrorKind::AccountInvalid));
    assert_eq!(failure.code, messages::AUTH_INVALID.0);
    assert!(
        failure
            .params
            .iter()
            .any(|(name, value)| *name == "api_code" && value == "8")
    );
}

/// Nothing the provider wrote survives into the failure, only the number beside it.
#[test]
fn the_providers_sentence_never_reaches_the_failure() {
    let leaky = r#"{"error":"token AT-7f3c9 rejected","error_code":8}"#;
    let failure = failure_from(401, None, &envelope(leaky)).expect("a refusal");
    assert!(!failure.message.contains("AT-7f3c9"));
    assert_eq!(failure.message, messages::AUTH_INVALID.1);
    assert!(
        failure
            .params
            .iter()
            .all(|(_, value)| !value.contains("AT-7f3c9"))
    );
}

#[test]
fn the_sign_in_group_invalidates_the_account() {
    for api_code in [8, 9, 12, 13, 14, 15] {
        let failure = classify_error(api_code, None);
        assert!(
            matches!(failure.kind, ErrorKind::AccountInvalid),
            "{api_code}"
        );
        assert_eq!(failure.code, messages::AUTH_INVALID.0, "{api_code}");
    }
}

#[test]
fn a_second_factor_asks_rather_than_invalidates() {
    for api_code in [10, 11] {
        let failure = classify_error(api_code, None);
        assert!(
            matches!(failure.kind, ErrorKind::AuthRequired),
            "{api_code}"
        );
        assert_eq!(failure.code, messages::TWO_FACTOR.0, "{api_code}");
    }
}

#[test]
fn a_missing_or_infringing_file_is_offline() {
    for api_code in [7, 24, 35] {
        let failure = classify_error(api_code, None);
        assert!(matches!(failure.kind, ErrorKind::Offline), "{api_code}");
        assert_eq!(failure.code, messages::FILE_OFFLINE.0, "{api_code}");
    }
}

/// A hoster the plan does not cover is a capability gap, not a wait: retrying it on a timer
/// would ask the same question for ever and get the same answer.
#[test]
fn an_uncovered_hoster_is_unsupported_rather_than_retried() {
    for api_code in [16, 20] {
        let failure = classify_error(api_code, None);
        assert!(matches!(failure.kind, ErrorKind::Unsupported), "{api_code}");
        assert_eq!(failure.code, messages::HOST_UNSUPPORTED.0, "{api_code}");
    }
}

#[test]
fn a_busy_provider_or_hoster_is_retried_after_five_minutes() {
    for api_code in [6, 17, 19, 21, 25] {
        let failure = classify_error(api_code, None);
        assert!(
            matches!(failure.kind, ErrorKind::Transient(Some(300))),
            "{api_code}"
        );
        assert_eq!(failure.code, messages::SERVER_BUSY.0, "{api_code}");
    }
}

#[test]
fn an_exhausted_quota_waits_an_hour() {
    for api_code in [18, 23, 36] {
        let failure = classify_error(api_code, None);
        assert!(
            matches!(failure.kind, ErrorKind::RateLimited(Some(3600))),
            "{api_code}"
        );
        assert_eq!(failure.code, messages::LIMIT_REACHED.0, "{api_code}");
    }
}

/// The credential is good and the address is not, so only this address is blocked. Marking
/// the account invalid here would stop every other download running under it.
#[test]
fn a_refused_address_blocks_the_address_and_not_the_account() {
    let failure = classify_error(22, None);
    assert!(matches!(failure.kind, ErrorKind::IpBlocked(None)));
    assert_eq!(failure.code, messages::IP_NOT_ALLOWED.0);
}

/// Both rate-limit codes wait, and a `Retry-After` the provider stated wins over the default —
/// which matters here more than elsewhere, because a refused request counts towards the very
/// cap that refused it.
#[test]
fn a_rate_limit_prefers_the_stated_wait() {
    for api_code in [5, 34] {
        let failure = classify_error(api_code, Some(90));
        assert!(
            matches!(failure.kind, ErrorKind::RateLimited(Some(90))),
            "{api_code}"
        );
        assert_eq!(failure.code, messages::RATE_LIMITED.0, "{api_code}");
    }
    assert!(matches!(
        classify_error(34, None).kind,
        ErrorKind::RateLimited(Some(60))
    ));
}

#[test]
fn an_undocumented_error_code_lands_in_the_generic_bucket() {
    let failure = classify_error(4242, None);
    assert!(matches!(failure.kind, ErrorKind::Permanent));
    assert_eq!(failure.code, messages::API_ERROR);
    assert!(failure.message.contains("4242"));
    assert!(
        failure
            .params
            .iter()
            .any(|(name, value)| *name == "api_code" && value == "4242")
    );
}

/// A refusal with no number at all still refuses; it is classified by its status instead.
#[test]
fn a_numberless_refusal_is_classified_by_status() {
    let failure =
        failure_from(503, None, &envelope(r#"{"error":"service unavailable"}"#)).expect("refusal");
    assert!(matches!(failure.kind, ErrorKind::Transient(None)));
    assert_eq!(failure.code, messages::SERVER_ERROR.0);
    let unauthorized = failure_from(401, None, &envelope("{}")).expect("refusal");
    assert!(matches!(unauthorized.kind, ErrorKind::AccountInvalid));
}

#[test]
fn ensure_http_status_maps_the_statuses_the_envelope_does_not_explain() {
    assert!(ensure_http_status(204, None).is_ok());
    assert!(matches!(
        ensure_http_status(429, Some(30)).expect_err("429").kind,
        ErrorKind::RateLimited(Some(30))
    ));
    assert!(matches!(
        ensure_http_status(410, None).expect_err("410").kind,
        ErrorKind::Offline
    ));
    let odd = ensure_http_status(418, None).expect_err("418");
    assert_eq!(odd.code, messages::HTTP_ERROR);
    assert!(odd.params.iter().any(|(name, _)| *name == "status"));
}

#[test]
fn retry_after_reads_seconds_and_ignores_a_date() {
    assert_eq!(retry_after_seconds(Some(" 42 ")), Some(42));
    assert_eq!(
        retry_after_seconds(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
        None
    );
    assert_eq!(retry_after_seconds(None), None);
}

#[test]
fn merge_hosters_lowercases_dedupes_and_sorts() {
    let domains: Vec<String> = serde_json::from_str(
        r#"["rapidgator.net", "1FICHIER.COM", "1fichier.com", "  ", "uptobox.com"]"#,
    )
    .expect("parse");
    assert_eq!(
        merge_hosters(domains),
        vec!["1fichier.com", "rapidgator.net", "uptobox.com"]
    );
}

/// An expired premium plan is not a premium plan. Real-Debrid keeps `type: "premium"` on the
/// account after the time runs out, and believing it would tell the person their plan is fine
/// while every unrestriction fails.
#[test]
fn premium_needs_both_the_type_and_time_left() {
    assert!(is_premium(Some("premium"), Some(86_400)));
    assert!(is_premium(Some("Premium"), None));
    assert!(!is_premium(Some("premium"), Some(0)));
    assert!(!is_premium(Some("free"), Some(86_400)));
    assert!(!is_premium(None, Some(86_400)));
}

#[test]
fn account_name_prefers_the_username_then_the_email_then_nothing() {
    assert_eq!(account_name(Some("alice"), Some("a@test")), Some("alice"));
    assert_eq!(account_name(Some("  "), Some("a@test")), Some("a@test"));
    assert_eq!(account_name(None, None), None);
}

#[test]
fn the_answers_the_plugin_reads_parse_as_documented() {
    let unrestricted: UnrestrictedLink = serde_json::from_str(
        r#"{"id":"XYZ","filename":"release.rar","mimeType":"application/x-rar",
            "filesize":4096,"link":"https://example.test/f/abc","host":"example.test",
            "chunks":16,"crc":1,"download":"https://34.download.real-debrid.com/d/T/release.rar",
            "streamable":0}"#,
    )
    .expect("parse");
    assert_eq!(
        unrestricted.download.as_deref(),
        Some("https://34.download.real-debrid.com/d/T/release.rar")
    );
    assert_eq!(unrestricted.filename.as_deref(), Some("release.rar"));
    assert_eq!(unrestricted.filesize, Some(4096));

    let checked: CheckedLink = serde_json::from_str(
        r#"{"host":"example.test","link":"https://example.test/f/abc",
            "filename":"release.rar","filesize":4096,"supported":1}"#,
    )
    .expect("parse");
    assert_eq!(checked.filename.as_deref(), Some("release.rar"));

    let user: UserInfo = serde_json::from_str(
        r#"{"id":1,"username":"alice","email":"a@test","points":300,"locale":"en",
            "avatar":"https://fcdn.real-debrid.com/x.png","type":"premium",
            "premium":1209600,"expiration":"2026-10-01T00:00:00.000Z"}"#,
    )
    .expect("parse");
    assert_eq!(user.username.as_deref(), Some("alice"));
    assert!(is_premium(user.account_type.as_deref(), user.premium));
}
