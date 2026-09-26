//! Fetching from a repository: https only, bounded, and replaceable in tests.

use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};

/// How long one fetch may take, index or package.
pub const FETCH_TIMEOUT_SECONDS: u64 = 120;
/// Most redirects one fetch follows. GitHub's `latest/download` takes two.
const MAX_REDIRECTS: usize = 5;

/// Reads one resource. The service holds this rather than a client so a test can serve an index
/// and a package without a TLS server.
#[async_trait::async_trait]
pub trait Fetcher: Send + Sync {
    /// Fetches `url`, refusing a body longer than `limit` bytes.
    async fn fetch(&self, url: &url::Url, limit: u64) -> Result<Vec<u8>>;
}

/// The production fetcher: `reqwest`, https at every hop.
pub struct HttpFetcher {
    client: reqwest::Client,
}

impl HttpFetcher {
    #[must_use]
    pub fn new() -> Self {
        // Every hop https, not only the first: a release asset URL redirects to a storage host,
        // and a redirect to plain http would carry the download out of TLS without a word.
        let redirects = reqwest::redirect::Policy::custom(|attempt| {
            let refused = refuse_redirect(attempt.url(), attempt.previous().len());
            match refused {
                Some(reason) => attempt.error(reason),
                None => attempt.follow(),
            }
        });
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(FETCH_TIMEOUT_SECONDS))
                .redirect(redirects)
                .https_only(true)
                .build()
                .unwrap_or_default(),
        }
    }
}

/// Why a redirect to `target`, after `hops` earlier ones, is not followed; `None` follows it.
///
/// Its own function so the rule is tested without a TLS server: `https_only` on the client
/// refuses a plain-http hop as well, and this names the reason in the repository's error.
pub(crate) fn refuse_redirect(target: &url::Url, hops: usize) -> Option<&'static str> {
    if target.scheme() != "https" {
        Some("a plugin repository redirected to a non-https address")
    } else if hops >= MAX_REDIRECTS {
        Some("a plugin repository redirected too often")
    } else {
        None
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Fetcher for HttpFetcher {
    async fn fetch(&self, url: &url::Url, limit: u64) -> Result<Vec<u8>> {
        ensure!(url.scheme() == "https", "{url} is not https");
        let mut response = self
            .client
            .get(url.clone())
            .send()
            .await
            .with_context(|| format!("fetch {url}"))?;
        if !response.status().is_success() {
            bail!("{url} answered {}", response.status());
        }
        // The declared length is checked first so an oversized body is refused before a byte of
        // it is read; the running count below catches a server that declares less than it sends.
        if response
            .content_length()
            .is_some_and(|length| length > limit)
        {
            bail!("{url} is larger than {limit} bytes");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .with_context(|| format!("read {url}"))?
        {
            body.extend_from_slice(&chunk);
            if u64::try_from(body.len()).unwrap_or(u64::MAX) > limit {
                bail!("{url} is larger than {limit} bytes");
            }
        }
        Ok(body)
    }
}

/// The transport half of the plugin-trust review (`docs/security/plugin-trust.md`, T-TLS).
#[cfg(test)]
mod tests {
    use super::*;

    /// T-TLS: every hop https, and a bounded number of them.
    #[test]
    fn a_redirect_off_https_or_past_the_hop_limit_is_not_followed() {
        let https = url::Url::parse("https://objects.example.test/a.rdplug").expect("url");
        let http = url::Url::parse("http://objects.example.test/a.rdplug").expect("url");
        assert_eq!(refuse_redirect(&https, 0), None);
        assert_eq!(refuse_redirect(&https, 4), None);
        assert!(refuse_redirect(&https, 5).is_some());
        assert!(refuse_redirect(&http, 0).is_some());
    }

    /// T-TLS: a plain-http address is refused before a request leaves.
    #[tokio::test]
    async fn the_production_fetcher_refuses_a_plain_http_address() {
        let url = url::Url::parse("http://127.0.0.1:9/index.json").expect("url");
        let error = HttpFetcher::new()
            .fetch(&url, 1024)
            .await
            .expect_err("plain http");
        assert!(error.to_string().contains("not https"), "{error}");
    }
}
