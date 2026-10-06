use super::*;

/// The `transfer/directdl` bodies below.
///
/// **Constructed, not captured.** They are written from premiumize's published API
/// documentation (<https://www.premiumize.me/api>, read 2026-09-20) plus the shapes that
/// page names as still being sent: the deprecated top-level `location`/`filename`/
/// `filesize` mirrors of the first `content` entry, the deprecated `content[].stream_link`
/// which is `null` for a non-video, and `content[].transcode_status`. The documentation
/// shows `size` as an integer for this endpoint and as a quoted string for
/// `cache/check`; nobody here has a live account, so the quoted and `null` variants are
/// what this plugin *tolerates*, not what anybody measured. Every URL is
/// `download.example.test`; no fixture carries an API key, a token or a real CDN link.
const DOCUMENTED: &str = include_str!("../../tests/fixtures/directdl-content-documented.json");
const QUOTED_SIZE: &str = include_str!("../../tests/fixtures/directdl-content-quoted-size.json");
const NULL_PATH: &str = include_str!("../../tests/fixtures/directdl-content-null-path.json");
const NULL_CONTENT: &str = include_str!("../../tests/fixtures/directdl-content-null.json");
const LEGACY_SINGLE: &str = include_str!("../../tests/fixtures/directdl-legacy-single-file.json");
const UNSUPPORTED: &str =
    include_str!("../../tests/fixtures/directdl-error-service-unsupported.json");
const SERVICE_DOWN: &str = include_str!("../../tests/fixtures/directdl-error-service-down.json");
const NO_CODE: &str = include_str!("../../tests/fixtures/directdl-error-without-code.json");

/// Reads a fixture the way `resolve` reads a response body.
fn direct(payload: &str) -> Result<Resolved, Failure> {
    let response = HttpResponse {
        status: 200,
        final_url: "https://www.premiumize.me/api/transfer/directdl".to_owned(),
        headers: Vec::new(),
        body: payload.as_bytes().to_vec(),
    };
    let parsed: DirectResponse = parse_json(&response)?;
    ensure_success(
        &parsed.status,
        parsed.code.as_deref(),
        parsed.message.as_deref(),
    )?;
    to_resolved(parsed)
}

#[test]
fn direct_download_reads_the_documented_shape() {
    let resolved = direct(DOCUMENTED).expect("documented shape");
    assert_eq!(
        resolved.url, "https://download.example.test/video1.mkv",
        "the first content entry is the file resolve hands back"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("video1.mkv"));
    assert_eq!(resolved.size, Some(123_456_789));
}

#[test]
fn direct_download_accepts_a_size_quoted_as_a_string() {
    let resolved = direct(QUOTED_SIZE).expect("a quoted size is still a size");
    assert_eq!(resolved.size, Some(425_146_614));
    assert_eq!(
        resolved.file_name.as_deref(),
        Some("outlander.s08e01.german.bdrip.x264-intention.rar")
    );
}

#[test]
fn direct_download_accepts_a_null_path_and_falls_back_to_the_top_level_name() {
    let resolved = direct(NULL_PATH).expect("a nameless entry is still a link");
    assert_eq!(
        resolved.url,
        "https://download.example.test/8x6wertoi51r8vptrojn"
    );
    assert_eq!(
        resolved.file_name.as_deref(),
        Some("outlander.s08e01.german.bdrip.x264-intention.rar"),
        "the deprecated top-level filename is the only name left"
    );
    assert_eq!(resolved.size, Some(425_146_614));
}

#[test]
fn direct_download_treats_a_null_content_like_a_missing_one() {
    let resolved = direct(NULL_CONTENT).expect("null content falls back to the top level");
    assert_eq!(
        resolved.url,
        "https://download.example.test/8x6wertoi51r8vptrojn"
    );
    assert_eq!(
        resolved.file_name.as_deref(),
        Some("outlander.s08e01.german.bdrip.x264-intention.rar")
    );
    assert_eq!(resolved.size, Some(425_146_614));
}

#[test]
fn direct_download_reads_the_legacy_single_file_answer() {
    let resolved = direct(LEGACY_SINGLE).expect("legacy single-file answer");
    assert_eq!(
        resolved.url,
        "https://download.example.test/8x6wertoi51r8vptrojn"
    );
    assert_eq!(
        resolved.file_name.as_deref(),
        Some("outlander.s08e01.german.bdrip.x264-intention.rar")
    );
    assert_eq!(resolved.size, Some(425_146_614));
}

#[test]
fn an_answer_with_no_file_at_all_is_permanent() {
    let failure = direct(r#"{"status":"success","content":[]}"#).expect_err("no file");
    assert_eq!(failure.kind, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("premiumize.no_file"));
}

#[test]
fn a_field_this_plugin_cannot_read_is_permanent_and_names_itself() {
    let failure = direct(r#"{"status":"success","content":[{"link":true}]}"#)
        .expect_err("a boolean is not a link");
    assert_eq!(
        failure.kind,
        FailureKind::Permanent,
        "a body that will never parse must not be retried to max_retries"
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("premiumize.invalid_response_field")
    );
    assert_eq!(
        failure
            .params
            .iter()
            .find(|(name, _)| name == "field")
            .map(|(_, value)| value.as_str()),
        Some("content[0].link"),
        "the reason must name the field instead of discarding it"
    );
}

#[test]
fn a_body_that_is_not_this_api_is_permanent() {
    let failure = direct("<html>maintenance</html>").expect_err("not JSON");
    assert_eq!(failure.kind, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("premiumize.invalid_response"));
}

#[test]
fn the_documented_unsupported_code_is_read_as_unsupported() {
    let failure = direct(UNSUPPORTED).expect_err("unsupported service");
    assert_eq!(
        failure.kind,
        FailureKind::Unsupported,
        "`service_unsupported` is premiumize's documented code for this"
    );
    assert_eq!(failure.code.as_deref(), Some("premiumize.api_error"));
}

#[test]
fn a_service_that_is_down_stays_retryable() {
    let failure = direct(SERVICE_DOWN).expect_err("service down");
    assert_eq!(
        failure.kind,
        FailureKind::Transient(None),
        "a documented transient code must not be buried in the permanent arm"
    );
}

#[test]
fn an_error_without_a_code_is_judged_by_its_message() {
    let failure = direct(NO_CODE).expect_err("no code");
    assert_eq!(failure.kind, FailureKind::Unsupported);
    assert_eq!(
        failure
            .params
            .iter()
            .find(|(name, _)| name == "message")
            .map(|(_, value)| value.as_str()),
        Some("Unsupported link for direct download.")
    );
}

#[test]
fn an_error_that_says_nothing_useful_stays_permanent() {
    let failure = direct(r#"{"status":"error","message":"Something went wrong."}"#)
        .expect_err("unknown error");
    assert_eq!(failure.kind, FailureKind::Permanent);
}

/// The label as `code(name=value)` parts, which is what the interface translates.
fn account_label(payload: &str) -> String {
    let account: AccountResponse = serde_json::from_str(payload).expect("parse");
    assert_eq!(account.status, "success");
    crate::account::label(account.customer_id, account.limit_used.as_ref())
        .into_parts()
        .into_iter()
        .map(|part| {
            let params: Vec<String> = part
                .params
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            format!("{}({})", part.code, params.join(","))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn account_info_reports_the_fair_use_share() {
    assert_eq!(
        account_label(
            r#"{"status":"success","customer_id":"4711","limit_used":0.4235,"space_used":1234567,"premium_until":1893456000}"#
        ),
        "plugin.account.user(user=4711) premiumize.account.fair_use(percent=42)"
    );
}

#[test]
fn account_info_without_optional_fields_stays_valid() {
    assert_eq!(account_label(r#"{"status":"success"}"#), "");
    assert_eq!(
        account_label(r#"{"status":"success","customer_id":"4711"}"#),
        "plugin.account.user(user=4711)"
    );
}

#[test]
fn account_info_tolerates_unexpected_limit_shapes() {
    // A textual fraction still counts, anything else is dropped instead of failing.
    assert_eq!(
        account_label(r#"{"status":"success","customer_id":"4711","limit_used":"0.5"}"#),
        "plugin.account.user(user=4711) premiumize.account.fair_use(percent=50)"
    );
    for limit in ["null", "true", r#"{"used":0.5}"#, r#""many""#, "-0.2"] {
        assert_eq!(
            account_label(&format!(
                r#"{{"status":"success","customer_id":"4711","limit_used":{limit}}}"#
            )),
            "plugin.account.user(user=4711)"
        );
    }
}

#[test]
fn account_info_caps_an_exceeded_limit() {
    assert_eq!(
        account_label(r#"{"status":"success","customer_id":"4711","limit_used":1.4}"#),
        "plugin.account.user(user=4711) premiumize.account.fair_use(percent=100)"
    );
}

#[test]
fn cache_check_maps_index_aligned_arrays() {
    let response: CacheCheckResponse = serde_json::from_str(
        r#"{"status":"success","response":[true,false],"filename":["a.rar",null],"filesize":["1024",null]}"#,
    )
    .expect("parse");
    let urls: Vec<String> = ["https://h/a", "https://h/b"]
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    let results = map_cache_check(&urls, &response);
    assert_eq!(results[0].status, LinkStatus::Cached);
    assert_eq!(results[0].file_name.as_deref(), Some("a.rar"));
    assert_eq!(results[0].size, Some(1024));
    assert_eq!(results[1].status, LinkStatus::Unknown);
}

/// RD-120-36: a cached file is `Cached`, and a file Premiumize knows but has not fetched
/// is `Online`. The two no longer collapse into one answer.
#[test]
fn cache_check_keeps_cached_apart_from_known() {
    let response: CacheCheckResponse = serde_json::from_str(
        r#"{"status":"success","response":[true,false,false],"filename":["a.rar","b.rar",null],"filesize":["1024","2048",null]}"#,
    )
    .expect("parse");
    let urls: Vec<String> = ["https://h/a", "https://h/b", "https://h/c"]
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    let statuses: Vec<LinkStatus> = map_cache_check(&urls, &response)
        .into_iter()
        .map(|result| result.status)
        .collect();
    assert_eq!(
        statuses,
        [LinkStatus::Cached, LinkStatus::Online, LinkStatus::Unknown]
    );
}

/// The status check `request` makes, on one answer.
fn classify_status(response: &HttpResponse) -> Result<(), Failure> {
    HTTP_ERROR
        .ensure_http_status(
            response.status,
            plugin_common::retry_after(&response.headers),
        )
        .map_err(Failure::from)
}

fn answer(status: u16, headers: &[(&str, &str)]) -> HttpResponse {
    HttpResponse {
        status,
        final_url: "https://www.premiumize.me/api/x".to_owned(),
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        body: Vec::new(),
    }
}

/// The shared mapping (RD-191-07): a `429` carries the provider's `Retry-After`, a `410` is
/// final (owner, 2026-10-04), a `451` offline and retried, and everything travels under this
/// plugin's one HTTP code with its status.
#[test]
fn a_bare_status_is_classified_by_the_shared_mapping() {
    assert!(classify_status(&answer(200, &[])).is_ok());
    let limited = classify_status(&answer(429, &[("Retry-After", "40")])).expect_err("429");
    assert_eq!(limited.kind, FailureKind::RateLimited(Some(40)));
    assert_eq!(limited.code.as_deref(), Some(messages::HTTP_ERROR));
    assert_eq!(
        classify_status(&answer(410, &[])).expect_err("410").kind,
        FailureKind::Permanent
    );
    assert_eq!(
        classify_status(&answer(451, &[])).expect_err("451").kind,
        FailureKind::Offline
    );
    assert_eq!(
        classify_status(&answer(403, &[])).expect_err("403").kind,
        FailureKind::AccountInvalid
    );
    let odd = classify_status(&answer(418, &[])).expect_err("418");
    assert_eq!(odd.kind, FailureKind::Permanent);
    assert_eq!(odd.params, vec![("status".to_owned(), "418".to_owned())]);
}
