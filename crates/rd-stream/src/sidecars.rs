//! Files captured beside a recording (RD-080-09).
//!
//! The rule that matters here is that **a sidecar that was asked for and could not be
//! captured says so**. Silently producing nothing is indistinguishable from the recording
//! having forgotten, and somebody who ticked "save the chat" needs to learn that this
//! provider does not expose one — not to go looking for a file that was never going to exist.
//!
//! Sidecars are named after the recording's own stem, so which recording a file belongs to is
//! visible from its name and survives the folder being moved.

use std::{path::Path, time::Duration};

use rd_core::{SidecarOutcome, SidecarPolicy, SidecarStatus};
use rd_http::SharedNetworkDefaults;
use rd_scheduler::{ToolNetwork, ToolProxy};

use crate::StreamCookies;

/// Largest thumbnail accepted.
const MAX_THUMBNAIL_BYTES: usize = 8 * 1024 * 1024;

/// How long one thumbnail request may take in total, connection included.
const THUMBNAIL_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the connection alone may take.
const THUMBNAIL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Redirects a thumbnail address may follow before it is abandoned.
const MAX_THUMBNAIL_REDIRECTS: usize = 5;

/// Captures what the policy asked for, reporting every requested kind.
///
/// The metadata streamlink reports is the source for all of it: it is the only description of
/// the stream available without a provider-specific client. Both fetches carry the recording's
/// cookie profile (RD-1320-03).
#[expect(
    clippy::too_many_arguments,
    reason = "one recording's context, each part used once"
)]
pub async fn capture(
    clients: &SidecarClients,
    streamlink: &Path,
    url: &str,
    directory: &Path,
    stem: &str,
    policy: SidecarPolicy,
    network: &ToolNetwork,
    cookies: Option<&StreamCookies>,
) -> Vec<SidecarOutcome> {
    let mut outcomes = Vec::new();
    let probe = crate::probe::probe_json(streamlink, url, network, cookies)
        .await
        .ok();

    if policy.metadata {
        outcomes.push(match &probe {
            Some(json) => write_text(directory, &format!("{stem}.metadata.json"), json).await,
            None => outcome("metadata", SidecarStatus::NotOffered, None),
        });
    }
    if policy.thumbnail {
        let thumbnail = Thumbnail {
            proxy: network.proxy(),
            cookies,
        };
        outcomes
            .push(capture_thumbnail(clients, &thumbnail, probe.as_deref(), directory, stem).await);
    }
    if policy.subtitles {
        // streamlink hands the muxed stream over as-is; a separate subtitle track is not
        // something it exposes, so this is honest rather than silently absent.
        outcomes.push(outcome("subtitles", SidecarStatus::Unsupported, None));
    }
    if policy.chat {
        // Live chat needs a per-provider client — Twitch IRC, YouTube's live-chat endpoint —
        // which is a feature of its own rather than something streamlink can be asked for.
        outcomes.push(outcome("chat", SidecarStatus::Unsupported, None));
    }
    outcomes
}

/// The client every thumbnail fetch shares, the trust roots it was built with, and the address
/// rule every fetch keeps to.
///
/// A thumbnail address is site-controlled input: it comes out of the provider's own metadata
/// block, by way of streamlink. `reqwest::get` builds a throwaway client for it with no
/// timeout at all and an unbounded redirect chain, so one unresponsive or looping CDN would
/// hold a finished recording open for as long as it liked. The caps above are therefore kept
/// here rather than taken from `ClientPool`, whose clients are built for downloads: they carry
/// no total timeout and a ten-redirect budget, and adopting them would quietly give both back.
///
/// What the pool *does* own and this used to miss is the operator's custom CA. It arrives as
/// [`SharedNetworkDefaults`] — the same handle `rd-usenet` takes, so a news server, an FTPS
/// server and a thumbnail host are all trusted by one decision — and `tls_revision` is part of
/// the cache key, so replacing the roots replaces the client instead of leaving a recording
/// fetching through the trust it started with.
///
/// The default is an empty set of roots, which builds exactly the client this had before.
///
/// The recording's proxy applies to its thumbnail as to its stream (RD-1240-22). A proxied
/// fetch gets a client of its own, built per recording: there is one thumbnail per recording,
/// and a cached client would carry one recording's proxy into the next.
///
/// The address is the provider's, so it is held to the rule for an address a website handed
/// over: public addresses only, never one of the service's own ports, checked on every
/// redirect hop as well as at connect time.
#[derive(Default)]
pub struct SidecarClients {
    network: SharedNetworkDefaults,
    /// Public addresses only; the default knows none of the service's own ports.
    pub(crate) policy: rd_http::AddressPolicy,
    /// The client and the `tls_revision` it was built for.
    cached: tokio::sync::Mutex<Option<(u64, reqwest::Client)>>,
}

impl SidecarClients {
    /// Sidecar fetches trusting the platform store alone — the behaviour of an installation
    /// with no custom CA configured.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sidecar fetches trusting what the whole service trusts.
    #[must_use]
    pub fn with_network_defaults(network: SharedNetworkDefaults) -> Self {
        Self {
            network,
            policy: rd_http::AddressPolicy::default(),
            cached: tokio::sync::Mutex::new(None),
        }
    }

    /// Holds every thumbnail fetch to `policy` instead of the default (public addresses only),
    /// typically the same rule plus the service's own listeners.
    #[must_use]
    pub fn with_address_policy(mut self, policy: rd_http::AddressPolicy) -> Self {
        self.policy = policy;
        // A client built under the previous rule must not outlive it.
        self.cached = tokio::sync::Mutex::new(None);
        self
    }

    /// The client for a thumbnail fetched through `proxy`, or directly when there is none;
    /// the direct one is built on first use and rebuilt when the trust roots change.
    ///
    /// `None` when the roots cannot be parsed or the client cannot be built. A recording is
    /// never failed for it: the thumbnail is reported as `Failed` and the recording itself,
    /// which is the thing the person asked for, is untouched. A proxy that cannot be set up
    /// is the same `None`: the thumbnail is never fetched past it.
    async fn client(&self, proxy: Option<&ToolProxy>) -> Option<reqwest::Client> {
        let (custom_ca_pem, tls_revision) = {
            let defaults = self.network.read().await;
            (defaults.custom_ca_pem.clone(), defaults.tls_revision)
        };
        if proxy.is_some() {
            return build_client(&custom_ca_pem, proxy, &self.policy);
        }
        let mut cached = self.cached.lock().await;
        if let Some((revision, client)) = cached.as_ref()
            && *revision == tls_revision
        {
            return Some(client.clone());
        }
        let client = build_client(&custom_ca_pem, None, &self.policy)?;
        *cached = Some((tls_revision, client.clone()));
        Some(client)
    }
}

/// A thumbnail client with the caps above, the custom CA and, when given, the proxy alone:
/// the service's own proxy variables do not apply beside a profile, as in `rd-http`.
///
/// Redirects are not followed by the client: [`capture_thumbnail`] follows them itself so each
/// hop is checked against `policy`. Names are resolved through the guarded resolver, so a name
/// that pointed elsewhere at the check cannot point inside at connect time; behind a proxy the
/// proxy is handed the checked address, as in every guarded request (`ToolProxy::route`).
fn build_client(
    custom_ca_pem: &[Vec<u8>],
    proxy: Option<&ToolProxy>,
    policy: &rd_http::AddressPolicy,
) -> Option<reqwest::Client> {
    let mut builder = rd_http::client_builder(rd_http::ClientPurpose::Service)
        .connect_timeout(THUMBNAIL_CONNECT_TIMEOUT)
        .timeout(THUMBNAIL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none());
    match proxy {
        None => builder = builder.dns_resolver(rd_http::GuardedResolver::system(policy.clone())),
        Some(proxy) => match proxy.route(builder.no_proxy(), Some(policy), custom_ca_pem) {
            Ok(routed) => builder = routed,
            Err(error) => {
                tracing::warn!(
                    error = %format!("{error:#}"),
                    "no proxy for the thumbnail; it is not fetched without it"
                );
                return None;
            }
        },
    }
    for pem in custom_ca_pem {
        match reqwest::Certificate::from_pem(pem) {
            Ok(certificate) => builder = builder.add_root_certificate(certificate),
            // Skipped rather than fatal: a thumbnail is not worth failing a recording
            // over, and if that root was the one this host needed, the handshake refuses
            // and the sidecar is reported as `Failed` anyway.
            Err(error) => {
                tracing::warn!(%error, "ignoring an unparsable custom CA for sidecars");
            }
        }
    }
    match builder.build() {
        Ok(client) => Some(client),
        Err(error) => {
            tracing::warn!(%error, "no HTTP client for sidecars");
            None
        }
    }
}

/// How the thumbnail is fetched: the recording's proxy and cookies.
struct Thumbnail<'a> {
    proxy: Option<&'a ToolProxy>,
    cookies: Option<&'a StreamCookies>,
}

async fn capture_thumbnail(
    clients: &SidecarClients,
    fetch: &Thumbnail<'_>,
    probe: Option<&str>,
    directory: &Path,
    stem: &str,
) -> SidecarOutcome {
    let Some(url) = probe.and_then(thumbnail_url) else {
        return outcome("thumbnail", SidecarStatus::NotOffered, None);
    };
    let Some(client) = clients.client(fetch.proxy).await else {
        return outcome("thumbnail", SidecarStatus::Failed, None);
    };
    let Some(mut response) = follow(&clients.policy, &client, fetch.cookies, &url).await else {
        return outcome("thumbnail", SidecarStatus::Failed, None);
    };
    if response
        .content_length()
        .is_some_and(|length| length > MAX_THUMBNAIL_BYTES as u64)
    {
        return outcome("thumbnail", SidecarStatus::Failed, None);
    }
    // The declared length is a claim, not a guarantee: a chunked response declares none at
    // all, and `bytes()` would buffer whatever arrives before anyone measured it. Collected
    // chunk by chunk and abandoned the moment it passes the cap, so a provider cannot turn
    // a missing `Content-Length` into unbounded memory use on the recording host.
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                bytes.extend_from_slice(&chunk);
                if bytes.len() > MAX_THUMBNAIL_BYTES {
                    return outcome("thumbnail", SidecarStatus::Failed, None);
                }
            }
            Ok(None) => break,
            Err(_) => return outcome("thumbnail", SidecarStatus::Failed, None),
        }
    }
    let extension = thumbnail_extension(&url);
    let name = format!("{stem}.thumbnail.{extension}");
    match tokio::fs::write(directory.join(&name), &bytes).await {
        Ok(()) => outcome("thumbnail", SidecarStatus::Captured, Some(name)),
        Err(_) => outcome("thumbnail", SidecarStatus::Failed, None),
    }
}

/// Requests `address`, following up to [`MAX_THUMBNAIL_REDIRECTS`] redirects by hand so every
/// hop is checked against `policy` first; the successful response, or `None`.
///
/// The recording's cookies go to the thumbnail's own origin alone, never to a hop elsewhere.
async fn follow(
    policy: &rd_http::AddressPolicy,
    client: &reqwest::Client,
    cookies: Option<&StreamCookies>,
    address: &str,
) -> Option<reqwest::Response> {
    let first = url::Url::parse(address).ok()?;
    let cookie = cookies
        .and_then(|cookies| cookies.header_for(address))
        .and_then(|header| reqwest::header::HeaderValue::from_str(&header).ok())
        .map(|mut value| {
            value.set_sensitive(true);
            value
        });
    let mut current = first.clone();
    for _ in 0..=MAX_THUMBNAIL_REDIRECTS {
        if !hop_permitted(policy, &current).await {
            tracing::info!(
                host = current.host_str().unwrap_or_default(),
                "thumbnail address not permitted by the address policy"
            );
            return None;
        }
        let mut request = client.get(current.clone());
        if let Some(cookie) = &cookie
            && current.origin() == first.origin()
        {
            request = request.header(reqwest::header::COOKIE, cookie.clone());
        }
        let response = request.send().await.ok()?;
        let status = response.status();
        if status.is_redirection() {
            current = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|location| location.to_str().ok())
                .and_then(|location| current.join(location).ok())?;
            continue;
        }
        return status.is_success().then_some(response);
    }
    None
}

/// Whether `target` may be requested: scheme, port and a literal address by the policy's hop
/// rule, and every address a name resolves to here. A name this machine cannot resolve is
/// left to the connection, which fails on it or, behind a proxy that resolves names itself,
/// hands it on.
async fn hop_permitted(policy: &rd_http::AddressPolicy, target: &url::Url) -> bool {
    if policy.hop_refusal(target).is_some() {
        return false;
    }
    !matches!(
        rd_http::check_target(policy, &rd_http::SystemLookup, target).await,
        Err(rd_http::TargetRefusal::Refused(_))
    )
}

/// Pulls a thumbnail address out of streamlink's metadata block.
#[must_use]
pub fn thumbnail_url(probe: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(probe).ok()?;
    value
        .get("metadata")?
        .get("thumbnail")?
        .as_str()
        .filter(|url| url.starts_with("http"))
        .map(str::to_owned)
}

/// The extension of a thumbnail address, defaulting to `jpg`.
///
/// Taken from the path only: a query string routinely contains dots, and using it would
/// produce names like `show.thumbnail.jpg?width=640`.
#[must_use]
pub fn thumbnail_extension(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .filter(|extension| {
            (1..=5).contains(&extension.len()) && extension.chars().all(char::is_alphanumeric)
        })
        .unwrap_or_else(|| "jpg".to_owned())
}

async fn write_text(directory: &Path, name: &str, contents: &str) -> SidecarOutcome {
    match tokio::fs::write(directory.join(name), contents).await {
        Ok(()) => outcome_owned("metadata", SidecarStatus::Captured, Some(name.to_owned())),
        Err(_) => outcome("metadata", SidecarStatus::Failed, None),
    }
}

fn outcome(kind: &str, status: SidecarStatus, file_name: Option<String>) -> SidecarOutcome {
    outcome_owned(kind, status, file_name)
}

fn outcome_owned(kind: &str, status: SidecarStatus, file_name: Option<String>) -> SidecarOutcome {
    SidecarOutcome {
        kind: kind.to_owned(),
        status,
        file_name,
    }
}

#[cfg(test)]
#[path = "sidecars_tests.rs"]
mod tests;
