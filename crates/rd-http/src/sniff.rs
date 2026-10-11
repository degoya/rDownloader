//! Reading the start of a response, and the rest of the same response when the start says
//! it is worth it (RD-130-18).
//!
//! A link whose server names no useful content type is identified by its first bytes. When
//! those turn out to begin a document the caller wants whole — a torrent — asking the server
//! a second time would count as a second grab at an indexer that counts them, so the answer
//! already on the wire is read to its end instead.
//!
//! The request asks for the sniff alone (`Range: bytes=0-<prefix-1>`, RD-1240-33): without it a
//! server sent the whole file, and against one throttled to 256 KiB/s every online check of a
//! 4 MiB link took fifteen seconds while the package could not be queued. A server that honours
//! the range answers with the sniff and nothing more; a torrent it served that way is fetched
//! whole afterwards, which costs a second request only where the server could have told the
//! torrent apart by its content type and did not. One that ignores the range is read as before:
//! the sniff, then the connection is dropped.

use futures_util::StreamExt;
use reqwest::{Client, StatusCode, header};
use url::Url;

/// What [`fetch_sniffed`] read.
#[derive(Clone, Debug)]
pub struct SniffedBody {
    /// The first `prefix` bytes, or the whole body when `complete`.
    pub bytes: Vec<u8>,
    /// Whether `bytes` is the entire response body.
    pub complete: bool,
}

/// Reads the first `prefix` bytes of a GET, and the rest of the same response up to `limit`
/// when `read_on` accepts that beginning.
///
/// `None` for a failed request or a non-success status, as for a plain sniff. A body that
/// ends within `prefix` is complete without asking `read_on`. One that runs past `limit`, or
/// breaks off while being read on, is handed back as its first `prefix` bytes — the sniff it
/// would have been — and marked incomplete.
pub async fn fetch_sniffed(
    client: &Client,
    url: Url,
    headers: &[(String, String)],
    prefix: usize,
    limit: usize,
    read_on: impl Fn(&[u8]) -> bool,
) -> Option<SniffedBody> {
    let response = crate::probe::apply_headers(client.get(url), headers)
        .header(
            header::RANGE,
            format!("bytes=0-{}", prefix.max(1).saturating_sub(1)),
        )
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    // The entity's whole length when the server sent only the range asked for: the body is
    // complete only if that length fits in it. `None` for a full body.
    let ranged_total = (response.status() == StatusCode::PARTIAL_CONTENT).then(|| {
        response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(crate::probe::parse_content_range_total)
    });
    let mut collected = Vec::new();
    let mut reading_on = false;
    let mut stream = response.bytes_stream();
    loop {
        let chunk = match stream.next().await {
            None => {
                let complete = ranged_total
                    .is_none_or(|total| total.is_some_and(|total| collected.len() as u64 >= total));
                return Some(SniffedBody {
                    bytes: collected,
                    complete,
                });
            }
            Some(Ok(chunk)) => chunk,
            Some(Err(_)) if reading_on => break,
            Some(Err(_)) => return None,
        };
        collected.extend_from_slice(&chunk);
        if !reading_on && collected.len() >= prefix {
            if !read_on(&collected[..prefix]) {
                break;
            }
            reading_on = true;
        }
        if collected.len() > limit {
            break;
        }
    }
    collected.truncate(prefix);
    Some(SniffedBody {
        bytes: collected,
        complete: false,
    })
}

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};

    use super::fetch_sniffed;

    async fn serve(body: &'static [u8]) -> url::Url {
        let app = Router::new().route("/doc", get(move || async move { body }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        format!("http://{address}/doc").parse().expect("URL")
    }

    #[tokio::test]
    async fn a_beginning_the_caller_wants_is_read_to_the_end() {
        let url = serve(b"d8:announce and everything after it").await;
        let client = reqwest::Client::new();

        let whole = fetch_sniffed(&client, url.clone(), &[], 4, 1024, |head| head == b"d8:a")
            .await
            .expect("read");
        assert!(whole.complete);
        assert_eq!(whole.bytes, b"d8:announce and everything after it");

        let sniff = fetch_sniffed(&client, url.clone(), &[], 4, 1024, |_| false)
            .await
            .expect("read");
        assert!(!sniff.complete);
        assert_eq!(sniff.bytes, b"d8:a");

        let too_long = fetch_sniffed(&client, url, &[], 4, 8, |_| true)
            .await
            .expect("read");
        assert!(!too_long.complete);
        assert_eq!(too_long.bytes, b"d8:a");
    }

    /// RD-1240-33: a server that honours the range sends the sniff and nothing more; against
    /// one that would trickle a whole file the sniff ends at once, and the rest of a ranged
    /// entity is not taken for the whole.
    #[tokio::test]
    async fn the_sniff_asks_for_its_prefix_only() {
        let app = Router::new().route(
            "/file",
            get(|request_headers: axum::http::HeaderMap| async move {
                let asked = request_headers
                    .get(axum::http::header::RANGE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                match asked.as_deref() {
                    Some("bytes=0-3") => (
                        axum::http::StatusCode::PARTIAL_CONTENT,
                        [(axum::http::header::CONTENT_RANGE, "bytes 0-3/4194304")],
                        axum::body::Body::from(&b"d8:a"[..]),
                    ),
                    // Without the range: a body that never ends.
                    _ => (
                        axum::http::StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
                        axum::body::Body::from_stream(futures_util::stream::pending::<
                            Result<Vec<u8>, std::io::Error>,
                        >()),
                    ),
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let url: url::Url = format!("http://{address}/file").parse().expect("URL");
        let sniff = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            fetch_sniffed(&reqwest::Client::new(), url, &[], 4, 1024, |_| true),
        )
        .await
        .expect("the sniff ends at once")
        .expect("read");
        assert_eq!(sniff.bytes, b"d8:a");
        assert!(!sniff.complete, "four bytes of four MiB are not the file");
    }

    #[tokio::test]
    async fn a_body_shorter_than_the_sniff_is_complete() {
        let url = serve(b"d1:").await;
        let short = fetch_sniffed(&reqwest::Client::new(), url, &[], 1024, 4096, |_| false)
            .await
            .expect("read");
        assert!(short.complete);
        assert_eq!(short.bytes, b"d1:");
    }
}
