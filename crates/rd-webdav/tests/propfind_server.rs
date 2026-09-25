//! `PROPFIND` against a minimal in-process WebDAV server.
//!
//! The parser is unit-tested against fixed bodies; this exercises the parts that only
//! appear over a real connection: the request shape, the status matrix, `Accept-Ranges`
//! detection and the body size limit.

use std::sync::{Arc, Mutex};

use rd_core::RemoteTarget;
use reqwest::Client;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// How the fixture should answer the next request.
#[derive(Clone)]
struct Reply {
    status: &'static str,
    accept_ranges: bool,
    body: String,
}

impl Reply {
    fn multistatus(body: impl Into<String>) -> Self {
        Self {
            status: "207 Multi-Status",
            accept_ranges: true,
            body: body.into(),
        }
    }

    fn status(status: &'static str) -> Self {
        Self {
            status,
            accept_ranges: false,
            body: String::new(),
        }
    }
}

#[derive(Clone)]
struct Fixture {
    reply: Arc<Mutex<Reply>>,
    /// The request line and headers of the last request, for asserting the request shape.
    seen: Arc<Mutex<Vec<String>>>,
    port: u16,
}

impl Fixture {
    async fn start(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let fixture = Self {
            reply: Arc::new(Mutex::new(reply)),
            seen: Arc::new(Mutex::new(Vec::new())),
            port,
        };
        let served = fixture.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let session = served.clone();
                tokio::spawn(async move {
                    let _ = session.serve(socket).await;
                });
            }
        });
        fixture
    }

    async fn serve(&self, socket: tokio::net::TcpStream) -> std::io::Result<()> {
        let (read_half, mut write) = socket.into_split();
        let mut reader = BufReader::new(read_half);
        let mut headers = Vec::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await? == 0 {
                return Ok(());
            }
            let trimmed = line.trim_end().to_owned();
            if trimmed.is_empty() {
                break;
            }
            if let Some(value) = trimmed.strip_prefix("content-length: ") {
                content_length = value.trim().parse().unwrap_or(0);
            } else if let Some(value) = trimmed.strip_prefix("Content-Length: ") {
                content_length = value.trim().parse().unwrap_or(0);
            }
            headers.push(trimmed);
        }
        if content_length > 0 {
            let mut body = vec![0u8; content_length];
            tokio::io::AsyncReadExt::read_exact(&mut reader, &mut body).await?;
        }
        *self.seen.lock().expect("seen") = headers;

        let reply = self.reply.lock().expect("reply").clone();
        let mut response = format!(
            "HTTP/1.1 {}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\n",
            reply.status,
            reply.body.len()
        );
        if reply.accept_ranges {
            response.push_str("Accept-Ranges: bytes\r\n");
        }
        response.push_str("Connection: close\r\n\r\n");
        response.push_str(&reply.body);
        write.write_all(response.as_bytes()).await?;
        write.flush().await?;
        Ok(())
    }

    fn target(&self, path: &str) -> RemoteTarget {
        RemoteTarget::parse(
            &format!("webdav://127.0.0.1:{}{path}", self.port)
                .parse()
                .expect("url"),
        )
        .expect("target")
    }

    fn saw_header(&self, needle: &str) -> bool {
        self.seen.lock().expect("seen").iter().any(|line| {
            line.to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
        })
    }
}

const COLLECTION: &str = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/dav/share/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat>
  </D:response>
  <D:response><D:href>/dav/share/movie.mkv</D:href>
    <D:propstat><D:prop>
      <D:resourcetype/><D:getcontentlength>1234</D:getcontentlength>
    </D:prop></D:propstat>
  </D:response>
</D:multistatus>"#;

#[tokio::test]
async fn a_propfind_lists_a_collection_and_reports_range_support() {
    let fixture = Fixture::start(Reply::multistatus(COLLECTION)).await;
    let client = Client::new();

    let probed = rd_webdav::probe(&client, &fixture.target("/dav/share/"))
        .await
        .expect("probe");
    let listing = match probed {
        rd_webdav::Probed::Resolved(listing) => listing,
        rd_webdav::Probed::Failed(failure) => panic!("probe failed: {failure:?}"),
    };
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].path, "movie.mkv");
    assert!(
        listing.supports_resume,
        "the fixture advertises byte ranges"
    );

    // Depth 1, not `infinity`: the latter is refused by most servers and would return an
    // unbounded body from the rest.
    assert!(fixture.saw_header("depth: 1"));
    assert!(fixture.saw_header("PROPFIND /dav/share/"));
}

#[tokio::test]
async fn a_server_without_accept_ranges_is_marked_as_not_resumable() {
    let mut reply = Reply::multistatus(COLLECTION);
    reply.accept_ranges = false;
    let fixture = Fixture::start(reply).await;

    let probed = rd_webdav::probe(&Client::new(), &fixture.target("/dav/share/"))
        .await
        .expect("probe");
    let rd_webdav::Probed::Resolved(listing) = probed else {
        panic!("expected a listing");
    };
    // The download still works; it just cannot be continued, and the UI says so rather
    // than silently restarting from zero later.
    assert!(!listing.supports_resume);
}

#[tokio::test]
async fn an_unauthorised_share_reports_that_credentials_are_needed() {
    let fixture = Fixture::start(Reply::status("401 Unauthorized")).await;

    let probed = rd_webdav::probe(&Client::new(), &fixture.target("/dav/share/"))
        .await
        .expect("probe");
    match probed {
        rd_webdav::Probed::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_webdav::AUTH_REQUIRED));
        }
        rd_webdav::Probed::Resolved(_) => panic!("401 must not resolve"),
    }
}

#[tokio::test]
async fn a_plain_http_server_is_reported_as_not_speaking_webdav() {
    let fixture = Fixture::start(Reply::status("405 Method Not Allowed")).await;

    let probed = rd_webdav::probe(&Client::new(), &fixture.target("/dav/share/"))
        .await
        .expect("probe");
    match probed {
        rd_webdav::Probed::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_webdav::PROPFIND_FAILED));
            assert!(!failure.category.is_retryable());
        }
        rd_webdav::Probed::Resolved(_) => panic!("405 must not resolve"),
    }
}

#[tokio::test]
async fn an_oversized_listing_is_refused_rather_than_buffered() {
    // Well-formed, but far past the limit: the guard has to be on size, not on validity.
    let filler = "<D:response><D:href>/dav/share/x</D:href></D:response>".repeat(200_000);
    let body = format!("<D:multistatus xmlns:D=\"DAV:\">{filler}</D:multistatus>");
    assert!(body.len() > rd_webdav::MAX_BODY_BYTES);
    let fixture = Fixture::start(Reply::multistatus(body)).await;

    let probed = rd_webdav::probe(&Client::new(), &fixture.target("/dav/share/"))
        .await
        .expect("probe");
    match probed {
        rd_webdav::Probed::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_webdav::LISTING_TOO_LARGE));
        }
        rd_webdav::Probed::Resolved(_) => panic!("an oversized body must be refused"),
    }
}

#[tokio::test]
async fn a_listing_pointing_outside_the_collection_is_refused() {
    let body = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/dav/share/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat>
  </D:response>
  <D:response><D:href>/etc/passwd</D:href>
    <D:propstat><D:prop><D:getcontentlength>1</D:getcontentlength></D:prop></D:propstat>
  </D:response>
</D:multistatus>"#;
    let fixture = Fixture::start(Reply::multistatus(body)).await;

    let probed = rd_webdav::probe(&Client::new(), &fixture.target("/dav/share/"))
        .await
        .expect("probe");
    match probed {
        rd_webdav::Probed::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_webdav::PATH_ESCAPES_ROOT));
            // The offending href is server-controlled text and must not be echoed on.
            assert!(!failure.message.contains("passwd"));
            assert!(
                failure
                    .params
                    .values()
                    .all(|value| !value.contains("passwd"))
            );
        }
        rd_webdav::Probed::Resolved(_) => panic!("a traversing href must be refused"),
    }
}
