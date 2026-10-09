//! Migration `0132` (RD-1200-05): a site rule stored before the origin existed.

use rd_db::SiteRuleOriginKind;
use sqlx::{Connection, SqliteConnection};

use super::schema_at;

/// A rule from before RD-1200-05 reads as of unknown origin, with no signer and no sequence:
/// nothing then recorded whether it came from the signed file, and a guess would claim a
/// signature for a body that may have been edited since. Its body and switch survive.
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
             VALUES ('scnlog', 'scnlog.me', 'board', 1, '{\"id\":\"scnlog\"}',
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
    assert_eq!(rules[0].origin.kind, SiteRuleOriginKind::Unknown);
    assert_eq!(rules[0].origin.signer, None);
    assert_eq!(rules[0].origin.sequence, None);
    assert!(rules[0].enabled);
    assert_eq!(rules[0].rule["id"], "scnlog");
    // No signer has a mark yet: the first signed file after the upgrade sets it.
    assert_eq!(
        database
            .record_site_rule_pack("rdownloader-siterules-v1", 9)
            .await
            .expect("record"),
        None
    );
}
