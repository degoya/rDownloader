//! Persisted sessions: what the inventory shows and what revoking actually does.

use chrono::{Duration, Utc};
use rd_core::{SessionId, SessionLimits};

fn limits() -> SessionLimits {
    SessionLimits::default()
}

/// Moves a session's sign-in and last use into the past.
///
/// No API writes a past time, and waiting hours for a limit to pass is not a test, so the row
/// is aged directly — the way `rd-api`'s torrent tests age a finished package.
async fn age(
    directory: &std::path::Path,
    digest: &str,
    signed_in_hours_ago: i64,
    used_hours_ago: i64,
) {
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        directory.join("sessions.sqlite3").display()
    ))
    .await
    .expect("pool");
    let now = Utc::now();
    let changed =
        sqlx::query("UPDATE sessions SET created_at = ?, last_used_at = ? WHERE token_sha256 = ?")
            .bind(now - Duration::hours(signed_in_hours_ago))
            .bind(now - Duration::hours(used_hours_ago))
            .bind(digest)
            .execute(&pool)
            .await
            .expect("age the session");
    assert_eq!(changed.rows_affected(), 1, "no session {digest}");
    pool.close().await;
}

async fn is_live(database: &rd_db::Database, digest: &str, limits: SessionLimits) -> bool {
    database
        .session_for_digest(digest, limits)
        .await
        .expect("lookup")
        .is_some()
}

async fn database(directory: &std::path::Path) -> rd_db::Database {
    rd_db::Database::open(directory.join("sessions.sqlite3"))
        .await
        .expect("database")
}

async fn open(database: &rd_db::Database, digest: &str, agent: Option<&str>) -> rd_core::Session {
    database
        .create_session(
            SessionId::new(),
            digest.to_owned(),
            agent.map(str::to_owned),
            Some("192.168.1.20".to_owned()),
            12,
        )
        .await
        .expect("session")
}

#[tokio::test]
async fn a_session_is_found_by_its_digest_and_never_by_its_bearer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;

    let created = open(&database, "digest-a", Some("Firefox")).await;
    let found = database
        .session_for_digest("digest-a", limits())
        .await
        .expect("lookup")
        .expect("a live session");
    assert_eq!(found.id, created.id);
    assert_eq!(found.user_agent.as_deref(), Some("Firefox"));
    assert_eq!(found.client_ip.as_deref(), Some("192.168.1.20"));

    assert!(
        database
            .session_for_digest("digest-b", limits())
            .await
            .expect("lookup")
            .is_none()
    );
}

#[tokio::test]
async fn revoking_a_session_takes_effect_at_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;

    let session = open(&database, "digest-a", None).await;
    assert!(database.revoke_session(session.id).await.expect("revoke"));
    assert!(
        database
            .session_for_digest("digest-a", limits())
            .await
            .expect("lookup")
            .is_none(),
        "a revoked session still authenticated"
    );
    // Revoking twice is not an error, and reports that nothing was live.
    assert!(!database.revoke_session(session.id).await.expect("revoke"));
}

/// The action exists so a person can end sessions they do not recognise. Ending their own in
/// the process would make it a tool nobody can use with confidence.
#[tokio::test]
async fn signing_out_everywhere_else_keeps_the_caller_signed_in() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;

    open(&database, "mine", Some("This laptop")).await;
    open(&database, "other-1", Some("A phone")).await;
    open(&database, "other-2", Some("Something else")).await;

    let ended = database
        .revoke_other_sessions("mine".to_owned())
        .await
        .expect("revoke others");
    assert_eq!(ended, 2);

    assert!(
        database
            .session_for_digest("mine", limits())
            .await
            .expect("lookup")
            .is_some(),
        "the caller signed themselves out"
    );
    for digest in ["other-1", "other-2"] {
        assert!(
            database
                .session_for_digest(digest, limits())
                .await
                .expect("lookup")
                .is_none(),
            "{digest} survived"
        );
    }
    let live = database.list_sessions(limits()).await.expect("list");
    assert_eq!(live.len(), 1);
}

#[tokio::test]
async fn the_inventory_lists_live_sessions_by_most_recent_use() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;

    open(&database, "older", Some("A")).await;
    open(&database, "newer", Some("B")).await;
    // Touching moves it to the front, which is what makes the list readable.
    assert!(
        database
            .touch_session("older".to_owned())
            .await
            .expect("touch")
    );

    let sessions = database.list_sessions(limits()).await.expect("list");
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].user_agent.as_deref(), Some("A"));
}

/// Touching a session that is gone must report so rather than silently succeeding: it is how
/// the request path learns that a revocation elsewhere has taken effect.
#[tokio::test]
async fn touching_a_revoked_session_reports_that_it_is_gone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;

    let session = open(&database, "digest-a", None).await;
    database.revoke_session(session.id).await.expect("revoke");
    assert!(
        !database
            .touch_session("digest-a".to_owned())
            .await
            .expect("touch")
    );
}

/// The expiry fixed at sign-in still ends a session, whatever the limits say: a longer setting
/// applies to the next sign-in, not to a cookie issued for the old one.
#[tokio::test]
async fn an_expired_session_does_not_authenticate_and_is_not_listed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;

    database
        .create_session(
            SessionId::new(),
            "stale".to_owned(),
            None,
            None,
            // Negative hours put the expiry in the past, which is how a lapsed session is
            // reached without waiting twelve hours for one.
            -1,
        )
        .await
        .expect("session");

    assert!(
        database
            .session_for_digest("stale", limits())
            .await
            .expect("lookup")
            .is_none()
    );
    assert!(
        database
            .list_sessions(limits())
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn sessions_survive_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    {
        let database = database(directory.path()).await;
        open(&database, "digest-a", Some("Firefox")).await;
    }
    // Reopening the same file is how a restart is tested elsewhere in this repository.
    let database = database(directory.path()).await;
    let found = database
        .session_for_digest("digest-a", limits())
        .await
        .expect("lookup");
    assert!(
        found.is_some(),
        "the session did not survive; it used to live in a HashMap and this is the change"
    );
}

/// Long-dead rows are cleared, but a recently expired one stays visible long enough to be
/// recognised in the inventory.
#[tokio::test]
async fn only_long_expired_sessions_are_purged() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;

    database
        .create_session(SessionId::new(), "recent".to_owned(), None, None, -1)
        .await
        .expect("session");
    database
        .create_session(SessionId::new(), "ancient".to_owned(), None, None, -24 * 40)
        .await
        .expect("session");

    let purged = database
        .purge_expired_sessions(limits())
        .await
        .expect("purge");
    assert_eq!(purged, 1, "the recently lapsed session was thrown away too");
}

/// The idle limit slides: it counts from the last use, not from the sign-in (RD-130-09).
#[tokio::test]
async fn a_session_unused_for_longer_than_the_idle_limit_ends() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;
    let limits = SessionLimits {
        idle_hours: 2,
        max_hours: 720,
    };

    open(&database, "idle", None).await;
    open(&database, "busy", None).await;
    // Both signed in ten hours ago; one was last used three hours ago, the other just now.
    age(directory.path(), "idle", 10, 3).await;
    age(directory.path(), "busy", 10, 0).await;

    assert!(
        !is_live(&database, "idle", limits).await,
        "idled past the limit"
    );
    assert!(
        is_live(&database, "busy", limits).await,
        "a session in use ended although it is well inside the maximum"
    );
    let listed = database.list_sessions(limits).await.expect("list");
    assert_eq!(listed.len(), 1, "the idle session is still listed");
    // The inventory says when the session ends if it is left alone: two hours after its last
    // use, not the expiry written at sign-in.
    let ends = listed[0].last_used_at + Duration::hours(2);
    assert_eq!(listed[0].expires_at, ends);
}

/// The maximum lifetime ends a session however busy it is, and it is checked when the session
/// is read — so lowering it ends the sessions already past the new value at once (RD-130-09).
#[tokio::test]
async fn a_shorter_maximum_ends_sessions_that_are_already_past_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;

    // Created under a twelve-hour lifetime by `open`; signed in five hours ago, used just now.
    open(&database, "session", None).await;
    age(directory.path(), "session", 5, 0).await;

    let before = SessionLimits {
        idle_hours: 12,
        max_hours: 6,
    };
    let after = SessionLimits {
        idle_hours: 12,
        max_hours: 4,
    };
    assert!(is_live(&database, "session", before).await);
    assert!(
        !is_live(&database, "session", after).await,
        "a shorter maximum left an older session alive"
    );
    assert!(
        database
            .list_sessions(after)
            .await
            .expect("list")
            .is_empty(),
        "the inventory still lists a session the shorter maximum ended"
    );
}

/// A row that ended by idling is purged once the grace period after *that* has passed, not
/// kept for the rest of a maximum lifetime it will never reach.
#[tokio::test]
async fn a_session_that_idled_out_long_ago_is_purged() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;

    // Expiring ninety days from now by its stored date, but last used forty days ago.
    database
        .create_session(SessionId::new(), "idled".to_owned(), None, None, 24 * 90)
        .await
        .expect("session");
    age(directory.path(), "idled", 40 * 24, 40 * 24).await;
    open(&database, "fresh", None).await;

    let purged = database
        .purge_expired_sessions(SessionLimits {
            idle_hours: 12,
            max_hours: 24 * 90,
        })
        .await
        .expect("purge");
    assert_eq!(purged, 1);
    assert!(is_live(&database, "fresh", limits()).await);
}
