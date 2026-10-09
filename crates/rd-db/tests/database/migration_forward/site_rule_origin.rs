//! Migrations `0132` (RD-1200-05) and `0134` (RD-1230-03): where a site rule came from, and the
//! signature that went again.

use rd_db::SiteRuleOriginKind;
use sqlx::{Connection, SqliteConnection};

use super::schema_at;

/// A rule from before RD-1200-05 reads as of unknown origin: nothing then recorded whether it
/// came from the signed file, and a guess would claim a signature for a body that may have
/// been edited since. Its body and switch survive both migrations.
#[tokio::test]
async fn a_rule_from_before_the_origin_reads_as_unknown() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 130)
        .await
        .expect("schema at 0130");
    {
        let url = format!("sqlite://{}", path.display());
        let mut connection = SqliteConnection::connect(&url).await.expect("connect");
        sqlx::query(
            "INSERT INTO site_rules (id, name, rule_group, enabled, rule_json, created_at,
                                     updated_at)
             VALUES ('old-rule', 'An old rule', 'board', 1, '{\"id\":\"old-rule\"}',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(&mut connection)
        .await
        .expect("insert a site rule on the 0130 schema");
        connection.close().await.expect("close");
    }

    let database = rd_db::Database::open(&path).await.expect("upgrade");
    let rules = database.list_site_rules().await.expect("site rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].origin, SiteRuleOriginKind::Unknown);
    assert!(rules[0].enabled);
    assert_eq!(rules[0].rule["id"], "old-rule");
}

/// A rule the signed file brought, on the schema right before `0134`: it keeps its body and
/// switch, reads as of unknown origin, and the signer, the sequence and the table of accepted
/// sequences are gone.
#[tokio::test]
async fn a_rule_from_the_signed_file_survives_the_signature_s_removal() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 133)
        .await
        .expect("schema at 0133");
    let url = format!("sqlite://{}", path.display());
    {
        let mut connection = SqliteConnection::connect(&url).await.expect("connect");
        sqlx::query(
            "INSERT INTO site_rules (id, name, rule_group, enabled, rule_json, created_at,
                                     updated_at, origin, origin_signer, origin_sequence)
             VALUES ('from-the-file', 'From the file', 'board', 0, '{\"id\":\"from-the-file\"}',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'signed',
                     'rdownloader-siterules-v1', 9)",
        )
        .execute(&mut connection)
        .await
        .expect("insert a signed rule on the 0133 schema");
        sqlx::query(
            "INSERT INTO site_rule_pack_sequences (signer, sequence, accepted_at)
             VALUES ('rdownloader-siterules-v1', 9, '2026-01-01T00:00:00Z')",
        )
        .execute(&mut connection)
        .await
        .expect("insert a sequence mark on the 0133 schema");
        connection.close().await.expect("close");
    }

    let database = rd_db::Database::open(&path).await.expect("upgrade");
    let rules = database.list_site_rules().await.expect("site rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].origin, SiteRuleOriginKind::Unknown);
    assert!(!rules[0].enabled);
    assert_eq!(rules[0].rule["id"], "from-the-file");
    drop(database);

    let mut connection = SqliteConnection::connect(&url).await.expect("connect");
    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'site_rule_pack_sequences'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("read the schema");
    assert_eq!(tables, 0);
    let columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('site_rules')
         WHERE name IN ('origin_signer', 'origin_sequence')",
    )
    .fetch_one(&mut connection)
    .await
    .expect("read the columns");
    assert_eq!(columns, 0);
    connection.close().await.expect("close");
}
