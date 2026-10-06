use secrecy::SecretString;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::{NntpClient, NntpServerConfig, answered_message_id, status_code, tls_connector};

/// The news server is trusted by the same rule as everything else: a bundle that parses to
/// nothing is refused here exactly as `rd_http::tls_client_config` refuses it, instead of
/// quietly falling back to the platform roots the way a verifier built here once did.
#[test]
fn the_connector_comes_from_the_one_place_tls_trust_is_decided() {
    assert!(tls_connector(&[]).is_ok());
    assert!(tls_connector(&[b"not a certificate".to_vec()]).is_err());
}

#[test]
fn parses_status_codes() {
    assert_eq!(status_code("222 0 <id> body follows\r\n").ok(), Some(222));
    assert!(status_code("no").is_err());
}

#[test]
fn the_message_id_on_the_222_line_is_read_and_its_absence_is_told_apart() {
    assert_eq!(
        answered_message_id("222 0 <part-1@example.test> body follows\r\n").as_deref(),
        Some("part-1@example.test")
    );
    assert_eq!(
        answered_message_id("222 12345 <a@b>\r\n").as_deref(),
        Some("a@b")
    );
    assert_eq!(answered_message_id("222 body follows\r\n"), None);
    assert_eq!(answered_message_id("222 0 <> body follows\r\n"), None);
    assert_eq!(answered_message_id("222\r\n"), None);
}

#[tokio::test]
async fn authenticates_and_reads_a_dot_stuffed_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fake NNTP listener");
    let address = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("client");
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        write
            .write_all(b"200 fake server ready\r\n")
            .await
            .expect("greeting");
        let mut line = String::new();
        read.read_line(&mut line).await.expect("username");
        assert_eq!(line, "AUTHINFO USER reader\r\n");
        write
            .write_all(b"381 password required\r\n")
            .await
            .expect("user response");
        line.clear();
        read.read_line(&mut line).await.expect("password");
        assert_eq!(line, "AUTHINFO PASS secret\r\n");
        write
            .write_all(b"281 authentication accepted\r\n")
            .await
            .expect("auth response");
        line.clear();
        read.read_line(&mut line).await.expect("body command");
        assert_eq!(line, "BODY <message-id@example.test>\r\n");
        write
            .write_all(b"222 body follows\r\nfirst\r\n..second\r\n.\r\n")
            .await
            .expect("article");
    });
    let config = NntpServerConfig {
        host: "127.0.0.1".to_owned(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: Some("reader".to_owned()),
        password: Some(SecretString::from("secret".to_owned())),
        proxy: None,
        max_article_bytes: 1024,
        max_connections: 1,
    };

    let mut client = NntpClient::connect(&config).await.expect("connect");
    let body = client.body("message-id@example.test").await.expect("body");
    assert_eq!(body, b"first\r\n.second\r\n");
}
