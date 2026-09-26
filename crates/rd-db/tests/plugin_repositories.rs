//! The plugin repository store (RD-140-01, migration 0097): the seeded official repository,
//! the replay floor that only rises, disable without deletion, and a key withdrawal that also
//! drops the matching trusted key.

use rd_core::EventKind;
use rd_db::{
    Database, NewPluginRepository, NewPluginTrustedKey, OFFICIAL_REPOSITORY_ID,
    PluginRepositoryInstall, PluginWithdrawnKey, RepositoryCheck,
};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("repositories.sqlite"))
        .await
        .expect("database")
}

fn third_party(id: &str, url: &str) -> NewPluginRepository {
    NewPluginRepository {
        id: id.to_owned(),
        name: "Community".to_owned(),
        url: url.to_owned(),
        key_id: "community-v1".to_owned(),
        public_key: "AAAA".to_owned(),
        fingerprint: "ab".repeat(32),
    }
}

#[tokio::test]
async fn the_official_repository_is_seeded_enabled_and_cannot_be_deleted() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    let repositories = database.list_plugin_repositories().await.expect("list");
    assert_eq!(repositories.len(), 1);
    let official = &repositories[0];
    assert_eq!(official.id, OFFICIAL_REPOSITORY_ID);
    assert!(official.is_official());
    assert!(official.enabled);
    assert_eq!(official.url, None);
    assert_eq!(official.sequence, None);
    assert!(
        !database
            .delete_plugin_repository(OFFICIAL_REPOSITORY_ID.to_owned())
            .await
            .expect("delete"),
        "the official repository was deleted"
    );
    assert_eq!(
        database
            .list_plugin_repositories()
            .await
            .expect("list")
            .len(),
        1
    );
}

#[tokio::test]
async fn a_third_party_repository_is_added_disabled_and_removed() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    let mut events = database.subscribe();
    let added = database
        .add_plugin_repository(third_party("r1", "https://plugins.example.test/index.json"))
        .await
        .expect("add");
    assert_eq!(added.kind, "third_party");
    assert!(added.enabled);
    assert_eq!(added.key_id.as_deref(), Some("community-v1"));
    // The same address twice is one repository, not two.
    assert!(
        database
            .add_plugin_repository(third_party("r2", "https://plugins.example.test/index.json"))
            .await
            .is_err()
    );
    let event = events.try_recv().expect("event");
    assert_eq!(event.kind, EventKind::PluginChanged);
    assert!(
        !event.payload.to_string().contains("example.test"),
        "the address reached the bus"
    );

    database
        .record_plugin_repository_install(PluginRepositoryInstall {
            plugin_id: "p".to_owned(),
            version: "1.0.0".to_owned(),
            digest: "cd".repeat(32),
            repository_id: "r1".to_owned(),
            installed_at: "2026-09-26T00:00:00Z".to_owned(),
        })
        .await
        .expect("install record");
    assert!(
        database
            .update_plugin_repository("r1".to_owned(), Some(false), None)
            .await
            .expect("disable")
    );
    let disabled = database
        .plugin_repository("r1")
        .await
        .expect("read")
        .expect("row");
    assert!(!disabled.enabled);
    // Disabling stops offers; it forgets nothing that was installed.
    assert_eq!(
        database
            .list_plugin_repository_installs()
            .await
            .expect("installs")
            .len(),
        1
    );
    assert!(
        database
            .delete_plugin_repository("r1".to_owned())
            .await
            .expect("delete")
    );
    assert!(
        database
            .plugin_repository("r1")
            .await
            .expect("read")
            .is_none()
    );
    assert!(
        database
            .list_plugin_repository_installs()
            .await
            .expect("installs")
            .is_empty()
    );
}

#[tokio::test]
async fn the_replay_floor_only_rises_and_a_failure_keeps_it() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    let official = OFFICIAL_REPOSITORY_ID.to_owned();
    for sequence in [7, 5] {
        database
            .record_plugin_repository_check(
                official.clone(),
                RepositoryCheck::Accepted {
                    sequence,
                    issued_at: "2026-09-26T00:00:00Z".to_owned(),
                },
            )
            .await
            .expect("accept");
    }
    database
        .record_plugin_repository_check(
            official.clone(),
            RepositoryCheck::Failed {
                code: "plugin_index.stale".to_owned(),
            },
        )
        .await
        .expect("fail");
    let row = database
        .plugin_repository(OFFICIAL_REPOSITORY_ID)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(row.sequence, Some(7));
    assert_eq!(row.last_error.as_deref(), Some("plugin_index.stale"));
    assert!(row.last_success_at.is_some());
}

#[tokio::test]
async fn a_withdrawn_key_is_recorded_once_and_untrusts_the_matching_key() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    let fingerprint = "ef".repeat(32);
    for (key_id, print) in [
        ("leaked-v1", fingerprint.clone()),
        ("other-v1", "01".repeat(32)),
    ] {
        database
            .trust_plugin_key(NewPluginTrustedKey {
                key_id: key_id.to_owned(),
                public_key: "AAAA".to_owned(),
                fingerprint: print,
                plugin_name: None,
            })
            .await
            .expect("trust");
    }
    let withdrawal = PluginWithdrawnKey {
        fingerprint: fingerprint.clone(),
        key_id: "leaked-v1".to_owned(),
        repository_id: OFFICIAL_REPOSITORY_ID.to_owned(),
        withdrawn_at: "2026-09-26T00:00:00Z".to_owned(),
    };
    let mut events = database.subscribe();
    assert!(
        database
            .withdraw_plugin_key(withdrawal.clone())
            .await
            .expect("withdraw")
    );
    assert!(
        !database
            .withdraw_plugin_key(withdrawal)
            .await
            .expect("repeat")
    );
    let trust_events = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| event.kind == EventKind::PluginTrustChanged)
        .count();
    assert_eq!(trust_events, 1, "a repeated withdrawal was announced again");
    let keys = database.list_plugin_trusted_keys().await.expect("keys");
    assert_eq!(
        keys.iter()
            .map(|key| key.key_id.as_str())
            .collect::<Vec<_>>(),
        vec!["other-v1"]
    );
    let withdrawn = database
        .list_plugin_withdrawn_keys()
        .await
        .expect("withdrawn");
    assert_eq!(withdrawn.len(), 1);
    assert_eq!(withdrawn[0].fingerprint, fingerprint);
}
