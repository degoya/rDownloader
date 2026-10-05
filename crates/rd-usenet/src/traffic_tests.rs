//! Traffic per Usenet server (RD-1100-05): counted where the pool reads a body, written by the
//! flush, ordered by quota when the pool is built.

use std::{sync::Arc, time::Duration};

use chrono::Utc;
use rd_core::{UsenetQuota, UsenetQuotaAction, UsenetServer, UsenetServerId};
use rd_db::{Database, NewUsenetServer};
use tokio_util::sync::CancellationToken;

use crate::{
    NntpPool, NntpServerConfig, UsenetTraffic, order_by_quota,
    test_support::{
        FixtureTiming, SEGMENT_BYTES, article_set, completed, import_single_file, run_limits,
        spawn_fixture,
    },
    worker::download_file,
};

async fn open(directory: &tempfile::TempDir) -> Database {
    Database::open(directory.path().join("traffic.sqlite"))
        .await
        .expect("database")
}

async fn stored_server(database: &Database, name: &str, priority: i32) -> UsenetServerId {
    database
        .create_usenet_server(NewUsenetServer {
            name: name.to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 119,
            tls: false,
            username: None,
            password_ref: None,
            proxy_profile_id: None,
            priority,
            max_connections: 2,
            enabled: true,
        })
        .await
        .expect("server")
        .id
}

fn endpoint(address: std::net::SocketAddr) -> NntpServerConfig {
    NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 64 * 1024,
        max_connections: 2,
    }
}

/// The yEnc overhead the count may carry over the payload: escapes and line breaks, at most
/// a few per cent, plus the `=ybegin`/`=ypart`/`=yend` lines of every article.
fn within_overhead(counted: u64, articles: u64) -> bool {
    let payload = articles * SEGMENT_BYTES as u64;
    counted >= payload && counted <= payload + payload / 20 + articles * 200
}

/// A file served by two servers: each is credited with the bodies it sent, the refusals cost
/// the primary nothing, and together they come to the payload plus the yEnc overhead.
#[tokio::test]
async fn two_servers_are_each_credited_with_what_they_delivered() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open(&directory).await;
    let primary_id = stored_server(&database, "Primary", 0).await;
    let backup_id = stored_server(&database, "Backup", 1).await;
    // The primary lacks parts 2 and 5; the backup has every part, with the same bytes.
    let primary_set = article_set(6, &[2, 5]);
    let backup_set = article_set(6, &[]);
    let (primary, _) =
        spawn_fixture(Arc::clone(&primary_set.articles), FixtureTiming::default()).await;
    let (backup, backup_log) =
        spawn_fixture(Arc::clone(&backup_set.articles), FixtureTiming::default()).await;
    let traffic = UsenetTraffic::default();
    let pool = NntpPool::metered(
        vec![
            (endpoint(primary), Some(traffic.counter(primary_id))),
            (endpoint(backup), Some(traffic.counter(backup_id))),
        ],
        None,
    )
    .expect("pool");
    let file = import_single_file(&database, "file.bin", &backup_set.segments).await;
    let staging = directory.path().join("staging");
    let destination = directory.path().join("destination");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let outcome = download_file(
        &database,
        &pool,
        &CancellationToken::new(),
        &file,
        &staging,
        &destination,
        &run_limits(),
    )
    .await;
    let (path, missing) = completed(outcome);
    assert_eq!(missing, 0);
    assert_eq!(
        tokio::fs::read(&path).await.expect("output"),
        backup_set.expected
    );
    assert!(backup_log.bytes_sent() > 0, "the backup served the gaps");

    traffic.flush(&database).await.expect("flush");
    assert!(traffic.pending().is_empty(), "the flush took every count");
    let rows = database
        .list_usenet_server_traffic(Utc::now().date_naive())
        .await
        .expect("traffic");
    let total = |id: UsenetServerId| {
        rows.iter()
            .find(|row| row.server_id == id)
            .map(|row| row.total)
            .expect("server row")
    };
    let (primary_bytes, backup_bytes) = (total(primary_id), total(backup_id));
    assert!(
        within_overhead(primary_bytes, 4),
        "the primary delivered four parts: {primary_bytes}"
    );
    assert!(
        within_overhead(backup_bytes, 2),
        "the backup delivered two parts: {backup_bytes}"
    );
    assert!(within_overhead(primary_bytes + backup_bytes, 6));
}

/// A flush the database refuses gives the counts back; the next one carries them.
#[tokio::test]
async fn a_refused_flush_keeps_the_counts() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open(&directory).await;
    let server = stored_server(&database, "Primary", 0).await;
    let traffic = UsenetTraffic::default();
    traffic
        .counter(server)
        .fetch_add(4_096, std::sync::atomic::Ordering::AcqRel);
    database.close().await.expect("close");
    assert!(
        traffic.flush(&database).await.is_err(),
        "a closed writer refuses"
    );
    assert_eq!(traffic.pending(), [(server, 4_096)]);
}

/// The flusher writes on its interval and once more when it is stopped.
#[tokio::test]
async fn the_flusher_writes_what_is_left_when_it_stops() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = open(&directory).await;
    let server = stored_server(&database, "Primary", 0).await;
    let traffic = UsenetTraffic::default();
    let flusher = traffic.start_flushing(database.clone(), Duration::from_secs(3_600));
    traffic
        .counter(server)
        .fetch_add(777, std::sync::atomic::Ordering::AcqRel);
    flusher.shutdown().await;
    let rows = database
        .list_usenet_server_traffic(Utc::now().date_naive())
        .await
        .expect("traffic");
    assert_eq!(rows[0].total, 777);
    assert!(traffic.pending().is_empty());
}

fn server(name: &str, enabled: bool, quota: Option<(UsenetQuotaAction, bool)>) -> UsenetServer {
    UsenetServer {
        id: UsenetServerId::new(),
        name: name.to_owned(),
        host: format!("{name}.example.test"),
        port: 563,
        tls: true,
        username: None,
        has_password: false,
        proxy_profile_id: None,
        priority: 0,
        max_connections: 4,
        enabled,
        quota: quota.map(|(action, reached)| UsenetQuota {
            limit_bytes: 1_000,
            action,
            reset_on: None,
            used_bytes: if reached { 1_000 } else { 10 },
            reached_at: reached.then(Utc::now),
        }),
    }
}

/// A used-up `backup` quota moves its server behind the others, a used-up `pause` quota leaves
/// it out, and a quota with room changes nothing.
#[test]
fn a_used_up_quota_moves_or_pauses_its_server() {
    let (ordered, paused) = order_by_quota(vec![
        server("block-a", true, Some((UsenetQuotaAction::Backup, true))),
        server("main", true, Some((UsenetQuotaAction::Pause, false))),
        server("paused", true, Some((UsenetQuotaAction::Pause, true))),
        server("off", false, None),
        server("block-b", true, Some((UsenetQuotaAction::Backup, true))),
        server("fill", true, None),
    ]);
    let names: Vec<_> = ordered.iter().map(|server| server.name.as_str()).collect();
    assert_eq!(names, ["main", "fill", "block-a", "block-b"]);
    assert_eq!(paused, 1);
}
