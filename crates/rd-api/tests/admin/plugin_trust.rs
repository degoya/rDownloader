//! Revoking a plugin signing key: the database row first, then the live verifier, and a failed
//! write is an error rather than a success (audit 1.9.1, API-13; RD-1120-19).
//!
//! The handler used to drop the key from the live verifier first and write the row with
//! `.ok()`: a write that failed answered "revoked", the running service stopped trusting the key,
//! and the next start trusted it again from the row nobody had removed.

use crate::common;

use axum::http::StatusCode;
use common::{delete_json, test_harness};

const KEY: &str = "revocation-order-key";

#[tokio::test]
async fn a_key_the_database_could_not_revoke_stays_trusted_and_the_refusal_says_so() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let author = rd_plugin_host::generate_signing_key();
    let verifier = harness.state.plugins.verifier();
    verifier
        .trust_key_base64(KEY.to_owned(), &author.public_base64)
        .expect("trust the key");
    let uri = format!("/api/v1/plugins/keys/{KEY}");

    // The table out of reach, so the row cannot be written.
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", harness.database_path.display()))
        .await
        .expect("pool");
    sqlx::query("ALTER TABLE plugin_trusted_keys RENAME TO plugin_trusted_keys_away")
        .execute(&pool)
        .await
        .expect("move the table away");
    let (status, body) = delete_json(&harness.router, &uri).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(
        verifier.is_trusted(KEY).expect("the verifier"),
        "the live verifier dropped a key the database still holds"
    );

    sqlx::query("ALTER TABLE plugin_trusted_keys_away RENAME TO plugin_trusted_keys")
        .execute(&pool)
        .await
        .expect("put the table back");
    pool.close().await;
    let (status, body) = delete_json(&harness.router, &uri).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "plugin.key_revoked");
    assert!(
        !verifier.is_trusted(KEY).expect("the verifier"),
        "a revoked key is still trusted by the running service"
    );
}
