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
    network: &ToolNetwork,
) -> Vec<SidecarOutcome> {
    let mut outcomes = Vec::new();
    let probe = crate::probe::probe_json(streamlink, url, network)
        .await
        .ok();

    if policy.metadata {
        outcomes.push(match &probe {
            Some(json) => write_text(directory, &format!("{stem}.metadata.json"), json).await,
            None => outcome("metadata", SidecarStatus::NotOffered, None),
        });
    }
    if policy.thumbnail {
        outcomes.push(
            capture_thumbnail(clients, network.proxy(), probe.as_deref(), directory, stem).await,
        );
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
///
/// The recording's proxy applies to its thumbnail as to its stream (RD-1240-22). A proxied
/// fetch gets a client of its own, built per recording: there is one thumbnail per recording,
/// and a cached client would carry one recording's proxy into the next.
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
            return build_client(&custom_ca_pem, proxy);
        }
        let mut cached = self.cached.lock().await;
        if let Some((revision, client)) = cached.as_ref()
            && *revision == tls_revision
        {
            return Some(client.clone());
        }
        let client = build_client(&custom_ca_pem, None)?;
        *cached = Some((tls_revision, client.clone()));
        Some(client)
    }
}

/// A thumbnail client with the caps above, the custom CA and, when given, the proxy alone:
/// the service's own proxy variables do not apply beside a profile, as in `rd-http`.
fn build_client(custom_ca_pem: &[Vec<u8>], proxy: Option<&ToolProxy>) -> Option<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(THUMBNAIL_CONNECT_TIMEOUT)
        .timeout(THUMBNAIL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(MAX_THUMBNAIL_REDIRECTS));
    if let Some(proxy) = proxy {
        match proxy.http_proxy() {
            Ok(proxy) => builder = builder.no_proxy().proxy(proxy),
            Err(error) => {
                tracing::warn!(
                    error = %format!("{error:#}"),
                    "no proxy for the thumbnail; it is not fetched without it"
                );
                return None;
            }
        }
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

async fn capture_thumbnail(
    clients: &SidecarClients,
    proxy: Option<&ToolProxy>,
    probe: Option<&str>,
    directory: &Path,
    stem: &str,
) -> SidecarOutcome {
    let Some(url) = probe.and_then(thumbnail_url) else {
        return outcome("thumbnail", SidecarStatus::NotOffered, None);
    };
    let Some(client) = clients.client(proxy).await else {
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

/// RD-1240-22 — a recording's thumbnail is fetched through the recording's proxy.
#[cfg(test)]
mod proxy_tests {
    use rd_core::{ProxyKind, SidecarStatus};
    use rd_scheduler::ToolProxy;
    use secrecy::SecretString;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{SidecarClients, capture_thumbnail};

    const THUMBNAIL: &[u8] = b"\x89PNG-thumbnail";

    /// A proxy that answers one request with the thumbnail and hands back the head it read.
    async fn one_shot_proxy() -> (std::net::SocketAddr, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut head = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).await.expect("read");
                if read == 0 {
                    break;
                }
                head.extend_from_slice(&buffer[..read]);
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                THUMBNAIL.len()
            );
            socket.write_all(response.as_bytes()).await.expect("write");
            socket.write_all(THUMBNAIL).await.expect("write");
            let _ = socket.shutdown().await;
            String::from_utf8_lossy(&head).into_owned()
        });
        (address, server)
    }

    #[tokio::test]
    async fn the_thumbnail_goes_through_the_recordings_proxy() {
        let (address, server) = one_shot_proxy().await;
        let proxy = ToolProxy::new(
            ProxyKind::Http,
            format!("http://{address}").parse().expect("endpoint"),
            Some("alice"),
            Some(&SecretString::from("pr0xy-secret".to_owned())),
        )
        .expect("proxy");
        let directory = tempfile::tempdir().expect("directory");
        // A name nothing resolves: only the proxy can have answered.
        let probe = r#"{"metadata":{"thumbnail":"http://thumbnails.invalid/show.png"}}"#;

        let outcome = capture_thumbnail(
            &SidecarClients::new(),
            Some(&proxy),
            Some(probe),
            directory.path(),
            "show",
        )
        .await;
        assert_eq!(outcome.status, SidecarStatus::Captured);
        assert_eq!(
            std::fs::read(directory.path().join("show.thumbnail.png")).expect("thumbnail"),
            THUMBNAIL
        );
        let head = server.await.expect("server");
        assert!(
            head.starts_with("GET http://thumbnails.invalid/show.png HTTP/1.1\r\n"),
            "{head}"
        );
        // `alice:pr0xy-secret`, as the proxy's own sign-in, never in the address.
        assert!(
            head.lines()
                .any(|line| line.split_once(':').is_some_and(|(name, value)| name
                    .eq_ignore_ascii_case("proxy-authorization")
                    && value.trim() == "Basic YWxpY2U6cHIweHktc2VjcmV0")),
            "{head}"
        );
    }
}
