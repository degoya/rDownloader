//! Mirror groups on the LinkGrabber's candidates (RD-110-18).
//!
//! The persistence half of `rd_collector::group_mirrors`: it reads one package's links back,
//! asks that function which of them point at the same file, and writes the answer onto the
//! rows. Everything about *what* a mirror is lives there; what lives here is when the answer
//! is recomputed and how it survives a restart.
//!
//! It is recomputed twice: once at intake, where a site rule's declaration is all there is to
//! go on, and once whenever the batch is regrouped — which is what happens after the online
//! check, when the real file names and the sizes have arrived and sources two and three can
//! finally say anything. The derived columns are rewritten in full each time; the columns
//! holding what the source *declared* are never touched again, so a regroup cannot lose it.
//!
//! Since RD-110-34 it also reads what a person *refused*: `collector_mirror_separations`
//! holds the pairs of links somebody stated are not the same file, and they are handed to the
//! grouping so a dissolved proposal cannot be rebuilt by the very next recompute. The pairs
//! are read per package, because a group lives inside one package and a pair whose ends sit
//! in two of them can never form a group anyway.
//!
//! `state` is not read and not written here. A mirror is not a duplicate, and the one place
//! the two meet is the rule that two links with the same address are never mirrors — which
//! `rd_collector` applies on the address itself, not on the state.

use anyhow::Result;
use rd_core::{CandidateId, CollectorPackageId, MirrorHint, MirrorPreference, MirrorSource};
use rd_core::{EventEnvelope, EventKind};
use sqlx::{Connection, FromRow, SqliteConnection};

/// Settings key holding the standing mirror preference (RD-110-19).
///
/// Its own key rather than a field of the `service.settings` blob, for two reasons. The blob
/// is written back whole by the settings page, so a preference changed from the LinkGrabber
/// toolbar would race it; and this crate has to read the value inside a write transaction,
/// where the typed reader on `Database` — which goes to the read pool — is not available.
pub const MIRROR_PREFERENCE_KEY: &str = "collector.mirror_preference";

/// One candidate, as far as mirror grouping is concerned.
#[derive(FromRow)]
struct MirrorRow {
    id: String,
    url: String,
    file_name: Option<String>,
    file_name_declared: i64,
    size: Option<i64>,
    mirror_pinned: i64,
    mirror_declared: Option<String>,
    mirror_quality: Option<String>,
    mirror_language: Option<String>,
}

const MIRROR_SELECT: &str = "SELECT id, url, file_name, file_name_declared, size, \
     mirror_pinned, mirror_declared, mirror_quality, mirror_language FROM link_candidates \
     WHERE package_id = ? ORDER BY position, created_at, id";

/// The standing preference, read on the connection the regroup already holds.
///
/// A missing or malformed value reads as "nothing preferred" rather than failing the whole
/// regroup: a preference is a default, and a broken default must not stop the links from
/// being grouped at all.
pub(crate) async fn read_preference(connection: &mut SqliteConnection) -> Result<MirrorPreference> {
    let raw: Option<String> = sqlx::query_scalar("SELECT value_json FROM settings WHERE key = ?")
        .bind(MIRROR_PREFERENCE_KEY)
        .fetch_optional(&mut *connection)
        .await?;
    Ok(raw
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default())
}

/// Makes one candidate its group's chosen mirror, or takes that choice back.
///
/// The siblings are cleared in the same transaction, so "at most one pinned member per group"
/// is a property of the write rather than a hope about the callers. Regrouping the package
/// afterwards is what actually moves `mirror_selected`: this function states the decision and
/// [`assign`] applies it, so there is exactly one place that decides which member is chosen.
pub(crate) async fn set_pin(
    connection: &mut SqliteConnection,
    id: CandidateId,
    pinned: bool,
) -> Result<Option<CollectorPackageId>> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT package_id, mirror_group FROM link_candidates WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *connection)
            .await?;
    let Some((Some(package), Some(group))) = row else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE link_candidates SET mirror_pinned = 0 WHERE package_id = ? AND mirror_group = ?",
    )
    .bind(&package)
    .bind(&group)
    .execute(&mut *connection)
    .await?;
    if pinned {
        sqlx::query("UPDATE link_candidates SET mirror_pinned = 1 WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *connection)
            .await?;
    }
    Ok(Some(crate::parse_id(&package)?))
}

/// Every package that currently holds links, for a preference change that touches all of them.
pub(crate) async fn all_packages(
    connection: &mut SqliteConnection,
) -> Result<Vec<CollectorPackageId>> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT package_id FROM link_candidates WHERE package_id IS NOT NULL",
    )
    .fetch_all(&mut *connection)
    .await?;
    rows.iter().map(|value| crate::parse_id(value)).collect()
}

/// The pairs of this package's links a person stated are not mirrors of each other.
///
/// Both ends have to sit in the package being regrouped: a group lives inside one package, so
/// a pair reaching outside it could not be grouped in the first place and reading it would
/// only make the set bigger.
async fn read_separations(
    connection: &mut SqliteConnection,
    package: CollectorPackageId,
) -> Result<rd_collector::MirrorSeparations> {
    let pairs: Vec<(String, String)> = sqlx::query_as(
        "SELECT s.left_id, s.right_id FROM collector_mirror_separations s \
         JOIN link_candidates l ON l.id = s.left_id \
         JOIN link_candidates r ON r.id = s.right_id \
         WHERE l.package_id = ? AND r.package_id = ?",
    )
    .bind(package.to_string())
    .bind(package.to_string())
    .fetch_all(&mut *connection)
    .await?;
    Ok(rd_collector::MirrorSeparations::new(pairs))
}

/// Recomputes the mirror groups of every named package.
///
/// Per package rather than per batch: a group lives inside one package, so moving a link to
/// another package has to be able to rebuild both sides without touching anything else.
pub(crate) async fn assign(
    connection: &mut SqliteConnection,
    packages: &[CollectorPackageId],
) -> Result<()> {
    let preference = read_preference(&mut *connection).await?;
    for package in packages {
        let rows: Vec<MirrorRow> = sqlx::query_as(MIRROR_SELECT)
            .bind(package.to_string())
            .fetch_all(&mut *connection)
            .await?;
        if rows.is_empty() {
            continue;
        }
        let separations = read_separations(&mut *connection, *package).await?;
        // The hints have to outlive the inputs that borrow them, so they are built first and
        // in full — one entry per row, `None` where the source said nothing.
        let hints: Vec<Option<MirrorHint>> = rows
            .iter()
            .map(|row| {
                row.mirror_declared
                    .as_deref()
                    .map(str::trim)
                    .filter(|group| !group.is_empty())
                    .map(|group| MirrorHint {
                        group: group.to_owned(),
                        quality: row.mirror_quality.clone(),
                        language: row.mirror_language.clone(),
                    })
            })
            .collect();
        let inputs: Vec<rd_collector::MirrorInput<'_>> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| rd_collector::MirrorInput {
                id: &row.id,
                url: &row.url,
                file_name: row.file_name.as_deref(),
                file_name_declared: row.file_name_declared != 0,
                // A negative size is not a size. It cannot occur — intake stores what it
                // converted from a `ByteCount` — but reading it as an unknown is the answer
                // that overstates nothing.
                size: row.size.and_then(|value| u64::try_from(value).ok()),
                hint: hints[index].as_ref(),
                pinned: row.mirror_pinned != 0,
            })
            .collect();
        let groups = rd_collector::group_mirrors(&inputs, &preference, &separations);
        for (row, group) in rows.iter().zip(groups) {
            sqlx::query(
                "UPDATE link_candidates SET mirror_group = ?, mirror_source = ?, \
                 mirror_selected = ?, mirror_pinned = ?, mirror_quality = ?, \
                 mirror_language = ? WHERE id = ?",
            )
            .bind(group.as_ref().map(|entry| entry.group.clone()))
            .bind(
                group
                    .as_ref()
                    .map(|entry| crate::collector_store::enum_string(entry.source))
                    .transpose()?,
            )
            .bind(i64::from(
                group.as_ref().is_some_and(|entry| entry.selected),
            ))
            // A pin on a link that is no longer in a group is a decision about nothing, and
            // leaving it would let it come back the next time the same name reappears.
            .bind(i64::from(group.as_ref().is_some_and(|entry| entry.pinned)))
            // The facets are written back so a reader of the row alone sees what the group
            // says about this mirror, whether the source named it or the release name did.
            .bind(
                group
                    .as_ref()
                    .and_then(|entry| entry.quality.clone())
                    .or_else(|| row.mirror_quality.clone()),
            )
            .bind(
                group
                    .as_ref()
                    .and_then(|entry| entry.language.clone())
                    .or_else(|| row.mirror_language.clone()),
            )
            .bind(&row.id)
            .execute(&mut *connection)
            .await?;
        }
    }
    Ok(())
}

/// Stores the standing preference and re-chooses every group under it.
///
/// One transaction on purpose: a preference the interface has stored while the rows still
/// carry the previous choice is exactly the inconsistency that makes somebody queue the mirror
/// they thought they had moved away from.
pub(crate) async fn store_preference(
    connection: &mut SqliteConnection,
    preference: &MirrorPreference,
) -> Result<EventEnvelope> {
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at) VALUES (?, ?, ?) \
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, \
         updated_at = excluded.updated_at",
    )
    .bind(MIRROR_PREFERENCE_KEY)
    .bind(serde_json::to_string(preference)?)
    .bind(chrono::Utc::now())
    .execute(&mut *tx)
    .await?;
    let packages = all_packages(&mut tx).await?;
    assign(&mut tx, &packages).await?;
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "mirror_preference": packages.len() }),
    );
    crate::collector_store::insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Records that a person chose this mirror by hand, or takes that back, and regroups.
///
/// Returns `false` when the link is not in a mirror group at all, which the REST layer turns
/// into a refusal rather than a silent success: pinning a link that is a mirror of nothing is
/// a request about something that does not exist.
pub(crate) async fn store_pin(
    connection: &mut SqliteConnection,
    id: CandidateId,
    pinned: bool,
) -> Result<Option<EventEnvelope>> {
    let mut tx = connection.begin().await?;
    let Some(package) = set_pin(&mut tx, id, pinned).await? else {
        tx.rollback().await?;
        return Ok(None);
    };
    assign(&mut tx, &[package]).await?;
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "mirror_pinned": pinned, "package_id": package }),
    );
    crate::collector_store::insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(Some(event))
}

/// What became of a request to take a mirror group apart (RD-110-34).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirrorDissolve {
    /// The group is gone; its members are single candidates and stay that way.
    Dissolved,
    /// The link is in no mirror group, so there was nothing to take apart.
    NotGrouped,
    /// The group is a declaration or a name-and-size agreement, and those are refused.
    ///
    /// Not a question the interface may talk somebody past: a contradiction against those two
    /// is a finding about the *source* — a site rule that declares wrongly, or two files that
    /// genuinely agree on name and size — and clicking it away would fix one package while
    /// leaving the rule to do the same thing on the next page.
    NotProposed,
}

/// Takes a proposed mirror group apart and records that its links are not the same file.
///
/// The decision is stored as pairs rather than as an absence of a group, because the group is
/// derived: the very next regroup would rebuild it. Every pair of the group's members is
/// written, so no two of them are put back together whatever a later source would make of
/// them, and a link that joins later finds the set it would join already refused.
///
/// A pin needs no separate handling. [`assign`] writes `mirror_pinned` back from the grouping
/// and a link that is no longer in a group is no longer pinned, so the two decisions cannot
/// end up contradicting one another.
pub(crate) async fn store_dissolve(
    connection: &mut SqliteConnection,
    id: CandidateId,
) -> Result<(MirrorDissolve, Option<EventEnvelope>)> {
    let mut tx = connection.begin().await?;
    let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT package_id, mirror_group, mirror_source FROM link_candidates WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((Some(package), Some(group), source)) = row else {
        tx.rollback().await?;
        return Ok((MirrorDissolve::NotGrouped, None));
    };
    if source.as_deref() != Some(&crate::collector_store::enum_string(MirrorSource::Name)?) {
        tx.rollback().await?;
        return Ok((MirrorDissolve::NotProposed, None));
    }
    // Ordered by identifier so the smaller one is always the pair's left side, which is what
    // makes one pair one row whichever member the request named.
    let members: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM link_candidates WHERE package_id = ? AND mirror_group = ? ORDER BY id",
    )
    .bind(&package)
    .bind(&group)
    .fetch_all(&mut *tx)
    .await?;
    if members.len() < 2 {
        tx.rollback().await?;
        return Ok((MirrorDissolve::NotGrouped, None));
    }
    let now = chrono::Utc::now();
    for (position, left) in members.iter().enumerate() {
        for right in &members[position + 1..] {
            sqlx::query(
                "INSERT OR IGNORE INTO collector_mirror_separations (left_id, right_id, \
                 created_at) VALUES (?, ?, ?)",
            )
            .bind(left)
            .bind(right)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
    }
    let package = crate::parse_id(&package)?;
    assign(&mut tx, &[package]).await?;
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "mirror_dissolved": members.len(), "package_id": package }),
    );
    crate::collector_store::insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((MirrorDissolve::Dissolved, Some(event)))
}
