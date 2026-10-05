//! Traffic per Usenet server and the quota counted against it (RD-1100-05).

use chrono::{DateTime, Duration, NaiveDate, Utc};
use rd_core::{EventKind, UsenetQuotaAction, UsenetServerId};

use crate::{Database, NewUsenetServer, UsenetQuotaInput};

async fn open() -> (tempfile::TempDir, Database) {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("traffic.sqlite"))
        .await
        .expect("database");
    (directory, database)
}

async fn server(database: &Database, name: &str, priority: i32) -> UsenetServerId {
    database
        .create_usenet_server(NewUsenetServer {
            name: name.to_owned(),
            host: format!("{}.example.test", name.to_lowercase()),
            port: 563,
            tls: true,
            username: None,
            password_ref: None,
            proxy_profile_id: None,
            priority,
            max_connections: 4,
            enabled: true,
        })
        .await
        .expect("server")
        .id
}

fn moment(text: &str) -> DateTime<Utc> {
    text.parse().expect("moment")
}

fn quota(limit: u64, action: UsenetQuotaAction) -> UsenetQuotaInput {
    UsenetQuotaInput {
        limit_bytes: Some(limit),
        action,
        reset_on: None,
        reset_usage: false,
    }
}

/// Bytes land on their server and their day, and every range sums the days inside it.
#[tokio::test]
async fn flushes_are_summed_per_server_and_range() {
    let (_directory, database) = open().await;
    let primary = server(&database, "Primary", 0).await;
    let block = server(&database, "Block", 1).await;
    let today = moment("2026-10-04T12:00:00Z");
    for (when, counts) in [
        (today - Duration::days(400), vec![(primary, 1)]),
        (today - Duration::days(100), vec![(primary, 10)]),
        (today - Duration::days(20), vec![(primary, 100)]),
        (
            today - Duration::days(3),
            vec![(primary, 1_000), (block, 7)],
        ),
        (today, vec![(primary, 10_000), (block, 70)]),
        (today, vec![(primary, 20_000), (block, 0)]),
    ] {
        database
            .record_usenet_traffic_at(counts, when)
            .await
            .expect("flush");
    }
    let traffic = database
        .list_usenet_server_traffic(today.date_naive())
        .await
        .expect("traffic");
    let figures: Vec<_> = traffic
        .iter()
        .map(|row| {
            (
                row.name.as_str(),
                row.today,
                row.week,
                row.month,
                row.year,
                row.total,
            )
        })
        .collect();
    assert_eq!(
        figures,
        [
            ("Primary", 30_000, 31_000, 31_100, 31_110, 31_111),
            ("Block", 70, 77, 77, 77, 77),
        ]
    );
    assert_eq!(traffic[0].server_id, primary);
    assert!(traffic[0].quota.is_none(), "no quota was set");
}

/// The flush that crosses the limit marks it and announces it once; later ones stay quiet.
#[tokio::test]
async fn a_quota_is_announced_once_when_a_flush_crosses_it() {
    let (_directory, database) = open().await;
    let block = server(&database, "Block", 1).await;
    database
        .set_usenet_quota(block, quota(1_000, UsenetQuotaAction::Pause))
        .await
        .expect("quota");
    let mut events = database.subscribe();
    let now = Utc::now();
    let reached = database
        .record_usenet_traffic_at(vec![(block, 600)], now)
        .await
        .expect("first");
    assert!(reached.is_empty(), "600 of 1000 is not the limit");
    let reached = database
        .record_usenet_traffic_at(vec![(block, 500)], now)
        .await
        .expect("second");
    assert_eq!(reached.len(), 1);
    assert_eq!(reached[0].server_id, block);
    let event = loop {
        let event = events.recv().await.expect("event");
        if event.payload["resource"] == "usenet_quota" {
            break event;
        }
    };
    assert_eq!(event.kind, EventKind::UsenetChanged);
    assert_eq!(event.payload["quota_reached"], true);
    assert_eq!(event.payload["action"], "pause");
    assert_eq!(event.payload["limit_bytes"], 1_000);
    let again = database
        .record_usenet_traffic_at(vec![(block, 100)], now)
        .await
        .expect("third");
    assert!(again.is_empty(), "a reached quota is not announced twice");
    let servers = database.list_usenet_servers().await.expect("servers");
    let stored = servers[0].quota.as_ref().expect("quota");
    assert_eq!(stored.used_bytes, 1_200);
    assert!(stored.is_reached());
    assert_eq!(stored.action, UsenetQuotaAction::Pause);
}

/// The figure counts only under a quota; a lower limit applies at once, a raised one gives the
/// server back, and removing the quota forgets the figure.
#[tokio::test]
async fn changing_the_limit_moves_the_mark_with_it() {
    let (_directory, database) = open().await;
    let block = server(&database, "Block", 1).await;
    let flush = |bytes: u64| {
        let database = database.clone();
        async move {
            database
                .record_usenet_traffic_at(vec![(block, bytes)], Utc::now())
                .await
                .expect("flush")
        }
    };
    flush(700).await;
    let server = database
        .set_usenet_quota(block, quota(10_000, UsenetQuotaAction::Backup))
        .await
        .expect("set");
    assert_eq!(
        server.quota.expect("quota").used_bytes,
        0,
        "traffic from before the quota does not count against it"
    );
    flush(5_000).await;
    let server = database
        .set_usenet_quota(block, quota(4_000, UsenetQuotaAction::Backup))
        .await
        .expect("below");
    let stored = server.quota.expect("quota");
    assert_eq!(stored.used_bytes, 5_000);
    assert!(
        stored.is_reached(),
        "a limit below the figure applies at once"
    );
    let server = database
        .set_usenet_quota(block, quota(9_000, UsenetQuotaAction::Backup))
        .await
        .expect("raised");
    assert!(!server.quota.expect("quota").is_reached());
    let server = database
        .set_usenet_quota(
            block,
            UsenetQuotaInput {
                reset_usage: true,
                ..quota(9_000, UsenetQuotaAction::Backup)
            },
        )
        .await
        .expect("reset");
    assert_eq!(server.quota.expect("quota").used_bytes, 0);
    flush(300).await;
    let server = database
        .set_usenet_quota(
            block,
            UsenetQuotaInput {
                limit_bytes: None,
                ..quota(0, UsenetQuotaAction::Backup)
            },
        )
        .await
        .expect("removed");
    assert!(server.quota.is_none());
    flush(50).await;
    let server = database
        .set_usenet_quota(block, quota(9_000, UsenetQuotaAction::Backup))
        .await
        .expect("set again");
    assert_eq!(
        server.quota.expect("quota").used_bytes,
        0,
        "a removed quota forgot its figure, and nothing counted without one"
    );
}

/// A reset day that has come empties the figure for every reader, and the next flush stores it.
#[tokio::test]
async fn a_due_reset_day_starts_the_figure_again() {
    let (_directory, database) = open().await;
    let block = server(&database, "Block", 1).await;
    let today = Utc::now().date_naive();
    database
        .set_usenet_quota(
            block,
            UsenetQuotaInput {
                reset_on: Some(today + Duration::days(1)),
                ..quota(1_000, UsenetQuotaAction::Pause)
            },
        )
        .await
        .expect("quota");
    database
        .record_usenet_traffic_at(vec![(block, 1_500)], Utc::now())
        .await
        .expect("flush");
    let before = database
        .list_usenet_server_traffic(today)
        .await
        .expect("today");
    assert!(before[0].quota.as_ref().expect("quota").is_reached());
    let tomorrow: NaiveDate = today + Duration::days(1);
    let after = database
        .list_usenet_server_traffic(tomorrow)
        .await
        .expect("tomorrow");
    let quota_then = after[0].quota.as_ref().expect("quota");
    assert_eq!(quota_then.used_bytes, 0);
    assert!(!quota_then.is_reached());
    assert_eq!(quota_then.reset_on, None, "a reset day applies once");
    database
        .record_usenet_traffic_at(vec![(block, 10)], Utc::now() + Duration::days(1))
        .await
        .expect("flush after the reset");
    let stored = database.list_usenet_servers().await.expect("servers")[0]
        .quota
        .clone()
        .expect("quota");
    assert_eq!(stored.used_bytes, 10, "the flush stored the reset");
    assert_eq!(stored.reset_on, None);
}

/// Bytes of a server deleted meanwhile are dropped; the other servers' bytes are kept.
#[tokio::test]
async fn a_deleted_server_does_not_fail_the_flush() {
    let (_directory, database) = open().await;
    let kept = server(&database, "Kept", 0).await;
    let gone = server(&database, "Gone", 1).await;
    database.delete_usenet_server(gone).await.expect("delete");
    database
        .record_usenet_traffic_at(vec![(gone, 5), (kept, 9)], Utc::now())
        .await
        .expect("flush");
    let traffic = database
        .list_usenet_server_traffic(Utc::now().date_naive())
        .await
        .expect("traffic");
    assert_eq!(traffic.len(), 1);
    assert_eq!(traffic[0].total, 9);
}

/// Clearing the statistics clears the traffic per server too, and leaves the quota figure.
#[tokio::test]
async fn clearing_the_statistics_keeps_the_quota_figure() {
    let (_directory, database) = open().await;
    let block = server(&database, "Block", 1).await;
    database
        .set_usenet_quota(block, quota(1_000, UsenetQuotaAction::Backup))
        .await
        .expect("quota");
    database
        .record_usenet_traffic_at(vec![(block, 400)], Utc::now())
        .await
        .expect("flush");
    assert_eq!(database.count_transfer_stats().await.expect("count"), 1);
    assert_eq!(database.clear_transfer_stats().await.expect("clear"), 1);
    let traffic = database
        .list_usenet_server_traffic(Utc::now().date_naive())
        .await
        .expect("traffic");
    assert_eq!(traffic[0].total, 0);
    assert_eq!(traffic[0].quota.as_ref().expect("quota").used_bytes, 400);
}
