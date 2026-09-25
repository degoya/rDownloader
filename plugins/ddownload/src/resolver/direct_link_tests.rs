//! The one line a silent fallback leaves behind.
//!
//! Driven against a mock [`PluginHost`] rather than through the `Resolver` trait, because the
//! question is what the plugin told the *host*: the native adapter forwards `log` straight into
//! `tracing`, where a test would have to install a subscriber to see anything at all.
//!
//! RD-120-13: four `.ok()?` in a row used to write nothing whatsoever, which is why the running
//! installation's error log held no line about the provider while a user spent an evening on an
//! expired cookie session. Each case below drives one of the five ways the undocumented
//! `file/direct_link` endpoint can come to nothing, and asserts the same three things: the
//! fallback still happens, it is explained exactly once, and the explanation carries nothing
//! that came off the wire.

use std::{cell::RefCell, collections::VecDeque};

use plugin_common::{
    CaptchaAnswer, CaptchaChallenge, CaptchaSolution, Failure, FailureKind, HttpRequest,
    HttpResponse, PluginHost,
};

use super::direct_link;

/// A file code that is deliberately recognisable, so a log line leaking it is visible.
const CODE: &str = "abc123xyz";

struct MockHost {
    answers: RefCell<VecDeque<Result<HttpResponse, Failure>>>,
    logs: RefCell<Vec<(String, String)>>,
}

impl MockHost {
    fn answering(answer: Result<HttpResponse, Failure>) -> Self {
        Self {
            answers: RefCell::new(VecDeque::from(vec![answer])),
            logs: RefCell::new(Vec::new()),
        }
    }
}

impl PluginHost for MockHost {
    async fn http(&self, _request: HttpRequest) -> Result<HttpResponse, Failure> {
        self.answers.borrow_mut().pop_front().unwrap_or_else(|| {
            Err(Failure::coded(
                FailureKind::Permanent,
                "mock.exhausted",
                "missing mock response",
            ))
        })
    }

    async fn cookies(&self, _account_id: &str, _url: &str) -> Vec<(String, String)> {
        Vec::new()
    }

    async fn secret_available(&self, _account_id: &str, _reference: &str) -> bool {
        false
    }

    async fn random_bytes(&self, _count: u32) -> Vec<u8> {
        Vec::new()
    }

    async fn wait(&self, _seconds: u32) -> Result<(), Failure> {
        Ok(())
    }

    async fn solve_challenge(&self, challenge: CaptchaChallenge) -> Result<CaptchaAnswer, Failure> {
        self.solve_captcha(challenge)
            .await
            .map(|solution| CaptchaAnswer::Token(solution.token))
    }

    async fn solve_captcha(
        &self,
        _challenge: CaptchaChallenge,
    ) -> Result<CaptchaSolution, Failure> {
        Err(Failure::coded(
            FailureKind::NeedsCaptcha,
            "captcha.no_solver",
            "No captcha solver is configured",
        ))
    }

    async fn now_unix_seconds(&self) -> u64 {
        0
    }

    fn log(&self, level: &str, message: &str) {
        self.logs
            .borrow_mut()
            .push((level.to_owned(), message.to_owned()));
    }
}

fn json(body: &str) -> Result<HttpResponse, Failure> {
    Ok(HttpResponse {
        status: 200,
        final_url: "https://api-v2.ddownload.com/api/file/direct_link".to_owned(),
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    })
}

/// Every way the attempt can end with no link, and the phrase each one is expected to produce.
fn failing_answers() -> Vec<(&'static str, Result<HttpResponse, Failure>)> {
    vec![
        (
            "the request failed",
            Err(Failure::coded(
                FailureKind::Transient(None),
                "mock.offline",
                "the network is down",
            )),
        ),
        (
            "the answer was not the documented JSON envelope",
            Ok(HttpResponse {
                status: 200,
                final_url: "https://api-v2.ddownload.com/api/file/direct_link".to_owned(),
                headers: Vec::new(),
                body: b"<html>not json at all</html>".to_vec(),
            }),
        ),
        (
            "the API answered with an error",
            json(r#"{"status":400,"msg":"Invalid key"}"#),
        ),
        (
            "the link it returned is not a URL",
            json(r#"{"status":200,"msg":"OK","result":{"url":"not a url"}}"#),
        ),
        (
            "the link it returned points at another host",
            json(
                r#"{"status":200,"msg":"OK","result":{"url":"https://evil.test/abc123xyz/release.rar"}}"#,
            ),
        ),
    ]
}

#[tokio::test]
async fn every_failing_direct_link_attempt_is_explained_exactly_once() {
    for (reason, answer) in failing_answers() {
        let host = MockHost::answering(answer);
        assert!(
            direct_link(&host, CODE).await.is_none(),
            "{reason}: the fallback to the cookie flow must still happen"
        );
        let logs = host.logs.borrow();
        assert_eq!(
            logs.len(),
            1,
            "{reason}: exactly one line, never two and never none"
        );
        let (level, line) = &logs[0];
        assert_eq!(level, "info");
        assert!(line.contains(reason), "{reason}: unexpected line {line}");
        assert!(
            line.starts_with("DDownload: "),
            "{reason}: the line names the provider: {line}"
        );
    }
}

/// The secret half of the same assertion: whatever the attempt saw, none of it reaches the log.
#[tokio::test]
async fn the_explanation_carries_no_file_code_key_or_address() {
    for (_, answer) in failing_answers() {
        let host = MockHost::answering(answer);
        let _ = direct_link(&host, CODE).await;
        let logs = host.logs.borrow();
        let line = &logs[0].1;
        assert!(
            !line.contains(CODE),
            "the file code must not travel: {line}"
        );
        assert!(!line.contains("://"), "no address may travel: {line}");
        assert!(
            !line.contains("secret"),
            "no credential marker may travel: {line}"
        );
        assert!(
            !line.contains("Invalid key"),
            "no provider text may travel: {line}"
        );
    }
}

/// The success path is silent, because there is nothing to explain.
#[tokio::test]
async fn a_direct_link_that_works_writes_nothing() {
    let host = MockHost::answering(json(
        r#"{"status":200,"msg":"OK","result":{"url":"https://cdn.ddownload.com/d/abc123xyz/release.rar","size":"1024"}}"#,
    ));
    let resolved = direct_link(&host, CODE)
        .await
        .expect("a usable direct link");
    assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
    assert!(
        host.logs.borrow().is_empty(),
        "nothing to report, nothing written"
    );
}
