use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{NntpPool, NntpServerConfig};

#[tokio::test]
async fn authenticated_connection_is_reused_within_server_limit() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("single client");
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        write
            .write_all(b"200 fixture ready\r\n")
            .await
            .expect("greeting");
        for message_id in ["one@example.test", "two@example.test"] {
            let mut command = String::new();
            read.read_line(&mut command).await.expect("BODY command");
            assert_eq!(command, format!("BODY <{message_id}>\r\n"));
            let crc = crc32fast::hash(&[1]);
            let article = format!(
                "222 0 <{message_id}> body follows\r\n=ybegin line=128 size=1 name=file.bin\r\n+\r\n=yend size=1 crc32={crc:08x}\r\n.\r\n"
            );
            write.write_all(article.as_bytes()).await.expect("article");
        }
    });
    let pool = NntpPool::new(vec![NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 1024,
        max_connections: 1,
    }])
    .expect("pool");
    assert_eq!(pool.max_parallel(), 1);

    for message_id in ["one@example.test", "two@example.test"] {
        let fetched = pool.fetch_decoded(message_id).await.expect("article");
        assert_eq!(fetched.article.data, [1]);
        assert_eq!(fetched.attempts, 1);
    }
    fixture.await.expect("fixture task");
}

#[tokio::test]
async fn two_permits_allow_two_articles_to_progress_concurrently() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let fixture = tokio::spawn(async move {
        let mut handlers = Vec::new();
        for _ in 0..2 {
            let (stream, _) = listener.accept().await.expect("parallel client");
            let barrier = barrier.clone();
            handlers.push(tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut read = BufReader::new(read);
                write
                    .write_all(b"200 fixture ready\r\n")
                    .await
                    .expect("greeting");
                let message_id = read_command(&mut read).await;
                barrier.wait().await;
                let crc = crc32fast::hash(&[1]);
                let article = format!(
                    "222 0 <{message_id}> body follows\r\n=ybegin line=128 size=1 name=file.bin\r\n+\r\n=yend size=1 crc32={crc:08x}\r\n.\r\n"
                );
                write.write_all(article.as_bytes()).await.expect("article");
            }));
        }
        for handler in handlers {
            handler.await.expect("client handler");
        }
    });
    let pool = NntpPool::new(vec![NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 1024,
        max_connections: 2,
    }])
    .expect("pool");
    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(
            pool.fetch_decoded("first@example.test"),
            pool.fetch_decoded("second@example.test")
        )
    })
    .await
    .expect("both permits made progress");
    assert!(first.is_ok());
    assert!(second.is_ok());
    fixture.await.expect("fixture task");
}

// --- RD-108-25: two requests per connection ---------------------------------------------

fn single_connection(address: std::net::SocketAddr, max_article_bytes: usize) -> NntpPool {
    NntpPool::new(vec![NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes,
        max_connections: 1,
    }])
    .expect("pool")
}

async fn read_command(read: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> String {
    let mut command = String::new();
    read.read_line(&mut command).await.expect("BODY command");
    command
        .trim()
        .strip_prefix("BODY <")
        .and_then(|rest| rest.strip_suffix('>'))
        .expect("bracketed BODY command")
        .to_owned()
}

/// A one-byte article, its `222` line naming `message_id` the way a real server does.
fn article(message_id: &str, byte: u8) -> Vec<u8> {
    let crc = crc32fast::hash(&[byte]);
    format!(
        "222 0 <{message_id}> body follows\r\n=ybegin line=128 size=1 name=file.bin\r\n{}\r\n=yend size=1 crc32={crc:08x}\r\n.\r\n",
        (byte.wrapping_add(42)) as char
    )
    .into_bytes()
}

/// (a) The second `BODY` leaves before the first body has been read: the fixture answers the
/// first command only once it has seen the second, so a client that waited for body one
/// before sending command two would hang here.
#[tokio::test]
async fn the_second_command_goes_out_before_the_first_body_is_read() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("single client");
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        write
            .write_all(b"200 fixture ready\r\n")
            .await
            .expect("greeting");
        let first = read_command(&mut read).await;
        let second = read_command(&mut read).await;
        write
            .write_all(&article(&first, 1))
            .await
            .expect("first body");
        write
            .write_all(&article(&second, 2))
            .await
            .expect("second body");
        (first, second)
    });
    let pool = single_connection(address, 1024);
    assert_eq!(pool.max_parallel(), 1);
    assert_eq!(pool.max_parallel_requests(), 2);

    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(
            pool.fetch_decoded("first@example.test"),
            pool.fetch_decoded("second@example.test")
        )
    })
    .await
    .expect("both commands were on the line before either body");
    assert_eq!(first.expect("first article").article.data, [1]);
    assert_eq!(second.expect("second article").article.data, [2]);
    let (first, second) = fixture.await.expect("fixture task");
    let mut seen = [first, second];
    seen.sort();
    assert_eq!(seen, ["first@example.test", "second@example.test"]);
}

/// (b) A refusal is a one-line answer, so the line stays in step: the request behind it gets
/// its body, and the connection is used again afterwards rather than dropped. A refusal
/// names no message-id, though, so one read beside another request is asked once more on
/// the line alone before it is believed (RD-108-27) - the fixture sees `missing` twice.
#[tokio::test]
async fn a_refused_request_does_not_take_the_one_behind_it_with_it() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let (first_seen, first_on_the_line) = tokio::sync::oneshot::channel();
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("single client");
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        write
            .write_all(b"200 fixture ready\r\n")
            .await
            .expect("greeting");
        let mut seen = vec![read_command(&mut read).await];
        first_seen.send(()).expect("test is waiting");
        seen.push(read_command(&mut read).await);
        write
            .write_all(b"430 No such article\r\n")
            .await
            .expect("refusal");
        write
            .write_all(&article("second@example.test", 2))
            .await
            .expect("second body");
        seen.push(read_command(&mut read).await);
        write
            .write_all(b"430 No such article\r\n")
            .await
            .expect("refusal, asked alone");
        seen.push(read_command(&mut read).await);
        write
            .write_all(&article("third@example.test", 3))
            .await
            .expect("third body");
        seen
    });
    let pool = single_connection(address, 1024);

    let (missing, second) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let missing = tokio::spawn({
            let pool = pool.clone();
            async move { pool.fetch_decoded("missing@example.test").await }
        });
        first_on_the_line
            .await
            .expect("fixture saw the first command");
        let second = pool.fetch_decoded("second@example.test").await;
        (missing.await.expect("first request task"), second)
    })
    .await
    .expect("neither request hung");
    let error = missing
        .err()
        .expect("the refused article fails")
        .to_string();
    assert!(error.contains("430"), "unexpected error: {error}");
    assert_eq!(second.expect("second article").article.data, [2]);
    // The same connection - the fixture accepts only one - serves the next request.
    let third = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        pool.fetch_decoded("third@example.test"),
    )
    .await
    .expect("the line was kept")
    .expect("third article");
    assert_eq!(third.article.data, [3]);
    assert_eq!(
        fixture.await.expect("fixture task"),
        [
            "missing@example.test",
            "second@example.test",
            "missing@example.test",
            "third@example.test"
        ]
    );
    assert_eq!(
        pool.pipeline_depth(0),
        crate::pool::PIPELINE_DEPTH,
        "a refusal in step does not cost the server its pipelining"
    );
}

/// (c) A line that broke mid-body is never handed out again: what it still holds belongs to
/// nobody. The request that was queued behind the break is fetched again on a fresh
/// connection, and the broken one sees no further command.
#[tokio::test]
async fn a_line_broken_mid_body_is_retired_and_the_request_behind_it_moves_on() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let (first_seen, first_on_the_line) = tokio::sync::oneshot::channel();
    let fixture = tokio::spawn(async move {
        // First connection: two commands, then a body far beyond the client's size limit.
        let (stream, _) = listener.accept().await.expect("first client");
        let (read, mut write) = stream.into_split();
        let mut broken_read = BufReader::new(read);
        write
            .write_all(b"200 fixture ready\r\n")
            .await
            .expect("greeting");
        let mut broken_line = vec![read_command(&mut broken_read).await];
        first_seen.send(()).expect("test is waiting");
        broken_line.push(read_command(&mut broken_read).await);
        let mut oversized = b"222 body follows\r\n".to_vec();
        oversized.extend(std::iter::repeat_n(b'x', 4096));
        oversized.extend_from_slice(b"\r\n.\r\n");
        write.write_all(&oversized).await.expect("oversized body");
        write
            .write_all(&article("second@example.test", 2))
            .await
            .expect("stranded body");
        // Second connection: the request behind the break arrives here, alone.
        let (stream, _) = listener.accept().await.expect("second client");
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        write
            .write_all(b"200 fixture ready\r\n")
            .await
            .expect("greeting");
        let fresh_line = read_command(&mut read).await;
        write
            .write_all(&article("second@example.test", 2))
            .await
            .expect("second body");
        // The broken connection is dropped, not reused: its read half sees the end - as an
        // EOF, or as a reset, because the client left the stranded body unread.
        let mut trailing = String::new();
        let closed = matches!(broken_read.read_line(&mut trailing).await, Ok(0) | Err(_));
        (broken_line, fresh_line, closed)
    });
    let pool = single_connection(address, 256);
    // The oversized request's last attempt dials the fixture after it has gone. Linux refuses
    // that at once; Windows retries the SYN and reports the refusal only after about two
    // seconds, which alone used up the whole bound there.
    let bound = std::time::Duration::from_secs(if cfg!(windows) { 8 } else { 2 });

    let (oversized, second) = tokio::time::timeout(bound, async {
        let oversized = tokio::spawn({
            let pool = pool.clone();
            async move { pool.fetch_decoded("oversized@example.test").await }
        });
        first_on_the_line
            .await
            .expect("fixture saw the first command");
        let second = pool.fetch_decoded("second@example.test").await;
        (oversized.await.expect("first request task"), second)
    })
    .await
    .expect("neither request hung");
    let error = oversized
        .err()
        .expect("the oversized article fails")
        .to_string();
    assert!(error.contains("size limit"), "unexpected error: {error}");
    let second = second.expect("second article, from a fresh connection");
    assert_eq!(second.article.data, [2]);
    assert_eq!(second.attempts, 1, "the retry stayed on the same server");
    let (broken_line, fresh_line, closed) = fixture.await.expect("fixture task");
    assert_eq!(
        broken_line,
        ["oversized@example.test", "second@example.test"]
    );
    assert_eq!(fresh_line, "second@example.test");
    assert!(
        closed,
        "the broken connection was closed rather than reused"
    );
}

/// A connect that is dropped mid-handshake - a shutdown does this - must not keep counting
/// against the connection limit. With one connection allowed, a leaked reservation would make
/// every later request wait forever.
#[tokio::test]
async fn a_cancelled_connect_releases_its_place_in_the_limit() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        // The first client is accepted and never greeted, so its connect cannot finish.
        let (held, _) = listener.accept().await.expect("stalled client");
        let (stream, _) = listener.accept().await.expect("second client");
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        write
            .write_all(b"200 fixture ready\r\n")
            .await
            .expect("greeting");
        let seen = read_command(&mut read).await;
        write.write_all(&article(&seen, 1)).await.expect("body");
        drop(held);
        seen
    });
    let pool = single_connection(address, 1024);

    let stalled = Box::pin(pool.fetch_decoded("stalled@example.test"));
    let mut stalled = stalled;
    let not_yet = tokio::time::timeout(std::time::Duration::from_millis(100), &mut stalled).await;
    assert!(
        not_yet.is_err(),
        "the stalled connect finished without a greeting"
    );
    drop(stalled);

    let fetched = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        pool.fetch_decoded("after@example.test"),
    )
    .await
    .expect("the reservation of the dropped connect was released")
    .expect("article");
    assert_eq!(fetched.article.data, [1]);
    assert_eq!(fixture.await.expect("fixture task"), "after@example.test");
}

/// The in-flight depth follows the primary server alone: backups only ever see what the
/// primary refused, so requests beyond its capacity would wait in memory, not on a line.
#[test]
fn the_request_depth_is_the_primary_servers_connections_twice() {
    let server = |max_connections: u16| NntpServerConfig {
        host: "news.example".to_owned(),
        port: 119,
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 1024,
        max_connections,
    };
    let pool = NntpPool::new(vec![server(10), server(20), server(20)]).expect("pool");
    assert_eq!(pool.max_parallel(), 50);
    assert_eq!(pool.max_parallel_requests(), 20);

    let capped =
        NntpPool::with_connection_cap(vec![server(10), server(20)], Some(8)).expect("pool");
    assert_eq!(capped.max_parallel(), 16, "the cap applies per server");
    assert_eq!(capped.max_parallel_requests(), 16);
}
