use super::{
    ErrorEnvelope, ErrorKind, Stage, StatusEnvelope, classify_error, classify_not_available,
    ensure_http_status, error_envelope, explore_files, failure_from, form_body, is_safe_request_id,
    name_from_url, permille, place, remove_body, retry_after_seconds, sanitize_error, stage_of,
    status_detail, status_word,
};
use crate::messages;

#[test]
fn a_magnet_survives_the_form_body_whole() {
    // The value that has to come out the other side unchanged. `&` and `=` separate the
    // magnet's own parameters, and a body that did not encode them would submit a magnet cut
    // off after its first field.
    let magnet = "magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709&dn=A+B";
    let text = String::from_utf8(form_body(&[("url", magnet)])).expect("utf-8");
    assert!(
        text.starts_with("url=magnet%3A%3Fxt%3Durn%3Abtih%3A"),
        "{text}"
    );
    assert!(!text[4..].contains('&'), "{text}");
}

#[test]
fn every_stage_offcloud_names_maps_to_one_the_contract_has() {
    assert_eq!(stage_of("created"), Stage::Preparing(10));
    assert_eq!(stage_of("queued"), Stage::Preparing(30));
    assert_eq!(stage_of("downloading"), Stage::Working(30));
    assert_eq!(stage_of("downloaded"), Stage::Ready);
    assert_eq!(stage_of("error"), Stage::Failed(messages::JOB_FAILED));
    assert_eq!(stage_of("canceled"), Stage::Failed(messages::JOB_CANCELED));
    // Both spellings, because the provider's documentation and its clients disagree.
    assert_eq!(stage_of("cancelled"), Stage::Failed(messages::JOB_CANCELED));
    // Case and padding are the provider's business, not a new state.
    assert_eq!(stage_of("  DOWNLOADED "), Stage::Ready);
    // A word this build does not know is something happening, not a reason to throw a
    // download away.
    assert_eq!(stage_of("transcoding"), Stage::Preparing(60));
    assert_eq!(stage_of(""), Stage::Preparing(60));
}

#[test]
fn there_is_no_stage_that_waits_for_a_person() {
    // Stated as its own test because it is the one structural difference from the Real-Debrid
    // remote job: Offcloud fetches the whole of what it was given, so no word it can answer
    // with means "nothing moves until somebody chooses".
    for word in [
        "created",
        "queued",
        "downloading",
        "downloaded",
        "error",
        "canceled",
        "waiting_files_selection",
    ] {
        assert!(
            !matches!(stage_of(word), Stage::Failed(message) if message == messages::NO_SELECTION),
            "{word}"
        );
    }
}

#[test]
fn a_status_answer_is_read_in_both_the_shapes_it_is_described_in() {
    let nested: StatusEnvelope = serde_json::from_str(
        r#"{"status":{"requestId":"REDACTEDREQUEST01","status":"downloading","amount":425,"fileSize":1000}}"#,
    )
    .expect("nested status");
    assert_eq!(status_word(&nested), Some("downloading"));
    let detail = status_detail(&nested).expect("a detail");
    assert_eq!(permille(detail.amount, detail.file_size), Some(425));

    let bare: StatusEnvelope =
        serde_json::from_str(r#"{"status":"downloaded"}"#).expect("bare status");
    assert_eq!(status_word(&bare), Some("downloaded"));
    assert!(status_detail(&bare).is_none());

    let silent: StatusEnvelope = serde_json::from_str("{}").expect("empty status");
    assert_eq!(status_word(&silent), None);
}

#[test]
fn progress_is_none_unless_both_figures_are_there() {
    assert_eq!(permille(Some(500.0), Some(1000.0)), Some(500));
    assert_eq!(permille(Some(1500.0), Some(1000.0)), Some(1000));
    // A bar reading 0 % because nothing was measured is worse than no bar, and the contract
    // has a way to say "not measured".
    assert_eq!(permille(Some(500.0), None), None);
    assert_eq!(permille(None, Some(1000.0)), None);
    assert_eq!(permille(Some(1.0), Some(0.0)), None);
    assert_eq!(permille(Some(f64::NAN), Some(1000.0)), None);
    assert_eq!(permille(Some(1.0), Some(f64::INFINITY)), None);
}

#[test]
fn an_explore_answer_is_read_in_every_shape_it_arrives_in() {
    let detailed = explore_files(
        br#"{"files":[{"path":"Example.Release/ep01.mkv","size":10,"url":"https://s1.offcloud.com/cloud/REDACTED01/ep01.mkv"}]}"#,
    );
    assert_eq!(detailed.len(), 1);
    assert_eq!(
        detailed[0].path.as_deref(),
        Some("Example.Release/ep01.mkv")
    );
    assert_eq!(detailed[0].size, Some(10));

    // A bare array of addresses, which is what `format=simple` answers. This is the shape an
    // untagged enum got wrong: a struct can be deserialised from a sequence, so the array
    // became a `files` list of length zero and a finished job arrived as an empty package.
    let simple = explore_files(
        br#"["https://s1.offcloud.com/cloud/REDACTED01/ep01.mkv","https://s1.offcloud.com/cloud/REDACTED01/ep02.mkv"]"#,
    );
    assert_eq!(simple.len(), 2);
    assert!(simple.iter().all(|file| file.url.is_some()));
    assert!(simple.iter().all(|file| file.path.is_none()));

    // A bare array of the same objects: the third shape in the field.
    let objects = explore_files(
        br#"[{"path":"Example.Release/ep01.mkv","url":"https://s1.offcloud.com/cloud/REDACTED01/ep01.mkv"}]"#,
    );
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].path.as_deref(), Some("Example.Release/ep01.mkv"));

    // Nothing to explore is an empty list, not a failure: a one-file job has no tree, and the
    // address `cloud/status` carries is the answer for it.
    assert!(explore_files(br#"{"files":[]}"#).is_empty());
    assert!(explore_files(b"not json at all").is_empty());
    assert!(explore_files(b"").is_empty());
}

#[test]
fn a_file_keeps_its_name_and_the_job_becomes_its_folder() {
    assert_eq!(
        place("Example.Release", "Example.Release/ep01.mkv"),
        (
            Some("ep01.mkv".to_owned()),
            Some("Example.Release".to_owned())
        )
    );
    // The job name is not repeated when the path already begins with it, which is what keeps
    // a package out of an `Example.Release/Example.Release` folder.
    assert_eq!(
        place("Example.Release", "Example.Release/Sample/s.mkv"),
        (
            Some("s.mkv".to_owned()),
            Some("Example.Release/Sample".to_owned())
        )
    );
    // A path that does not begin with it keeps both.
    assert_eq!(
        place("Example.Release", "Subs/en.srt"),
        (
            Some("en.srt".to_owned()),
            Some("Example.Release/Subs".to_owned())
        )
    );
    // Dot segments never become folders: a path from a provider is not a path to trust.
    assert_eq!(
        place("Example.Release", "../../etc/passwd"),
        (
            Some("passwd".to_owned()),
            Some("Example.Release/etc".to_owned())
        )
    );
    assert_eq!(place("", "ep01.mkv"), (Some("ep01.mkv".to_owned()), None));
}

#[test]
fn a_name_can_be_recovered_from_an_address_when_nothing_else_carries_one() {
    assert_eq!(
        name_from_url("https://s1.offcloud.com/cloud/REDACTED01/My%20Release.mkv").as_deref(),
        Some("My Release.mkv")
    );
    assert_eq!(
        name_from_url("https://s1.offcloud.com/cloud/REDACTED01/a.bin?token=x").as_deref(),
        Some("a.bin")
    );
    assert_eq!(
        name_from_url("https://s1.offcloud.com/cloud/REDACTED01/").as_deref(),
        Some("REDACTED01")
    );
    assert_eq!(name_from_url("https://"), None);
}

#[test]
fn an_identifier_is_checked_before_it_reaches_a_path_or_a_body() {
    assert!(is_safe_request_id("REDACTEDREQUEST01"));
    assert!(is_safe_request_id("a-b_c1"));
    assert!(!is_safe_request_id(""));
    // Each of these would be a request to somewhere else on the one host this plugin may
    // reach, or a JSON body with a second field in it.
    for hostile in [
        "../../account/info",
        "a/b",
        "a\"],\"x\":[\"b",
        "a b",
        "a?key=x",
    ] {
        assert!(!is_safe_request_id(hostile), "{hostile}");
    }
    assert!(!is_safe_request_id(&"a".repeat(129)));
}

#[test]
fn the_remove_body_names_exactly_one_request() {
    assert_eq!(
        String::from_utf8(remove_body("REDACTEDREQUEST01")).expect("utf-8"),
        r#"{"requestIds":["REDACTEDREQUEST01"]}"#
    );
}

#[test]
fn only_a_code_shaped_word_survives_sanitising() {
    assert_eq!(sanitize_error("NOAUTH").as_deref(), Some("noauth"));
    // Prose, and prose that quotes what was sent to it. Dropped whole rather than filtered.
    assert_eq!(sanitize_error("The request could not be processed."), None);
    assert_eq!(sanitize_error("key VHK7OoGO57kH1JOO is unknown"), None);
}

#[test]
fn the_one_stable_word_ends_the_job_and_an_unreadable_sentence_waits() {
    let failure = classify_error("NOAUTH");
    assert_eq!(failure.kind, ErrorKind::AccountInvalid);
    assert_eq!(failure.code, messages::AUTH_INVALID.0);

    let failure = classify_error("SOMETHING_ELSE");
    assert_eq!(failure.kind, ErrorKind::Permanent);
    assert_eq!(
        failure.params,
        vec![("api_code", "something_else".to_owned())]
    );

    let failure = classify_error("Sorry, this could not be processed right now.");
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
    let envelope = ErrorEnvelope {
        error: Some("NOAUTH".to_owned()),
        not_available: None,
    };
    let failure = failure_from(200, None, &envelope).expect("a refusal");
    assert_eq!(failure.kind, ErrorKind::AccountInvalid);
    assert!(failure_from(200, None, &ErrorEnvelope::default()).is_none());
}

#[test]
fn a_status_outranks_prose_and_a_stable_word_outranks_the_status() {
    // The ordering the fixtures caught. Before it, a 429 carrying a sentence was a five-minute
    // wait rather than the two minutes the header asked for, because the sentence was read
    // first and a sentence carries no figure.
    let prose = ErrorEnvelope {
        error: Some("Too many requests, please slow down.".to_owned()),
        not_available: None,
    };
    let failure = failure_from(429, Some(120), &prose).expect("a refusal");
    assert_eq!(failure.kind, ErrorKind::RateLimited(Some(120)));
    assert_eq!(failure.code, messages::RATE_LIMITED.0);

    // The other direction: a stable word says more than the status does.
    let stable = ErrorEnvelope {
        error: Some("NOAUTH".to_owned()),
        not_available: None,
    };
    let failure = failure_from(500, None, &stable).expect("a refusal");
    assert_eq!(failure.kind, ErrorKind::AccountInvalid);

    // Prose on a 2xx is still a refusal: something said no and nothing says what.
    let failure = failure_from(200, None, &prose).expect("a refusal");
    assert_eq!(failure.kind, ErrorKind::Transient(Some(300)));
}

#[test]
fn a_status_decides_when_no_document_explains_itself() {
    assert!(ensure_http_status(200, None).is_ok());
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
    assert_eq!(
        ensure_http_status(429, None).expect_err("refused").kind,
        ErrorKind::RateLimited(Some(3600))
    );
    assert_eq!(
        ensure_http_status(502, None).expect_err("refused").kind,
        ErrorKind::Transient(Some(300))
    );
}

#[test]
fn a_retry_after_is_read_in_seconds_and_a_date_is_ignored() {
    assert_eq!(retry_after_seconds(Some(" 120 ")), Some(120));
    assert_eq!(
        retry_after_seconds(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
        None
    );
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
