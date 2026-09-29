//! HEAD/range probe for direct HTTP links (no provider involved).

use rd_core::{ByteCount, FailureKind, LinkCheckResult, LinkStatus};
use rd_http::HttpDownloadError;
use url::Url;

/// Probes one direct link; 404/410/451 mean offline, other failures stay unknown.
pub async fn probe_direct(
    client: &reqwest::Client,
    headers: &[(String, String)],
    url: Url,
) -> LinkCheckResult {
    // Deliberately no fallback to the address's last segment: intake already stored that,
    // and a name recorded by the check is marked as one the *source* declared. A segment two
    // links happen to share — `/download`, or an indexer's `/api` — is no such name, and
    // mirror detection and package grouping both read that flag.
    let declared_name = |disposition: Option<&str>| disposition.and_then(disposition_file_name);
    match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        rd_http::probe_with_headers(client, url.clone(), headers),
    )
    .await
    {
        // The one judgement about "is this a file", asked rather than rebuilt: an address
        // that answers with markup, or with a text body too small to be the payload, is not
        // a download and must not be stored as one (RD-110-07). The very same function
        // decides it mid-transfer in `rd_http::engine`, which is why there is no second
        // opinion to drift from this one.
        Ok(Ok(result)) if !result.looks_downloadable() => LinkCheckResult {
            url,
            status: LinkStatus::Unresolvable,
            file_name: None,
            size: None,
            media: None,
        },
        Ok(Ok(result)) => LinkCheckResult {
            url,
            status: LinkStatus::Online,
            file_name: declared_name(result.content_disposition.as_deref()),
            size: result
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
    }
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
        std::time::Duration::from_secs(30),
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
    Nzb,
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
    const SNIFF_BYTES: usize = 1024;

    let body = tokio::time::timeout(
        std::time::Duration::from_secs(30),
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
    if body.complete && rd_torrent::parse_torrent(&body.bytes).is_ok() {
        return Some(Sniffed::Torrent(Some(body.bytes)));
    }
    let head = String::from_utf8_lossy(&body.bytes[..body.bytes.len().min(SNIFF_BYTES)]);
    let head = head.trim_start();
    // A torrent is bencode and begins with a dictionary; the announce key follows almost
    // immediately in every file a tracker hands out.
    if head.starts_with("d8:announce") || head.starts_with("d7:comment") {
        return Some(Sniffed::Torrent(None));
    }
    // An NZB is XML whose root element is `<nzb>`; the doctype and the declaration above
    // it are optional, so the element is what is looked for.
    let lowercase = head.to_ascii_lowercase();
    (lowercase.contains("<nzb") && lowercase.contains("xml")).then_some(Sniffed::Nzb)
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

/// Parses `filename*=UTF-8''…` (RFC 5987) or `filename="…"`.
#[must_use]
pub fn disposition_file_name(value: &str) -> Option<String> {
    let mut plain = None;
    for part in value.split(';').map(str::trim) {
        if let Some(rest) = part.strip_prefix("filename*=") {
            let encoded = rest.trim_matches('"');
            let encoded = encoded.splitn(3, '\'').nth(2).unwrap_or(encoded);
            if let Ok(decoded) = percent_encoding::percent_decode_str(encoded).decode_utf8()
                && !decoded.is_empty()
            {
                return Some(decoded.into_owned());
            }
        } else if let Some(rest) = part.strip_prefix("filename=") {
            let name = rest.trim_matches(['"', '\'']).trim();
            if !name.is_empty() {
                plain = Some(name.to_owned());
            }
        }
    }
    plain
}

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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
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
