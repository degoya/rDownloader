//! Enricher fields on packages and queue rows: carried from the link candidates at the enqueue,
//! or onto the row one candidate became when the enricher answers late. Split from
//! `package_store.rs`, which re-exports it, so neither file outgrows the 500-line rule
//! (RD-1280-08).

use anyhow::Result;
use rd_core::PackageId;
use sqlx::{Connection, SqliteConnection};

/// Carries enricher fields from the link candidates onto the queue rows they became
/// (RD-107-02).
///
/// One transaction for the whole package, because the two halves are one statement about the
/// same enqueue: a package that claims fields none of its files carry would be worse than a
/// package with no fields at all.
///
/// A replace, not an append. Running it twice writes the same rows, which is what makes a
/// retried enqueue harmless. An empty list clears the column.
pub(crate) async fn carry_enrichment(
    connection: &mut SqliteConnection,
    package_id: PackageId,
    package_fields: &[rd_core::EnrichmentField],
    files: &[(rd_core::DownloadId, Vec<rd_core::EnrichmentField>)],
) -> Result<()> {
    let mut transaction = connection.begin().await?;
    sqlx::query("UPDATE packages SET enrichment_json = ? WHERE id = ?")
        .bind(encode_enrichment(package_fields)?)
        .bind(package_id.to_string())
        .execute(&mut *transaction)
        .await?;
    for (id, fields) in files {
        sqlx::query("UPDATE downloads SET enrichment_json = ? WHERE id = ?")
            .bind(encode_enrichment(fields)?)
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// An empty list is `NULL` rather than `[]`, so "nothing was enriched" reads the same on a row
/// written before this column existed and on one written after.
pub(crate) fn encode_enrichment(fields: &[rd_core::EnrichmentField]) -> Result<Option<String>> {
    if fields.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(fields)?))
}

/// Carries enricher fields onto the queue rows one link candidate became (RD-108-15).
///
/// Storing the fields and copying them into the queue are two writer commands with no order
/// between them: an enricher that answers while the auto-queue promotion is already running
/// used to leave its fields on a candidate row the enqueue detaches moments later, so neither
/// the package nor the queue row ever saw them. Carrying them forward here makes both orders
/// end in the same place.
///
/// The row is found by source address, the same way the enqueue matches a candidate to the row
/// it became; the newest one wins, because that is the enqueue this write belongs to. Returns
/// whether a row was found at all.
pub(crate) async fn carry_enrichment_for_source(
    connection: &mut SqliteConnection,
    source_url: &str,
    fields: &[rd_core::EnrichmentField],
) -> Result<bool> {
    if fields.is_empty() {
        return Ok(false);
    }
    let Some((download_id, package_id)) = sqlx::query_as::<_, (String, String)>(
        "SELECT id, package_id FROM downloads WHERE source_url = ? \
         ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .bind(source_url)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(false);
    };
    sqlx::query("UPDATE downloads SET enrichment_json = ? WHERE id = ?")
        .bind(encode_enrichment(fields)?)
        .bind(&download_id)
        .execute(&mut *connection)
        .await?;
    let stored: Option<String> =
        sqlx::query_scalar("SELECT enrichment_json FROM packages WHERE id = ?")
            .bind(&package_id)
            .fetch_optional(&mut *connection)
            .await?
            .flatten();
    let stored = crate::models::parse_enrichment(stored.as_deref(), "packages", &package_id);
    let merged = merge_enrichment(&stored, fields);
    sqlx::query("UPDATE packages SET enrichment_json = ? WHERE id = ?")
        .bind(encode_enrichment(&merged)?)
        .bind(&package_id)
        .execute(&mut *connection)
        .await?;
    Ok(true)
}

/// The package header keeps one entry per plugin and name, the one that was there first.
///
/// Deduplicated exactly like the union the enqueue builds, so a field that arrives late reads
/// the same as one that was there in time, and the other files' fields are left alone.
fn merge_enrichment(
    existing: &[rd_core::EnrichmentField],
    added: &[rd_core::EnrichmentField],
) -> Vec<rd_core::EnrichmentField> {
    let mut merged = existing.to_vec();
    for field in added {
        if merged
            .iter()
            .any(|kept| kept.plugin_id == field.plugin_id && kept.name == field.name)
        {
            continue;
        }
        merged.push(field.clone());
    }
    merged
}
