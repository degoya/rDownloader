//! The parts that touch no host: the envelope, the state machine, the bodies and the paths.

use serde_json::json;

use super::{
    ApiFailure, CachedEntry, ErrorEnvelope, ErrorKind, JobEntry, Stage, boundary, cached_entries,
    check_cached_path, classify_error, control_body, download_address, ensure_http_status,
    failure_from, is_safe_remote_id, multipart, permille, place, retry_after_seconds, stage_of,
};
use crate::{messages, source::Kind};

fn envelope(body: serde_json::Value) -> ErrorEnvelope {
    serde_json::from_value(body).expect("an envelope")
}

fn entry(body: serde_json::Value) -> JobEntry {
    serde_json::from_value(body).expect("an entry")
}

/// A refusal inside a 200 is still a refusal, and a success carrying no word is not one.
#[test]
fn the_envelope_decides_before_the_status_does() {
    let refusal = failure_from(
        200,
        None,
        &envelope(json!({"success": false, "error": "BAD_TOKEN"})),
    )
    .expect("a refusal");
    assert_eq!(refusal.kind, ErrorKind::AccountInvalid);
    assert_eq!(refusal.code, messages::AUTH_INVALID.0);
    assert!(
        failure_from(
            200,
            None,
            &envelope(json!({"success": true, "error": null}))
        )
        .is_none()
    );
    // `success: false` with no word at all is still a refusal, classified by its status.
    let refusal = failure_from(200, None, &envelope(json!({"success": false}))).expect("a refusal");
    assert_eq!(refusal.kind, ErrorKind::Permanent);
    // A 4xx carrying no word is classified by its status alone.
    let refusal = failure_from(429, Some(120), &envelope(json!({}))).expect("a refusal");
    assert_eq!(refusal.kind, ErrorKind::RateLimited(Some(120)));
    assert_eq!(refusal.code, messages::RATE_LIMITED.0);
}

/// The word travels and the sentence does not.
#[test]
fn a_refusal_carries_the_word_and_never_the_sentence() {
    let refusal: ApiFailure = classify_error("MONTHLY_LIMIT", None);
    assert_eq!(refusal.code, messages::LIMIT_REACHED.0);
    assert_eq!(
        refusal.params,
        vec![("api_code", "MONTHLY_LIMIT".to_owned())]
    );
    let sentence = "the link https://example.invalid/secret?key=abc was refused";
    let carried = failure_from(
        400,
        None,
        &envelope(json!({"success": false, "error": "LINK_OFFLINE", "detail": sentence})),
    )
    .expect("a refusal");
    assert!(!carried.message.contains("example.invalid"));
    assert!(
        carried
            .params
            .iter()
            .all(|(_, value)| !value.contains("example.invalid"))
    );
}

/// Every bucket a caller acts on differently.
#[test]
fn each_documented_word_lands_in_the_bucket_that_matches_it() {
    for (word, kind, code) in [
        (
            "BAD_TOKEN",
            ErrorKind::AccountInvalid,
            messages::AUTH_INVALID.0,
        ),
        (
            "NO_AUTH",
            ErrorKind::AccountInvalid,
            messages::AUTH_INVALID.0,
        ),
        (
            "DUPLICATE_ITEM",
            ErrorKind::Duplicate,
            messages::JOB_EXISTS.0,
        ),
        ("ITEM_NOT_FOUND", ErrorKind::Offline, messages::JOB_GONE.0),
        ("LINK_OFFLINE", ErrorKind::Offline, messages::SOURCE_GONE.0),
        (
            "PLAN_RESTRICTED_FEATURE",
            ErrorKind::Unsupported,
            messages::NOT_PERMITTED.0,
        ),
        (
            "ACTIVE_LIMIT",
            ErrorKind::RateLimited(Some(3600)),
            messages::LIMIT_REACHED.0,
        ),
        (
            "COOLDOWN_LIMIT",
            ErrorKind::RateLimited(Some(600)),
            messages::COOLDOWN.0,
        ),
        (
            "DATABASE_ERROR",
            ErrorKind::Transient(Some(300)),
            messages::SERVER_BUSY.0,
        ),
        (
            "DOWNLOAD_TOO_LARGE",
            ErrorKind::Permanent,
            messages::TOO_LARGE.0,
        ),
    ] {
        let refusal = classify_error(word, None);
        assert_eq!(refusal.kind, kind, "{word}");
        assert_eq!(refusal.code, code, "{word}");
    }
    // A word this build has not heard of keeps the word and says so.
    let unknown = classify_error("SOMETHING_NEW", None);
    assert_eq!(unknown.code, messages::API_ERROR.0);
    assert!(unknown.message.contains("SOMETHING_NEW"));
}

#[test]
fn an_http_status_no_word_explains_is_mapped_on_its_own() {
    assert!(ensure_http_status(204, None).is_ok());
    for (status, kind) in [
        (401_u16, ErrorKind::AccountInvalid),
        (404, ErrorKind::Offline),
        (429, ErrorKind::RateLimited(Some(60))),
        (503, ErrorKind::Transient(Some(300))),
    ] {
        let refusal = ensure_http_status(status, None).expect_err("a refusal");
        assert_eq!(refusal.kind, kind, "{status}");
    }
    assert_eq!(retry_after_seconds(Some(" 90 ")), Some(90));
    // A date-shaped header is ignored rather than guessed at.
    assert_eq!(
        retry_after_seconds(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
        None
    );
}

/// The two booleans decide, and the word only says what is happening while they are false.
#[test]
fn a_job_is_ready_when_its_bytes_are_there_and_not_before() {
    let ready = entry(json!({
        "download_state": "completed",
        "download_finished": true,
        "download_present": true
    }));
    assert_eq!(stage_of(&ready), Stage::Ready);
    // Finished and not yet servable is the short window before TorBox publishes.
    let publishing = entry(json!({
        "download_state": "completed",
        "download_finished": true,
        "download_present": false
    }));
    assert_eq!(stage_of(&publishing), Stage::Preparing(10));
    // A cache hit is not a state of its own: it reaches `Ready` by the same rule.
    let cached = entry(json!({
        "download_state": "cached",
        "download_finished": true,
        "download_present": true
    }));
    assert_eq!(stage_of(&cached), Stage::Ready);
}

#[test]
fn every_state_word_maps_the_way_a_caller_needs_it_to() {
    for (word, stage) in [
        ("downloading", Stage::Working(30)),
        ("uploading", Stage::Working(30)),
        ("stalled (no seeds)", Stage::Working(60)),
        ("stalledDL", Stage::Working(60)),
        ("metaDL", Stage::Preparing(15)),
        ("queued", Stage::Preparing(30)),
        ("paused", Stage::Preparing(120)),
        ("error", Stage::Failed(messages::JOB_FAILED)),
        ("missingFiles", Stage::Failed(messages::JOB_INCOMPLETE)),
        // A word this build has not heard of keeps the job alive.
        ("teleporting", Stage::Preparing(60)),
        ("", Stage::Preparing(30)),
    ] {
        let entry = entry(json!({"download_state": word}));
        assert_eq!(stage_of(&entry), stage, "{word}");
    }
    // A failed job is failed whatever the booleans say: bytes that are there are the wrong
    // bytes when TorBox says the job ended in an error.
    let broken = entry(json!({
        "download_state": "error",
        "download_finished": true,
        "download_present": true
    }));
    assert_eq!(stage_of(&broken), Stage::Failed(messages::JOB_FAILED));
}

/// TorBox states a fraction and Real-Debrid a percentage; reading one as the other is a bar
/// that never leaves zero.
#[test]
fn progress_is_read_as_a_fraction() {
    assert_eq!(permille(Some(0.425)), Some(425));
    assert_eq!(permille(Some(0.0)), Some(0));
    assert_eq!(permille(Some(1.0)), Some(1000));
    // Nothing outside the range and nothing that is not a number.
    assert_eq!(permille(Some(4.2)), Some(1000));
    assert_eq!(permille(Some(-1.0)), Some(0));
    assert_eq!(permille(Some(f64::NAN)), None);
    assert_eq!(permille(None), None);
}

/// The job's name is the package, and it is not repeated inside itself.
#[test]
fn a_file_keeps_its_name_and_the_folder_it_sat_in() {
    assert_eq!(
        place("Example.Release", "Example.Release/ep01.mkv"),
        (
            Some("ep01.mkv".to_owned()),
            Some("Example.Release".to_owned())
        )
    );
    assert_eq!(
        place("Example.Release", "Example.Release/Sample/sample.mkv"),
        (
            Some("sample.mkv".to_owned()),
            Some("Example.Release/Sample".to_owned())
        )
    );
    // A single-file job whose path is the bare name.
    assert_eq!(
        place("Example.Release", "file.bin"),
        (
            Some("file.bin".to_owned()),
            Some("Example.Release".to_owned())
        )
    );
    // Nothing a path escapes through.
    assert_eq!(
        place("Example.Release", "../../etc/passwd"),
        (
            Some("passwd".to_owned()),
            Some("Example.Release/etc".to_owned())
        )
    );
}

/// The address a finished file is handed back under is stable and carries no credential.
#[test]
fn the_finished_address_names_the_file_and_nothing_else() {
    let address = download_address(Kind::Torrent, "4711", 3);
    assert_eq!(
        address,
        "https://api.torbox.app/v1/api/torrents/requestdl?torrent_id=4711&file_id=3"
    );
    assert!(!address.contains("token"), "{address}");
    assert!(download_address(Kind::Usenet, "4711", 0).contains("usenet_id=4711"));
    assert!(download_address(Kind::Web, "4711", 0).contains("web_id=4711"));
}

/// TorBox spells the web download's identifier one way when it mints an address and another
/// when it deletes one, and a single spelling would be wrong at one of the two ends.
#[test]
fn the_control_body_names_the_job_the_way_that_endpoint_spells_it() {
    let body = control_body(Kind::Web, "4711", "delete");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(value["webdl_id"], "4711");
    assert_eq!(value["operation"], "delete");
    let torrent: serde_json::Value =
        serde_json::from_slice(&control_body(Kind::Torrent, "1", "delete")).expect("JSON");
    assert_eq!(torrent["torrent_id"], "1");
}

/// An identifier comes back from TorBox and goes out again in a query.
#[test]
fn a_provider_identifier_is_checked_before_it_is_spliced_into_a_request() {
    assert!(is_safe_remote_id("4711"));
    assert!(is_safe_remote_id("abc-DEF_123"));
    for refused in ["", "../x", "a&b=c", "a b", &"x".repeat(65)] {
        assert!(!is_safe_remote_id(refused), "{refused}");
    }
    // A numeric identifier arrives as a number and leaves as the string a handle carries.
    let listed = entry(json!({"id": 4711, "hash": "ABC"}));
    assert_eq!(listed.identifier().as_deref(), Some("4711"));
    let hostile = entry(json!({"id": "../../x"}));
    assert_eq!(hostile.identifier(), None);
}

#[test]
fn a_multipart_body_carries_the_parts_it_was_given_and_closes_itself() {
    let mark = boundary(&[0xde, 0xad, 0xbe, 0xef]);
    assert_eq!(mark, "rdownloaderdeadbeef");
    let body = multipart(
        &mark,
        &[("magnet", "magnet:?xt=urn:btih:abc")],
        Some(("file", "upload.torrent", b"d4:infod0:ee")),
    );
    let text = String::from_utf8_lossy(&body);
    assert!(text.starts_with("--rdownloaderdeadbeef\r\n"));
    assert!(text.contains("name=\"magnet\""));
    assert!(text.contains("magnet:?xt=urn:btih:abc"));
    assert!(text.contains("filename=\"upload.torrent\""));
    assert!(text.ends_with("--rdownloaderdeadbeef--\r\n"), "{text}");
    // Text-only is the same body without the file part.
    let plain = multipart(&mark, &[("link", "https://example.invalid/x")], None);
    assert!(!String::from_utf8_lossy(&plain).contains("filename"));
}

fn cached(body: serde_json::Value) -> Option<Vec<CachedEntry>> {
    cached_entries(&serde_json::to_vec(&body).expect("JSON"))
}

/// RD-130-11: `data` keyed by hash and `data` as a list are both read, and each entry keeps
/// its name and size.
#[test]
fn a_cache_answer_is_read_keyed_by_hash_and_as_a_list() {
    let keyed = cached(json!({
        "success": true,
        "error": null,
        "detail": "Found 1 cached torrent.",
        "data": {
            "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709": {
                "name": "Example.Release",
                "size": 31,
                "hash": "da39a3ee5e6b4b0d3255bfef95601890afd80709"
            }
        }
    }))
    .expect("an answer");
    assert_eq!(
        keyed,
        vec![CachedEntry {
            hash: "da39a3ee5e6b4b0d3255bfef95601890afd80709".to_owned(),
            name: Some("Example.Release".to_owned()),
            size: Some(31),
        }]
    );
    // An entry without its own `hash` is known by its key.
    let by_key = cached(json!({"data": {"abc123": {"name": "x", "size": 1}}})).expect("answer");
    assert_eq!(by_key[0].hash, "abc123");

    let listed = cached(json!({
        "success": true,
        "data": [
            {"name": "One", "size": 5, "hash": "aaa"},
            {"name": "", "hash": "bbb"},
            {"name": "No hash"},
            "ccc"
        ]
    }))
    .expect("an answer");
    assert_eq!(
        listed.len(),
        3,
        "an entry without a hash is nothing to match"
    );
    assert_eq!(listed[1].name, None, "an empty name is no name");
    assert_eq!(listed[2].hash, "ccc");

    // A single entry as `data` is read as a list of one.
    let single = cached(json!({"data": {"hash": "ddd", "name": "Single"}})).expect("answer");
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].name.as_deref(), Some("Single"));
}

/// Every spelling of "nothing held" is an empty answer, not a failure -- and an answer that is
/// not one is refused.
#[test]
fn nothing_held_has_many_spellings_and_none_is_a_failure() {
    for data in [json!(null), json!(false), json!({}), json!([])] {
        assert_eq!(
            cached(json!({"success": true, "data": data})),
            Some(Vec::new()),
            "{data}"
        );
    }
    assert_eq!(cached_entries(b"<html>busy</html>"), None);
    assert_eq!(cached(json!({"success": true})), None);
    assert_eq!(cached(json!([1, 2])), None);
}

/// TorBox's SDK types a size as a float. It is truncated; a negative one is no size.
#[test]
fn a_cached_size_may_be_a_float_and_is_never_negative() {
    let entries = cached(json!({"data": [
        {"hash": "a", "size": 1536.9},
        {"hash": "b", "size": -4},
        {"hash": "c", "size": -0.5},
        {"hash": "d", "size": "12"},
        {"hash": "e", "size": 1e30}
    ]}))
    .expect("an answer");
    assert_eq!(entries[0].size, Some(1536));
    assert_eq!(entries[1].size, None);
    assert_eq!(entries[2].size, None);
    assert_eq!(entries[3].size, None, "a string is not a size");
    assert_eq!(entries[4].size, Some(u64::MAX), "saturated, not wrapped");
}

#[test]
fn each_kind_has_its_own_cache_endpoint() {
    assert_eq!(check_cached_path(Kind::Torrent), "/torrents/checkcached");
    assert_eq!(check_cached_path(Kind::Usenet), "/usenet/checkcached");
    assert_eq!(check_cached_path(Kind::Web), "/webdl/checkcached");
}
