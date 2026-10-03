//! The checks a test restore and a restore run on the unpacked copy (RD-160-03): credentials
//! against the sealed settings bundle, references the secret store here cannot open, the
//! stored `.torrent` files, dangling foreign keys and the folders of unfinished transfers.
//!
//! A finding names rows and paths, never a credential: the examples are `table.column id` and
//! folder names.

use std::collections::BTreeMap;
use std::path::Path;

use rd_backup::BackupKey;
use rd_backup::manifest::TORRENT_FILES_PREFIX;
use rd_backup::restore::plan::PathFindings;
use rd_db::restore_copy::{
    self, BACKUP_KEY_REF, BUNDLED_SECRET_COLUMNS, CopyUpdate, DOWNLOAD_SOURCE,
    UNBUNDLED_SECRET_COLUMNS,
};

use crate::restore_dto::RestoreProblemResponse;
use crate::settings_backup_crypto::decrypt_secrets;
use crate::settings_backup_dto::SettingsBundle;
use crate::{ApiError, AppState};

/// How many examples a finding lists; the count is always whole.
const EXAMPLES: usize = 20;

/// What the checks found, merged by code.
#[derive(Default)]
pub(crate) struct Findings {
    problems: Vec<RestoreProblemResponse>,
    /// Credentials the settings bundle carries a value for.
    pub(crate) restored_credentials: usize,
}

impl Findings {
    fn push(&mut self, severity: &str, code: &str, count: usize, examples: Vec<String>) {
        if count == 0 {
            return;
        }
        if let Some(problem) = self
            .problems
            .iter_mut()
            .find(|problem| problem.code == code && problem.severity == severity)
        {
            problem.count += count;
            let room = EXAMPLES.saturating_sub(problem.examples.len());
            problem.examples.extend(examples.into_iter().take(room));
            return;
        }
        self.problems.push(RestoreProblemResponse {
            severity: severity.to_owned(),
            code: code.to_owned(),
            count,
            examples: examples.into_iter().take(EXAMPLES).collect(),
        });
    }

    pub(crate) fn error(&mut self, code: &str, count: usize, examples: Vec<String>) {
        self.push("error", code, count, examples);
    }

    pub(crate) fn warn(&mut self, code: &str, count: usize, examples: Vec<String>) {
        self.push("warning", code, count, examples);
    }

    fn examples(paths: &PathFindings) -> Vec<String> {
        paths
            .examples
            .iter()
            .map(|example| format!("{}: {}", example.location, example.value))
            .collect()
    }

    pub(crate) fn error_paths(&mut self, code: &str, paths: &PathFindings) {
        self.error(code, paths.count, Self::examples(paths));
    }

    pub(crate) fn warn_paths(&mut self, code: &str, paths: &PathFindings) {
        self.warn(code, paths.count, Self::examples(paths));
    }

    pub(crate) fn has_errors(&self) -> bool {
        self.problems
            .iter()
            .any(|problem| problem.severity == "error")
    }

    /// Errors first, then warnings, each by code.
    pub(crate) fn into_problems(mut self) -> Vec<RestoreProblemResponse> {
        self.problems.sort_by(|left, right| {
            (left.severity != "error", &left.code).cmp(&(right.severity != "error", &right.code))
        });
        self.problems
    }
}

/// What the credential check reads.
pub(crate) struct Credentials<'a> {
    pub(crate) copy: &'a Path,
    pub(crate) bundle: &'a SettingsBundle,
    pub(crate) passphrase: &'a str,
    /// The archive's key, derived from the passphrase: the restored backup schedule keeps it
    /// when the copy's schedule was sealing under it.
    pub(crate) key: &'a BackupKey,
    /// Put the credentials into the secret store (a restore) or only count them (a test).
    pub(crate) mint: bool,
}

/// The bundle's slot per row id, in the order of `BUNDLED_SECRET_COLUMNS`.
fn bundle_slots(bundle: &SettingsBundle) -> Vec<BTreeMap<String, String>> {
    fn pairs(items: impl Iterator<Item = (String, Option<String>)>) -> BTreeMap<String, String> {
        items
            .filter_map(|(id, slot)| slot.map(|slot| (id, slot)))
            .collect()
    }
    let mut slots = vec![
        pairs(
            bundle
                .accounts
                .iter()
                .map(|value| (value.id.to_string(), value.secret_slot.clone())),
        ),
        pairs(
            bundle
                .accounts
                .iter()
                .map(|value| (value.id.to_string(), value.cookies_slot.clone())),
        ),
        pairs(
            bundle
                .proxy_profiles
                .iter()
                .map(|value| (value.id.to_string(), value.secret_slot.clone())),
        ),
        pairs(
            bundle
                .usenet_servers
                .iter()
                .map(|value| (value.id.to_string(), value.password_slot.clone())),
        ),
        pairs(
            bundle
                .subscriptions
                .iter()
                .map(|value| (value.id.to_string(), value.secret_slot.clone())),
        ),
        pairs(
            bundle
                .auth_profiles
                .iter()
                .map(|value| (value.id.to_string(), value.secret_slot.clone())),
        ),
        pairs(
            bundle
                .auth_profiles
                .iter()
                .map(|value| (value.id.to_string(), value.certificate_slot.clone())),
        ),
        pairs(
            bundle
                .indexers
                .iter()
                .map(|value| (value.id.to_string(), value.secret_slot.clone())),
        ),
    ];
    // RD-190-04: the archive passwords, in the order of their four columns at the end.
    for table in BUNDLED_SECRET_COLUMNS[slots.len()..]
        .iter()
        .map(|column| column.table)
    {
        slots.push(pairs(
            bundle
                .archive_passwords
                .iter()
                .filter(|value| value.table == table)
                .map(|value| (value.id.clone(), Some(value.slot.clone()))),
        ));
    }
    debug_assert_eq!(slots.len(), BUNDLED_SECRET_COLUMNS.len());
    slots
}

/// Matches every credential reference of the copy with the bundle. A restore puts each value
/// into the secret store and points the copy at it; a reference the bundle has no value for
/// keeps its old value only where the store here still opens it (a restore on the same
/// machine), and is cleared otherwise — the account then asks for its credential again.
pub(crate) async fn credentials(
    state: &AppState,
    input: Credentials<'_>,
    updates: &mut Vec<CopyUpdate>,
    minted: &mut Vec<String>,
    findings: &mut Findings,
) -> Result<(), ApiError> {
    let values = match &input.bundle.secrets {
        Some(encrypted) => decrypt_secrets(input.passphrase, encrypted).await?,
        None => BTreeMap::new(),
    };
    let slots = bundle_slots(input.bundle);
    let cells = restore_copy::read_cells(input.copy, BUNDLED_SECRET_COLUMNS).await?;
    let mut missing = Vec::new();
    let mut missing_count = 0;
    for ((column, cells), slots) in BUNDLED_SECRET_COLUMNS.iter().zip(cells).zip(slots) {
        for cell in cells {
            let value = slots
                .get(&cell.key)
                .and_then(|slot| values.get(slot))
                .filter(|value| !value.is_empty());
            if let Some(value) = value {
                findings.restored_credentials += 1;
                if input.mint {
                    let reference = state.secrets.put_string(value.clone()).await?;
                    minted.push(reference.clone());
                    updates.push(CopyUpdate {
                        column: *column,
                        key: cell.key,
                        value: Some(reference),
                    });
                }
                continue;
            }
            if state.secrets.get(&cell.value).await.is_ok() {
                continue;
            }
            missing_count += 1;
            missing.push(format!("{}.{} {}", column.table, column.column, cell.key));
            if input.mint {
                updates.push(CopyUpdate {
                    column: *column,
                    key: cell.key,
                    value: None,
                });
            }
        }
    }
    findings.warn("backup.restore_secret_missing", missing_count, missing);

    // What the bundle never carries: named, left as it is.
    let cells = restore_copy::read_cells(input.copy, UNBUNDLED_SECRET_COLUMNS).await?;
    let (mut unavailable, mut second_factor) = (Vec::new(), 0);
    let mut unavailable_count = 0;
    for (column, cells) in UNBUNDLED_SECRET_COLUMNS.iter().zip(cells) {
        let mut count = 0;
        for cell in cells {
            if state.secrets.get(&cell.value).await.is_err() {
                count += 1;
            }
        }
        if column.table == "mfa_credentials" {
            second_factor += count;
        } else if count > 0 {
            unavailable_count += count;
            unavailable.push(format!("{}.{} ({count})", column.table, column.column));
        }
    }
    findings.warn(
        "backup.restore_secret_unavailable",
        unavailable_count,
        unavailable,
    );
    findings.warn("backup.restore_mfa_unavailable", second_factor, Vec::new());

    // The backup schedule's key: put back when the copy was sealing under this archive's key.
    match restore_copy::backup_key_of(input.copy).await? {
        Some((fingerprint, _)) if fingerprint == input.key.fingerprint() => {
            if input.mint {
                let reference = state.secrets.put_bytes(input.key.key_bytes()).await?;
                minted.push(reference.clone());
                updates.push(CopyUpdate {
                    column: BACKUP_KEY_REF,
                    key: "1".to_owned(),
                    value: Some(reference),
                });
            }
        }
        Some(_) => {
            if input.mint {
                restore_copy::clear_backup_key(input.copy).await?;
            }
            findings.warn("backup.restore_backup_key_cleared", 1, Vec::new());
        }
        None => {}
    }
    Ok(())
}

/// Points every queued torrent whose `.torrent` came with the archive at the file's place in
/// this installation's torrent folder; names the ones that did not come and are not there.
pub(crate) async fn torrent_sources(
    state: &AppState,
    copy: &Path,
    members: &[String],
    updates: &mut Vec<CopyUpdate>,
    findings: &mut Findings,
) -> Result<(), ApiError> {
    let cells = restore_copy::read_cells(copy, &[DOWNLOAD_SOURCE]).await?;
    let directory = state.torrent.torrent_file_directory();
    let directory = std::path::absolute(&directory).unwrap_or(directory);
    let (mut missing, mut count) = (Vec::new(), 0);
    for cell in cells.into_iter().flatten() {
        let Ok(url) = url::Url::parse(&cell.value) else {
            continue;
        };
        if url.scheme() != "file" {
            continue;
        }
        let segments: Vec<&str> = url
            .path_segments()
            .map(|parts| parts.collect())
            .unwrap_or_default();
        let [.., folder, name] = segments.as_slice() else {
            continue;
        };
        if *folder != TORRENT_FILES_PREFIX {
            continue;
        }
        if members.iter().any(|member| member.as_str() == *name) {
            if let Ok(target) = url::Url::from_file_path(directory.join(name)) {
                updates.push(CopyUpdate {
                    column: DOWNLOAD_SOURCE,
                    key: cell.key,
                    value: Some(target.to_string()),
                });
            }
        } else if !url.to_file_path().is_ok_and(|path| path.is_file()) {
            count += 1;
            missing.push((*name).to_owned());
        }
    }
    findings.warn("backup.restore_torrent_file_missing", count, missing);
    Ok(())
}

/// Foreign keys of the copy that name no row: categories without their storage root, and
/// every other reference the schema declares.
pub(crate) async fn references(copy: &Path, findings: &mut Findings) -> Result<(), ApiError> {
    let dangling = restore_copy::dangling_references(copy).await?;
    let count = dangling
        .iter()
        .map(|entry| usize::try_from(entry.rows).unwrap_or(usize::MAX))
        .sum();
    findings.error(
        "backup.restore_reference_dangling",
        count,
        dangling
            .iter()
            .map(|entry| format!("{} -> {} ({})", entry.table, entry.parent, entry.rows))
            .collect(),
    );
    Ok(())
}

/// Unfinished transfers whose folder is not on this machine after the remap: they cannot
/// resume from their part files and start again.
pub(crate) async fn partial_transfers(
    copy: &Path,
    findings: &mut Findings,
) -> Result<(), ApiError> {
    let (mut missing, mut count) = (Vec::new(), 0);
    for folder in restore_copy::unfinished_destinations(copy).await? {
        if !tokio::fs::metadata(&folder)
            .await
            .is_ok_and(|metadata| metadata.is_dir())
        {
            count += 1;
            missing.push(folder);
        }
    }
    findings.warn("backup.restore_partial_missing", count, missing);
    Ok(())
}

/// Takes what a restore put into the secret store out again: the restore did not happen.
pub(crate) async fn forget_minted(state: &AppState, minted: &[String]) {
    for reference in minted {
        if let Err(error) = state.secrets.remove(reference).await {
            tracing::warn!(%error, "a restore's credential could not be removed again");
        }
    }
}
