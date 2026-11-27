//! HEAD/range probe for direct HTTP links (no provider involved).

use std::time::Duration;

use rd_core::{ByteCount, FailureKind, LinkCheckResult, LinkStatus};
use rd_http::HttpDownloadError;
use url::Url;

/// How many bytes identify a document that did not announce itself (RD-080-11).
const SNIFF_BYTES: usize = 1024;

/// How long each network step of a direct link's check may take (RD-1490-13). A step past its
/// deadline ends the way a failed one does, so no check stays "checking" for good.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Deadlines {
    /// The probe: connection, TLS, redirects and the answer's headers.
    pub(crate) probe: Duration,
    /// Reading an answer: the sniff, or a container on to its end.
    pub(crate) read: Duration,
}

impl Deadlines {
    /// The deadlines of the online check.
    pub(crate) const CHECK: Self = Self {
        probe: Duration::from_secs(15),
        read: Duration::from_secs(30),
    };
}

/// What the direct probe learnt, handed on so the rest of the check does not ask the address a
/// second time (RD-1490-02): an indexer proxy counts every GET as a grab.
pub(crate) enum DirectProbe {
    /// Nothing was asked: there was no client. The reclassification probes for itself.
    Unasked,
    /// The address answered with an error, could not be reached or did not answer within the
    /// probe's deadline. Asking it again would only wait as long a second time (RD-1490-13).
    Failed,
    Answered(Box<Probed>),
}

/// What a probe learnt about an address.
pub(crate) struct Probed {
    pub(crate) probe: rd_http::ProbeResult,
    /// The probe's own answer as far as it was read, with its headers, when the server ignored
    /// the one-byte range and sent the whole entity: to the end for a container, else the
    /// sniff. `None` when nothing but the probe was asked for, or the answer stalled.
    pub(crate) body: Option<(rd_http::SniffedBody, reqwest::header::HeaderMap)>,
    /// The answer stopped sending within the read's deadline: nothing more is read from this
    /// address, neither a sniff nor a manifest (RD-1490-13).
    pub(crate) stalled: bool,
}

/// Probes one direct link; 404/410/451 mean offline, other failures stay unknown.
pub async fn probe_direct(
    client: &reqwest::Client,
    headers: &[(String, String)],
    url: Url,
) -> LinkCheckResult {
    probe_direct_kept(client, headers, url).await.0
}

/// [`probe_direct`], keeping what the probe read for the reclassification.
pub(crate) async fn probe_direct_kept(
    client: &reqwest::Client,
    headers: &[(String, String)],
    url: Url,
) -> (LinkCheckResult, DirectProbe) {
    probe_direct_within(client, headers, url, Deadlines::CHECK).await
}

/// [`probe_direct_kept`] within the given deadlines.
pub(crate) async fn probe_direct_within(
    client: &reqwest::Client,
    headers: &[(String, String)],
    url: Url,
    deadlines: Deadlines,
) -> (LinkCheckResult, DirectProbe) {
    // Deliberately no fallback to the address's last segment: intake already stored that,
    // and a name recorded by the check is marked as one the *source* declared. A segment two
    // links happen to share — `/download`, or an indexer's `/api` — is no such name, and
    // mirror detection and package grouping both read that flag.
    let declared_name = |disposition: Option<&str>| disposition.and_then(disposition_file_name);
    let answer = tokio::time::timeout(
        deadlines.probe,
        rd_http::probe_keeping_answer(client, url.clone(), headers),
    )
    .await;
    let result = match &answer {
        // The one judgement about "is this a file", asked rather than rebuilt: an address
        // that answers with markup, or with a text body too small to be the payload, is not
        // a download and must not be stored as one (RD-110-07). The very same function
        // decides it mid-transfer in `rd_http::engine`, which is why there is no second
        // opinion to drift from this one.
        Ok(Ok((probe, _))) if !probe.looks_downloadable() => LinkCheckResult {
            url,
            status: LinkStatus::Unresolvable,
            file_name: None,
            size: None,
            media: None,
        },
        Ok(Ok((probe, _))) => LinkCheckResult {
            url,
            status: LinkStatus::Online,
            file_name: declared_name(probe.content_disposition.as_deref()),
            size: probe
                .total_bytes
                .and_then(|value| ByteCount::new(value).ok()),
            media: None,
        },
        Ok(Err(HttpDownloadError::Failure(failure)))
            if failure.category == FailureKind::Permanent && failure.message.contains("HTTP 4") =>
        {
            LinkCheckResult {
                url,
                status: LinkStatus::Offline,
                file_name: None,
                size: None,
                media: None,
            }
        }
        _ => LinkCheckResult {
            url,
            status: LinkStatus::Unknown,
            file_name: None,
            size: None,
            media: None,
        },
    };
    let probe = match answer {
        Ok(Ok((probe, answer))) => {
            DirectProbe::Answered(Box::new(read_answer(probe, answer, deadlines.read).await))
        }
        Ok(Err(_)) | Err(_) => DirectProbe::Failed,
    };
    (result, probe)
}

/// The probe of a direct link, for a reclassification the direct probe did not ask; `None`
/// for one that failed or ran out of time.
pub(crate) async fn probe_sniffed(
    client: &reqwest::Client,
    headers: &[(String, String)],
    url: &Url,
) -> Option<Probed> {
    let deadlines = Deadlines::CHECK;
    let probed = rd_http::probe_keeping_answer(client, url.clone(), headers);
    let (probe, answer) = tokio::time::timeout(deadlines.probe, probed)
        .await
        .ok()?
        .ok()?;
    Some(read_answer(probe, answer, deadlines.read).await)
}

/// Reads the probe's own answer when the server sent it whole: on to its end when it is a
/// container — a torrent or an NZB, by its content type or its first bytes — else the sniff.
/// Bounded in time like [`sniff_document`]; an answer that stalls is dropped and marked, and
/// nothing more is read from the address.
async fn read_answer(
    probe: rd_http::ProbeResult,
    answer: Option<rd_http::WholeAnswer>,
    deadline: Duration,
) -> Probed {
    let Some(answer) = answer else {
        return Probed {
            probe,
            body: None,
            stalled: false,
        };
    };
    let answered = answer.headers().clone();
    let essence = content_essence(probe.content_type.as_deref());
    let read = answer.read(SNIFF_BYTES, |head| {
        if rd_core::TORRENT_CONTENT_TYPES.contains(&essence.as_str()) || looks_like_bencode(head) {
            Some(rd_torrent::MAX_TORRENT_BYTES)
        } else if rd_core::NZB_CONTENT_TYPES.contains(&essence.as_str()) || looks_like_nzb(head) {
            Some(rd_collector::MAX_NZB_BYTES)
        } else {
            None
        }
    });
    let (body, stalled) = match tokio::time::timeout(deadline, read).await {
        Ok(body) => (body.map(|body| (body, answered)), false),
        Err(_) => (None, true),
    };
    Probed {
        probe,
        body,
        stalled,
    }
}

/// A content type's essence, lower-case and without its parameters; empty without one.
pub(crate) fn content_essence(content_type: Option<&str>) -> String {
    content_type
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default()
}

/// Reads a link that answered as a `.torrent`, for its name and file tree (RD-120-68).
///
/// Bounded by the same 16 MiB an upload is, and `None` for anything that is not a torrent
/// after all: a failure here only means the link is named the way it was before.
///
/// The bytes are kept for the download (RD-130-18), so queueing the link does not fetch the
/// file a second time. Failing to keep them costs exactly that second fetch, nothing more.
/// `sniffed` is the file when the sniff already read it whole, and then nothing is fetched.
pub(crate) async fn read_torrent(
    client: &reqwest::Client,
    headers: &[(String, String)],
    candidate: &rd_core::LinkCandidate,
    torrent: &rd_torrent::TorrentService,
    sniffed: Option<Vec<u8>>,
) -> Option<rd_torrent::ParsedTorrent> {
    let bytes = if let Some(bytes) = sniffed {
        bytes
    } else {
        fetch_torrent(client, headers, candidate).await?
    };
    let parsed = rd_torrent::parse_torrent(&bytes)
        .inspect_err(|error| {
            tracing::debug!(url = %rd_core::redact_url(&candidate.url), %error, "re-routed torrent is not a torrent");
        })
        .ok()?;
    if let Err(error) = torrent.keep_prefetched(candidate.id, &bytes).await {
        tracing::warn!(%error, "a re-routed torrent could not be kept for its download");
    }
    Some(parsed)
}

async fn fetch_torrent(
    client: &reqwest::Client,
    headers: &[(String, String)],
    candidate: &rd_core::LinkCandidate,
) -> Option<Vec<u8>> {
    match tokio::time::timeout(
        Deadlines::CHECK.read,
        rd_http::fetch_bytes(
            client,
            candidate.url.clone(),
            headers,
            rd_torrent::MAX_TORRENT_BYTES,
        ),
    )
    .await
    {
        Ok(Ok(bytes)) => Some(bytes),
        Ok(Err(error)) => {
            tracing::debug!(url = %rd_core::redact_url(&candidate.url), %error, "re-routed torrent could not be read");
            None
        }
        Err(_) => None,
    }
}

/// What the first bytes of a direct link say it is (RD-080-11).
pub(crate) enum Sniffed {
    /// An NZB, with the whole document when the read holds it complete (RD-1490-02).
    Nzb(Option<Vec<u8>>),
    /// A torrent, with the whole file when the sniff could read it in the same response
    /// (RD-130-18).
    Torrent(Option<Vec<u8>>),
}

/// Identifies a container from the start of the document itself.
///
/// The content type is the better signal and is tried first, but it is the server's to get
/// right and plenty of indexers hand out an NZB as `application/octet-stream` or
/// `text/xml`. Routed as an ordinary link such a hit is downloaded *as a document* into the
/// download folder, which is exactly the outcome this whole path exists to prevent, so a
/// few hundred bytes are worth reading before giving up.
///
/// A beginning that looks like bencode is read on to the end of the same response, bounded
/// like an upload (RD-130-18): asking again for the whole torrent would be a second grab.
/// Everything else stays a 1 KiB sniff.
pub(crate) async fn sniff_document(
    client: &reqwest::Client,
    headers: &[(String, String)],
    url: &url::Url,
) -> Option<Sniffed> {
    let body = tokio::time::timeout(
        Deadlines::CHECK.read,
        rd_http::fetch_sniffed(
            client,
            url.clone(),
            headers,
            SNIFF_BYTES,
            rd_torrent::MAX_TORRENT_BYTES,
            looks_like_bencode,
        ),
    )
    .await
    .ok()??;
    sniffed_kind(body)
}

/// What a sniffed body says it is — one [`sniff_document`] read, or the probe's own answer
/// when the server sent it whole (RD-1490-02).
pub(crate) fn sniffed_kind(body: rd_http::SniffedBody) -> Option<Sniffed> {
    if body.complete && rd_torrent::parse_torrent(&body.bytes).is_ok() {
        return Some(Sniffed::Torrent(Some(body.bytes)));
    }
    let (torrent, nzb) = {
        let head = String::from_utf8_lossy(&body.bytes[..body.bytes.len().min(SNIFF_BYTES)]);
        let head = head.trim_start();
        // A torrent is bencode and begins with a dictionary; the announce key follows almost
        // immediately in every file a tracker hands out.
        (
            head.starts_with("d8:announce") || head.starts_with("d7:comment"),
            looks_like_nzb(head.as_bytes()),
        )
    };
    if torrent {
        return Some(Sniffed::Torrent(None));
    }
    nzb.then(|| Sniffed::Nzb(body.complete.then_some(body.bytes)))
}

/// Whether a document begins the way an NZB does. An NZB is XML whose root element is
/// `<nzb>`; the doctype and the declaration above it are optional, so the element is what is
/// looked for.
fn looks_like_nzb(head: &[u8]) -> bool {
    let lowercase = String::from_utf8_lossy(head).to_ascii_lowercase();
    lowercase.contains("<nzb") && lowercase.contains("xml")
}

/// Whether a document begins the way a torrent does: a bencoded dictionary whose first key
/// is a byte string — `d8:announce`, `d4:info`, `d13:creation date` (RD-130-18). Only such a
/// beginning is worth reading a response past the sniff for.
fn looks_like_bencode(head: &[u8]) -> bool {
    let Some(rest) = head.trim_ascii_start().strip_prefix(b"d") else {
        return false;
    };
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    (1..=3).contains(&digits) && rest.get(digits) == Some(&b':')
}

/// Parses `filename*=UTF-8''…` (RFC 5987) or `filename="…"`; shared with the download
/// worker, which names a file its address could not (RD-1240-33).
pub use rd_files::disposition_file_name;

#[cfg(test)]
#[path = "link_check_probe_deadline_tests.rs"]
mod deadline_tests;

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};

    use super::{disposition_file_name, looks_like_bencode, probe_direct};

    #[test]
    fn only_a_bencoded_dictionary_is_read_past_the_sniff() {
        assert!(looks_like_bencode(b"d8:announce35:http://tracker"));
        assert!(looks_like_bencode(b"d4:infod6:length"));
        assert!(looks_like_bencode(b"\r\nd13:creation datei1e"));
        assert!(!looks_like_bencode(b"<?xml version=\"1.0\"?><nzb"));
        assert!(!looks_like_bencode(b"#EXTM3U"));
        assert!(!looks_like_bencode(b"d:"));
        assert!(!looks_like_bencode(b"dx:"));
        assert!(!looks_like_bencode(b"d1234:"));
    }

    #[test]
    fn parses_content_disposition_variants() {
        assert_eq!(
            disposition_file_name("attachment; filename=\"a b.bin\"").as_deref(),
            Some("a b.bin")
        );
        assert_eq!(
            disposition_file_name("attachment; filename=x.bin; filename*=UTF-8''%C3%A4.bin")
                .as_deref(),
            Some("ä.bin")
        );
        assert_eq!(disposition_file_name("attachment;"), None);
    }

    #[tokio::test]
    async fn direct_probe_reports_name_size_and_offline() {
        let app = Router::new().route(
            "/file",
            get(|| async {
                (
                    [(
                        axum::http::header::CONTENT_DISPOSITION,
                        "attachment; filename=\"real.bin\"",
                    )],
                    "payload",
                )
            }),
        );
        let server = rd_testkit::serve(app).await;
        let address = server.addr();
        let client = reqwest::Client::new();
        let online = probe_direct(
            &client,
            &[],
            format!("http://{address}/file").parse().expect("URL"),
        )
        .await;
        assert_eq!(online.status, rd_core::LinkStatus::Online);
        assert_eq!(online.file_name.as_deref(), Some("real.bin"));
        assert_eq!(online.size.map(|size| size.get()), Some(7));
        let offline = probe_direct(
            &client,
            &[],
            format!("http://{address}/missing").parse().expect("URL"),
        )
        .await;
        assert_eq!(offline.status, rd_core::LinkStatus::Offline);
    }

    /// The defect RD-110-07 is about, at the check: a page answers `200` with markup, and
    /// until now that was recorded as `Online` and queued -- which downloads the page and
    /// stores it under the link's name.
    #[tokio::test]
    async fn a_page_is_not_a_file_and_an_attachment_always_is() {
        let app = Router::new()
            .route(
                "/page",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
                        "<html><body>Please wait 30 minutes</body></html>",
                    )
                }),
            )
            // Real and common: a hoster serves the payload with an HTML content type and
            // names it in a disposition. The attachment wins, here as in the engine.
            .route(
                "/attachment",
                get(|| async {
                    (
                        [
                            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
                            (
                                axum::http::header::CONTENT_DISPOSITION,
                                "attachment; filename=\"episode.mkv\"",
                            ),
                        ],
                        "payload",
                    )
                }),
            );
        let server = rd_testkit::serve(app).await;
        let address = server.addr();
        let client = reqwest::Client::new();
        let page = probe_direct(
            &client,
            &[],
            format!("http://{address}/page").parse().expect("URL"),
        )
        .await;
        assert_eq!(page.status, rd_core::LinkStatus::Unresolvable);
        assert!(!rd_core::LinkCandidateState::Unresolvable.is_enqueueable());
        let attachment = probe_direct(
            &client,
            &[],
            format!("http://{address}/attachment").parse().expect("URL"),
        )
        .await;
        assert_eq!(attachment.status, rd_core::LinkStatus::Online);
        assert_eq!(attachment.file_name.as_deref(), Some("episode.mkv"));
    }
}
