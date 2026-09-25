//! The account-less flow, step by step: file page, Turnstile, the form's post, the answer,
//! the direct link's probe - and every way the site says no along it.

use rd_core::FailureKind;
use rd_plugin_api::{CaptchaChallenge, ClientIdentity, ResolveRequest, Resolver};

use super::super::KrakenfilesResolver;
use super::{
    CAPTCHA_INVALID, DIRECT_LINK, DOWNLOAD_OK, ERROR_PAGE, FILE_PAGE, FILE_PAGE_URL, MockHost,
    TURNSTILE_SITE_KEY, body_of, file, header_of, html, json, resolve_request,
};

const CANONICAL_LINK: &str = "https://krakenfiles.com/view/DP3nGKJNsX/file.html";

#[tokio::test]
async fn the_flow_solves_turnstile_posts_the_form_and_probes_the_link() {
    let host = MockHost::new(
        vec![html(200, FILE_PAGE), json(200, DOWNLOAD_OK), file(206)],
        Some("turnstile-token"),
    );
    let resolver = KrakenfilesResolver::new(host.clone());

    let resolved = resolver
        .resolve(resolve_request(CANONICAL_LINK))
        .await
        .expect("the free download resolves");

    assert_eq!(resolved.url.as_str(), DIRECT_LINK);
    assert_eq!(
        resolved.file_name.as_deref(),
        Some("EldenRing_Fix_Repair_Steam_Generic.rar")
    );
    assert_eq!(
        resolved.size.map(rd_core::ByteCount::get),
        Some(5_138_022),
        "the exact length from the probe's Content-Range"
    );
    assert_eq!(
        resolved
            .headers
            .iter()
            .find(|header| header.name == "Referer")
            .map(|header| header.value.as_str()),
        Some("https://krakenfiles.com/")
    );

    assert_eq!(host.request_count(), 3, "page, post, probe");
    let page = host.request(0);
    assert_eq!(page.method, "GET");
    assert_eq!(
        page.url.as_str(),
        FILE_PAGE_URL,
        "lowercased id, apex domain"
    );
    let post = host.request(1);
    assert_eq!(post.method, "POST");
    assert_eq!(
        post.url.as_str(),
        "https://krakenfiles.com/download/DP3nGKJNsX",
        "the form's own action, with the site's spelling of the hash"
    );
    assert_eq!(
        body_of(&post),
        "token=dl-token-redacted-0000000000000000000000&userdata=&fingerprint=\
         &cf-turnstile-response=turnstile-token",
        "the page's token, the empty fields as both references post them, the answer"
    );
    assert_eq!(
        header_of(&post, "Content-Type").as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        header_of(&post, "X-Requested-With").as_deref(),
        Some("XMLHttpRequest")
    );
    assert_eq!(header_of(&post, "hash").as_deref(), Some("DP3nGKJNsX"));
    assert_eq!(header_of(&post, "Referer").as_deref(), Some(FILE_PAGE_URL));
    let probe = host.request(2);
    assert_eq!(probe.url.as_str(), DIRECT_LINK);
    assert_eq!(header_of(&probe, "Range").as_deref(), Some("bytes=0-0"));
    assert_eq!(
        header_of(&probe, "Referer").as_deref(),
        Some("https://krakenfiles.com/")
    );

    let captchas = host.captchas.lock().expect("lock");
    assert_eq!(captchas.len(), 1);
    match &captchas[0] {
        CaptchaChallenge::Turnstile(widget) => {
            assert_eq!(widget.site_key, TURNSTILE_SITE_KEY);
            assert_eq!(widget.page_url, FILE_PAGE_URL);
            assert!(!widget.invisible);
        }
        other => panic!("expected a Turnstile challenge, got {other:?}"),
    }
}

/// The embed player's link names the same file; the flow starts at the file page for it.
#[tokio::test]
async fn an_embed_link_is_resolved_through_the_file_page() {
    let host = MockHost::new(
        vec![html(200, FILE_PAGE), json(200, DOWNLOAD_OK), file(206)],
        Some("turnstile-token"),
    );
    let resolver = KrakenfilesResolver::new(host.clone());

    resolver
        .resolve(resolve_request(
            "https://www.krakenfiles.com/embed-video/DP3NGKJNSX",
        ))
        .await
        .expect("the embed link resolves");

    assert_eq!(host.request(0).url.as_str(), FILE_PAGE_URL);
}

/// The site's 404 page is the file's absence, reported as such - no widget is spent on it.
#[tokio::test]
async fn a_deleted_file_is_reported_by_code() {
    let host = MockHost::new(vec![html(404, ERROR_PAGE)], Some("unused"));
    let resolver = KrakenfilesResolver::new(host.clone());

    let failure = resolver
        .resolve(resolve_request(CANONICAL_LINK))
        .await
        .expect_err("a deleted file fails");

    assert_eq!(failure.category, FailureKind::Offline);
    assert_eq!(
        failure.code.as_deref(),
        Some("krakenfiles.file_unavailable")
    );
    assert_eq!(host.request_count(), 1);
    assert_eq!(host.captcha_count(), 0);
}

/// The same notice under a 200 is the same absence.
#[tokio::test]
async fn the_gone_notice_counts_whatever_the_status() {
    let host = MockHost::new(vec![html(200, ERROR_PAGE)], Some("unused"));
    let resolver = KrakenfilesResolver::new(host);

    let failure = resolver
        .resolve(resolve_request(CANONICAL_LINK))
        .await
        .expect_err("a deleted file fails");

    assert_eq!(
        failure.code.as_deref(),
        Some("krakenfiles.file_unavailable")
    );
}

/// "captcha not valid" is tried once more from the top - a fresh page, token and challenge -
/// and reported as a rejected captcha the second time.
#[tokio::test]
async fn a_rejected_captcha_is_retried_from_the_top_then_reported() {
    let host = MockHost::new(
        vec![
            html(200, FILE_PAGE),
            json(500, CAPTCHA_INVALID),
            html(200, FILE_PAGE),
            json(500, CAPTCHA_INVALID),
        ],
        Some("turnstile-token"),
    );
    let resolver = KrakenfilesResolver::new(host.clone());

    let failure = resolver
        .resolve(resolve_request(CANONICAL_LINK))
        .await
        .expect_err("a twice-rejected captcha fails");

    assert_eq!(failure.category, FailureKind::CaptchaFailed);
    assert_eq!(
        failure.code.as_deref(),
        Some("krakenfiles.captcha_rejected")
    );
    assert_eq!(host.request_count(), 4, "page, post, page, post");
    assert_eq!(
        host.captcha_count(),
        2,
        "the retry fetches a fresh challenge"
    );
}

#[tokio::test]
async fn a_rejected_captcha_answered_well_the_second_time_resolves() {
    let host = MockHost::new(
        vec![
            html(200, FILE_PAGE),
            json(500, CAPTCHA_INVALID),
            html(200, FILE_PAGE),
            json(200, DOWNLOAD_OK),
            file(206),
        ],
        Some("turnstile-token"),
    );
    let resolver = KrakenfilesResolver::new(host.clone());

    let resolved = resolver
        .resolve(resolve_request(CANONICAL_LINK))
        .await
        .expect("the second attempt resolves");

    assert_eq!(resolved.url.as_str(), DIRECT_LINK);
    assert_eq!(host.request_count(), 5);
    assert_eq!(host.captcha_count(), 2);
}

/// An accepted post without a link is a coded failure, never the page URL handed on as the
/// download - the outcome that would save HTML under the file's name.
#[tokio::test]
async fn an_ok_answer_without_a_link_is_a_code_never_the_page() {
    for answer in [r#"{"status":"ok","url":"","msg":""}"#, r#"{"status":"ok"}"#] {
        let host = MockHost::new(
            vec![html(200, FILE_PAGE), json(200, answer)],
            Some("turnstile-token"),
        );
        let resolver = KrakenfilesResolver::new(host.clone());

        let failure = resolver
            .resolve(resolve_request(CANONICAL_LINK))
            .await
            .expect_err("no link, no download");

        assert_eq!(
            failure.category,
            FailureKind::Transient {
                retry_after_seconds: None
            }
        );
        assert_eq!(
            failure.code.as_deref(),
            Some("krakenfiles.direct_link_missing"),
            "{answer}"
        );
        assert_eq!(host.request_count(), 2, "nothing is probed: {answer}");
    }
}

/// Any other refusal is temporary in JDownloader's reading, and the site's words travel.
#[tokio::test]
async fn another_refusal_carries_the_sites_wording() {
    let host = MockHost::new(
        vec![
            html(200, FILE_PAGE),
            json(
                500,
                "{\"status\":\"error\",\"url\":\"\",\"msg\":\"Server  overloaded.\\n Please try again later.\"}",
            ),
        ],
        Some("turnstile-token"),
    );
    let resolver = KrakenfilesResolver::new(host);

    let failure = resolver
        .resolve(resolve_request(CANONICAL_LINK))
        .await
        .expect_err("refused");

    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: None
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("krakenfiles.download_refused")
    );
    assert_eq!(
        failure.params.get("message").map(String::as_str),
        Some("Server overloaded. Please try again later.")
    );
}

/// A link on a host the manifest does not allow is refused here, not followed.
#[tokio::test]
async fn a_link_outside_the_download_domains_is_refused() {
    let host = MockHost::new(
        vec![
            html(200, FILE_PAGE),
            json(
                200,
                r#"{"status":"ok","url":"https://cdn.example.net/file.rar"}"#,
            ),
        ],
        Some("turnstile-token"),
    );
    let resolver = KrakenfilesResolver::new(host.clone());

    let failure = resolver
        .resolve(resolve_request(CANONICAL_LINK))
        .await
        .expect_err("foreign host");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(
        failure.code.as_deref(),
        Some("krakenfiles.direct_link_foreign")
    );
    assert_eq!(
        failure.params.get("host").map(String::as_str),
        Some("cdn.example.net")
    );
    assert_eq!(
        host.request_count(),
        2,
        "the foreign link is never requested"
    );
}

/// 403, 404 and 405 on the direct link are one thing in JDownloader's reading - too many
/// connections, or a link that has expired - and the answer is to come back in an hour.
#[tokio::test]
async fn a_refused_or_expired_direct_link_is_a_rate_limit_for_an_hour() {
    for status in [403_u16, 404, 405] {
        let host = MockHost::new(
            vec![html(200, FILE_PAGE), json(200, DOWNLOAD_OK), file(status)],
            Some("turnstile-token"),
        );
        let resolver = KrakenfilesResolver::new(host);

        let failure = resolver
            .resolve(resolve_request(CANONICAL_LINK))
            .await
            .expect_err("refused link");

        assert_eq!(
            failure.category,
            FailureKind::RateLimited {
                retry_after_seconds: Some(3600)
            },
            "{status}"
        );
        assert_eq!(failure.code.as_deref(), Some("krakenfiles.rate_limited"));
        assert_eq!(
            failure.params.get("status").map(String::as_str),
            Some(status.to_string().as_str())
        );
    }
}

/// A page that is not the file page this plugin knows is a plugin/site mismatch, named.
#[tokio::test]
async fn a_page_without_the_form_reports_the_layout() {
    let host = MockHost::new(
        vec![html(
            200,
            "<html><head><title>Maintenance - Krakenfiles.com</title></head><body></body></html>",
        )],
        Some("unused"),
    );
    let resolver = KrakenfilesResolver::new(host.clone());

    let failure = resolver
        .resolve(resolve_request(CANONICAL_LINK))
        .await
        .expect_err("no form");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(
        failure.code.as_deref(),
        Some("krakenfiles.page_layout_changed")
    );
    assert_eq!(
        failure.params.get("diagnosis").map(String::as_str),
        Some("no download form on the page: page \"Maintenance - Krakenfiles.com\"")
    );
    assert_eq!(host.captcha_count(), 0);
}

/// Without a solver the flow reports the captcha; it never posts the form regardless.
#[tokio::test]
async fn a_captcha_without_a_solver_is_reported_verbatim() {
    let host = MockHost::new(vec![html(200, FILE_PAGE)], None);
    let resolver = KrakenfilesResolver::new(host.clone());

    let failure = resolver
        .resolve(resolve_request(CANONICAL_LINK))
        .await
        .expect_err("no solver");

    assert_eq!(failure.category, FailureKind::NeedsCaptcha);
    assert_eq!(failure.code.as_deref(), Some("captcha.no_solver"));
    assert_eq!(
        host.request_count(),
        1,
        "the form is not posted without an answer"
    );
}

/// A body that is not JSON is judged by its status: a 5xx is the site's trouble, a 200 that
/// is not the answer is an unreadable answer.
#[tokio::test]
async fn an_answer_that_is_not_json_is_judged_by_its_status() {
    let host = MockHost::new(
        vec![
            html(200, FILE_PAGE),
            html(502, "<title>Bad gateway</title>"),
        ],
        Some("turnstile-token"),
    );
    let failure = KrakenfilesResolver::new(host)
        .resolve(resolve_request(CANONICAL_LINK))
        .await
        .expect_err("502");
    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: None
        }
    );
    assert_eq!(failure.code.as_deref(), Some("krakenfiles.http_error"));
    assert_eq!(
        failure.params.get("status").map(String::as_str),
        Some("502")
    );

    let host = MockHost::new(
        vec![html(200, FILE_PAGE), html(200, "<title>Not JSON</title>")],
        Some("turnstile-token"),
    );
    let failure = KrakenfilesResolver::new(host)
        .resolve(resolve_request(CANONICAL_LINK))
        .await
        .expect_err("not JSON");
    assert_eq!(
        failure.code.as_deref(),
        Some("krakenfiles.invalid_response")
    );
}

/// Nothing survives a restart: a second resolve of the same link, on a fresh process, starts
/// at the page again and earns its own token and challenge.
#[tokio::test]
async fn a_resolve_after_a_restart_starts_from_the_page_again() {
    for _restart in 0..2 {
        let host = MockHost::new(
            vec![html(200, FILE_PAGE), json(200, DOWNLOAD_OK), file(206)],
            Some("turnstile-token"),
        );
        let resolver = KrakenfilesResolver::new(host.clone());
        resolver
            .resolve(resolve_request(CANONICAL_LINK))
            .await
            .expect("resolves");
        assert_eq!(host.request_count(), 3);
        assert_eq!(host.captcha_count(), 1);
    }
}

/// An account on the request cannot belong to this provider; the flow is the same.
#[tokio::test]
async fn an_account_on_the_request_is_ignored() {
    let host = MockHost::new(
        vec![html(200, FILE_PAGE), json(200, DOWNLOAD_OK), file(206)],
        Some("turnstile-token"),
    );
    let resolver = KrakenfilesResolver::new(host.clone());

    let resolved = resolver
        .resolve(ResolveRequest {
            url: CANONICAL_LINK.parse().expect("url"),
            client: ClientIdentity {
                account_id: Some(rd_core::AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 3,
            },
        })
        .await
        .expect("resolves");

    assert_eq!(resolved.url.as_str(), DIRECT_LINK);
    assert_eq!(
        resolved.client.tls_revision, 3,
        "the identity comes back untouched"
    );
}

/// An unsupported link is refused before any request is made.
#[tokio::test]
async fn an_unsupported_link_is_refused_without_a_request() {
    let host = MockHost::new(Vec::new(), Some("unused"));
    let resolver = KrakenfilesResolver::new(host.clone());

    let failure = resolver
        .resolve(resolve_request("https://krakenfiles.com/view/DP3nGKJNsX"))
        .await
        .expect_err("short form");

    assert_eq!(failure.category, FailureKind::Unsupported);
    assert_eq!(
        failure.code.as_deref(),
        Some("krakenfiles.unsupported_link")
    );
    assert_eq!(host.request_count(), 0);
}
