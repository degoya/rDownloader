//! Account-less (free) resolve coverage for Nitroflare's website flow.
//!
//! Mirrors JD's `NitroFlareCom.handleFreeDownload` website branch step by step: file page ->
//! `method=startTimer` -> captcha solved and countdown waited out -> `method=fetchDownload` ->
//! direct link. Each test drives the real resolver against queued mock responses and asserts
//! what the plugin *did* (which requests, which headers, which captcha, which wait), not only
//! what it returned.

use rd_core::FailureKind;
use rd_plugin_api::{CaptchaChallenge, ClientIdentity, ResolveRequest, Resolver};

use super::{MockHost, NitroflareResolver};

const FILE_ID: &str = "ABCDEFGHIJ";
const PAGE_URL: &str = "https://nitroflare.com/view/ABCDEFGHIJ";
const AJAX_URL: &str = "https://nitroflare.com/ajax/freeDownload.php";
const FINAL_URL: &str = "https://cdn7.nitroflare.com/d/tok3n/release.rar";

/// The account-less request the free flow answers.
fn free_request() -> ResolveRequest {
    ResolveRequest {
        url: PAGE_URL.parse().expect("URL"),
        client: ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

fn html(url: &str, body: &str) -> rd_plugin_api::HostHttpResponse {
    rd_plugin_api::HostHttpResponse {
        status: 200,
        final_url: url.parse().expect("URL"),
        headers: vec![rd_plugin_api::ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "text/html; charset=UTF-8".to_owned(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

fn file(url: &str) -> rd_plugin_api::HostHttpResponse {
    rd_plugin_api::HostHttpResponse {
        status: 206,
        final_url: url.parse().expect("URL"),
        headers: vec![rd_plugin_api::ResolvedHeader {
            name: "Content-Disposition".to_owned(),
            value: "attachment; filename=release.rar".to_owned(),
        }],
        body: vec![0],
    }
}

/// The file page: the countdown JD reads plus the reCAPTCHA widget.
const FILE_PAGE: &str = r#"<html><head><title>Nitroflare - release.rar</title></head><body>
<div id="CountDownTimer" data-timer="45"></div>
<div class="g-recaptcha" data-sitekey="6Lc-nitro-key"></div>
</body></html>"#;

const LINK_FRAGMENT: &str = r#"<div class="row"><a href="https://cdn7.nitroflare.com/d/tok3n/release.rar">Click here to download</a></div>"#;

fn happy_path_responses() -> Vec<rd_plugin_api::HostHttpResponse> {
    vec![
        html(PAGE_URL, FILE_PAGE),
        html(AJAX_URL, "1"),
        html(AJAX_URL, LINK_FRAGMENT),
        file(FINAL_URL),
    ]
}

fn header_of(request: &rd_plugin_api::HostHttpRequest, name: &str) -> Option<String> {
    request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value_template.clone())
}

fn body_of(request: &rd_plugin_api::HostHttpRequest) -> String {
    String::from_utf8_lossy(&request.body).into_owned()
}

#[tokio::test]
async fn the_free_flow_starts_the_timer_solves_the_captcha_then_waits_and_fetches_the_link() {
    let host = MockHost::free(happy_path_responses(), Some("captcha-token"));
    let resolver = NitroflareResolver::new(host.clone());

    let resolved = resolver
        .resolve(free_request())
        .await
        .expect("free download resolves");

    assert_eq!(resolved.url.as_str(), FINAL_URL);
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    // The transfer must carry the referer, or the hoster refuses the download link.
    assert_eq!(
        resolved
            .headers
            .iter()
            .find(|header| header.name == "Referer")
            .map(|header| header.value.as_str()),
        Some("https://nitroflare.com/")
    );

    let requests = host.requests.lock().expect("lock");
    assert_eq!(
        requests.len(),
        4,
        "file page, startTimer, fetchDownload, transfer"
    );

    // 1. The file page establishes the session the two ajax posts rely on.
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].url.as_str(), PAGE_URL);

    // 2. `startTimer` posts the file id with the XHR header the endpoint requires.
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].url.as_str(), AJAX_URL);
    assert_eq!(
        header_of(&requests[1], "X-Requested-With").as_deref(),
        Some("XMLHttpRequest"),
        "the ajax endpoint only answers XMLHttpRequest calls"
    );
    assert_eq!(
        header_of(&requests[1], "Referer").as_deref(),
        Some(PAGE_URL)
    );
    assert_eq!(
        body_of(&requests[1]),
        format!("method=startTimer&fileId={FILE_ID}")
    );

    // 3. `fetchDownload` carries the token under both names JD sends it under.
    assert_eq!(requests[2].method, "POST");
    assert_eq!(requests[2].url.as_str(), AJAX_URL);
    assert_eq!(
        header_of(&requests[2], "X-Requested-With").as_deref(),
        Some("XMLHttpRequest")
    );
    let posted = body_of(&requests[2]);
    assert!(posted.starts_with("method=fetchDownload"), "{posted}");
    assert!(posted.contains("captcha=captcha-token"), "{posted}");
    assert!(
        posted.contains("g-recaptcha-response=captcha-token"),
        "{posted}"
    );

    // The countdown was waited out, and the captcha was solved before it (JD's ordering, so the
    // token is as fresh as possible when it is finally submitted).
    assert_eq!(*host.waits.lock().expect("lock"), vec![45]);
    assert_eq!(
        *host.order.lock().expect("lock"),
        vec!["captcha", "wait"],
        "the captcha must be solved before the countdown is waited out"
    );
    let captchas = host.captchas.lock().expect("lock");
    assert_eq!(captchas.len(), 1, "one captcha per download");
    match &captchas[0] {
        CaptchaChallenge::RecaptchaV2(widget) => {
            assert_eq!(widget.site_key, "6Lc-nitro-key");
            assert_eq!(widget.page_url, PAGE_URL);
            assert!(!widget.invisible);
        }
        other => panic!("expected a reCAPTCHA v2 challenge, got {other:?}"),
    }
}

/// A page without a countdown is not an error: JD falls back to 60 seconds.
#[tokio::test]
async fn a_page_without_a_countdown_falls_back_to_sixty_seconds() {
    let mut responses = happy_path_responses();
    responses[0] = html(
        PAGE_URL,
        r#"<div class="g-recaptcha" data-sitekey="6Lc-nitro-key"></div>"#,
    );
    let host = MockHost::free(responses, Some("captcha-token"));
    let resolver = NitroflareResolver::new(host.clone());

    resolver.resolve(free_request()).await.expect("resolves");

    assert_eq!(*host.waits.lock().expect("lock"), vec![60]);
}

/// A stated limit must be reported as `IpBlocked` with the parsed wait, so the scheduler holds
/// back this hoster's other free links instead of burning a paid captcha on each of them.
#[tokio::test]
async fn a_stated_limit_becomes_an_ip_block_without_spending_a_captcha() {
    let host = MockHost::free(
        vec![html(
            PAGE_URL,
            "Free downloading is not possible. You have to wait 70 minutes to download your next file.",
        )],
        Some("unused"),
    );
    let resolver = NitroflareResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("a limit must fail");

    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(70 * 60)
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("nitroflare.free_limit_reached")
    );
    assert_eq!(
        failure.params.get("wait_seconds").map(String::as_str),
        Some("4200")
    );
    assert!(
        host.captchas.lock().expect("lock").is_empty(),
        "no captcha may be spent on a link that is blocked anyway"
    );
    assert!(host.waits.lock().expect("lock").is_empty());
}

/// The same limit stated by the `startTimer` answer rather than by the file page.
#[tokio::test]
async fn a_limit_reported_by_the_timer_answer_also_becomes_an_ip_block() {
    let mut responses = happy_path_responses();
    responses[1] = html(AJAX_URL, "Free downloading is not possible.");
    let host = MockHost::free(responses, Some("unused"));
    let resolver = NitroflareResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("a limit must fail");

    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: None
        }
    );
    assert!(
        host.captchas.lock().expect("lock").is_empty(),
        "the captcha is only solved once the countdown is running"
    );
}

/// Without a solver the flow must report the captcha verbatim, not wait and post regardless.
#[tokio::test]
async fn a_captcha_without_a_solver_is_reported_verbatim() {
    let mut responses = happy_path_responses();
    responses.truncate(2);
    let host = MockHost::free(responses, None);
    let resolver = NitroflareResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("no solver configured");

    assert_eq!(failure.category, FailureKind::NeedsCaptcha);
    assert_eq!(failure.code.as_deref(), Some("captcha.no_solver"));
    assert_eq!(
        host.requests.lock().expect("lock").len(),
        2,
        "the flow must stop after the timer start, before waiting or fetching anything"
    );
    assert!(
        host.waits.lock().expect("lock").is_empty(),
        "no countdown may be waited out for a captcha that was never solved"
    );
}

/// A rejected captcha answer is a clear, coded failure rather than a missing-link diagnosis.
#[tokio::test]
async fn a_rejected_captcha_is_reported_as_a_captcha_failure() {
    let mut responses = happy_path_responses();
    responses[2] = html(AJAX_URL, "The captcha wasn't entered correctly");
    let host = MockHost::free(responses, Some("captcha-token"));
    let resolver = NitroflareResolver::new(host);

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("a rejected captcha fails");

    assert_eq!(failure.category, FailureKind::CaptchaFailed);
    assert_eq!(failure.code.as_deref(), Some("nitroflare.captcha_rejected"));
}

/// A file page with no captcha widget is a plugin/site mismatch, reported with the page's own
/// wording rather than as a silently wrong URL.
#[tokio::test]
async fn a_page_without_a_captcha_widget_reports_a_diagnosis() {
    let host = MockHost::free(
        vec![html(
            PAGE_URL,
            "<html><head><title>Nitroflare - File doesn't exist</title></head><body></body></html>",
        )],
        Some("unused"),
    );
    let resolver = NitroflareResolver::new(host);

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("nothing to solve");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("nitroflare.no_free_markers"));
    assert!(
        failure
            .params
            .get("diagnosis")
            .is_some_and(|diagnosis| diagnosis.contains("File doesn't exist")),
        "the page's own wording must reach the user: {:?}",
        failure.params
    );
}

/// An unrecognised timer answer is transient: the link is fine, a fresh attempt opens a new
/// countdown.
#[tokio::test]
async fn an_unrecognised_timer_answer_is_transient() {
    let mut responses = happy_path_responses();
    responses[1] = html(AJAX_URL, "<b>maintenance</b>");
    let host = MockHost::free(responses, Some("captcha-token"));
    let resolver = NitroflareResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("the countdown never started");

    assert_eq!(
        failure.category,
        FailureKind::Transient {
            retry_after_seconds: Some(60)
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("nitroflare.timer_not_started")
    );
    assert!(failure.params.contains_key("answer"));
    assert!(
        host.captchas.lock().expect("lock").is_empty(),
        "the captcha is only worth solving once a countdown is running"
    );
}

/// A premium-only file is reported as such instead of being waited and solved for.
#[tokio::test]
async fn a_premium_only_file_is_reported_before_anything_is_spent() {
    let host = MockHost::free(
        vec![html(
            PAGE_URL,
            "<div>This file is available with premium key only</div>",
        )],
        Some("unused"),
    );
    let resolver = NitroflareResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("premium-only file");

    assert_eq!(failure.category, FailureKind::AuthRequired);
    assert_eq!(failure.code.as_deref(), Some("nitroflare.premium_required"));
    assert!(host.captchas.lock().expect("lock").is_empty());
}

/// A link on a foreign host is refused rather than followed.
#[tokio::test]
async fn a_download_link_on_a_foreign_host_is_refused() {
    let mut responses = happy_path_responses();
    responses[2] = html(
        AJAX_URL,
        r#"<a href="https://evil.example/get/abc">Click here to download</a>"#,
    );
    let host = MockHost::free(responses, Some("captcha-token"));
    let resolver = NitroflareResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("a foreign host must be refused");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(
        failure.code.as_deref(),
        Some("nitroflare.free_link_host_mismatch")
    );
    assert_eq!(
        failure.params.get("host").map(String::as_str),
        Some("evil.example")
    );
    assert_eq!(
        host.requests.lock().expect("lock").len(),
        3,
        "the foreign link must never be fetched"
    );
}

/// The free path must be reachable without any account at all — the whole point of the
/// `requires_account = false` metadata the host dispatches on.
#[test]
fn the_resolver_no_longer_requires_an_account() {
    let resolver = NitroflareResolver::new(MockHost::free(Vec::new(), None));
    assert!(!resolver.metadata().requires_account);
}
