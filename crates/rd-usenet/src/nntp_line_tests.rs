//! A line from a news server is bounded before it is read, not after (RD-1101-16, audit S19).
//!
//! The limits used to be checked once a line had arrived whole, so a server that sent a line
//! without an end had every byte of it buffered until the command timeout. Each case below
//! sends such a line and holds the connection open: bounded, the client gives up at the limit
//! at once; unbounded, it waits out the thirty-second timeout, which the outer timeout here
//! does not allow.

use std::time::Duration;

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

use crate::{NntpClient, NntpServerConfig};

/// Far shorter than the client's command timeout.
const PATIENCE: Duration = Duration::from_secs(5);

/// A line of `length` bytes that never ends.
fn endless_line(length: usize) -> Vec<u8> {
    vec![b'a'; length]
}

fn config(port: u16) -> NntpServerConfig {
    NntpServerConfig {
        host: "127.0.0.1".to_owned(),
        port,
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 1024,
        max_connections: 1,
    }
}

#[tokio::test]
async fn a_greeting_without_an_end_is_refused_at_the_limit() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        // Refused at the limit, the client stops reading; the rest may not be wanted.
        let _ = stream.write_all(&endless_line(64 * 1024)).await;
        tokio::time::sleep(Duration::from_secs(60)).await;
    });

    let error = tokio::time::timeout(PATIENCE, NntpClient::connect(&config(port)))
        .await
        .expect("refused at the limit, not at the timeout")
        .err()
        .expect("an endless greeting is refused");

    assert!(
        format!("{error:#}").contains("status line exceeds limit"),
        "{error:#}"
    );
}

#[tokio::test]
async fn a_body_line_without_an_end_is_refused_at_the_article_limit() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let (read, mut write) = stream.into_split();
        write.write_all(b"200 ready\r\n").await.expect("greeting");
        let mut command = String::new();
        BufReader::new(read)
            .read_line(&mut command)
            .await
            .expect("body command");
        write
            .write_all(b"222 body follows\r\n")
            .await
            .expect("status");
        let _ = write.write_all(&endless_line(64 * 1024)).await;
        tokio::time::sleep(Duration::from_secs(60)).await;
    });

    let mut client = NntpClient::connect(&config(port)).await.expect("connect");
    let Err(error) = tokio::time::timeout(PATIENCE, client.body("endless@example.test"))
        .await
        .expect("refused at the limit, not at the timeout")
    else {
        panic!("an endless line is refused");
    };

    assert!(
        format!("{error:#}").contains("exceeds configured size limit"),
        "{error:#}"
    );
}
