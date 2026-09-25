//! Reading the start of a response, and the rest of the same response when the start says
//! it is worth it (RD-130-18).
//!
//! A link whose server names no useful content type is identified by its first bytes. When
//! those turn out to begin a document the caller wants whole — a torrent — asking the server
//! a second time would count as a second grab at an indexer that counts them, so the answer
//! already on the wire is read to its end instead.

use futures_util::StreamExt;
use reqwest::Client;
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
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut collected = Vec::new();
    let mut reading_on = false;
    let mut stream = response.bytes_stream();
    loop {
        let chunk = match stream.next().await {
            None => {
                return Some(SniffedBody {
                    bytes: collected,
                    complete: true,
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
