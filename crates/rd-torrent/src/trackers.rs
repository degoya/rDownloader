//! Tracker inspection and control.
//!
//! librqbit takes a tracker list when a torrent is added and never exposes it again: there
//! is no runtime add or remove, no reannounce and no scrape. What is deliverable on top of
//! it is built here — rDownloader owns the list, applies it on every add, forces a fresh
//! announce by cycling the torrent, and runs its own scrape client.
//!
//! Tracker URLs carry passkeys on private trackers, so the full URL never leaves the
//! service: the API addresses entries by their derived id and only ever sees the redacted
//! form.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use rd_core::{DownloadId, TrackerScrape};

use crate::{TorrentService, error::TorrentError};

/// Shortest interval between two manual reannounces of one torrent.
///
/// A reannounce cycles the torrent through the engine, and trackers ban clients that
/// announce in a tight loop, so the limit protects both sides.
pub const REANNOUNCE_INTERVAL: Duration = Duration::from_secs(60);

/// How long a scrape result counts as current.
pub const SCRAPE_FRESHNESS: Duration = Duration::from_secs(15 * 60);

/// Timeout for one scrape round-trip.
const SCRAPE_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether a scrape result is too old to be presented as current.
#[must_use]
pub fn is_stale(
    scraped_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    (now - scraped_at)
        .to_std()
        .is_ok_and(|age| age > SCRAPE_FRESHNESS)
}

/// Derives the scrape URL of an HTTP announce URL as BEP 48 defines it.
///
/// Only an announce URL whose last path segment is exactly `announce` has a scrape
/// endpoint; anything else returns `None` rather than a guessed URL that would 404.
#[must_use]
pub fn scrape_url(announce: &str) -> Option<String> {
    let mut url = url::Url::parse(announce).ok()?;
    let segments: Vec<String> = url
        .path_segments()?
        .map(std::borrow::ToOwned::to_owned)
        .collect();
    let (last, head) = segments.split_last()?;
    let replaced = last
        .strip_prefix("announce")
        .map(|rest| format!("scrape{rest}"))?;
    let mut path = head.to_vec();
    path.push(replaced);
    url.set_path(&path.join("/"));
    Some(url.into())
}

/// Fetches the scrape counters of one tracker.
pub(crate) async fn scrape(announce: &str, info_hash: &str) -> Result<TrackerScrape> {
    let url = url::Url::parse(announce).context("tracker URL is invalid")?;
    // A tracker URL can come straight out of an untrusted `.torrent`, so it is a
    // request target an attacker chooses. Refuse the ones that only make sense as an
    // attack before anything is sent.
    let addresses = reject_internal_target(&url).await?;
    let raw = hex::decode(info_hash).context("info hash is not hex")?;
    match url.scheme() {
        "http" | "https" => scrape_http(announce, &raw).await,
        "udp" => scrape_udp(announce, &raw, &addresses).await,
        other => bail!("scrape is not defined for {other} trackers"),
    }
}

/// Refuses a tracker whose host resolves to an address that cannot be a real tracker, and
/// returns the addresses it checked.
///
/// A `.torrent` from a public site is attacker-controlled input, and a scrape is a request
/// rDownloader makes on the user's behalf, so a hostile announce URL is a server-side
/// request forgery primitive: `169.254.169.254` is the cloud metadata endpoint, loopback
/// reaches rDownloader's own API and anything else bound locally. The rule is the one a
/// Metalink's mirrors keep to (`rd_http::AddressPolicy`).
///
/// Private LAN ranges stay allowed on purpose — a self-hosted tracker on the local network
/// is a legitimate setup, and blocking it would break more than it protects.
async fn reject_internal_target(url: &url::Url) -> Result<Vec<std::net::IpAddr>> {
    rd_http::check_target(&tracker_policy(), &rd_http::SystemLookup, url)
        .await
        .map_err(|refusal| match refusal {
            rd_http::TargetRefusal::Refused(_) => {
                anyhow::anyhow!("tracker address is not routable on the internet")
            }
            rd_http::TargetRefusal::Unresolved(error) => {
                anyhow::Error::new(error).context("tracker host could not be resolved")
            }
        })
}

/// Public addresses and the person's own network; never this machine.
fn tracker_policy() -> rd_http::AddressPolicy {
    rd_http::AddressPolicy::new(true)
}

/// HTTP(S) scrape (BEP 48).
async fn scrape_http(announce: &str, info_hash: &[u8]) -> Result<TrackerScrape> {
    let base = scrape_url(announce).context("tracker offers no scrape endpoint")?;
    // `info_hash` is raw bytes, so it has to be percent-encoded by hand: the query-pair
    // encoder would mangle the non-UTF-8 bytes.
    let encoded: String = info_hash
        .iter()
        .map(|byte| format!("%{byte:02X}"))
        .collect();
    let separator = if base.contains('?') { '&' } else { '?' };
    let request = format!("{base}{separator}info_hash={encoded}");
    let client = reqwest::Client::builder()
        .timeout(SCRAPE_TIMEOUT)
        // A redirect would send the request to a host that was never validated, which is
        // exactly the check `reject_internal_target` just performed.
        .redirect(reqwest::redirect::Policy::none())
        // The name is resolved again to connect; the guard sees that answer too, so a name
        // that pointed elsewhere a moment ago cannot point inside now.
        .dns_resolver(rd_http::GuardedResolver::system(tracker_policy()))
        .build()
        .context("build scrape client")?;
    let body = client
        .get(&request)
        .send()
        .await
        .context("scrape request failed")?
        .bytes()
        .await
        .context("read scrape response")?;
    parse_http_scrape(&body, info_hash)
}

/// Reads the counters of the requested info hash out of a scrape response.
pub(crate) fn parse_http_scrape(body: &[u8], info_hash: &[u8]) -> Result<TrackerScrape> {
    let value = crate::bencode::decode(body).context("scrape response is not bencode")?;
    if let Some(reason) = value
        .get(b"failure reason")
        .and_then(crate::bencode::Value::bytes)
    {
        // Tracker-supplied text that ends up in front of the user: bounded so a hostile
        // tracker cannot push an essay into the error field.
        let reason: String = String::from_utf8_lossy(reason).chars().take(200).collect();
        bail!("tracker refused the scrape: {reason}");
    }
    let files = value
        .get(b"files")
        .context("scrape response has no files")?;
    let entries = files
        .entries()
        .context("scrape files is not a dictionary")?;
    // Prefer the requested hash; a tracker that returns a single entry may key it
    // differently, so a lone entry is accepted as the answer.
    let counters = entries
        .iter()
        .find(|(key, _)| *key == info_hash)
        .or_else(|| entries.first())
        .map(|(_, value)| value)
        .context("scrape response has no counters")?;
    let read = |key: &[u8]| -> u32 {
        counters
            .get(key)
            .and_then(crate::bencode::Value::integer)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or_default()
    };
    Ok(TrackerScrape {
        seeders: read(b"complete"),
        leechers: read(b"incomplete"),
        completed: read(b"downloaded"),
        scraped_at: chrono::Utc::now(),
    })
}

/// UDP scrape (BEP 15): connect handshake, then one scrape request — sent to the addresses
/// `reject_internal_target` checked rather than to a second lookup of the name, which could
/// answer differently.
async fn scrape_udp(
    announce: &str,
    info_hash: &[u8],
    addresses: &[std::net::IpAddr],
) -> Result<TrackerScrape> {
    let url = url::Url::parse(announce).context("tracker URL is invalid")?;
    let port = url.port().unwrap_or(80);
    let targets: Vec<std::net::SocketAddr> = addresses
        .iter()
        .map(|address| std::net::SocketAddr::new(*address, port))
        .collect();
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .context("bind scrape socket")?;
    socket
        .connect(&targets[..])
        .await
        .context("connect to the tracker")?;

    let transaction: u32 = rand_transaction();
    socket
        .send(&connect_request(transaction))
        .await
        .context("send connect request")?;
    let mut buffer = [0_u8; 1024];
    let read = tokio::time::timeout(SCRAPE_TIMEOUT, socket.recv(&mut buffer))
        .await
        .context("tracker did not answer the connect request")?
        .context("read connect response")?;
    let connection = parse_connect_response(&buffer[..read], transaction)?;

    let transaction = rand_transaction();
    socket
        .send(&scrape_request(connection, transaction, info_hash))
        .await
        .context("send scrape request")?;
    let read = tokio::time::timeout(SCRAPE_TIMEOUT, socket.recv(&mut buffer))
        .await
        .context("tracker did not answer the scrape request")?
        .context("read scrape response")?;
    parse_udp_scrape(&buffer[..read], transaction)
}

/// A transaction id; only has to be unpredictable enough to match request and response.
fn rand_transaction() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos() as u64)
            .unwrap_or_default(),
    );
    hasher.finish() as u32
}

/// The BEP 15 connect request: magic protocol id, action 0, transaction id.
pub(crate) fn connect_request(transaction: u32) -> [u8; 16] {
    let mut request = [0_u8; 16];
    request[..8].copy_from_slice(&0x0417_2710_1980_u64.to_be_bytes());
    request[8..12].copy_from_slice(&0_u32.to_be_bytes());
    request[12..].copy_from_slice(&transaction.to_be_bytes());
    request
}

/// Reads the connection id out of a connect response.
pub(crate) fn parse_connect_response(response: &[u8], transaction: u32) -> Result<u64> {
    anyhow::ensure!(response.len() >= 16, "connect response is too short");
    let action = u32::from_be_bytes(response[0..4].try_into()?);
    let answered = u32::from_be_bytes(response[4..8].try_into()?);
    anyhow::ensure!(action == 0, "tracker rejected the connect request");
    anyhow::ensure!(answered == transaction, "connect response does not match");
    Ok(u64::from_be_bytes(response[8..16].try_into()?))
}

/// The BEP 15 scrape request for exactly one info hash.
pub(crate) fn scrape_request(connection: u64, transaction: u32, info_hash: &[u8]) -> Vec<u8> {
    let mut request = Vec::with_capacity(36);
    request.extend_from_slice(&connection.to_be_bytes());
    request.extend_from_slice(&2_u32.to_be_bytes());
    request.extend_from_slice(&transaction.to_be_bytes());
    request.extend_from_slice(info_hash);
    request
}

/// Reads the counters out of a UDP scrape response.
pub(crate) fn parse_udp_scrape(response: &[u8], transaction: u32) -> Result<TrackerScrape> {
    anyhow::ensure!(response.len() >= 8, "scrape response is too short");
    let action = u32::from_be_bytes(response[0..4].try_into()?);
    let answered = u32::from_be_bytes(response[4..8].try_into()?);
    anyhow::ensure!(answered == transaction, "scrape response does not match");
    anyhow::ensure!(action == 2, "tracker rejected the scrape request");
    anyhow::ensure!(response.len() >= 20, "scrape response carries no counters");
    Ok(TrackerScrape {
        seeders: u32::from_be_bytes(response[8..12].try_into()?),
        completed: u32::from_be_bytes(response[12..16].try_into()?),
        leechers: u32::from_be_bytes(response[16..20].try_into()?),
        scraped_at: chrono::Utc::now(),
    })
}

impl TorrentService {
    /// Forces a fresh announce to every tracker of one torrent.
    ///
    /// The engine has no reannounce call, so the torrent is paused and unpaused, which
    /// makes it re-announce with the currently persisted tracker list.
    pub async fn reannounce(&self, id: DownloadId) -> Result<()> {
        let now = Instant::now();
        {
            let registry = self.inner.registry.read().await;
            let entry = registry
                .get(id)
                .context(TorrentError::not_active("torrent is not active"))?;
            if let Some(last) = entry.last_reannounce
                && now.duration_since(last) < REANNOUNCE_INTERVAL
            {
                let wait = REANNOUNCE_INTERVAL - now.duration_since(last);
                // Tagged, not merely worded: the caller answers `429` for this and `400` for
                // every other refusal here, and it used to tell them apart by looking for the
                // word "wait" in this very sentence.
                return Err(TorrentError::rate_limited(format!(
                    "wait {} seconds before announcing again",
                    wait.as_secs() + 1
                ))
                .into());
            }
        }
        let entry = self
            .inner
            .registry
            .read()
            .await
            .get(id)
            .cloned()
            .context(TorrentError::not_active("torrent is not active"))?;
        let session = self.session().await?;
        let handle = session
            .get(entry.handle())
            .context(TorrentError::not_active("torrent is not in the session"))?;
        session
            .pause(&handle)
            .await
            .context("pause for reannounce")?;
        session
            .unpause(&handle)
            .await
            .context("resume after reannounce")?;
        self.inner.registry.write().await.mark_reannounce(id, now);
        Ok(())
    }

    /// Scrapes every tracker of one torrent and stores the counters.
    ///
    /// One tracker failing never fails the whole call: its previous counters are kept and
    /// the reason is stored on that entry alone.
    pub async fn scrape_trackers(&self, id: DownloadId) -> Result<rd_core::TorrentJobState> {
        let mut state = self.job_state(id).await;
        let metadata = state
            .metadata
            .as_mut()
            .context("torrent metadata is not known yet")?;
        let info_hash = metadata.info_hash.clone();
        for tracker in &mut metadata.trackers {
            match scrape(&tracker.url, &info_hash).await {
                Ok(counters) => {
                    tracker.scrape = Some(counters);
                    tracker.last_error = None;
                }
                Err(error) => {
                    // The URL may hold a passkey, so only the reason is kept.
                    tracker.last_error = Some(format!("{error:#}"));
                }
            }
        }
        self.store_job_state(id, state.clone()).await;
        Ok(state)
    }

    /// The tracker list applied when the torrent is added.
    pub(crate) async fn tracker_urls(&self, id: DownloadId) -> Option<Vec<String>> {
        let state = self.job_state(id).await;
        let trackers = state.metadata.as_ref()?.trackers.clone();
        if trackers.is_empty() {
            return None;
        }
        Some(trackers.into_iter().map(|tracker| tracker.url).collect())
    }
}

#[cfg(test)]
#[path = "trackers_tests.rs"]
mod tests;
