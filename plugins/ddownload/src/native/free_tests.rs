//! Account-less (free) resolve coverage for the XFS two-form flow — and for the one-form page
//! DDownload serves since 2026-09-17.
//!
//! Mirrors JD's `XFileSharingProBasic.doFree` step by step (the method `DdownloadCom.doFree`
//! delegates to): file page -> `download1` posted in free mode -> captcha solved and countdown
//! waited out -> `download2` posted -> direct link. Each test drives the real resolver against
//! queued mock responses and asserts what the plugin *did* (which requests, which captcha, which
//! wait), not just what it returned.

use rd_core::FailureKind;
use rd_plugin_api::{CaptchaChallenge, ClientIdentity, ResolveRequest, Resolver};

use super::super::DdownloadResolver;
use super::{
    ERROR_PAGE_2026_09_17, FILE_PAGE_2026_09_17, MockHost, TURNSTILE_SITE_KEY, file,
    file_page_without_the_form, html,
};

/// The account-less request the free flow answers.
fn free_request() -> ResolveRequest {
    ResolveRequest {
        url: "https://ddownload.com/abc123xyz/release.rar"
            .parse()
            .expect("URL"),
        client: ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

/// The file page's first-step form, carrying ddownload's own `adblock_detected` field.
const DOWNLOAD1_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download1">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="fname" value="release.rar">
<input type="hidden" name="method_free" value="Free Download">
<input type="hidden" name="method_premium" value="">
<input type="hidden" name="adblock_detected" value="1">
</form>"#;

/// The answer to `download1`: ddownload's own countdown marker, a captcha and the second form.
const DOWNLOAD2_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download2">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="rand" value="r4nd">
<input type="hidden" name="method_free" value="Free Download">
<input type="hidden" name="method_premium" value="">
<div class="g-recaptcha" data-sitekey="6Lc-free-key"></div>
</form>
<span id="dk2CountdownNum" class="cd">30</span>"#;

/// The final page carrying the direct link.
const LINK_PAGE: &str = r#"<a href="https://fs7.ddownload.com/d/tok3n/release.rar">Download</a>"#;

fn body_of(request: &rd_plugin_api::HostHttpRequest) -> String {
    String::from_utf8_lossy(&request.body).into_owned()
}

#[tokio::test]
async fn the_free_flow_solves_the_captcha_waits_and_posts_both_forms() {
    let host = MockHost::free(
        vec![
            html(DOWNLOAD1_PAGE),
            html(DOWNLOAD2_PAGE),
            html(LINK_PAGE),
            file("https://fs7.ddownload.com/d/tok3n/release.rar"),
        ],
        Some("captcha-token"),
    );
    let resolver = DdownloadResolver::new(host.clone());

    let resolved = resolver
        .resolve(free_request())
        .await
        .expect("free download resolves");

    assert_eq!(
        resolved.url.as_str(),
        "https://fs7.ddownload.com/d/tok3n/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    // The transfer must carry the referer, or the hoster refuses the direct link.
    assert_eq!(
        resolved
            .headers
            .iter()
            .find(|header| header.name == "Referer")
            .map(|header| header.value.as_str()),
        Some("https://ddownload.com/")
    );

    let requests = host.requests.lock().expect("lock");
    assert_eq!(requests.len(), 4, "page, download1, download2, transfer");
    // download1 is posted in free mode: the free marker survives, premium is gone, and
    // ddownload's own `adblock_detected` field is cleared (JD `DdownloadCom.handleCaptcha`).
    let step_one = body_of(&requests[1]);
    assert_eq!(requests[1].method, "POST");
    assert!(step_one.contains("op=download1"), "{step_one}");
    assert!(step_one.contains("method_free=Free+Download"), "{step_one}");
    assert!(!step_one.contains("method_premium"), "{step_one}");
    assert!(step_one.contains("adblock_detected=0"), "{step_one}");
    // download2 carries the captcha token.
    let step_two = body_of(&requests[2]);
    assert!(step_two.contains("op=download2"), "{step_two}");
    assert!(
        step_two.contains("g-recaptcha-response=captcha-token"),
        "{step_two}"
    );
    assert!(!step_two.contains("method_premium"), "{step_two}");

    // The countdown was waited out, and the captcha was solved before it (JD's ordering, so the
    // token is as fresh as possible when the form is posted).
    assert_eq!(*host.waits.lock().expect("lock"), vec![30]);
    let captchas = host.captchas.lock().expect("lock");
    assert_eq!(captchas.len(), 1);
    match &captchas[0] {
        CaptchaChallenge::RecaptchaV2(widget) => {
            assert_eq!(widget.site_key, "6Lc-free-key");
            assert!(widget.page_url.starts_with("https://ddownload.com/"));
        }
        other => panic!("expected a reCAPTCHA v2 challenge, got {other:?}"),
    }
}

/// The page measured on 2026-09-17: no `download1` form, the `download2` form directly on the
/// file page with a Turnstile widget inside it and the countdown beside it. The flow starts at
/// the second step, answers the widget, waits, and posts the form with the token.
#[tokio::test]
async fn the_new_file_page_is_downloaded_without_a_download1_step() {
    let host = MockHost::free(
        vec![
            html(FILE_PAGE_2026_09_17),
            html(LINK_PAGE),
            file("https://fs7.ddownload.com/d/tok3n/release.rar"),
        ],
        Some("turnstile-token"),
    );
    let resolver = DdownloadResolver::new(host.clone());

    let resolved = resolver
        .resolve(free_request())
        .await
        .expect("the one-form page resolves");

    assert_eq!(
        resolved.url.as_str(),
        "https://fs7.ddownload.com/d/tok3n/release.rar"
    );
    let requests = host.requests.lock().expect("lock");
    assert_eq!(
        requests.len(),
        3,
        "page, download2, transfer - no download1"
    );
    let step_two = body_of(&requests[1]);
    assert_eq!(requests[1].method, "POST");
    assert_eq!(
        requests[1].url.as_str(),
        "https://ddownload.com/abc123xyz/release.rar",
        "download2 is posted back to the file page"
    );
    assert!(step_two.contains("op=download2"), "{step_two}");
    assert!(step_two.contains("id=og21jsivxm1m"), "{step_two}");
    assert!(
        step_two.contains("rand=fq5eytcd4ppxoxbmje4ld2eaf6dwnypjnrsev44mza"),
        "{step_two}"
    );
    assert!(step_two.contains("method_free=Free+Download"), "{step_two}");
    assert!(!step_two.contains("method_premium"), "{step_two}");
    assert!(
        step_two.contains("cf-turnstile-response=turnstile-token"),
        "{step_two}"
    );
    assert_eq!(
        *host.waits.lock().expect("lock"),
        vec![60],
        "the page's dk2CountdownNum is waited out"
    );
    let captchas = host.captchas.lock().expect("lock");
    assert_eq!(captchas.len(), 1);
    match &captchas[0] {
        CaptchaChallenge::Turnstile(widget) => {
            assert_eq!(widget.site_key, TURNSTILE_SITE_KEY);
            assert_eq!(
                widget.page_url,
                "https://ddownload.com/abc123xyz/release.rar"
            );
        }
        other => panic!("expected a Turnstile challenge, got {other:?}"),
    }
}

/// A page that carries the header's login link and an error message, but no form, is
/// reported by its message. The link is on every page a guest sees; it proves nothing.
#[tokio::test]
async fn a_page_with_a_message_and_the_navigation_link_reports_the_message() {
    let host = MockHost::free(vec![html(ERROR_PAGE_2026_09_17)], Some("unused"));
    let resolver = DdownloadResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("no form to post");

    assert_eq!(failure.code.as_deref(), Some("ddownload.no_free_form"));
    assert_eq!(
        failure.params.get("diagnosis").map(String::as_str),
        Some("page message: The file was removed by administrator")
    );
    assert!(
        !failure.message.contains("requires a login"),
        "{}",
        failure.message
    );
    assert!(host.captchas.lock().expect("lock").is_empty());
}

/// The same header without any message: the page is named by its title, and the login link
/// in that header is still not taken for a login wall.
#[tokio::test]
async fn a_page_with_only_the_navigation_link_is_not_called_a_login_wall() {
    let host = MockHost::free(vec![html(&file_page_without_the_form())], Some("unused"));
    let resolver = DdownloadResolver::new(host);

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("no form to post");

    assert_eq!(failure.code.as_deref(), Some("ddownload.no_free_form"));
    let diagnosis = failure
        .params
        .get("diagnosis")
        .expect("the diagnosis travels with the failure");
    assert!(
        !diagnosis.contains("requires a login"),
        "a header link is not a login wall: {diagnosis}"
    );
    assert!(
        diagnosis.starts_with("page \"Download Adults 2025"),
        "{diagnosis}"
    );
}

/// A hoster that serves the file straight from the page needs neither form nor captcha.
#[tokio::test]
async fn a_hotlink_short_circuits_the_whole_flow() {
    let host = MockHost::free(
        vec![file("https://fs7.ddownload.com/d/tok3n/release.rar")],
        Some("unused"),
    );
    let resolver = DdownloadResolver::new(host.clone());

    let resolved = resolver.resolve(free_request()).await.expect("hotlink");

    assert_eq!(
        resolved.url.as_str(),
        "https://fs7.ddownload.com/d/tok3n/release.rar"
    );
    assert_eq!(host.requests.lock().expect("lock").len(), 1);
    assert!(host.captchas.lock().expect("lock").is_empty());
    assert!(host.waits.lock().expect("lock").is_empty());
}

/// An IP limit must be reported as `IpBlocked` with the stated wait, so the scheduler holds back
/// the hoster's other free links instead of burning a captcha on each of them.
#[tokio::test]
async fn a_stated_ip_limit_becomes_an_ip_block_without_solving_anything() {
    let host = MockHost::free(
        vec![html(
            "<div class=\"err\">You have to wait 45 minutes, 30 seconds till next download</div>",
        )],
        Some("unused"),
    );
    let resolver = DdownloadResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("an IP limit must fail");

    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: Some(45 * 60 + 30)
        }
    );
    assert_eq!(
        failure.code.as_deref(),
        Some("ddownload.free_limit_reached")
    );
    assert_eq!(
        failure.params.get("wait_seconds").map(String::as_str),
        Some("2730")
    );
    assert!(
        host.captchas.lock().expect("lock").is_empty(),
        "no captcha may be spent on a link that is blocked anyway"
    );
}

/// A limit without a stated duration still blocks the hoster; the delay is the scheduler's.
#[tokio::test]
async fn a_limit_without_a_duration_still_blocks_the_hoster() {
    let host = MockHost::free(
        vec![html("<p>You have reached the download-limit</p>")],
        Some("unused"),
    );
    let resolver = DdownloadResolver::new(host);

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("an IP limit must fail");

    assert_eq!(
        failure.category,
        FailureKind::IpBlocked {
            retry_after_seconds: None
        }
    );
}

/// Without a solver the flow must report the captcha, not post the form regardless.
#[tokio::test]
async fn a_captcha_without_a_solver_is_reported_verbatim() {
    let host = MockHost::free(vec![html(DOWNLOAD1_PAGE), html(DOWNLOAD2_PAGE)], None);
    let resolver = DdownloadResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("no solver configured");

    assert_eq!(failure.category, FailureKind::NeedsCaptcha);
    assert_eq!(failure.code.as_deref(), Some("captcha.no_solver"));
    assert_eq!(
        host.requests.lock().expect("lock").len(),
        2,
        "the second form must not be posted without a captcha answer"
    );
}

/// A rejected captcha is retried once with a fresh challenge, the way JD retries it.
#[tokio::test]
async fn a_rejected_captcha_is_retried_once_then_reported() {
    let rejected = format!("<div class=\"err\">Wrong captcha</div>{DOWNLOAD2_PAGE}");
    let host = MockHost::free(
        vec![
            html(DOWNLOAD1_PAGE),
            html(DOWNLOAD2_PAGE),
            html(&rejected),
            html(&rejected),
        ],
        Some("captcha-token"),
    );
    let resolver = DdownloadResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("a twice-rejected captcha fails");

    assert_eq!(failure.category, FailureKind::CaptchaFailed);
    assert_eq!(failure.code.as_deref(), Some("ddownload.captcha_rejected"));
    assert_eq!(
        host.captchas.lock().expect("lock").len(),
        2,
        "the retry must fetch a fresh challenge"
    );
}

/// A page with no usable form is a plugin/site mismatch, not something to retry forever.
#[tokio::test]
async fn a_page_without_a_form_reports_a_diagnosis() {
    let host = MockHost::free(
        vec![html("<title>DDownload - File not found</title>")],
        Some("unused"),
    );
    let resolver = DdownloadResolver::new(host);

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("no form to post");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("ddownload.no_free_form"));
    assert!(
        failure.params.contains_key("diagnosis"),
        "the page's own wording must reach the user"
    );
}

/// The free path must be reachable without any account at all — the whole point of the
/// `requires_account = false` metadata the host dispatches on.
#[test]
fn the_resolver_no_longer_requires_an_account() {
    let resolver = DdownloadResolver::new(MockHost::full(Vec::new(), false, false));
    assert!(!resolver.metadata().requires_account);
}
