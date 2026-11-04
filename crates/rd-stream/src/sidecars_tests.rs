mod parsing {
    use super::super::{thumbnail_extension, thumbnail_url};

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
mod proxy_tests {
    use rd_core::{ProxyKind, SidecarStatus};
    use rd_scheduler::ToolProxy;
    use secrecy::SecretString;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::super::{SidecarClients, Thumbnail, capture_thumbnail};

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
        .expect("proxy")
        // The name below resolves nowhere: only a proxy that resolves names itself is handed
        // it, which is what this test observes.
        .resolving_names(true);
        let directory = tempfile::tempdir().expect("directory");
        // A name nothing resolves: only the proxy can have answered.
        let probe = r#"{"metadata":{"thumbnail":"http://thumbnails.invalid/show.png"}}"#;

        // The recording's cookies for that host travel with it (RD-1320-03).
        let cookies = crate::StreamCookies::from_rows(vec![rd_core::CookieRow {
            domain: "thumbnails.invalid".to_owned(),
            include_subdomains: false,
            path: "/".to_owned(),
            secure: false,
            expires: 0,
            name: "session".to_owned(),
            value: "thumb-secret".to_owned(),
        }])
        .expect("cookies");

        let outcome = capture_thumbnail(
            &SidecarClients::new(),
            &Thumbnail {
                proxy: Some(&proxy),
                cookies: Some(&cookies),
            },
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
        assert!(
            head.lines().any(|line| line
                .split_once(':')
                .is_some_and(|(name, value)| name.eq_ignore_ascii_case("cookie")
                    && value.trim() == "session=thumb-secret")),
            "{head}"
        );
    }
}

/// A thumbnail fetch keeps to the address policy, on the first request and on every hop.
mod address_policy_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use rd_core::SidecarStatus;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::super::{SidecarClients, Thumbnail, capture_thumbnail};

    /// A server answering every request with `response`, counting the connections it accepted.
    async fn counting_server(response: String) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut buffer = [0_u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        (address, seen)
    }

    fn image() -> String {
        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 3\r\n\
         Connection: close\r\n\r\nPNG"
            .to_owned()
    }

    fn redirect(location: &str) -> String {
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\
             Connection: close\r\n\r\n"
        )
    }

    /// Public addresses plus loopback, so a test server can stand in for a public host.
    fn loopback_policy() -> rd_http::AddressPolicy {
        rd_http::AddressPolicy::new(false).with_loopback()
    }

    async fn fetch(clients: &SidecarClients, address: &str) -> SidecarStatus {
        let directory = tempfile::tempdir().expect("directory");
        let probe = format!(r#"{{"metadata":{{"thumbnail":"{address}"}}}}"#);
        capture_thumbnail(
            clients,
            &Thumbnail {
                proxy: None,
                cookies: None,
            },
            Some(&probe),
            directory.path(),
            "show",
        )
        .await
        .status
    }

    #[tokio::test]
    async fn a_loopback_thumbnail_is_not_requested_under_the_default_policy() {
        let (address, seen) = counting_server(image()).await;
        let status = fetch(&SidecarClients::new(), &format!("http://{address}/t.png")).await;
        assert_eq!(status, SidecarStatus::Failed);
        assert_eq!(seen.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_permitted_thumbnail_is_captured() {
        let (address, seen) = counting_server(image()).await;
        let clients = SidecarClients::new().with_address_policy(loopback_policy());
        let status = fetch(&clients, &format!("http://{address}/t.png")).await;
        assert_eq!(status, SidecarStatus::Captured);
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_redirect_to_a_link_local_address_is_not_followed() {
        let (address, seen) = counting_server(redirect("http://169.254.169.254/t.png")).await;
        let clients = SidecarClients::new().with_address_policy(loopback_policy());
        let status = fetch(&clients, &format!("http://{address}/t.png")).await;
        assert_eq!(status, SidecarStatus::Failed);
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_redirect_to_one_of_the_services_own_ports_is_not_followed() {
        let (own, own_seen) = counting_server(image()).await;
        let (address, seen) =
            counting_server(redirect(&format!("http://127.0.0.1:{}/t.png", own.port()))).await;
        let clients = SidecarClients::new()
            .with_address_policy(loopback_policy().refusing_redirects_to(&[own.port()]));
        let status = fetch(&clients, &format!("http://{address}/t.png")).await;
        assert_eq!(status, SidecarStatus::Failed);
        assert_eq!(seen.load(Ordering::SeqCst), 1);
        assert_eq!(own_seen.load(Ordering::SeqCst), 0);
    }
}
