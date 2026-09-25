//! Account-less (free) resolve coverage for the XFS two-form flow.
//!
//! Mirrors JD's `XFileSharingProBasic.doFree` step by step: file page -> `download1` posted
//! in free mode -> captcha solved and countdown waited out -> `download2` posted -> direct
//! link. Each test drives the real resolver against queued mock responses and asserts what
//! the plugin *did* (which requests, which captcha, which wait), not just what it returned.

use rd_core::FailureKind;
use rd_plugin_api::{CaptchaChallenge, ClientIdentity, ResolveRequest, Resolver};

use super::super::KatfileResolver;
use super::{MockHost, file, html};

/// The account-less request the free flow answers.
fn free_request() -> ResolveRequest {
    ResolveRequest {
        url: "https://katfile.com/abc123xyz/release.rar"
            .parse()
            .expect("URL"),
        client: ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

/// The file page's first-step form.
const DOWNLOAD1_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download1">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="fname" value="release.rar">
<input type="hidden" name="method_free" value="Free Download">
<input type="hidden" name="method_premium" value="">
</form>"#;

/// The answer to `download1`: a countdown, a captcha and the second-step form.
const DOWNLOAD2_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download2">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="rand" value="r4nd">
<input type="hidden" name="method_free" value="Free Download">
<input type="hidden" name="method_premium" value="">
<div class="g-recaptcha" data-sitekey="6Lc-free-key"></div>
</form>
<span id="countdown_str">Wait <span id="cd">30</span> seconds</span>"#;

/// The final page carrying the direct link.
const LINK_PAGE: &str = r#"<a href="https://fs7.katfile.biz/d/tok3n/release.rar">Download</a>"#;

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
            file("https://fs7.katfile.biz/d/tok3n/release.rar"),
        ],
        Some("captcha-token"),
    );
    let resolver = KatfileResolver::new(host.clone());

    let resolved = resolver
        .resolve(free_request())
        .await
        .expect("free download resolves");

    assert_eq!(
        resolved.url.as_str(),
        "https://fs7.katfile.biz/d/tok3n/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    // The transfer must carry the referer, or the hoster refuses the direct link.
    assert_eq!(
        resolved
            .headers
            .iter()
            .find(|header| header.name == "Referer")
            .map(|header| header.value.as_str()),
        Some("https://katfile.biz/")
    );

    let requests = host.requests.lock().expect("lock");
    assert_eq!(requests.len(), 4, "page, download1, download2, transfer");
    // The alias host is rewritten before the first request.
    assert_eq!(
        requests[0].url.host_str(),
        Some("katfile.biz"),
        "the katfile.com alias must be canonicalized first"
    );
    // download1 is posted in free mode: the free marker survives, premium is gone.
    let step_one = body_of(&requests[1]);
    assert_eq!(requests[1].method, "POST");
    assert!(step_one.contains("op=download1"), "{step_one}");
    assert!(step_one.contains("method_free=Free+Download"), "{step_one}");
    assert!(!step_one.contains("method_premium"), "{step_one}");
    // download2 carries the captcha token.
    let step_two = body_of(&requests[2]);
    assert!(step_two.contains("op=download2"), "{step_two}");
    assert!(
        step_two.contains("g-recaptcha-response=captcha-token"),
        "{step_two}"
    );

    // The countdown was waited out, and the captcha was solved before it (JD's ordering, so
    // the token is as fresh as possible when the form is posted).
    assert_eq!(*host.waits.lock().expect("lock"), vec![30]);
    let captchas = host.captchas.lock().expect("lock");
    assert_eq!(captchas.len(), 1);
    match &captchas[0] {
        CaptchaChallenge::RecaptchaV2(widget) => {
            assert_eq!(widget.site_key, "6Lc-free-key");
            assert!(widget.page_url.starts_with("https://katfile.biz/"));
        }
        other => panic!("expected a reCAPTCHA v2 challenge, got {other:?}"),
    }
}

/// A hoster that serves the file straight from the page needs neither form nor captcha.
#[tokio::test]
async fn a_hotlink_short_circuits_the_whole_flow() {
    let host = MockHost::free(
        vec![file("https://fs7.katfile.biz/d/tok3n/release.rar")],
        Some("unused"),
    );
    let resolver = KatfileResolver::new(host.clone());

    let resolved = resolver.resolve(free_request()).await.expect("hotlink");

    assert_eq!(
        resolved.url.as_str(),
        "https://fs7.katfile.biz/d/tok3n/release.rar"
    );
    assert_eq!(host.requests.lock().expect("lock").len(), 1);
    assert!(host.captchas.lock().expect("lock").is_empty());
    assert!(host.waits.lock().expect("lock").is_empty());
}

/// An IP limit must be reported as `IpBlocked` with the stated wait, so the scheduler holds
/// back the hoster's other free links instead of burning a captcha on each of them.
#[tokio::test]
async fn a_stated_ip_limit_becomes_an_ip_block_without_solving_anything() {
    let host = MockHost::free(
        vec![html(
            "<div class=\"err\">You have to wait 45 minutes, 30 seconds till next download</div>",
        )],
        Some("unused"),
    );
    let resolver = KatfileResolver::new(host.clone());

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
    assert_eq!(failure.code.as_deref(), Some("katfile.free_limit_reached"));
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
    let resolver = KatfileResolver::new(host);

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
    let resolver = KatfileResolver::new(host.clone());

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
    let resolver = KatfileResolver::new(host.clone());

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("a twice-rejected captcha fails");

    assert_eq!(failure.category, FailureKind::CaptchaFailed);
    assert_eq!(failure.code.as_deref(), Some("katfile.captcha_rejected"));
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
        vec![html("<title>KatFile - File not found</title>")],
        Some("unused"),
    );
    let resolver = KatfileResolver::new(host);

    let failure = resolver
        .resolve(free_request())
        .await
        .expect_err("no form to post");

    assert_eq!(failure.category, FailureKind::Permanent);
    assert_eq!(failure.code.as_deref(), Some("katfile.no_free_form"));
    assert!(
        failure.params.contains_key("diagnosis"),
        "the page's own wording must reach the user"
    );
}

/// The free path must be reachable without any account at all — the whole point of the
/// `requires_account = false` metadata the host dispatches on.
#[test]
fn the_resolver_no_longer_requires_an_account() {
    let resolver = KatfileResolver::new(MockHost::bare(false, false));
    assert!(!resolver.metadata().requires_account);
}
