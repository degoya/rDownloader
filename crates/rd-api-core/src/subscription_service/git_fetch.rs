//! The forge requests of git-release subscriptions (RD-190-13).
//!
//! Through the scheduler's pooled client, like every other subscription request: the proxy, the
//! TLS settings and the user agent are the application's, and an auth profile that matches the
//! forge contributes its headers as it would to a feed. The token the subscription carries
//! replaces a profile's `Authorization` rather than travelling beside it, and is marked
//! sensitive so no debug output of the request prints it. On a redirect to another host the
//! client drops it, which is what keeps it from the storage a forge hands a download to.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

/// Makes [`rd_subscription::GitReleaseAdapter`]'s requests.
pub struct HttpApiFetcher {
    scheduler: rd_scheduler::SchedulerHandle,
}

impl HttpApiFetcher {
    #[must_use]
    pub fn new(scheduler: rd_scheduler::SchedulerHandle) -> Self {
        Self { scheduler }
    }

    async fn request(
        &self,
        url: &url::Url,
        headers: &[(String, String)],
    ) -> anyhow::Result<reqwest::Response> {
        let network = self.scheduler.direct_client(url).await?;
        let header = |(name, value): &(String, String)| {
            let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
            let mut value = HeaderValue::from_str(value).ok()?;
            if name == reqwest::header::AUTHORIZATION {
                value.set_sensitive(true);
            }
            Some((name, value))
        };
        let mut map = HeaderMap::new();
        for (name, value) in network.headers.iter().filter_map(header) {
            map.append(name, value);
        }
        // `insert` replaces: the subscription's own header wins over a profile's.
        for (name, value) in headers.iter().filter_map(header) {
            map.insert(name, value);
        }
        Ok(network.client.get(url.clone()).headers(map).send().await?)
    }
}

#[async_trait::async_trait]
impl rd_subscription::ApiFetcher for HttpApiFetcher {
    async fn get(
        &self,
        url: &url::Url,
        headers: &[(String, String)],
        limit: usize,
    ) -> anyhow::Result<rd_subscription::ApiResponse> {
        let response = self.request(url, headers).await?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
            })
            .collect();
        if status == 304 {
            return Ok(rd_subscription::ApiResponse {
                status,
                headers,
                body: None,
            });
        }
        let collected = crate::input_checks::read_body_prefix(response, limit).await?;
        Ok(rd_subscription::ApiResponse {
            status,
            headers,
            body: Some(String::from_utf8_lossy(&collected).into_owned()),
        })
    }

    async fn locate(
        &self,
        url: &url::Url,
        headers: &[(String, String)],
    ) -> anyhow::Result<url::Url> {
        // Only the status line and the headers are read; dropping the response closes it
        // before the file itself is transferred.
        let response = self.request(url, headers).await?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("HTTP {status}");
        }
        Ok(response.url().clone())
    }
}
