//! The documented XFS JSON API and the cookie session around it, for a plugin that has both
//! (RD-1120-10, PL-2).
//!
//! `katfile` cloned `ddownload`'s metadata call, its direct-link attempt, its link check and the
//! handful of link helpers byte for byte, different in nothing but the provider's name, its
//! domain, its API address and its translation codes. Those are an [`ApiSite`] now, and the code
//! is here once; a plugin's `resolver.rs` keeps the flows that really are its own (the sign-in,
//! the premium page) and calls these.
//!
//! The helpers at the bottom ([`matches`], [`second_path_segment`], [`premium_until`],
//! [`referer`]) need no API at all, so `filejoker` and `xfs-generic` use them too.

use plugin_common::failure::HttpError;
use plugin_common::{
    Failure, FailureKind, Header, HttpRequest, HttpResponse, LinkCheck, LinkStatus, PluginHost,
    Resolved,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::api::{ApiEnvelope, DirectLinkSkip, FlexibleU64};
use crate::glue::{ApiMessages, coded};

/// How many file codes one `file/info` call carries.
const CHECK_BATCH: usize = 50;

/// What `account/info` answers.
#[derive(Deserialize)]
pub struct AccountResult {
    pub email: String,
    pub premium_expire: String,
    pub traffic_left: Option<FlexibleU64>,
}

/// What `file/direct_link` answers.
#[derive(Deserialize)]
pub struct DirectLink {
    pub url: String,
    pub size: Option<FlexibleU64>,
}

/// One entry of what `file/info` answers.
#[derive(Deserialize)]
pub struct FileInfo {
    pub status: u16,
    pub name: Option<String>,
    pub size: Option<FlexibleU64>,
}

/// One XFS site with a documented API: who it is, where it lives and the codes it reports under.
#[derive(Clone, Copy)]
pub struct ApiSite {
    /// The display name, for the one log line a silent fallback leaves behind. A constant, never
    /// a value off the wire: that is what keeps a key or an address out of a log.
    pub provider: &'static str,
    /// The main domain: the cookie scope, the referer, and the host a direct link must be on.
    pub primary_domain: &'static str,
    /// An API call carrying the `{{secret:…}}` key marker the host expands.
    pub api_request: fn(&str, &[(&str, String)]) -> HttpRequest,
    /// The plugin's `http_error` code and text.
    pub http_error: HttpError,
    /// The codes the API's envelope failures are reported under.
    pub api_messages: ApiMessages,
    /// The `file_unavailable` words, for a file the metadata API knows but will not serve.
    pub file_unavailable: (&'static str, &'static str),
}

impl ApiSite {
    /// Classifies a transport status under the plugin's `http_error` (`crate::glue`).
    ///
    /// # Errors
    ///
    /// The classified refusal for every status that is not a `2xx`.
    pub fn ensure_http_status(&self, response: &HttpResponse) -> Result<(), Failure> {
        crate::glue::ensure_http_status(response, self.http_error.code, self.http_error.text)
    }

    /// An API answer: transport status first, then the envelope around its `result`.
    async fn api_result<T: DeserializeOwned, H: PluginHost>(
        &self,
        host: &H,
        request: HttpRequest,
    ) -> Result<T, Failure> {
        let response = host.http(request).await?;
        self.ensure_http_status(&response)?;
        let envelope: ApiEnvelope<T> = self.api_messages.parse_json(&response)?;
        envelope
            .into_result()
            .map_err(|error| self.api_messages.envelope_error(error))
    }

    /// `account/info` through `request`, which carries the key: the stored one's marker, or one
    /// the plugin read off the signed-in account page.
    ///
    /// # Errors
    ///
    /// The host's failure, the classified status, or the envelope's failure.
    pub async fn account_info<H: PluginHost>(
        &self,
        host: &H,
        request: HttpRequest,
    ) -> Result<AccountResult, Failure> {
        self.api_result(host, request).await
    }

    /// The metadata of one file. A file the API knows with any status but `200` is refused here
    /// rather than fetched: the premium flow would only report the same thing later.
    ///
    /// # Errors
    ///
    /// As [`Self::account_info`], and `file_unavailable` (`Permanent`) for such a file.
    pub async fn file_info<H: PluginHost>(
        &self,
        host: &H,
        code: &str,
    ) -> Result<Option<FileInfo>, Failure> {
        let request = (self.api_request)("file/info", &[("file_code", code.to_owned())]);
        let infos: Vec<FileInfo> = self.api_result(host, request).await?;
        let info = infos.into_iter().next();
        if info.as_ref().is_some_and(|item| item.status != 200) {
            return Err(coded(FailureKind::Permanent, self.file_unavailable));
        }
        Ok(info)
    }

    /// Some XFileSharing installations expose `file/direct_link` for premium API keys; neither
    /// DDownload nor KatFile documents it, so any failure falls back to the cookie flow.
    ///
    /// The fallback stays quiet as far as the download is concerned, but it is no longer silent.
    /// Four `.ok()?` in a row wrote nothing at all, which is why the running installation's error
    /// log held not one line about ddownload while a user spent an evening on an expired session
    /// (RD-120-13). The reason now goes out exactly once per attempt — one call, on the single
    /// path that has an answer to report — and it is one of [`DirectLinkSkip`]'s fixed phrases,
    /// so no file code, address or key can travel in it.
    pub async fn direct_link<H: PluginHost>(&self, host: &H, code: &str) -> Option<Resolved> {
        match self.direct_link_attempt(host, code).await {
            Ok(resolved) => Some(resolved),
            Err(skip) => {
                // `info`, not `warn`: for these providers the endpoint is expected to produce
                // nothing, so the fallback is normal and only its reason is diagnostic. `debug`
                // would have left it exactly as invisible as it was.
                host.log(
                    "info",
                    &crate::api::direct_link_skipped(self.provider, skip),
                );
                None
            }
        }
    }

    /// The attempt itself, with every way it can come to nothing named rather than swallowed.
    async fn direct_link_attempt<H: PluginHost>(
        &self,
        host: &H,
        code: &str,
    ) -> Result<Resolved, DirectLinkSkip> {
        let response = host
            .http((self.api_request)(
                "file/direct_link",
                &[("file_code", code.to_owned())],
            ))
            .await
            .map_err(|_| DirectLinkSkip::RequestFailed)?;
        let envelope: ApiEnvelope<DirectLink> = self
            .api_messages
            .parse_json(&response)
            .map_err(|_| DirectLinkSkip::NotJson)?;
        let link = envelope
            .into_result()
            .map_err(|_| DirectLinkSkip::ApiError)?;
        let url = Url::parse(&link.url).map_err(|_| DirectLinkSkip::UnparsableUrl)?;
        let domain = self.primary_domain;
        if !url
            .host_str()
            .is_some_and(|host| host == domain || host.ends_with(&format!(".{domain}")))
        {
            return Err(DirectLinkSkip::ForeignHost);
        }
        Ok(Resolved {
            file_name: url
                .path_segments()
                .and_then(|mut segments| segments.next_back())
                .filter(|name| !name.is_empty())
                .map(str::to_owned),
            size: link.size.and_then(FlexibleU64::into_u64),
            url: url.to_string(),
            headers: Vec::new(),
            checksum: None,
        })
    }

    /// How many cookies the account's jar holds for the site.
    pub async fn cookie_count<H: PluginHost>(&self, host: &H, account_id: &str) -> usize {
        host.cookies(account_id, &format!("https://{}/", self.primary_domain))
            .await
            .len()
    }

    /// A batched link check through `file/info`, fifty codes a call. `request` builds the call
    /// from the path and the arguments, so a plugin chooses which key it carries; a link this
    /// plugin cannot read a code from is `Unknown` without a request.
    ///
    /// # Errors
    ///
    /// The first call that fails, as [`Self::account_info`].
    pub async fn link_checks<H: PluginHost>(
        &self,
        host: &H,
        urls: &[String],
        file_code: fn(&Url) -> Option<&str>,
        request: impl Fn(&str, &[(&str, String)]) -> HttpRequest,
    ) -> Result<Vec<LinkCheck>, Failure> {
        let coded: Vec<(String, Option<String>)> = urls
            .iter()
            .map(|url| {
                let code = Url::parse(url)
                    .ok()
                    .and_then(|parsed| file_code(&parsed).map(str::to_owned));
                (url.clone(), code)
            })
            .collect();
        let mut results = Vec::with_capacity(coded.len());
        for chunk in coded.chunks(CHECK_BATCH) {
            let codes: Vec<&str> = chunk
                .iter()
                .filter_map(|(_, code)| code.as_deref())
                .collect();
            let infos: Vec<FileInfo> = if codes.is_empty() {
                Vec::new()
            } else {
                let call = request("file/info", &[("file_code", codes.join(","))]);
                self.api_result(host, call).await?
            };
            let mut infos = infos.into_iter();
            for (url, code) in chunk {
                results.push(match code {
                    None => LinkCheck::unknown(url),
                    Some(_) => link_check(url, infos.next()),
                });
            }
        }
        Ok(results)
    }

    /// The `Referer` a free transfer must carry, so the hoster sees the page that earned it.
    #[must_use]
    pub fn referer_header(&self) -> Header {
        referer(self.primary_domain)
    }
}

/// One link's row: `200` online, `404` offline, anything else (or no entry) unknown, with
/// whatever name and size the entry carried.
fn link_check(url: &str, info: Option<FileInfo>) -> LinkCheck {
    LinkCheck {
        url: url.to_owned(),
        status: match info.as_ref().map(|item| item.status) {
            Some(200) => LinkStatus::Online,
            Some(404) => LinkStatus::Offline,
            _ => LinkStatus::Unknown,
        },
        file_name: info.as_ref().and_then(|item| item.name.clone()),
        size: info
            .and_then(|item| item.size)
            .and_then(FlexibleU64::into_u64),
    }
}

/// Whether a link is on one of `hosts` and carries an XFS file code.
#[must_use]
pub fn matches(url: &str, hosts: &[&str]) -> bool {
    Url::parse(url)
        .ok()
        .as_ref()
        .and_then(|url| crate::api::file_code(url, hosts))
        .is_some()
}

/// The file name segment of a `/<code>/<name>` link.
#[must_use]
pub fn second_path_segment(url: &Url) -> Option<String> {
    url.path_segments()
        .and_then(|segments| segments.filter(|segment| !segment.is_empty()).nth(1))
        .map(str::to_owned)
}

/// Whether the account's premium period is still running.
///
/// `%Y-%m-%d %H:%M:%S`, as the XFS API reports it. An unreadable value is not premium: claiming
/// premium on a date nobody can parse is the one answer that cannot be right.
#[must_use]
pub fn premium_until(now_unix_seconds: u64, expiry: &str) -> bool {
    crate::api::parse_expiry_unix(expiry)
        .is_some_and(|expiry| expiry > i64::try_from(now_unix_seconds).unwrap_or(i64::MAX))
}

/// `Referer: https://<domain>/`, the page a transfer has to look as if it came from.
#[must_use]
pub fn referer(domain: &str) -> Header {
    Header::new("Referer", format!("https://{domain}/"))
}
