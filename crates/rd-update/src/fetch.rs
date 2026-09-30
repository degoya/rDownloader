//! Reaching GitHub: https at every hop, bounded, and replaceable in tests.
//!
//! Follows `rd_plugin_host::repository::fetch`: the service holds a [`Fetcher`] rather than a
//! client, so a test serves a manifest and an artifact from memory ([`MemoryFetcher`]) without a
//! TLS server, and the production [`HttpFetcher`] refuses to leave TLS at any redirect.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use futures_util::stream::{self, BoxStream, StreamExt};

/// How long fetching one small document (a manifest, the release list) may take.
pub const FETCH_TIMEOUT_SECONDS: u64 = 60;
/// How long a download may go without receiving a byte.
pub const READ_TIMEOUT_SECONDS: u64 = 60;
/// Most redirects one request follows. A release asset takes two.
const MAX_REDIRECTS: usize = 5;

/// An opened download: its declared length, when the server named one, and its bytes.
pub struct Download {
    pub length: Option<u64>,
    pub chunks: BoxStream<'static, Result<Bytes>>,
}

/// Reads what the update check and the download need.
#[async_trait::async_trait]
pub trait Fetcher: Send + Sync {
    /// Fetches `url`, refusing a body longer than `limit` bytes. `Ok(None)` is a `404`: nothing
    /// published there, which the check reports differently from a failure to reach it.
    async fn fetch(&self, url: &url::Url, limit: u64) -> Result<Option<Vec<u8>>>;
    /// Opens `url` for streaming.
    async fn open(&self, url: &url::Url) -> Result<Download>;
}

/// The production fetcher: `reqwest`, https at every hop.
pub struct HttpFetcher {
    client: reqwest::Client,
}

impl HttpFetcher {
    #[must_use]
    pub fn new() -> Self {
        let redirects = reqwest::redirect::Policy::custom(|attempt| {
            match refuse_redirect(attempt.url(), attempt.previous().len()) {
                Some(reason) => attempt.error(reason),
                None => attempt.follow(),
            }
        });
        Self {
            client: reqwest::Client::builder()
                // GitHub's API refuses a request without one.
                .user_agent(concat!("rDownloader/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(30))
                .read_timeout(Duration::from_secs(READ_TIMEOUT_SECONDS))
                .redirect(redirects)
                .https_only(true)
                .build()
                .unwrap_or_default(),
        }
    }

    async fn get(&self, url: &url::Url) -> Result<Option<reqwest::Response>> {
        ensure!(url.scheme() == "https", "{url} is not https");
        let response = self
            .client
            .get(url.clone())
            .header(
                reqwest::header::ACCEPT,
                "application/json, application/octet-stream",
            )
            .send()
            .await
            .with_context(|| format!("fetch {url}"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            bail!("{url} answered {}", response.status());
        }
        Ok(Some(response))
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a redirect to `target`, after `hops` earlier ones, is not followed; `None` follows it.
pub(crate) fn refuse_redirect(target: &url::Url, hops: usize) -> Option<&'static str> {
    if target.scheme() != "https" {
        Some("an update address redirected to a non-https address")
    } else if hops >= MAX_REDIRECTS {
        Some("an update address redirected too often")
    } else {
        None
    }
}

#[async_trait::async_trait]
impl Fetcher for HttpFetcher {
    async fn fetch(&self, url: &url::Url, limit: u64) -> Result<Option<Vec<u8>>> {
        let read = async {
            let Some(mut response) = self.get(url).await? else {
                return Ok(None);
            };
            // The declared length first, so an oversized body is refused before it is read;
            // the running count catches a server that declares less than it sends.
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
            Ok(Some(body))
        };
        tokio::time::timeout(Duration::from_secs(FETCH_TIMEOUT_SECONDS), read)
            .await
            .map_err(|_| {
                anyhow::anyhow!("no answer from {url} within {FETCH_TIMEOUT_SECONDS} seconds")
            })?
    }

    async fn open(&self, url: &url::Url) -> Result<Download> {
        let response = self
            .get(url)
            .await?
            .with_context(|| format!("{url} answered 404 Not Found"))?;
        let length = response.content_length();
        let chunks = response
            .bytes_stream()
            .map(|chunk| chunk.context("read the download"))
            .boxed();
        Ok(Download { length, chunks })
    }
}

/// A fetcher that serves fixed bodies from memory and records what was asked of it.
///
/// For tests here and in the service's API tests; an address it does not hold answers `404`.
#[derive(Clone, Default)]
pub struct MemoryFetcher {
    bodies: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl MemoryFetcher {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Serves `body` at `url` from now on.
    pub fn serve(&self, url: &str, body: impl Into<Vec<u8>>) {
        if let Ok(mut bodies) = self.bodies.lock() {
            bodies.insert(url.to_owned(), body.into());
        }
    }

    /// Every address asked for so far, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .map(|requests| requests.clone())
            .unwrap_or_default()
    }

    fn body(&self, url: &url::Url) -> Option<Vec<u8>> {
        if let Ok(mut requests) = self.requests.lock() {
            requests.push(url.to_string());
        }
        self.bodies.lock().ok()?.get(url.as_str()).cloned()
    }
}

#[async_trait::async_trait]
impl Fetcher for MemoryFetcher {
    async fn fetch(&self, url: &url::Url, limit: u64) -> Result<Option<Vec<u8>>> {
        let Some(body) = self.body(url) else {
            return Ok(None);
        };
        ensure!(
            u64::try_from(body.len()).unwrap_or(u64::MAX) <= limit,
            "{url} is larger than {limit} bytes"
        );
        Ok(Some(body))
    }

    async fn open(&self, url: &url::Url) -> Result<Download> {
        let body = self
            .body(url)
            .with_context(|| format!("{url} answered 404 Not Found"))?;
        let length = u64::try_from(body.len()).ok();
        // Several chunks, as a network delivers them, so the streaming path is what is tested.
        let chunks: Vec<Result<Bytes>> = body
            .chunks(7)
            .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
            .collect();
        Ok(Download {
            length,
            chunks: stream::iter(chunks).boxed(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redirect_off_https_or_past_the_hop_limit_is_not_followed() {
        let https = url::Url::parse("https://objects.githubusercontent.com/a").expect("url");
        let http = url::Url::parse("http://objects.githubusercontent.com/a").expect("url");
        assert_eq!(refuse_redirect(&https, 0), None);
        assert!(refuse_redirect(&https, 5).is_some());
        assert!(refuse_redirect(&http, 0).is_some());
    }

    #[tokio::test]
    async fn the_production_fetcher_refuses_a_plain_http_address() {
        let url = url::Url::parse("http://127.0.0.1:9/manifest.json").expect("url");
        let error = HttpFetcher::new()
            .fetch(&url, 1024)
            .await
            .expect_err("plain http");
        assert!(error.to_string().contains("not https"), "{error}");
    }

    #[tokio::test]
    async fn the_memory_fetcher_answers_404_for_what_it_does_not_hold() {
        let fetcher = MemoryFetcher::new();
        fetcher.serve("https://example.test/a", b"abc".to_vec());
        let known = url::Url::parse("https://example.test/a").expect("url");
        let unknown = url::Url::parse("https://example.test/b").expect("url");
        assert_eq!(
            fetcher.fetch(&known, 10).await.expect("a"),
            Some(b"abc".to_vec())
        );
        assert_eq!(fetcher.fetch(&unknown, 10).await.expect("b"), None);
        assert!(fetcher.fetch(&known, 2).await.is_err());
        assert_eq!(fetcher.requests().len(), 3);
    }
}
