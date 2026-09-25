//! `resolve` on the scripted host: the live success path, and every refusal the job names.

use rd_core::FailureKind;
use rd_plugin_api::{ResolvedHeader, Resolver};

use super::{
    API_ERROR_261, API_URL, DIRECT_URL, FILE_PAGE, FILE_URL, GET_INFO, GET_INFO_INVALID,
    GET_INFO_MISSING, KEY, MALWARE, MockHost, PAGE_URL, PASSWORD, THRESHOLD, html, html_at, json,
    resolve_request, resolver,
};

/// `file/get_info` for the key, then the file page: name, size, SHA-256 and a delivery-host
/// address, with the two requests the plan promises and nothing else.
#[tokio::test]
async fn a_public_file_resolves_to_its_direct_link_with_name_size_and_hash() {
    let host = MockHost::with_responses(vec![json(200, GET_INFO), html(FILE_PAGE)]);
    let resolved = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect("resolved");
    assert_eq!(resolved.url.as_str(), DIRECT_URL);
    assert_eq!(resolved.file_name.as_deref(), Some("test-10mb.bin"));
    assert_eq!(resolved.size.map(rd_core::ByteCount::get), Some(10_485_760));
    let checksum = resolved.checksum.expect("the API's SHA-256");
    assert_eq!(checksum.algorithm, rd_core::ChecksumAlgorithm::Sha256);
    assert_eq!(
        checksum.value,
        "e5b844cc57f57094ea4585e235f36c78c1cd222262bb89d53c94dcb4d6b3e55d"
    );
    assert!(resolved.headers.is_empty());

    let requests = host.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].url.as_str(), API_URL);
    let query: Vec<(&str, &str)> = requests[0]
        .query
        .iter()
        .map(|item| (item.name.as_str(), item.value_template.as_str()))
        .collect();
    assert!(query.contains(&("quick_key", KEY)));
    assert!(query.contains(&("response_format", "json")));
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].url.as_str(), PAGE_URL);
    assert!(
        requests[1]
            .headers
            .iter()
            .any(|header| header.name == "Range" && header.value_template == "bytes=0-0"),
        "the page is probed with a one-byte range"
    );
}

/// Every spelling of a file address resolves through the same canonical page.
#[tokio::test]
async fn every_file_form_reaches_the_same_canonical_page() {
    for url in [
        "https://www.mediafire.com/download/ipnyzofjcwri357",
        "https://www.mediafire.com/view/ipnyzofjcwri357",
        "https://mfi.re/?ipnyzofjcwri357",
        "https://www.mediafire.com/download.php?ipnyzofjcwri357",
        "https://app.mediafire.com/ipnyzofjcwri357",
    ] {
        let host = MockHost::with_responses(vec![json(200, GET_INFO), html(FILE_PAGE)]);
        let resolved = resolver(&host)
            .resolve(resolve_request(url))
            .await
            .unwrap_or_else(|failure| panic!("{url}: {failure:?}"));
        assert_eq!(resolved.url.as_str(), DIRECT_URL, "{url}");
        assert_eq!(host.requests()[1].url.as_str(), PAGE_URL, "{url}");
    }
}

/// Criterion 1: a page without the button ends in `no_direct_link`, never in the page address.
#[tokio::test]
async fn a_page_without_a_button_never_becomes_the_download() {
    let stripped = FILE_PAGE.replace("id=\"downloadButton\"", "");
    assert!(!stripped.contains("downloadButton"));
    // The stripped page still carries the delivery address in the anchor's href; a scan for
    // it is the third fallback, so the shell page (no address at all) is the real test.
    let shell = "<html><head><title>MediaFire - File sharing and storage made simple</title></head><body></body></html>";
    let host = MockHost::with_responses(vec![json(200, GET_INFO), html(shell)]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("no link");
    assert_eq!(failure.code.as_deref(), Some("mediafire.no_direct_link"));
    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(
        failure.params.get("diagnosis").map(String::as_str),
        Some("page titled \"MediaFire - File sharing and storage made simple\"")
    );
    assert!(!failure.message.contains(PAGE_URL));
}

/// A redirect to the file itself is accepted from a delivery host and from nowhere else.
#[tokio::test]
async fn a_redirect_to_the_file_is_accepted_only_from_a_delivery_host() {
    let file = |final_url: &str| rd_plugin_api::HostHttpResponse {
        status: 206,
        final_url: final_url.parse().expect("URL"),
        headers: vec![ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "application/octet-stream".to_owned(),
        }],
        body: vec![0],
    };
    let host = MockHost::with_responses(vec![json(200, GET_INFO), file(DIRECT_URL)]);
    let resolved = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect("resolved");
    assert_eq!(resolved.url.as_str(), DIRECT_URL);

    let host = MockHost::with_responses(vec![
        json(200, GET_INFO),
        file("https://cdn.evil.test/ipnyzofjcwri357/test-10mb.bin"),
    ]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some("mediafire.no_direct_link"));
}

#[tokio::test]
async fn the_api_refusals_end_in_their_own_codes_before_any_page_is_fetched() {
    for (body, status, code, kind) in [
        (
            GET_INFO_INVALID,
            404,
            "mediafire.file_unavailable",
            FailureKind::Offline,
        ),
        (
            GET_INFO_MISSING,
            400,
            "mediafire.invalid_link",
            FailureKind::Permanent,
        ),
        (
            API_ERROR_261,
            200,
            "mediafire.rate_limited",
            FailureKind::RateLimited {
                retry_after_seconds: None,
            },
        ),
        (
            br#"{"response":{"action":"file/get_info","message":"Access denied","error":114,"result":"Error"}}"#,
            403,
            "mediafire.private_file",
            FailureKind::Permanent,
        ),
        (
            br#"{"response":{"action":"file/get_info","message":"Something\tnew\n","error":900,"result":"Error"}}"#,
            400,
            "mediafire.api_error",
            FailureKind::Permanent,
        ),
        (
            b"<html>Bad gateway</html>",
            502,
            "mediafire.http_error",
            FailureKind::Transient {
                retry_after_seconds: None,
            },
        ),
        (
            b"not json at all",
            200,
            "mediafire.invalid_response",
            FailureKind::Permanent,
        ),
    ] {
        let host = MockHost::with_responses(vec![json(status, body)]);
        let failure = resolver(&host)
            .resolve(resolve_request(FILE_URL))
            .await
            .expect_err(code);
        assert_eq!(failure.code.as_deref(), Some(code));
        assert_eq!(failure.category, kind, "{code}");
        assert_eq!(host.requests().len(), 1, "{code}: no page fetch after a refusal");
        if code == "mediafire.api_error" {
            assert_eq!(
                failure.params.get("message").map(String::as_str),
                Some("Something new")
            );
        }
    }
}

/// A bare key the API calls "missing" is asked about as a folder before it is called invalid.
#[tokio::test]
async fn a_bare_folder_key_is_reported_as_a_folder() {
    let folder = br#"{"response":{"action":"folder/get_info","folder_info":{"folderkey":"rww7bhhi0yc1l","name":"Walls","privacy":"public"},"result":"Success"}}"#;
    let host = MockHost::with_responses(vec![json(400, GET_INFO_MISSING), json(200, folder)]);
    let failure = resolver(&host)
        .resolve(resolve_request("https://www.mediafire.com/?rww7bhhi0yc1l"))
        .await
        .expect_err("a folder");
    assert_eq!(failure.code.as_deref(), Some("mediafire.folder_not_file"));
    assert_eq!(failure.category, FailureKind::Unsupported);
    let requests = host.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].url.as_str().ends_with("/folder/get_info.php"));

    // The same key spelled as a file path is not asked twice: the path already decided.
    let host = MockHost::with_responses(vec![json(400, GET_INFO_MISSING)]);
    let failure = resolver(&host)
        .resolve(resolve_request(
            "https://www.mediafire.com/file/rww7bhhi0yc1l",
        ))
        .await
        .expect_err("invalid");
    assert_eq!(failure.code.as_deref(), Some("mediafire.invalid_link"));
    assert_eq!(host.requests().len(), 1);

    // A folder path is refused without a request at all.
    let host = MockHost::with_responses(Vec::new());
    let failure = resolver(&host)
        .resolve(resolve_request(
            "https://www.mediafire.com/folder/rww7bhhi0yc1l",
        ))
        .await
        .expect_err("a folder");
    assert_eq!(failure.code.as_deref(), Some("mediafire.folder_not_file"));
    assert!(host.requests().is_empty());
}

#[tokio::test]
async fn what_the_api_says_about_the_file_is_refused_before_the_page() {
    for (patch, code, kind) in [
        (
            ("\"privacy\": \"public\"", "\"privacy\": \"private\""),
            "mediafire.private_file",
            FailureKind::Permanent,
        ),
        (
            (
                "\"password_protected\": \"no\"",
                "\"password_protected\": \"yes\"",
            ),
            "mediafire.password_required",
            FailureKind::Permanent,
        ),
        (
            ("\"ready\": \"yes\"", "\"ready\": \"no\""),
            "mediafire.file_not_ready",
            FailureKind::Transient {
                retry_after_seconds: Some(300),
            },
        ),
    ] {
        let body = String::from_utf8_lossy(GET_INFO).replace(patch.0, patch.1);
        assert_ne!(
            body.as_bytes(),
            GET_INFO,
            "the fixture must carry {}",
            patch.0
        );
        let host = MockHost::with_responses(vec![json(200, body.as_bytes())]);
        let failure = resolver(&host)
            .resolve(resolve_request(FILE_URL))
            .await
            .expect_err(code);
        assert_eq!(failure.code.as_deref(), Some(code));
        assert_eq!(failure.category, kind);
        assert_eq!(host.requests().len(), 1, "{code}: the page is not fetched");
    }
}

/// The `error.php?errno=` table, read from the address the page request was redirected to.
#[tokio::test]
async fn the_error_pages_end_in_the_codes_of_the_errno_table() {
    for (errno, code, kind, reason) in [
        (
            320,
            "mediafire.file_unavailable",
            FailureKind::Offline,
            None,
        ),
        (
            323,
            "mediafire.file_blocked",
            FailureKind::Permanent,
            Some("dangerous_file"),
        ),
        (
            378,
            "mediafire.file_blocked",
            FailureKind::Permanent,
            Some("terms_violation"),
        ),
        (
            388,
            "mediafire.file_blocked",
            FailureKind::Permanent,
            Some("copyright_claim"),
        ),
        (
            382,
            "mediafire.file_blocked",
            FailureKind::Permanent,
            Some("account_suspended"),
        ),
        (
            394,
            "mediafire.owner_limit",
            FailureKind::Permanent,
            Some("encrypted_archive_limit"),
        ),
        (999, "mediafire.private_file", FailureKind::Permanent, None),
        (555, "mediafire.error_page", FailureKind::Permanent, None),
    ] {
        let target = format!("https://www.mediafire.com/error.php?errno={errno}&origin=download");
        let host = MockHost::with_responses(vec![
            json(200, GET_INFO),
            html_at(&target, "<html><title>Error</title></html>"),
        ]);
        let failure = resolver(&host)
            .resolve(resolve_request(FILE_URL))
            .await
            .expect_err(code);
        assert_eq!(failure.code.as_deref(), Some(code), "errno {errno}");
        assert_eq!(failure.category, kind, "errno {errno}");
        assert_eq!(
            failure.params.get("reason").map(String::as_str),
            reason,
            "errno {errno}"
        );
        if errno == 555 {
            assert_eq!(failure.params.get("errno").map(String::as_str), Some("555"));
        }
    }
    // The measured redirect target of 2026-09-21, verbatim.
    let measured = include_str!("../../tests/fixtures/error-redirect-errno-320-2026-09-21.txt");
    assert_eq!(
        mediafire_common::address::error_number(measured.trim()),
        Some(320)
    );
}

/// The page states the job names (all synthetic; none was seen live).
#[tokio::test]
async fn the_page_states_end_in_their_own_codes() {
    let host = MockHost::with_responses(vec![json(200, GET_INFO), html(THRESHOLD)]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("threshold");
    assert_eq!(
        failure.code.as_deref(),
        Some("mediafire.download_limit_reached")
    );
    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(3600)
        }
    );
    assert_eq!(
        failure.params.get("wait_seconds").map(String::as_str),
        Some("3600")
    );

    let host = MockHost::with_responses(vec![json(200, GET_INFO), html(MALWARE)]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("malware");
    assert_eq!(failure.code.as_deref(), Some("mediafire.malware_flagged"));
    assert_eq!(
        host.requests().len(),
        2,
        "the advisory is never clicked through"
    );

    let host = MockHost::with_responses(vec![json(200, GET_INFO), html(PASSWORD)]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("password");
    assert_eq!(failure.code.as_deref(), Some("mediafire.password_required"));

    let host = MockHost::with_responses(vec![
        json(200, GET_INFO),
        html(
            "<html><title>Temporarily Unavailable</title><p>retry your download again in 45 seconds</p></html>",
        ),
    ]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("wait");
    assert_eq!(
        failure.code.as_deref(),
        Some("mediafire.temporarily_unavailable")
    );
    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: Some(45)
        }
    );

    let host = MockHost::with_responses(vec![
        json(200, GET_INFO),
        html_at(
            "https://www.mediafire.com/download_repair.php?qkey=ipnyzofjcwri357",
            "<html></html>",
        ),
    ]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("repair");
    assert_eq!(
        failure.code.as_deref(),
        Some("mediafire.temporarily_unavailable")
    );
}

#[tokio::test]
async fn a_page_status_that_is_not_an_answer_is_reported() {
    let mut refused = html(FILE_PAGE);
    refused.status = 503;
    let host = MockHost::with_responses(vec![json(200, GET_INFO), refused]);
    let failure = resolver(&host)
        .resolve(resolve_request(FILE_URL))
        .await
        .expect_err("503");
    assert_eq!(failure.code.as_deref(), Some("mediafire.http_error"));
    assert_eq!(
        failure.params.get("status").map(String::as_str),
        Some("503")
    );
}

#[tokio::test]
async fn links_that_are_not_files_are_refused_without_a_request() {
    let host = MockHost::with_responses(Vec::new());
    let failure = resolver(&host)
        .resolve(resolve_request(
            "https://www.mediafire.com/upgrade/get_plan.php",
        ))
        .await
        .expect_err("unsupported");
    assert_eq!(failure.code.as_deref(), Some("mediafire.unsupported_link"));
    assert_eq!(failure.category, FailureKind::Unsupported);
    let failure = resolver(&host)
        .resolve(resolve_request(
            "https://www.mediafire.com/?ipnyzofjcwri357,8ipst0t9u6sibpx",
        ))
        .await
        .expect_err("a list");
    assert_eq!(failure.code.as_deref(), Some("mediafire.folder_not_file"));
    assert!(host.requests().is_empty());
}
