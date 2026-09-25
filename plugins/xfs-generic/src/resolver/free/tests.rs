//! The free flow, driven against a mock [`PluginHost`].
//!
//! Deliberately not the `Resolver` trait: while the domain list is empty, `matches()` and
//! `resolve()` refuse every link by design, so testing through them would only ever exercise the
//! refusal. [`super::resolve`] takes the parsed URL and file code as arguments, which is exactly
//! the seam that lets the flow itself be tested before a single clone has been vetted. When a
//! verified domain is added, these tests keep working unchanged.
//!
//! The pages below are the XFS script's own shapes, the ones `ddownload`, `katfile` and
//! `filejoker` already parse: a `download1` form on the file page, a `download2` form with a
//! countdown and a captcha widget on the answer, and a direct link on the last page.

use std::{cell::RefCell, collections::VecDeque};

use plugin_common::{
    CaptchaAnswer, CaptchaChallenge, CaptchaSolution, Failure, FailureKind, HttpRequest,
    HttpResponse, PluginHost, ResolveInput,
};
use url::Url;

const LINK: &str = "https://clone.test/abc123xyz/release.rar";
const CODE: &str = "abc123xyz";

const FILE_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download1">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="fname" value="release.rar">
<input type="hidden" name="method_free" value="Free Download">
<input type="hidden" name="method_premium" value="Premium Download">
</form>"#;

const COUNTDOWN_PAGE: &str = r#"<form name="F1" method="POST" action="">
<input type="hidden" name="op" value="download2">
<input type="hidden" name="id" value="abc123xyz">
<input type="hidden" name="rand" value="r4nd">
<input type="hidden" name="method_free" value="Free Download">
<input type="hidden" name="method_premium" value="Premium Download">
<div class="g-recaptcha" data-sitekey="6Lc-site-key"></div>
</form>
<span id="countdown_str">Wait <span id="xyz">17</span> seconds</span>"#;

const LINK_PAGE: &str =
    r#"<a href="https://dl7.clone.test/d/abc123xyz/release.rar">Click here to download</a>"#;

/// Records what the flow asked for and answers from a queue, so a whole run is asserted without
/// a network or a clock.
struct MockHost {
    responses: RefCell<VecDeque<HttpResponse>>,
    requests: RefCell<Vec<HttpRequest>>,
    waits: RefCell<Vec<u32>>,
    captchas: RefCell<Vec<CaptchaChallenge>>,
    captcha_token: Option<String>,
    /// Set if the flow ever asks for a cookie session. It must not: this plugin has no account,
    /// and asking would mean it expects one.
    cookies_asked: RefCell<bool>,
}

impl MockHost {
    fn new(responses: Vec<HttpResponse>, captcha_token: Option<&str>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
            requests: RefCell::new(Vec::new()),
            waits: RefCell::new(Vec::new()),
            captchas: RefCell::new(Vec::new()),
            captcha_token: captcha_token.map(str::to_owned),
            cookies_asked: RefCell::new(false),
        }
    }
}

impl PluginHost for MockHost {
    async fn http(&self, request: HttpRequest) -> Result<HttpResponse, Failure> {
        self.requests.borrow_mut().push(request);
        self.responses.borrow_mut().pop_front().ok_or_else(|| {
            Failure::coded(
                FailureKind::Permanent,
                "mock.exhausted",
                "missing mock response",
            )
        })
    }

    async fn cookies(&self, _account_id: &str, _url: &str) -> Vec<(String, String)> {
        *self.cookies_asked.borrow_mut() = true;
        Vec::new()
    }

    async fn secret_available(&self, _account_id: &str, _reference: &str) -> bool {
        false
    }

    /// No entropy here on purpose: this mock answers every call the same way, and a resolver
    /// that started depending on randomness should fail visibly rather than get a constant.
    async fn random_bytes(&self, _count: u32) -> Vec<u8> {
        Vec::new()
    }

    async fn wait(&self, seconds: u32) -> Result<(), Failure> {
        self.waits.borrow_mut().push(seconds);
        Ok(())
    }

    async fn solve_challenge(&self, challenge: CaptchaChallenge) -> Result<CaptchaAnswer, Failure> {
        self.solve_captcha(challenge)
            .await
            .map(|solution| CaptchaAnswer::Token(solution.token))
    }

    async fn solve_captcha(&self, challenge: CaptchaChallenge) -> Result<CaptchaSolution, Failure> {
        self.captchas.borrow_mut().push(challenge);
        match &self.captcha_token {
            Some(token) => Ok(CaptchaSolution {
                token: token.clone(),
            }),
            None => Err(Failure::coded(
                FailureKind::NeedsCaptcha,
                "captcha.no_solver",
                "No captcha solver is configured",
            )),
        }
    }

    async fn now_unix_seconds(&self) -> u64 {
        0
    }

    fn log(&self, _level: &str, _message: &str) {}
}

fn html(final_url: &str, body: &str) -> HttpResponse {
    HttpResponse {
        status: 200,
        final_url: final_url.to_owned(),
        headers: vec![(
            "Content-Type".to_owned(),
            "text/html; charset=UTF-8".to_owned(),
        )],
        body: body.as_bytes().to_vec(),
    }
}

fn file(final_url: &str) -> HttpResponse {
    HttpResponse {
        status: 206,
        final_url: final_url.to_owned(),
        headers: vec![(
            "Content-Disposition".to_owned(),
            "attachment; filename=release.rar".to_owned(),
        )],
        body: vec![0],
    }
}

fn input() -> ResolveInput {
    ResolveInput {
        url: LINK.to_owned(),
        account_id: None,
    }
}

async fn run(host: &MockHost) -> Result<plugin_common::Resolved, Failure> {
    let parsed = Url::parse(LINK).expect("URL");
    super::resolve(host, &input(), &parsed, CODE).await
}

/// The whole standard flow: file page, `download1`, captcha, countdown, `download2`, direct link.
#[tokio::test]
async fn the_standard_free_flow_yields_the_direct_link() {
    let host = MockHost::new(
        vec![
            html(LINK, FILE_PAGE),
            html(LINK, COUNTDOWN_PAGE),
            html(LINK, LINK_PAGE),
            file("https://dl7.clone.test/d/abc123xyz/release.rar"),
        ],
        Some("solved-token"),
    );
    let resolved = run(&host).await.expect("the flow completes");

    assert_eq!(
        resolved.url,
        "https://dl7.clone.test/d/abc123xyz/release.rar"
    );
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    // The countdown is waited out and the captcha handed over exactly once.
    assert_eq!(*host.waits.borrow(), vec![17]);
    assert_eq!(host.captchas.borrow().len(), 1);
    // Nothing here may look for an account.
    assert!(!*host.cookies_asked.borrow());
    // The direct link is fetched with the page that earned it as referer.
    assert_eq!(
        resolved
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case("referer"))
            .map(|header| header.value.as_str()),
        Some("https://clone.test/")
    );
}

/// A clone that serves the file straight from the link, with no form in between.
#[tokio::test]
async fn a_hotlinking_clone_needs_no_forms() {
    let host = MockHost::new(vec![file(LINK)], None);
    let resolved = run(&host).await.expect("the flow completes");
    assert_eq!(resolved.url, LINK);
    assert_eq!(host.requests.borrow().len(), 1);
}

/// A clone that deviates from the standard flow must say so, not answer with nothing.
#[tokio::test]
async fn a_clone_without_the_standard_form_fails_with_a_named_cause() {
    let host = MockHost::new(
        vec![html(
            LINK,
            "<html><body>Some other download page</body></html>",
        )],
        None,
    );
    let failure = run(&host).await.expect_err("no form to post");
    assert_eq!(failure.code.as_deref(), Some("xfs_generic.no_free_form"));
    assert!(failure.params.iter().any(|(name, _)| name == "diagnosis"));
}

/// An IP limit stops the flow before any wait or captcha is spent on it.
#[tokio::test]
async fn an_ip_limit_stops_the_flow_and_carries_its_delay() {
    let page =
        "<html><body>You have to wait 5 minutes, 30 seconds till next download</body></html>";
    let host = MockHost::new(vec![html(LINK, page)], Some("solved-token"));
    let failure = run(&host).await.expect_err("the IP is blocked");

    assert_eq!(
        failure.code.as_deref(),
        Some("xfs_generic.free_limit_reached")
    );
    assert!(matches!(failure.kind, FailureKind::IpBlocked(Some(330))));
    assert!(host.captchas.borrow().is_empty());
    assert!(host.waits.borrow().is_empty());
}

/// A rejected captcha is retried once with whatever the rejection page asks for, then given up
/// on rather than looped over.
#[tokio::test]
async fn a_rejected_captcha_is_retried_once_and_then_reported() {
    let rejected = format!("{COUNTDOWN_PAGE}<div>Wrong captcha</div>");
    let host = MockHost::new(
        vec![
            html(LINK, FILE_PAGE),
            html(LINK, COUNTDOWN_PAGE),
            html(LINK, &rejected),
            html(LINK, &rejected),
        ],
        Some("solved-token"),
    );
    let failure = run(&host).await.expect_err("the captcha stays rejected");

    assert_eq!(
        failure.code.as_deref(),
        Some("xfs_generic.captcha_rejected")
    );
    assert_eq!(failure.kind, FailureKind::CaptchaFailed);
    assert_eq!(host.captchas.borrow().len(), 2);
}

/// The last page without a link is reported as such, with the page's own diagnosis attached.
#[tokio::test]
async fn a_last_page_without_a_link_is_reported() {
    let host = MockHost::new(
        vec![
            html(LINK, FILE_PAGE),
            html(LINK, COUNTDOWN_PAGE),
            html(LINK, "<html><body>Nothing to see here</body></html>"),
        ],
        Some("solved-token"),
    );
    let failure = run(&host).await.expect_err("no link on the last page");
    assert_eq!(failure.code.as_deref(), Some("xfs_generic.no_free_link"));
}

/// A link on an unrelated host is not accepted as the file, however the page presents it.
#[tokio::test]
async fn a_link_pointing_off_the_site_is_not_followed() {
    let foreign = r#"<a href="https://cdn.elsewhere.test/d/abc123xyz/release.rar">Download</a>"#;
    let host = MockHost::new(
        vec![
            html(LINK, FILE_PAGE),
            html(LINK, COUNTDOWN_PAGE),
            html(LINK, foreign),
        ],
        Some("solved-token"),
    );
    let failure = run(&host).await.expect_err("the link is not on this site");
    assert_eq!(failure.code.as_deref(), Some("xfs_generic.no_free_link"));
}
