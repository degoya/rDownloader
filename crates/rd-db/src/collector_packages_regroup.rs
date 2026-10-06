//! Regrouping auto-named packages once the online check revealed the real file names.

use anyhow::Result;
use rd_core::{BatchId, CollectorPackageId, DownloadPriority, EventEnvelope};
use sqlx::{Connection, SqliteConnection};

use super::{collector_event, delete_empty_packages, insert};
use crate::{collector_store::insert_event, parse_id};

/// `(id, file_name, url, package_id, category_id, priority, provider, file_name_declared)`
/// row used while regrouping.
///
/// The provider and whether the name was declared travel with the row because grouping needs
/// them: a container the source named is a release of its own and must not be merged back
/// into a package named after the indexer's host.
type RegroupRow = (
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    i64,
    Option<String>,
    i64,
);

/// One batch's candidates together with the packages grouping proposes for them, carried from
/// the read phase into the write transaction.
type RegroupPlan = (BatchId, Vec<RegroupRow>, Vec<rd_collector::Group>);

/// Re-derives auto-named packages of the given batches from current file names
/// (multipart sets become one package once the online check revealed the real names).
pub(crate) async fn regroup(
    connection: &mut SqliteConnection,
    batch_ids: &[BatchId],
) -> Result<EventEnvelope> {
    // Read and group first, write afterwards. Grouping parses a URL and analyses a name for
    // every candidate of every batch, and it used to run inside the write transaction — the
    // one the serialized writer holds, so the whole database waited on it. Nothing can slip in
    // between the two phases: every mutation goes through the single writer connection this
    // function borrows for the whole command, so the shorter transaction costs no lost update.
    let mut planned: Vec<RegroupPlan> = Vec::new();
    for batch_id in batch_ids {
        let candidates: Vec<RegroupRow> = sqlx::query_as(
            "SELECT c.id, c.file_name, c.url, c.package_id, p.category_id, p.priority, c.provider, c.file_name_declared FROM link_candidates c \
             JOIN collector_packages p ON p.id = c.package_id \
             WHERE c.batch_id = ? AND p.auto_named = 1 AND c.state NOT IN ('enqueued', 'resolving') \
             ORDER BY p.position, c.position",
        )
        .bind(batch_id.to_string())
        .fetch_all(&mut *connection)
        .await?;
        if candidates.is_empty() {
            continue;
        }
        let groups = {
            let hosts: Vec<String> = candidates
                .iter()
                .map(|(_, _, url, ..)| {
                    url::Url::parse(url)
                        .ok()
                        .and_then(|value| value.host_str().map(str::to_owned))
                        .unwrap_or_default()
                })
                .collect();
            let inputs: Vec<rd_collector::GroupInput<'_>> = candidates
                .iter()
                .enumerate()
                .map(|(index, (_, name, _, _, _, _, provider, declared))| {
                    rd_collector::GroupInput {
                        index,
                        file_name: name.as_deref(),
                        host: &hosts[index],
                        standalone: *declared != 0
                            && matches!(
                                provider.as_deref(),
                                Some(rd_core::NZB_PROVIDER | rd_core::TORRENT_PROVIDER)
                            ),
                        // Regrouping an existing batch: the folder a link came from is not
                        // stored on the candidate, so there is nothing to hint with here.
                        // It does not have to be: a package a source named is not auto-named
                        // (RD-120-17) and therefore never reaches this query at all.
                        package_hint: None,
                    }
                })
                .collect();
            rd_collector::group_links(&inputs, None, "Links")
        };
        planned.push((*batch_id, candidates, groups));
    }
    let mut tx = connection.begin().await?;
    let mut changed = 0_usize;
    let mut touched: Vec<CollectorPackageId> = Vec::new();
    for (batch_id, candidates, groups) in &planned {
        for group in groups {
            let members: Vec<&RegroupRow> = group
                .members
                .iter()
                .map(|index| &candidates[*index])
                .collect();
            // Reuse the package most members already live in when its name matches.
            let existing: Option<String> = sqlx::query_scalar(
                "SELECT id FROM collector_packages WHERE batch_id = ? AND auto_named = 1 AND name = ? COLLATE NOCASE",
            )
            .bind(batch_id.to_string())
            .bind(&group.name)
            .fetch_optional(&mut *tx)
            .await?;
            let package_id = match existing {
                Some(id) => id,
                None => {
                    let (category, priority) = (&members[0].4, members[0].5);
                    insert(
                        &mut tx,
                        *batch_id,
                        &group.name,
                        true,
                        category.as_deref().map(parse_id).transpose()?,
                        DownloadPriority::from_i32(i32::try_from(priority).unwrap_or_default()),
                    )
                    .await?
                    .to_string()
                }
            };
            for (position, member) in members.iter().enumerate() {
                if member.3 != package_id {
                    changed += 1;
                    touched.push(parse_id(&member.3)?);
                }
                sqlx::query("UPDATE link_candidates SET package_id = ?, position = ? WHERE id = ?")
                    .bind(&package_id)
                    .bind(i64::try_from(position)? + 1)
                    .bind(&member.0)
                    .execute(&mut *tx)
                    .await?;
            }
            touched.push(parse_id(&package_id)?);
        }
    }
    // The regroup after the online check is where sources two and three finally have
    // something to say: until the probe came back the names were the address's last segment
    // and no size was known (RD-110-18). Every package the pass touched is recomputed,
    // including the ones links were taken *out* of, whose groups may now have one member.
    touched.sort_unstable();
    touched.dedup();
    crate::collector_mirrors::assign(&mut tx, &touched).await?;
    delete_empty_packages(&mut tx).await?;
    let event = collector_event(serde_json::json!({ "regrouped_candidates": changed }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}
