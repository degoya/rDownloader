//! RD-1270-21: the connections per server and per running file, as the queue reads them.

use std::{sync::Arc, time::Duration};

use rd_core::{DownloadId, UsenetServerId};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    NntpPool, NntpServerConfig,
    connections::{FileGauge, LineFiles, UsenetConnections, shares},
    pool::PoolServer,
};

#[test]
fn files_that_fit_keep_their_own_count() {
    let (first, second) = (DownloadId::new(), DownloadId::new());
    let mut split = shares(vec![(first, 5), (second, 5)], 10);
    split.sort_by_key(|(_, connections)| *connections);
    assert_eq!(split, vec![(first, 5), (second, 5)]);
    assert_eq!(shares(vec![(first, 3)], 10), vec![(first, 3)]);
}

#[test]
fn shared_lines_are_split_so_the_files_never_exceed_the_busy_lines() {
    // Ten lines, each carrying a request of both files: counted per file that is twenty.
    let (first, second) = (DownloadId::new(), DownloadId::new());
    let split = shares(vec![(first, 10), (second, 10)], 10);
    assert_eq!(split.iter().map(|(_, count)| count).sum::<u32>(), 10);
    assert!(split.iter().all(|(_, count)| *count == 5), "{split:?}");
    // Uneven: 7 + 5 lines counted, 9 busy - the remainder goes to the larger fraction.
    let split = shares(vec![(first, 7), (second, 5)], 9);
    assert_eq!(split.iter().map(|(_, count)| count).sum::<u32>(), 9);
    assert_eq!(split, vec![(first, 5), (second, 4)]);
}

#[test]
fn a_line_counts_once_per_file_however_many_of_its_requests_it_carries() {
    let line = LineFiles::default();
    let (first, second) = (
        Arc::new(FileGauge::default()),
        Arc::new(FileGauge::default()),
    );
    line.enter(&first);
    line.enter(&first);
    line.enter(&second);
    assert_eq!((first.lines(), second.lines()), (1, 1));
    line.leave(&first);
    assert_eq!(first.lines(), 1, "one of its requests is still on the line");
    line.leave(&first);
    line.leave(&second);
    assert_eq!((first.lines(), second.lines()), (0, 0));
    // A request that never entered takes nothing away.
    line.leave(&first);
    assert_eq!(first.lines(), 0);
}

#[tokio::test]
async fn two_files_on_two_connections_read_as_one_each_and_two_busy() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    // The answers wait until the test has read the gauges.
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let held = Arc::clone(&release);
    let fixture = tokio::spawn(async move {
        let mut handlers = Vec::new();
        for _ in 0..2 {
            let (stream, _) = listener.accept().await.expect("client");
            let held = Arc::clone(&held);
            handlers.push(tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut read = BufReader::new(read);
                write
                    .write_all(b"200 fixture ready\r\n")
                    .await
                    .expect("greeting");
                let mut command = String::new();
                read.read_line(&mut command).await.expect("BODY command");
                let message_id = command
                    .trim()
                    .trim_start_matches("BODY <")
                    .trim_end_matches('>')
                    .to_owned();
                held.acquire().await.expect("release").forget();
                let crc = crc32fast::hash(&[1]);
                let article = format!(
                    "222 0 <{message_id}> body follows\r\n=ybegin line=128 size=1 name=file.bin\r\n+\r\n=yend size=1 crc32={crc:08x}\r\n.\r\n"
                );
                write.write_all(article.as_bytes()).await.expect("article");
            }));
        }
        for handler in handlers {
            handler.await.expect("handler");
        }
    });
    let connections = UsenetConnections::default();
    let server = UsenetServerId::new();
    let gauges = connections.servers(&[server]);
    let pool = NntpPool::gauged(
        vec![PoolServer {
            config: NntpServerConfig {
                host: address.ip().to_string(),
                port: address.port(),
                tls: false,
                custom_ca_pem: Vec::new(),
                username: None,
                password: None,
                proxy: None,
                max_article_bytes: 1024,
                max_connections: 2,
            },
            traffic: None,
            gauge: gauges.into_iter().next(),
        }],
        None,
    )
    .expect("pool");
    let (first, second) = (DownloadId::new(), DownloadId::new());
    let (first_file, second_file) = (connections.track(first), connections.track(second));

    let watch = async {
        let reading = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let reading = connections.reading();
                if reading.servers.first().is_some_and(|load| load.busy == 2)
                    && reading.files.len() == 2
                {
                    return reading;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("both requests on their own line");
        release.add_permits(2);
        reading
    };
    let (reading, one, two) = tokio::join!(
        watch,
        pool.fetch_decoded_for("one@example.test", Some(first_file.gauge())),
        pool.fetch_decoded_for("two@example.test", Some(second_file.gauge())),
    );
    one.expect("first article");
    two.expect("second article");
    assert_eq!(reading.servers.len(), 1);
    assert_eq!(reading.servers[0].server, server);
    assert_eq!(reading.servers[0].limit, 2);
    assert_eq!(reading.files.get(&first), Some(&1));
    assert_eq!(reading.files.get(&second), Some(&1));

    // Answered: the lines stay open and idle, and nobody holds a connection any more.
    let idle = connections.reading();
    assert_eq!(idle.servers[0].busy, 0);
    assert!(idle.files.is_empty(), "{:?}", idle.files);
    drop(first_file);
    drop(second_file);
    drop(pool);
    fixture.await.expect("fixture task");
}

#[test]
fn a_finished_file_leaves_the_reading_and_a_removed_server_too() {
    let connections = UsenetConnections::default();
    let (kept, dropped) = (UsenetServerId::new(), UsenetServerId::new());
    let first = connections.servers(&[kept, dropped]);
    first[0].set_limit(10);
    let second = connections.servers(&[kept]);
    assert!(
        Arc::ptr_eq(&first[0], &second[0]),
        "a server keeps its gauge"
    );
    let reading = connections.reading();
    assert_eq!(reading.servers.len(), 1);
    assert_eq!(reading.servers[0].limit, 10);

    let file = DownloadId::new();
    let tracked = connections.track(file);
    let line = LineFiles::default();
    line.enter(tracked.gauge());
    connections.servers(&[kept])[0].line_busy();
    assert_eq!(connections.reading().files.get(&file), Some(&1));
    line.leave(tracked.gauge());
    drop(tracked);
    assert!(connections.reading().files.is_empty());
}
