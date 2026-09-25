//! Account-less (free) resolve coverage for Rapidgator's website flow.
//!
//! Mirrors JD's `RapidGatorNet.handleDownloadWebsite` step by step: file page -> timer start ->
//! captcha solved and countdown waited out -> `AjaxGetDownloadLink` -> `/download/captcha` posted
//! -> final link. Each test drives the real resolver against queued mock responses and asserts
//! what the plugin *did* (which requests, which headers, which captcha, which wait), not only
//! what it returned.

use rd_core::FailureKind;
use rd_plugin_api::{CaptchaChallenge, ClientIdentity, ResolveRequest, Resolver};

use super::{MockHost, RapidgatorResolver};

/// The account-less request the free flow answers.
fn free_request() -> ResolveRequest {
    ResolveRequest {
        url: super::FILE_URL.parse().expect("URL"),
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

fn json(url: &str, body: &str) -> rd_plugin_api::HostHttpResponse {
    rd_plugin_api::HostHttpResponse {
        status: 200,
        final_url: url.parse().expect("URL"),
        headers: vec![rd_plugin_api::ResolvedHeader {
            name: "Content-Type".to_owned(),
            value: "application/json".to_owned(),
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

const PAGE_URL: &str = "https://rapidgator.net/file/123456";
const CAPTCHA_URL: &str = "https://rapidgator.net/download/captcha";
const FINAL_URL: &str = "https://pr7.rapidgator.net//?r=download/index&session_id=Ab12Cd34";

/// The file page: the three countdown markers plus the reCAPTCHA widget.
const FILE_PAGE: &str = r#"<html><head><title>Rapidgator - release.rar</title></head><body>
<script>
  var fid = 7654321;
  var secs = 30;
  var startTimerUrl = '/download/AjaxStartTimer';
</script>
<div class="g-recaptcha" data-sitekey="6Lc-free-key"></div>
</body></html>"#;

/// `/download/captcha`, carrying the form JD looks up by `id="captchaform"`.
const CAPTCHA_PAGE: &str = r#"<html><body>
<form id="captchaform" method="post" action="/download/captcha">
<input type="hidden" name="DownloadCaptchaForm[captchaType]" value="recaptcha">
<input type="hidden" name="DownloadCaptchaForm[verifyCode]" value="">
<div class="g-recaptcha" data-sitekey="6Lc-free-key"></div>
</form></body></html>"#;

const LINK_PAGE: &str = r#"<script>window.open('https://pr7.rapidgator.net//?r=download/index&amp;session_id=Ab12Cd34');</script>"#;

fn happy_path_responses() -> Vec<rd_plugin_api::HostHttpResponse> {
    vec![
        html(PAGE_URL, FILE_PAGE),
        json(
            "https://rapidgator.net/download/AjaxStartTimer",
            r#"{"state":"started","sid":"s1d-value"}"#,
        ),
        json(
            "https://rapidgator.net/download/AjaxGetDownloadLink",
            r#"{"state":"done"}"#,
        ),
        html(CAPTCHA_URL, CAPTCHA_PAGE),
        html(CAPTCHA_URL, LINK_PAGE),
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

fn query_of(request: &rd_plugin_api::HostHttpRequest, name: &str) -> Option<String> {
    request
        .query
        .iter()
        .find(|value| value.name == name)
        .map(|value| value.value_template.clone())
}

fn body_of(request: &rd_plugin_api::HostHttpRequest) -> String {
    String::from_utf8_lossy(&request.body).into_owned()
}

#[tokio::test]
async fn the_free_flow_starts_the_timer_solves_the_captcha_then_waits_and_posts_the_form() {
    let host = MockHost::free(happy_path_responses(), Some("captcha-token"));
    let resolver = RapidgatorResolver::new(host.clone());

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
        Some("https://rapidgator.net/")
    );

    let requests = host.requests.lock().expect("lock");
    assert_eq!(
        requests.len(),
        6,
        "file page, timer start, AjaxGetDownloadLink, captcha page, captcha post, transfer"
    );

    // 1. The file page is fetched on the main domain, from the link's file id.
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].url.as_str(), PAGE_URL);

    // 2. The timer-start URL is joined against the page and carries `fid` plus the XHR header.
    assert_eq!(
        requests[1].url.as_str(),
        "https://rapidgator.net/download/AjaxStartTimer"
    );
    assert_eq!(query_of(&requests[1], "fid").as_deref(), Some("7654321"));
    assert_eq!(
        header_of(&requests[1], "X-Requested-With").as_deref(),
        Some("XMLHttpRequest"),
        "the countdown endpoint only answers XMLHttpRequest calls"
    );

    // 3. The countdown session id is carried into `AjaxGetDownloadLink`, again as an XHR.
    assert_eq!(
        requests[2].url.as_str(),
        "https://rapidgator.net/download/AjaxGetDownloadLink"
    );
    assert_eq!(query_of(&requests[2], "sid").as_deref(), Some("s1d-value"));
    assert_eq!(
        header_of(&requests[2], "X-Requested-With").as_deref(),
        Some("XMLHttpRequest")
    );

    // 4. + 5. The captcha page is fetched and its form posted with a matching Referer.
    assert_eq!(requests[3].url.as_str(), CAPTCHA_URL);
    assert_eq!(requests[4].method, "POST");
    assert_eq!(requests[4].url.as_str(), CAPTCHA_URL);
    assert_eq!(
        header_of(&requests[4], "Referer").as_deref(),
        Some(CAPTCHA_URL)
    );
    let posted = body_of(&requests[4]);
    // The token reaches both fields JD fills, and the form's own field survives.
    assert!(
        posted.contains("DownloadCaptchaForm%5BverifyCode%5D=captcha-token"),
        "{posted}"
    );
    assert!(
        posted.contains("g-recaptcha-response=captcha-token"),
        "{posted}"
    );
    assert!(
        posted.contains("DownloadCaptchaForm%5BcaptchaType%5D="),
        "{posted}"
    );

    // The countdown was waited out, and the captcha was solved before it (JD's ordering, so the
    // token is as fresh as possible when the form is finally posted).
    assert_eq!(*host.waits.lock().expect("lock"), vec![30]);
    assert_eq!(
        *host.order.lock().expect("lock"),
        vec!["captcha", "wait"],
        "the captcha must be solved before the countdown is waited out"
    );
    let captchas = host.captchas.lock().expect("lock");
    assert_eq!(captchas.len(), 1, "one captcha per download, no re-solve");
    match &captchas[0] {
        CaptchaChallenge::RecaptchaV2(widget) => {
            assert_eq!(widget.site_key, "6Lc-free-key");
            assert_eq!(widget.page_url, PAGE_URL);
            assert!(!widget.invisible);
        }
        other => panic!("expected a reCAPTCHA v2 challenge, got {other:?}"),
    }
}

/// A hoster that serves the file straight from the file page needs neither countdown nor captcha.
#[tokio::test]
async fn a_hotlink_short_circuits_the_whole_flow() {
    let host = MockHost::free(
        vec![file("https://pr7.rapidgator.net/d/tok3n/release.rar")],
        Some("unused"),
    );
    let resolver = RapidgatorResolver::new(host.clone());

    let resolved = resolver.resolve(free_request()).await.expect("hotlink");

    assert_eq!(
        resolved.url.as_str(),
        "https://pr7.rapidgator.net/d/tok3n/release.rar"
    );
    assert_eq!(host.requests.lock().expect("lock").len(), 1);
    assert!(host.captchas.lock().expect("lock").is_empty());
    assert!(host.waits.lock().expect("lock").is_empty());
}

/// A stated limit must be reported as `IpBlocked` with the parsed wait, so the scheduler holds
/// back this hoster's other free links instead of burning a paid captcha on each of them.
#[tokio::test]
async fn a_stated_limit_becomes_an_ip_block_without_spending_a_captcha() {
    let host = MockHost::free(
        vec![html(
            PAGE_URL,
            "<div class=\"error\">Delay between downloads must be not less than 120 min.</div>",
        )],
        Some("unused"),
    );
    let resolver = RapidgatorResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("a limit must fail");

    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(120 * 60)
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("rapidgator.free_limit_reached")
    );
    assert_eq!(
        failure.params.get("wait_seconds").map(String::as_str),
        Some("7200")
    );
    assert!(
        host.captchas.lock().expect("lock").is_empty(),
        "no captcha may be spent on a link that is blocked anyway"
    );
    assert!(host.waits.lock().expect("lock").is_empty());
}

/// A daily-limit notice states no duration; the hold-off is then the scheduler's own.
#[tokio::test]
async fn a_limit_without_a_duration_still_blocks_the_hoster() {
    let host = MockHost::free(
        vec![html(
            PAGE_URL,
            "<p>Error. Link expired. You have reached your daily limit of downloads.</p>",
        )],
        Some("unused"),
    );
    let resolver = RapidgatorResolver::new(host);

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
    assert!(!failure.params.contains_key("wait_seconds"));
}

/// Without a solver the flow must report the captcha verbatim, not wait and post regardless.
#[tokio::test]
async fn a_captcha_without_a_solver_is_reported_verbatim() {
    let mut responses = happy_path_responses();
    responses.truncate(2);
    let host = MockHost::free(responses, None);
    let resolver = RapidgatorResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("no solver configured");

    assert_eq!(failure.category, FailureKind::NeedsCaptcha);
    assert_eq!(failure.code.as_deref(), Some("captcha.no_solver"));
    assert_eq!(
        host.requests.lock().expect("lock").len(),
        2,
        "the flow must stop at the timer start, before waiting or posting anything"
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
    responses[4] = html(
        CAPTCHA_URL,
        "<div>Please fix the following input errors</div>",
    );
    let host = MockHost::free(responses, Some("captcha-token"));
    let resolver = RapidgatorResolver::new(host);

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("a rejected captcha fails");

    assert_eq!(failure.category, FailureKind::CaptchaFailed);
    assert_eq!(failure.code.as_deref(), Some("rapidgator.captcha_rejected"));
}

/// A page whose countdown markers are gone is a plugin/site mismatch, reported with the page's
/// own wording rather than as a silently wrong URL.
#[tokio::test]
async fn a_page_without_the_countdown_markers_reports_a_diagnosis() {
    let host = MockHost::free(
        vec![html(
            PAGE_URL,
            "<html><head><title>Rapidgator - File not found</title></head><body></body></html>",
        )],
        Some("unused"),
    );
    let resolver = RapidgatorResolver::new(host);

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("nothing to work with");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("rapidgator.no_free_markers"));
    assert!(
        failure
            .params
            .get("diagnosis")
            .is_some_and(|diagnosis| diagnosis.contains("File not found")),
        "the page's own wording must reach the user: {:?}",
        failure.params
    );
}

/// The countdown session failing to start is transient: the link is fine, a fresh attempt opens a
/// new session.
#[tokio::test]
async fn a_countdown_that_does_not_start_is_transient() {
    let mut responses = happy_path_responses();
    responses[1] = json(
        "https://rapidgator.net/download/AjaxStartTimer",
        r#"{"state":"error","code":7}"#,
    );
    let host = MockHost::free(responses, Some("captcha-token"));
    let resolver = RapidgatorResolver::new(host.clone());

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
        Some("rapidgator.timer_not_started")
    );
    assert_eq!(
        failure.params.get("state").map(String::as_str),
        Some("error")
    );
    assert!(
        host.captchas.lock().expect("lock").is_empty(),
        "the captcha is only worth solving once a countdown is running"
    );
}

/// A link on a foreign host is refused rather than followed — JD's own final-URL regex accepts
/// any domain, so this is the plugin's own guard against a drifted or injected page.
#[tokio::test]
async fn a_download_link_on_a_foreign_host_is_refused() {
    let mut responses = happy_path_responses();
    responses[4] = html(
        CAPTCHA_URL,
        "<script>window.open('https://evil.example//?r=download/index&session_id=Ab12');</script>",
    );
    let host = MockHost::free(responses, Some("captcha-token"));
    let resolver = RapidgatorResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("a foreign host must be refused");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(
        failure.code.as_deref(),
        Some("rapidgator.free_link_host_mismatch")
    );
    assert_eq!(
        failure.params.get("host").map(String::as_str),
        Some("evil.example")
    );
    assert_eq!(
        host.requests.lock().expect("lock").len(),
        5,
        "the foreign link must never be fetched"
    );
}

/// The free path must be reachable without any account at all — the whole point of the
/// `requires_account = false` metadata the host dispatches on.
#[test]
fn the_resolver_no_longer_requires_an_account() {
    let resolver = RapidgatorResolver::new(MockHost::free(Vec::new(), None));
    assert!(!resolver.metadata().requires_account);
}
