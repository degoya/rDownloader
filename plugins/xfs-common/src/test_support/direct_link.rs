//! The one line a silent fallback leaves behind ([`ApiSite::direct_link`]).
//!
//! RD-120-13: four `.ok()?` in a row used to write nothing whatsoever, which is why the running
//! installation's error log held no line about the provider while a user spent an evening on an
//! expired cookie session. Each failing answer below is one of the five ways the undocumented
//! `file/direct_link` endpoint can come to nothing, and the cases assert the same three things:
//! the fallback still happens, it is explained exactly once, and the explanation carries nothing
//! that came off the wire.

use plugin_common::{Failure, FailureKind, HttpResponse};

use super::{LogHost, json};
use crate::site::ApiSite;

/// A file code that is deliberately recognisable, so a log line leaking it is visible.
pub const CODE: &str = "abc123xyz";

/// One site's direct-link cases.
#[derive(Clone, Copy)]
pub struct DirectLinkCase {
    /// The site under test.
    pub site: ApiSite,
    /// The address the endpoint answers from.
    pub api_url: &'static str,
    /// A direct link on the site's own delivery host, named `release.rar`.
    pub link: &'static str,
}

impl DirectLinkCase {
    /// Every way the attempt can end with no link, and the phrase each one is expected to
    /// produce.
    fn failing_answers(&self) -> Vec<(&'static str, Result<HttpResponse, Failure>)> {
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
                Ok(json(self.api_url, "<html>not json at all</html>")),
            ),
            (
                "the API answered with an error",
                Ok(json(self.api_url, r#"{"status":400,"msg":"Invalid key"}"#)),
            ),
            (
                "the link it returned is not a URL",
                Ok(json(
                    self.api_url,
                    r#"{"status":200,"msg":"OK","result":{"url":"not a url"}}"#,
                )),
            ),
            (
                "the link it returned points at another host",
                Ok(json(
                    self.api_url,
                    r#"{"status":200,"msg":"OK","result":{"url":"https://evil.test/abc123xyz/release.rar"}}"#,
                )),
            ),
        ]
    }

    /// Each failing attempt falls back and is explained in exactly one `info` line naming the
    /// provider.
    pub async fn every_failing_attempt_is_explained_exactly_once(&self) {
        let prefix = format!("{}: ", self.site.provider);
        for (reason, answer) in self.failing_answers() {
            let host = LogHost::new(vec![answer]);
            assert!(
                self.site.direct_link(&host, CODE).await.is_none(),
                "{reason}: the fallback to the cookie flow must still happen"
            );
            let logs = host.logs();
            assert_eq!(
                logs.len(),
                1,
                "{reason}: exactly one line, never two and never none"
            );
            let (level, line) = &logs[0];
            assert_eq!(level, "info");
            assert!(line.contains(reason), "{reason}: unexpected line {line}");
            assert!(
                line.starts_with(&prefix),
                "{reason}: the line names the provider: {line}"
            );
        }
    }

    /// The secret half of the same assertion: whatever the attempt saw, none of it reaches the
    /// log.
    pub async fn the_explanation_carries_no_file_code_key_or_address(&self) {
        for (_, answer) in self.failing_answers() {
            let host = LogHost::new(vec![answer]);
            let _ = self.site.direct_link(&host, CODE).await;
            let logs = host.logs();
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
    pub async fn a_direct_link_that_works_writes_nothing(&self) {
        let body = format!(
            r#"{{"status":200,"msg":"OK","result":{{"url":"{}","size":"1024"}}}}"#,
            self.link
        );
        let host = LogHost::new(vec![Ok(json(self.api_url, &body))]);
        let resolved = self
            .site
            .direct_link(&host, CODE)
            .await
            .expect("a usable direct link");
        assert_eq!(resolved.file_name.as_deref(), Some("release.rar"));
        assert!(host.logs().is_empty(), "nothing to report, nothing written");
    }
}
