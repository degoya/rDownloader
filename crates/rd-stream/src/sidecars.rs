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
/// the stream available without a provider-specific client.
pub async fn capture(
    clients: &SidecarClients,
    streamlink: &Path,
    url: &str,
    directory: &Path,
    stem: &str,
    policy: SidecarPolicy,
) -> Vec<SidecarOutcome> {
    let mut outcomes = Vec::new();
    let probe = crate::probe::probe_json(streamlink, url).await.ok();

    if policy.metadata {
        outcomes.push(match &probe {
            Some(json) => write_text(directory, &format!("{stem}.metadata.json"), json).await,
            None => outcome("metadata", SidecarStatus::NotOffered, None),
        });
    }
    if policy.thumbnail {
        outcomes.push(capture_thumbnail(clients, probe.as_deref(), directory, stem).await);
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

/// The client every thumbnail fetch shares, and the trust roots it was built with.
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
#[derive(Default)]
pub struct SidecarClients {
    network: SharedNetworkDefaults,
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
            cached: tokio::sync::Mutex::new(None),
        }
    }

    /// The current client, built on first use and rebuilt when the trust roots change.
    ///
    /// `None` when the roots cannot be parsed or the client cannot be built. A recording is
    /// never failed for it: the thumbnail is reported as `Failed` and the recording itself,
    /// which is the thing the person asked for, is untouched.
    async fn client(&self) -> Option<reqwest::Client> {
        let (custom_ca_pem, tls_revision) = {
            let defaults = self.network.read().await;
            (defaults.custom_ca_pem.clone(), defaults.tls_revision)
        };
        let mut cached = self.cached.lock().await;
        if let Some((revision, client)) = cached.as_ref()
            && *revision == tls_revision
        {
            return Some(client.clone());
        }
        let mut builder = reqwest::Client::builder()
            .connect_timeout(THUMBNAIL_CONNECT_TIMEOUT)
            .timeout(THUMBNAIL_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(MAX_THUMBNAIL_REDIRECTS));
        for pem in &custom_ca_pem {
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
            Ok(client) => {
                *cached = Some((tls_revision, client.clone()));
                Some(client)
            }
            Err(error) => {
                tracing::warn!(%error, "no HTTP client for sidecars");
                None
            }
        }
    }
}

async fn capture_thumbnail(
    clients: &SidecarClients,
    probe: Option<&str>,
    directory: &Path,
    stem: &str,
) -> SidecarOutcome {
    let Some(url) = probe.and_then(thumbnail_url) else {
        return outcome("thumbnail", SidecarStatus::NotOffered, None);
    };
    let Some(client) = clients.client().await else {
        return outcome("thumbnail", SidecarStatus::Failed, None);
    };
    let Ok(mut response) = client.get(&url).send().await else {
        return outcome("thumbnail", SidecarStatus::Failed, None);
    };
    if !response.status().is_success() {
        return outcome("thumbnail", SidecarStatus::Failed, None);
    }
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
mod tests {
    use super::{thumbnail_extension, thumbnail_url};

    #[test]
    fn a_thumbnail_address_is_read_from_the_metadata_block() {
        let probe = r#"{"metadata":{"title":"Show","thumbnail":"https://cdn.test/t.png"}}"#;
        assert_eq!(
            thumbnail_url(probe).as_deref(),
            Some("https://cdn.test/t.png")
        );
    }

    #[test]
    fn a_missing_or_non_http_thumbnail_is_not_offered() {
        assert!(thumbnail_url(r#"{"metadata":{"title":"Show"}}"#).is_none());
        assert!(thumbnail_url(r#"{"streams":{}}"#).is_none());
        // A relative or data address is not something to fetch.
        assert!(thumbnail_url(r#"{"metadata":{"thumbnail":"/t.png"}}"#).is_none());
        assert!(thumbnail_url("not json").is_none());
    }

    #[test]
    fn the_extension_comes_from_the_path_and_not_the_query() {
        // Without this the file would be called `show.thumbnail.jpg?width=640`.
        assert_eq!(
            thumbnail_extension("https://cdn.test/t.png?width=640"),
            "png"
        );
        assert_eq!(thumbnail_extension("https://cdn.test/t.jpeg"), "jpeg");
        assert_eq!(thumbnail_extension("https://cdn.test/t.WEBP"), "webp");
    }

    #[test]
    fn an_address_with_no_usable_extension_defaults_to_jpg() {
        assert_eq!(thumbnail_extension("https://cdn.test/thumb"), "jpg");
        assert_eq!(
            thumbnail_extension("https://cdn.test/a.b.c.verylongext"),
            "jpg"
        );
        assert_eq!(thumbnail_extension("https://cdn.test/t.p%20g"), "jpg");
    }
}
