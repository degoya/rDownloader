//! An article that waits holds up nothing but itself (RD-108-26).
//!
//! The assembly used to run strictly front to back: the fetch stream yielded articles in
//! order, so one that was slow - a second attempt, an outlier on a busy server - stopped the
//! stream from asking for the next ones at all, and every connection but the one it was on
//! fell idle. Articles carry their own byte range, so nothing about that order was ever
//! necessary: each one is written where it belongs.

use std::{sync::Arc, time::Duration};

use rd_db::Database;
use tokio_util::sync::CancellationToken;

use crate::{
    NntpPool, NntpServerConfig,
    test_support::{
        FixtureBehaviour, FixtureTiming, Gate, article_set, import_single_file, run_limits,
        spawn_fixture_with,
    },
    worker::{FileOutcome, download_file},
};

#[tokio::test]
async fn an_article_that_waits_does_not_stop_the_stream_from_asking_for_the_next_ones() {
    let set = article_set(8, &[]);
    let gate = Gate::on("part-1@example.test");
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("order.sqlite"))
        .await
        .expect("database");
    let file = import_single_file(&database, "file.bin", &set.segments).await;
    let staging = directory.path().join("staging");
    let destination = directory.path().join("destination");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let (address, log) = spawn_fixture_with(
        Arc::clone(&set.articles),
        FixtureTiming::default(),
        FixtureBehaviour {
            gate: Some(gate.clone()),
            ..FixtureBehaviour::default()
        },
    )
    .await;
    // Two connections, four requests in flight. The first article is held on one of them; a
    // client that needs it before it may yield anything else runs out of requests at four.
    let pool = NntpPool::new(vec![NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 64 * 1024,
        max_connections: 2,
    }])
    .expect("pool");
    assert_eq!(pool.max_parallel_requests(), 4);

    let download = tokio::spawn({
        let database = database.clone();
        let pool = pool.clone();
        async move {
            download_file(
                &database,
                &pool,
                &CancellationToken::new(),
                &file,
                &staging,
                &destination,
                &run_limits(),
            )
            .await
        }
    });

    let asked = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let asked = log.requests().len();
            if asked > 4 {
                return asked;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the stream kept asking while the first article waited");
    assert!(
        asked > 4,
        "only {asked} articles were requested while one was held"
    );

    gate.open();
    let outcome = tokio::time::timeout(Duration::from_secs(10), download)
        .await
        .expect("the download finishes once the held article arrives")
        .expect("download task");
    let path = match outcome.expect("download") {
        FileOutcome::Completed { path, missing } => {
            assert_eq!(missing, 0);
            path
        }
        FileOutcome::Cancelled => panic!("download was cancelled"),
    };
    assert_eq!(
        tokio::fs::read(&path).await.expect("assembled file"),
        set.expected,
        "every article landed where its own range says it belongs"
    );
}

/// The pool is the runner's, not the file's (RD-108-26).
///
/// Two files in a row used to mean two pools: ten TCP connections, ten TLS handshakes and ten
/// `AUTHINFO` exchanges thrown away and made again at every file boundary, with every
/// connection idle while that happened. The fixture counts connections, so the second file
/// proves it by opening none.
#[tokio::test]
async fn a_second_file_opens_no_new_connection() {
    let set = article_set(4, &[]);
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("pool.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("secret store");
    let (address, log) = spawn_fixture_with(
        Arc::clone(&set.articles),
        FixtureTiming::default(),
        FixtureBehaviour::default(),
    )
    .await;
    database
        .create_usenet_server(rd_db::NewUsenetServer {
            name: "fixture".to_owned(),
            host: address.ip().to_string(),
            port: address.port(),
            tls: false,
            username: None,
            password_ref: None,
            proxy_profile_id: None,
            priority: 10,
            max_connections: 2,
            enabled: true,
        })
        .await
        .expect("usenet server");
    let runner = crate::UsenetRunner::new(
        database.clone(),
        secrets,
        crate::UsenetRunnerConfig::default(),
    );

    let first = runner.pool(None).await.expect("first pool");
    first
        .fetch_decoded("part-1@example.test")
        .await
        .expect("first article");
    let opened = log.connection_count();
    assert_eq!(opened, 1, "one article needs one connection");

    let second = runner.pool(None).await.expect("second pool");
    second
        .fetch_decoded("part-2@example.test")
        .await
        .expect("second article");
    assert_eq!(
        log.connection_count(),
        opened,
        "the next file reuses the connection the previous one authenticated"
    );

    // Change the settings and the pool is given up, whatever it holds open.
    let server = database
        .list_usenet_servers()
        .await
        .expect("servers")
        .remove(0);
    database
        .update_usenet_server(
            server.id,
            rd_db::NewUsenetServer {
                name: "fixture".to_owned(),
                host: address.ip().to_string(),
                port: address.port(),
                tls: false,
                username: None,
                password_ref: None,
                proxy_profile_id: None,
                priority: 10,
                max_connections: 4,
                enabled: true,
            },
        )
        .await
        .expect("updated server");
    let third = runner.pool(None).await.expect("third pool");
    third
        .fetch_decoded("part-3@example.test")
        .await
        .expect("third article");
    assert_eq!(
        log.connection_count(),
        opened + 1,
        "changed settings are a new pool, and a new connection"
    );
}
