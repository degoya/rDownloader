//! A vault entry lives exactly as long as a cell names it.
//!
//! DB-02: a sign-in that is cancelled, restarted or replaced lets its tokens go from the vault in
//! the same call, and so does deleting its account. DB-03: what a stop between a value and its
//! row leaves behind is removed by the sweep at start, which never touches an entry a column or
//! a settings document names.

use std::path::Path;

use rd_core::{AccountId, AuthFlowState, RemoteJobSourceKind};
use rd_db::{ClaimRemoteJob, Database, NewAccount, UpsertAuthFlow};

async fn open(directory: &Path) -> Database {
    let database = Database::open(directory.join("vault.sqlite"))
        .await
        .expect("database");
    database
        .install_file_vault(directory.join("secrets"))
        .await
        .expect("vault");
    database
}

async fn account(database: &Database) -> AccountId {
    database
        .create_account(NewAccount {
            provider: "demo".to_owned(),
            label: "Demo".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account")
        .id
}

async fn put(database: &Database, value: &str) -> String {
    database
        .secret_vault()
        .expect("vault")
        .put_string(value.to_owned())
        .await
        .expect("put")
}

async fn present(database: &Database, reference: &str) -> bool {
    database
        .secret_vault()
        .expect("vault")
        .get(reference)
        .await
        .is_ok()
}

fn flow(
    account_id: AccountId,
    refresh_ref: Option<String>,
    access_ref: Option<String>,
    key_ref: Option<String>,
) -> UpsertAuthFlow {
    UpsertAuthFlow {
        account_id,
        plugin_id: "demo-plugin".to_owned(),
        state: AuthFlowState::Authorized,
        verification_url: None,
        user_code: None,
        expires_at: None,
        next_poll_at: None,
        message: None,
        token_expires_at: None,
        refresh_ref,
        access_ref,
        key_ref,
        callback_state: None,
        flow_state: None,
    }
}

/// Cancelling a sign-in takes its three tokens and its parts out of the vault.
#[tokio::test]
async fn deleting_a_sign_in_forgets_its_tokens_and_parts() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let id = account(&database).await;
    let refresh = put(&database, "refresh").await;
    let access = put(&database, "access").await;
    let key = put(&database, "key").await;
    let part = put(&database, "part").await;
    database
        .upsert_auth_flow(flow(
            id,
            Some(refresh.clone()),
            Some(access.clone()),
            Some(key.clone()),
        ))
        .await
        .expect("flow");
    database
        .set_auth_flow_part(id, "client_secret".to_owned(), part.clone())
        .await
        .expect("part");

    database.delete_auth_flow(id).await.expect("delete");

    for reference in [&refresh, &access, &key, &part] {
        assert!(!present(&database, reference).await, "{reference} leaves");
    }
    assert!(database.auth_flow(id).await.expect("flow").is_none());
}

/// A restarted sign-in writes no references; the previous one's go. A step that carries them
/// over keeps them.
#[tokio::test]
async fn restarting_a_sign_in_forgets_the_tokens_it_replaced_and_keeps_the_carried_ones() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let id = account(&database).await;
    let refresh = put(&database, "refresh").await;
    let access = put(&database, "access").await;
    database
        .upsert_auth_flow(flow(id, Some(refresh.clone()), Some(access.clone()), None))
        .await
        .expect("flow");

    // Carried over unchanged, as the service does when a plugin reports success.
    database
        .upsert_auth_flow(flow(id, Some(refresh.clone()), Some(access.clone()), None))
        .await
        .expect("carried");
    assert!(present(&database, &refresh).await);
    assert!(present(&database, &access).await);

    // Started again: nothing carried.
    database
        .upsert_auth_flow(UpsertAuthFlow {
            state: AuthFlowState::WaitingForUser,
            callback_state: Some("echo".to_owned()),
            ..flow(id, None, None, None)
        })
        .await
        .expect("restart");
    assert!(!present(&database, &refresh).await);
    assert!(!present(&database, &access).await);
}

/// A renewal that brings a new token drops the key of the old session, from the row and from
/// the vault.
#[tokio::test]
async fn a_renewal_with_a_new_token_forgets_the_old_sessions_key() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let id = account(&database).await;
    let key = put(&database, "key").await;
    let access = put(&database, "access").await;
    database
        .upsert_auth_flow(flow(id, None, Some(access), Some(key.clone())))
        .await
        .expect("flow");

    let renewed = put(&database, "renewed").await;
    database
        .set_auth_flow_renewal(id, None, None, Some(renewed))
        .await
        .expect("renewal");

    assert!(!present(&database, &key).await);
    let stored = database.auth_flow(id).await.expect("flow").expect("row");
    assert!(stored.key_ref.is_none());
}

/// Deleting the account cascades its sign-in away; the tokens go with it.
#[tokio::test]
async fn deleting_an_account_forgets_its_sign_in() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let id = account(&database).await;
    let refresh = put(&database, "refresh").await;
    let part = put(&database, "part").await;
    database
        .upsert_auth_flow(flow(id, Some(refresh.clone()), None, None))
        .await
        .expect("flow");
    database
        .set_auth_flow_part(id, "client_secret".to_owned(), part.clone())
        .await
        .expect("part");

    database.delete_account(id).await.expect("delete");

    assert!(!present(&database, &refresh).await);
    assert!(!present(&database, &part).await);
}

/// The sweep removes what nothing names -- an orphaned value and a stopped write's temporary --
/// and keeps what a column or a settings document names.
#[tokio::test]
async fn the_sweep_removes_only_what_no_cell_names() {
    let directory = tempfile::tempdir().expect("tempdir");
    let secrets = directory.path().join("secrets");
    let database = open(directory.path()).await;
    let id = account(&database).await;
    let in_column = put(&database, "refresh").await;
    let in_document = put(&database, "client secret").await;
    let orphan = put(&database, "orphan").await;
    let stopped = rd_secrets::SecretStore::new_reference();
    let temporary = secrets.join(format!(".{}.tmp", stopped.trim_start_matches("vault://")));
    std::fs::write(&temporary, b"partial").expect("temporary");
    database
        .upsert_auth_flow(flow(id, Some(in_column.clone()), None, None))
        .await
        .expect("flow");
    database
        .set_setting(
            "captcha".to_owned(),
            serde_json::json!({ "solver": "none", "api_key_ref": in_document }),
        )
        .await
        .expect("setting");

    assert_eq!(database.sweep_vault().await.expect("sweep"), 2);

    assert!(present(&database, &in_column).await);
    assert!(present(&database, &in_document).await);
    assert!(!present(&database, &orphan).await);
    assert!(!temporary.exists());
    assert!(secrets.join("master.key").exists(), "the key is no entry");
    assert_eq!(database.sweep_vault().await.expect("again"), 0);
}

/// RA-DB-06: a blob is read like a text. A reference inside one (here a remote job's container
/// bytes, behind bytes that are no UTF-8) keeps its entry, and the blob does not fail the sweep.
#[tokio::test]
async fn the_sweep_keeps_what_a_blob_cell_names() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let id = account(&database).await;
    let in_blob = put(&database, "tracker passkey").await;
    let orphan = put(&database, "orphan").await;
    let mut source = vec![0xff, 0xfe, 0x00];
    source.extend(
        format!("magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709&x={in_blob}")
            .into_bytes(),
    );
    database
        .claim_remote_job(ClaimRemoteJob {
            id: rd_core::RemoteJobId::new(),
            account_id: id,
            plugin_id: "demo-plugin".to_owned(),
            content_key: "torrent:blob".to_owned(),
            source_kind: RemoteJobSourceKind::Container,
            source,
            source_name: None,
            package_id: None,
        })
        .await
        .expect("remote job");

    assert_eq!(database.sweep_vault().await.expect("sweep"), 1);

    assert!(present(&database, &in_blob).await);
    assert!(!present(&database, &orphan).await);
}

/// Without a vault there is nothing to sweep, and nothing fails.
#[tokio::test]
async fn the_sweep_without_a_vault_does_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("plain.sqlite"))
        .await
        .expect("database");
    assert_eq!(database.sweep_vault().await.expect("sweep"), 0);
}

/// The columns `restore_copy` lists are in the schema. The sweep itself reads every text and
/// blob cell whatever the column is declared as (RA-DB-06), so a reference column added later
/// needs no list entry to keep its entries.
#[tokio::test]
async fn every_listed_reference_column_is_in_the_schema() {
    use sqlx::Connection;

    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("schema.sqlite");
    drop(Database::open(&path).await.expect("database"));
    let mut connection = sqlx::SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("connect");
    let columns: Vec<(String, String)> = sqlx::query_as(
        "SELECT m.name, p.name FROM sqlite_master m \
         JOIN pragma_table_info(m.name) p WHERE m.type = 'table'",
    )
    .fetch_all(&mut connection)
    .await
    .expect("schema");
    let listed = rd_db::restore_copy::BUNDLED_SECRET_COLUMNS
        .iter()
        .chain(rd_db::restore_copy::UNBUNDLED_SECRET_COLUMNS)
        .chain([&rd_db::restore_copy::BACKUP_KEY_REF]);
    for column in listed {
        assert!(
            columns
                .iter()
                .any(|(table, name)| table == column.table && name == column.column),
            "{}.{} is listed but not in the schema",
            column.table,
            column.column
        );
    }
}
