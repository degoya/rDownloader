use super::{
    AccountInfo, AccountState, ErrorEnvelope, ErrorKind, SiteEntry, account_name, account_state,
    classify_error, classify_not_available, ensure_http_status, error_envelope, failure_from,
    form_body, merge_hosters, retry_after_seconds, sanitize_error,
};
use crate::messages;

fn envelope(error: Option<&str>, not_available: Option<&str>) -> ErrorEnvelope {
    ErrorEnvelope {
        error: error.map(str::to_owned),
        not_available: not_available.map(str::to_owned),
    }
}

#[test]
fn a_form_body_percent_encodes_everything_an_address_is_made_of() {
    let body = form_body(&[("url", "https://example.invalid/a b?x=1&y=2")]);
    assert_eq!(
        String::from_utf8_lossy(&body),
        "url=https%3A%2F%2Fexample.invalid%2Fa%20b%3Fx%3D1%26y%3D2"
    );
}

#[test]
fn a_magnet_survives_the_form_body_whole() {
    // The one value that has to come out the other side unchanged. `&` and `=` separate the
    // magnet's own parameters, and a body that did not encode them would submit a magnet cut
    // off after its first field.
    let magnet = "magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709&dn=A+B";
    let body = form_body(&[("url", magnet)]);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.starts_with("url=magnet%3A%3Fxt%3Durn%3Abtih%3A"),
        "{text}"
    );
    assert!(!text[4..].contains('&'), "{text}");
}

#[test]
fn only_a_code_shaped_word_survives_sanitising() {
    assert_eq!(sanitize_error("NOAUTH").as_deref(), Some("noauth"));
    assert_eq!(
        sanitize_error("not_available").as_deref(),
        Some("not_available")
    );
    // Prose, and prose that quotes what was sent to it. Dropped whole rather than filtered:
    // filtering an answer that echoed a credential would keep its digits.
    assert_eq!(sanitize_error("Please enter a valid email address."), None);
    assert_eq!(sanitize_error("key VHK7OoGO57kH1JOO is unknown"), None);
    assert_eq!(sanitize_error("   "), None);
}

#[test]
fn the_one_stable_word_is_an_invalid_credential_and_the_rest_are_not() {
    let failure = classify_error("NOAUTH");
    assert_eq!(failure.kind, ErrorKind::AccountInvalid);
    assert_eq!(failure.code, messages::AUTH_INVALID.0);
    assert!(failure.params.is_empty());

    let failure = classify_error("SOMETHING_ELSE");
    assert_eq!(failure.kind, ErrorKind::Permanent);
    assert_eq!(failure.code, messages::API_ERROR.0);
    assert_eq!(
        failure.params,
        vec![("api_code", "something_else".to_owned())]
    );

    // An unreadable sentence says nothing about this link, so it waits rather than ending it.
    let failure = classify_error("The request could not be processed at this time.");
    assert_eq!(failure.kind, ErrorKind::Transient(Some(300)));
    assert!(failure.params.is_empty());
}

#[test]
fn a_missing_addon_is_unsupported_and_names_which_one() {
    for reason in ["premium", "links", "proxy", "cloud", "video"] {
        let failure = classify_not_available(reason);
        assert_eq!(failure.kind, ErrorKind::Unsupported, "{reason}");
        assert_eq!(failure.code, messages::ADDON_REQUIRED.0);
        assert_eq!(failure.params, vec![("addon", reason.to_owned())]);
    }
}

#[test]
fn a_refusal_carried_by_a_200_is_still_a_refusal() {
    // Offcloud answers a refusal with a document and a 200, so the document decides first.
    let failure = failure_from(200, None, &envelope(Some("NOAUTH"), None)).expect("a refusal");
    assert_eq!(failure.kind, ErrorKind::AccountInvalid);
    let failure = failure_from(200, None, &envelope(None, Some("cloud"))).expect("a refusal");
    assert_eq!(failure.code, messages::ADDON_REQUIRED.0);
    // `not_available` is read before `error`: an answer carrying both is about the add-on,
    // which is the part a person can act on.
    let failure =
        failure_from(200, None, &envelope(Some("NOAUTH"), Some("premium"))).expect("a refusal");
    assert_eq!(failure.code, messages::ADDON_REQUIRED.0);
    assert!(failure_from(200, None, &envelope(None, None)).is_none());
}

#[test]
fn a_status_outranks_prose_and_a_stable_word_outranks_the_status() {
    // The ordering the fixtures caught. Before it, a 429 carrying a sentence was a five-minute
    // wait rather than the two minutes the header asked for, because the sentence was read
    // first and a sentence carries no figure.
    let failure = failure_from(
        429,
        Some(120),
        &envelope(Some("Too many requests, please slow down."), None),
    )
    .expect("a refusal");
    assert_eq!(failure.kind, ErrorKind::RateLimited(Some(120)));
    assert_eq!(failure.code, messages::RATE_LIMITED.0);

    // The other direction: a stable word says more than the status does, and Offcloud sends it
    // with a 200 as readily as with a 401.
    let failure = failure_from(500, None, &envelope(Some("NOAUTH"), None)).expect("a refusal");
    assert_eq!(failure.kind, ErrorKind::AccountInvalid);

    // Prose on a 2xx is still a refusal: something said no and nothing says what.
    let failure =
        failure_from(200, None, &envelope(Some("Not right now, sorry."), None)).expect("a refusal");
    assert_eq!(failure.kind, ErrorKind::Transient(Some(300)));
}

#[test]
fn a_status_decides_when_no_document_explains_itself() {
    assert!(ensure_http_status(204, None).is_ok());
    assert_eq!(
        ensure_http_status(401, None).expect_err("refused").kind,
        ErrorKind::AccountInvalid
    );
    assert_eq!(
        ensure_http_status(404, None).expect_err("refused").kind,
        ErrorKind::Offline
    );
    assert_eq!(
        ensure_http_status(429, Some(120))
            .expect_err("refused")
            .kind,
        ErrorKind::RateLimited(Some(120))
    );
    // Without a header the bucket's own figure is used rather than an immediate retry.
    assert_eq!(
        ensure_http_status(429, None).expect_err("refused").kind,
        ErrorKind::RateLimited(Some(3600))
    );
    assert_eq!(
        ensure_http_status(503, None).expect_err("refused").kind,
        ErrorKind::Transient(Some(300))
    );
    let failure = ensure_http_status(418, None).expect_err("refused");
    assert_eq!(failure.code, messages::HTTP_ERROR.0);
    assert_eq!(failure.params, vec![("status", "418".to_owned())]);
}

#[test]
fn a_retry_after_is_read_in_seconds_and_a_date_is_ignored() {
    assert_eq!(retry_after_seconds(Some(" 120 ")), Some(120));
    assert_eq!(
        retry_after_seconds(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
        None
    );
    assert_eq!(retry_after_seconds(None), None);
}

#[test]
fn the_catalogue_is_flattened_lower_cased_and_deduplicated() {
    let entries = vec![
        SiteEntry {
            name: Some("Rapidgator".to_owned()),
            hosts: vec!["rapidgator.net".to_owned(), "RAPIDGATOR.NET".to_owned()],
            ..SiteEntry::default()
        },
        SiteEntry {
            domains: vec!["www.1fichier.com".to_owned()],
            ..SiteEntry::default()
        },
        SiteEntry {
            domain: Some("mediafire.com".to_owned()),
            ..SiteEntry::default()
        },
        // A row that names a site and no host contributes nothing: the queue matches on hosts.
        SiteEntry {
            name: Some("Some Video Site".to_owned()),
            ..SiteEntry::default()
        },
    ];
    assert_eq!(
        merge_hosters(entries),
        vec!["1fichier.com", "mediafire.com", "rapidgator.net"]
    );
}

#[test]
fn an_entry_that_only_names_itself_by_domain_still_counts() {
    // The one shape where `name` is the host. Accepted because it is a host, and rejected in
    // the test above because "Some Video Site" is not one.
    let entries = vec![SiteEntry {
        name: Some("uploaded.to".to_owned()),
        ..SiteEntry::default()
    }];
    assert_eq!(merge_hosters(entries), vec!["uploaded.to"]);
}

#[test]
fn the_two_ways_of_saying_no_are_kept_apart() {
    let free = AccountInfo {
        is_premium: Some(false),
        can_download: Some(true),
        ..AccountInfo::default()
    };
    assert_eq!(account_state(&free), AccountState::Free);

    let blocked = AccountInfo {
        is_premium: Some(true),
        can_download: Some(false),
        ..AccountInfo::default()
    };
    assert_eq!(account_state(&blocked), AccountState::Blocked);

    let usable = AccountInfo {
        is_premium: Some(true),
        can_download: Some(true),
        ..AccountInfo::default()
    };
    assert_eq!(account_state(&usable), AccountState::Usable);

    // An answer that says nothing about `canDownload` is not saying no.
    let quiet = AccountInfo {
        is_premium: Some(true),
        ..AccountInfo::default()
    };
    assert_eq!(account_state(&quiet), AccountState::Usable);
}

#[test]
fn both_spellings_of_every_account_field_are_read() {
    // The failure this guards against does not look like one: a camel-cased answer read
    // through snake-cased fields parses cleanly and reports every account as free.
    for body in [
        r#"{"userId":"REDACTEDUSER01","isPremium":true,"canDownload":true,"expirationDate":"2027-01-31"}"#,
        r#"{"user_id":"REDACTEDUSER01","is_premium":true,"can_download":true,"expiration_date":"2027-01-31"}"#,
    ] {
        let info: AccountInfo = serde_json::from_str(body).expect("account info");
        assert_eq!(account_state(&info), AccountState::Usable, "{body}");
        assert_eq!(info.expiration_date.as_deref(), Some("2027-01-31"));
        assert_eq!(account_name(&info), Some("REDACTEDUSER01"));
    }
}

#[test]
fn an_address_is_preferred_over_the_opaque_identifier_and_neither_is_invented() {
    let info = AccountInfo {
        user_id: Some("REDACTEDUSER01".to_owned()),
        email: Some("person@example.invalid".to_owned()),
        ..AccountInfo::default()
    };
    assert_eq!(account_name(&info), Some("person@example.invalid"));
    assert_eq!(account_name(&AccountInfo::default()), None);
    let blank = AccountInfo {
        email: Some("   ".to_owned()),
        ..AccountInfo::default()
    };
    assert_eq!(account_name(&blank), None);
}

#[test]
fn an_array_answer_is_never_read_as_a_refusal() {
    // The trap this guard exists for: serde builds a struct from a sequence too, taking the
    // elements in field order, so a bare list of addresses read straight into `ErrorEnvelope`
    // becomes `{error: <first>, not_available: <second>}` -- and both `cloud/explore` and
    // `cloud/history` answer with arrays. Before the guard, a finished job whose file tree came
    // back in the bare-address shape was classified as a missing add-on.
    let array = br#"["https://s1.offcloud.com/a.mkv","https://s1.offcloud.com/b.mkv"]"#;
    let envelope = error_envelope(array);
    assert!(envelope.error.is_none(), "{:?}", envelope.error);
    assert!(envelope.not_available.is_none());
    assert!(failure_from(200, None, &envelope).is_none());

    // An object still is one.
    let envelope = error_envelope(br#"{"error":"NOAUTH"}"#);
    assert_eq!(envelope.error.as_deref(), Some("NOAUTH"));
    // And something that is not JSON at all says nothing rather than something wrong.
    assert!(error_envelope(b"<html>").error.is_none());
}
