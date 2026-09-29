//! What a restore changes in the unpacked copy before it may become the installation
//! (RD-160-03): the storage roots it moves, every stored path below them, and the torrent
//! session's output folders.
//!
//! The plan is computed the same way for a test restore and a restore: the test restore applies
//! it to its throwaway copy and throws the copy away, the restore applies it to the copy it
//! stages. A path that would leave its root refuses the whole plan.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rd_db::restore_copy::{self, CopyColumn, CopyUpdate, PATH_COLUMNS, STORAGE_ROOT_PATH};

use super::RestoreError;
use super::paths::{ForeignPath, Remapped, RootMapping, is_native_absolute, remap};

/// How many example paths a finding lists; the count is always whole.
pub const EXAMPLES_KEPT: usize = 20;

/// One stored path a finding names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathExample {
    /// `table.column`, or `torrent-session/session.json` for the torrent session.
    pub location: String,
    pub value: String,
}

/// A kind of stored path: how many, and the first few.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PathFindings {
    pub count: usize,
    pub examples: Vec<PathExample>,
}

impl PathFindings {
    fn add(&mut self, location: impl Into<String>, value: &str) {
        self.count += 1;
        if self.examples.len() < EXAMPLES_KEPT {
            self.examples.push(PathExample {
                location: location.into(),
                value: value.to_owned(),
            });
        }
    }
}

/// A storage root as the copy names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopyRoot {
    pub id: String,
    pub path: String,
    /// Whether `path` is an absolute path on this system.
    pub native: bool,
    /// Where the plan moves it; `None` keeps it.
    pub mapped_to: Option<PathBuf>,
}

/// The changes a restore makes to the copy's paths.
#[derive(Clone, Debug, Default)]
pub struct PathPlan {
    pub roots: Vec<CopyRoot>,
    pub mappings: Vec<RootMapping>,
    pub updates: Vec<CopyUpdate>,
    /// Paths moved with their root.
    pub moved: usize,
    /// Paths that would leave their root: any of them refuses the restore.
    pub escapes: PathFindings,
    /// Absolute paths of another system that no mapping moves: they will not be found here.
    pub foreign: PathFindings,
}

/// A mapping as a request names it: a root of the copy by id, and its place on this system.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestedMapping {
    pub storage_root_id: String,
    pub path: String,
}

fn location(column: CopyColumn) -> String {
    format!("{}.{}", column.table, column.column)
}

/// The mappings a request asks for, checked against the copy's roots.
fn checked_mappings(
    roots: &BTreeMap<String, String>,
    requested: &[RequestedMapping],
) -> Result<Vec<RootMapping>, RestoreError> {
    let mut ids = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut mapped = Vec::with_capacity(requested.len());
    for request in requested {
        let from = roots.get(&request.storage_root_id).ok_or_else(|| {
            RestoreError::new(
                "backup.restore_mapping_unknown_root",
                format!("the backup has no storage root {}", request.storage_root_id),
            )
        })?;
        let mapping = RootMapping::new(request.storage_root_id.clone(), from, &request.path)
            .map_err(|problem| RestoreError::new(problem.code(), &request.path))?;
        if !ids.insert(mapping.root_id.clone()) || !targets.insert(mapping.to.clone()) {
            return Err(RestoreError::new(
                "backup.restore_mapping_duplicate",
                format!(
                    "storage root {} is mapped twice, or onto another root's place",
                    mapping.root_id
                ),
            ));
        }
        mapped.push(mapping);
    }
    Ok(mapped)
}

/// Checks a moved path against its root on disk, where the root already exists: the
/// symbolic-link rule of `rd_files::StorageRoot::resolve`, which every download goes through.
async fn resolves_inside(path: &Path, mappings: &[RootMapping]) -> bool {
    let Some(mapping) = mappings
        .iter()
        .filter(|mapping| path.starts_with(&mapping.to))
        .max_by_key(|mapping| mapping.to.components().count())
    else {
        return false;
    };
    let Ok(relative) = path.strip_prefix(&mapping.to) else {
        return false;
    };
    match rd_files::StorageRoot::open_existing(
        rd_core::StorageRootId::new(),
        mapping.root_id.clone(),
        mapping.to.clone(),
    )
    .await
    {
        Ok(Some(root)) => root.resolve(relative).is_ok(),
        // Not there yet: the lexical check in `remap` is all there is to check.
        Ok(None) => true,
        Err(_) => false,
    }
}

/// [`remap`], and a moved path that leaves its root on disk counts as escaping.
async fn checked_remap(value: &str, mappings: &[RootMapping]) -> Remapped {
    let remapped = remap(value, mappings);
    if let Remapped::Moved(path) = &remapped
        && !resolves_inside(path, mappings).await
    {
        return Remapped::Escapes;
    }
    remapped
}

/// Computes the path plan for a migrated copy.
///
/// # Errors
///
/// A mapping's own code when a mapping is refused; `backup.restore_failed` when the copy
/// cannot be read.
pub async fn plan_paths(
    copy: &Path,
    requested: &[RequestedMapping],
) -> Result<PathPlan, RestoreError> {
    let read =
        |error: anyhow::Error| RestoreError::new("backup.restore_failed", format!("{error:#}"));
    let mut columns = vec![STORAGE_ROOT_PATH];
    columns.extend_from_slice(PATH_COLUMNS);
    let cells = restore_copy::read_cells(copy, &columns)
        .await
        .map_err(read)?;
    let Some((root_cells, path_cells)) = cells.split_first() else {
        return Err(RestoreError::new(
            "backup.restore_failed",
            "the copy's columns were not read",
        ));
    };
    let stored: BTreeMap<String, String> = root_cells
        .iter()
        .map(|cell| (cell.key.clone(), cell.value.clone()))
        .collect();
    let mappings = checked_mappings(&stored, requested)?;
    let mut plan = PathPlan::default();

    for cell in root_cells {
        let mapped_to = mappings
            .iter()
            .find(|mapping| mapping.root_id == cell.key)
            .map(|mapping| mapping.to.clone());
        let native = is_native_absolute(&cell.value);
        if let Some(to) = &mapped_to {
            plan.updates.push(CopyUpdate {
                column: STORAGE_ROOT_PATH,
                key: cell.key.clone(),
                value: Some(to.to_string_lossy().into_owned()),
            });
        } else if !native {
            plan.foreign.add(location(STORAGE_ROOT_PATH), &cell.value);
        }
        plan.roots.push(CopyRoot {
            id: cell.key.clone(),
            path: cell.value.clone(),
            native,
            mapped_to,
        });
    }

    for (column, cells) in PATH_COLUMNS.iter().zip(path_cells) {
        for cell in cells {
            match checked_remap(&cell.value, &mappings).await {
                Remapped::Moved(path) => {
                    plan.moved += 1;
                    plan.updates.push(CopyUpdate {
                        column: *column,
                        key: cell.key.clone(),
                        value: Some(path.to_string_lossy().into_owned()),
                    });
                }
                Remapped::Escapes => {
                    plan.escapes.add(location(*column), &cell.value);
                }
                Remapped::Unmapped => {
                    if ForeignPath::parse(&cell.value).is_some() && !is_native_absolute(&cell.value)
                    {
                        plan.foreign.add(location(*column), &cell.value);
                    }
                }
            }
        }
    }
    plan.mappings = mappings;
    Ok(plan)
}

/// The torrent session file below the unpacked `torrent-session` folder.
pub const SESSION_FILE: &str = "session.json";

/// What the torrent session holds, and what a remap made of it.
#[derive(Clone, Debug, Default)]
pub struct SessionReport {
    pub torrents: usize,
    /// Info hashes whose `.torrent` is not in the session folder.
    pub missing_files: Vec<String>,
    pub moved: usize,
    pub escapes: PathFindings,
    pub foreign: PathFindings,
}

/// Reads the torrent session in `folder`, moves its output folders by `mappings` and, unless
/// `dry_run`, writes it back by rename. A folder without a session is an empty report.
///
/// # Errors
///
/// `backup.restore_session_invalid` when the session file is no JSON object of torrents.
pub async fn rewrite_session(
    folder: &Path,
    mappings: &[RootMapping],
    dry_run: bool,
) -> Result<SessionReport, RestoreError> {
    let invalid = |error: &dyn std::fmt::Display| {
        RestoreError::new("backup.restore_session_invalid", error.to_string())
    };
    let file = folder.join(SESSION_FILE);
    let bytes = match tokio::fs::read(&file).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SessionReport::default());
        }
        Err(error) => return Err(invalid(&error)),
    };
    let mut document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| invalid(&error))?;
    let torrents = document
        .get_mut("torrents")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| invalid(&"the session names no torrents"))?;
    let mut report = SessionReport {
        torrents: torrents.len(),
        ..SessionReport::default()
    };
    let location = format!("torrent-session/{SESSION_FILE}");
    for torrent in torrents.values_mut() {
        if let Some(hash) = torrent.get("info_hash").and_then(serde_json::Value::as_str) {
            let safe = hash.chars().all(|character| character.is_ascii_hexdigit());
            if !safe || !folder.join(format!("{hash}.torrent")).is_file() {
                report.missing_files.push(hash.to_owned());
            }
        }
        let Some(output) = torrent
            .get("output_folder")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        match checked_remap(&output, mappings).await {
            Remapped::Moved(path) => {
                report.moved += 1;
                torrent["output_folder"] =
                    serde_json::Value::String(path.to_string_lossy().into_owned());
            }
            Remapped::Escapes => report.escapes.add(&location, &output),
            Remapped::Unmapped => {
                if ForeignPath::parse(&output).is_some() && !is_native_absolute(&output) {
                    report.foreign.add(&location, &output);
                }
            }
        }
    }
    if !dry_run && report.moved > 0 && report.escapes.count == 0 {
        let temporary = folder.join(format!("{SESSION_FILE}.restore"));
        let encoded = serde_json::to_vec(&document).map_err(|error| invalid(&error))?;
        tokio::fs::write(&temporary, encoded)
            .await
            .map_err(|error| invalid(&error))?;
        tokio::fs::rename(&temporary, &file)
            .await
            .map_err(|error| invalid(&error))?;
    }
    Ok(report)
}
