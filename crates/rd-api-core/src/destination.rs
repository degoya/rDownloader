//! Category → storage-root destination resolution shared by handlers and services.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::CategoryId;
use rd_db::Database;

use crate::{ApiError, AppState};

/// Resolves the destination directory for a category (or the default category).
///
/// Without any applicable category the default storage root is used — a package with no
/// category still belongs into configured storage, not into the service's own download
/// folder. Returns `None` only when no storage root is configured at all.
pub async fn resolve_destination(
    database: &Database,
    selected: Option<CategoryId>,
) -> Result<Option<PathBuf>> {
    let categories = database.list_categories().await?;
    let category = selected
        .and_then(|id| categories.iter().find(|category| category.id == id))
        .or_else(|| categories.iter().find(|category| category.is_default));
    let Some(category) = category else {
        let Some(fallback) = database.default_storage_root().await? else {
            return Ok(None);
        };
        let allowlist =
            rd_files::StorageRoot::create(fallback.id, fallback.name, PathBuf::from(fallback.path))
                .await?;
        return Ok(Some(allowlist.path().to_path_buf()));
    };
    let root = database
        .list_storage_roots()
        .await?
        .into_iter()
        .find(|root| root.id == category.storage_root_id)
        .ok_or(Unresolvable::RootMissing)?;
    let allowlist =
        rd_files::StorageRoot::create(root.id, root.name, PathBuf::from(root.path)).await?;
    let destination = allowlist
        .resolve(Path::new(&category.relative_path))
        .map_err(|error| {
            // A folder that cannot be read is the filesystem's failure; everything else the
            // allowlist refuses is the category's folder pointing out of its root.
            if error.chain().any(|cause| cause.is::<std::io::Error>()) {
                error
            } else {
                error.context(Unresolvable::OutsideRoot)
            }
        })?;
    Ok(Some(destination))
}

/// The two ways a category's destination fails that the person fixes in its settings, as
/// opposed to a store or filesystem failure (audit 1.9.1, RA-API-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unresolvable {
    /// The category names a storage root that is no longer configured.
    RootMissing,
    /// The category's folder leaves its storage root (`..`, absolute, through a symlink).
    OutsideRoot,
}

impl std::fmt::Display for Unresolvable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::RootMissing => "Category references a storage root that does not exist",
            Self::OutsideRoot => "Category folder lies outside its storage root",
        })
    }
}

impl std::error::Error for Unresolvable {}

pub async fn download_destination(
    state: &AppState,
    selected: Option<rd_core::CategoryId>,
) -> Result<Option<PathBuf>, ApiError> {
    resolve_destination(&state.database, selected)
        .await
        .map_err(destination_unresolved)
}

/// Where new work for a category goes -- its destination, or the service's download folder
/// when no storage root is configured -- refused while that root is below its free-space
/// threshold (`storage.capacity_blocked`).
///
/// The one "category → target → may it take more" step of every path that puts new work into
/// the queue (audit 1.9.1, API-08). The LinkGrabber, torrents and watched folders each had
/// the free-space stop; the NZB enqueue over REST and MCP and SABnzbd's `addfile` had the
/// destination without it, so a full disk stopped every intake except those three. Takes the
/// components rather than `AppState` because a watched folder enqueues without one.
///
/// # Errors
///
/// `400 category.destination_unresolved`, `409 storage.capacity_blocked`, or `500` when the
/// store or the filesystem fails.
pub async fn intake_destination(
    database: &Database,
    scheduler: &rd_scheduler::SchedulerHandle,
    selected: Option<CategoryId>,
) -> Result<PathBuf, ApiError> {
    intake_target(database, scheduler, selected)
        .await
        .map_err(|failure| match failure {
            IntakeTargetError::Refused(refusal) => refusal,
            IntakeTargetError::Failed(error) => error.into(),
        })
}

/// Why [`intake_target`] has no destination.
#[derive(Debug)]
pub enum IntakeTargetError {
    /// A refusal the person can act on: the category's destination or the free-space stop.
    Refused(ApiError),
    /// The store or the filesystem failed; the cause is kept whole for the caller's log.
    Failed(anyhow::Error),
}

/// [`intake_destination`] for a caller without a client to answer: a watched folder keeps a
/// store failure's cause for its own failure record instead of an `internal.error` whose text
/// says nothing (audit 1.9.1, RA-API-01).
///
/// # Errors
///
/// [`IntakeTargetError::Refused`] or [`IntakeTargetError::Failed`].
pub async fn intake_target(
    database: &Database,
    scheduler: &rd_scheduler::SchedulerHandle,
    selected: Option<CategoryId>,
) -> Result<PathBuf, IntakeTargetError> {
    let destination = match resolve_destination(database, selected).await {
        Ok(destination) => {
            destination.unwrap_or_else(|| scheduler.downloads_directory().to_path_buf())
        }
        Err(error) if error.downcast_ref::<Unresolvable>().is_some() => {
            return Err(IntakeTargetError::Refused(destination_unresolved(error)));
        }
        Err(error) => return Err(IntakeTargetError::Failed(error)),
    };
    crate::storage_capacity::ensure_intake_allowed(&scheduler.capacity(), &destination)
        .await
        .map_err(IntakeTargetError::Refused)?;
    Ok(destination)
}

/// `400 category.destination_unresolved` for what the category's settings fix; a store or
/// filesystem failure is a `500` whose cause goes to the log, redacted, and never to the client
/// -- the text named tables and storage paths, and SABnzbd handed it on to an *arr
/// (audit 1.9.1, RA-API-01).
fn destination_unresolved(error: anyhow::Error) -> ApiError {
    if error.downcast_ref::<Unresolvable>().is_some() {
        ApiError::bad_request(
            "category.destination_unresolved",
            "Download destination could not be resolved: the category's storage root is \
             missing or its folder lies outside it",
        )
    } else {
        error.into()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use rd_core::StorageRootId;
    use rd_db::{Database, NewCategory, NewStorageRoot};

    use super::{Unresolvable, destination_unresolved, resolve_destination};

    async fn database(directory: &Path) -> Database {
        Database::open(directory.join("destination-test.sqlite3"))
            .await
            .expect("database")
    }

    fn new_root(name: &str, path: &Path, is_default: bool) -> NewStorageRoot {
        NewStorageRoot {
            name: name.to_owned(),
            path: path.to_string_lossy().into_owned(),
            is_default,
            minimum_free_bytes: None,
        }
    }

    #[tokio::test]
    async fn falls_back_to_the_default_storage_root_without_any_category() {
        let temporary = tempfile::tempdir().expect("tempdir");
        // The resolver hands back canonical paths, and on macOS the temp dir is under
        // a symlink (`/var` -> `/private/var`), so the expectation is built from the same --
        // through `dunce`, as the resolver does, so Windows has no `\\?\` prefix on it.
        let base = dunce::canonicalize(temporary.path()).expect("canonical tempdir");
        let database = database(temporary.path()).await;
        let alphabetically_first = base.join("archive");
        let marked_default = base.join("media");
        database
            .create_storage_root(
                StorageRootId::new(),
                new_root("Archive", &alphabetically_first, false),
            )
            .await
            .expect("first root");
        database
            .create_storage_root(
                StorageRootId::new(),
                new_root("Media", &marked_default, true),
            )
            .await
            .expect("default root");

        let destination = resolve_destination(&database, None)
            .await
            .expect("destination")
            .expect("a root is configured");
        assert_eq!(destination, marked_default);
    }

    #[tokio::test]
    async fn falls_back_to_the_first_root_which_the_invariant_made_the_default() {
        let temporary = tempfile::tempdir().expect("tempdir");
        // The resolver hands back canonical paths, and on macOS the temp dir is under
        // a symlink (`/var` -> `/private/var`), so the expectation is built from the same --
        // through `dunce`, as the resolver does, so Windows has no `\\?\` prefix on it.
        let base = dunce::canonicalize(temporary.path()).expect("canonical tempdir");
        let database = database(temporary.path()).await;
        let only = base.join("storage");
        database
            .create_storage_root(StorageRootId::new(), new_root("Storage", &only, false))
            .await
            .expect("root");

        let destination = resolve_destination(&database, None)
            .await
            .expect("destination")
            .expect("a root is configured");
        assert_eq!(destination, only);
    }

    #[tokio::test]
    async fn without_any_storage_root_the_caller_decides() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let database = database(temporary.path()).await;
        assert!(
            resolve_destination(&database, None)
                .await
                .expect("destination")
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_default_category_still_wins_over_the_bare_root() {
        let temporary = tempfile::tempdir().expect("tempdir");
        // The resolver hands back canonical paths, and on macOS the temp dir is under
        // a symlink (`/var` -> `/private/var`), so the expectation is built from the same --
        // through `dunce`, as the resolver does, so Windows has no `\\?\` prefix on it.
        let base = dunce::canonicalize(temporary.path()).expect("canonical tempdir");
        let database = database(temporary.path()).await;
        let root_path = base.join("storage");
        let root = database
            .create_storage_root(StorageRootId::new(), new_root("Storage", &root_path, true))
            .await
            .expect("root");
        database
            .create_category(NewCategory {
                name: "Downloads".to_owned(),
                color: "#4F46E5".to_owned(),
                storage_root_id: root.id,
                relative_path: "downloads".to_owned(),
                is_default: true,
                postprocess_level: None,
                script: None,
                cleanup_extensions: None,
                recursive_unpack: None,
                unpack_to_subfolder: None,
                unwrap_package_folder: None,
                direct_unpack: None,
                malware_scan: None,
                sfv_verify: None,
                safe_postproc: None,
                delete_par2: None,
                upload_enabled: None,
                upload_remote: None,
            })
            .await
            .expect("category");

        let destination = resolve_destination(&database, None)
            .await
            .expect("destination")
            .expect("a category applies");
        assert_eq!(destination, PathBuf::from(&root_path).join("downloads"));
    }

    fn default_category(root: StorageRootId, relative_path: &str) -> NewCategory {
        NewCategory {
            name: "Downloads".to_owned(),
            color: "#4F46E5".to_owned(),
            storage_root_id: root,
            relative_path: relative_path.to_owned(),
            is_default: true,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            unpack_to_subfolder: None,
            unwrap_package_folder: None,
            direct_unpack: None,
            malware_scan: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        }
    }

    #[tokio::test]
    async fn a_folder_outside_its_root_is_the_categorys_400_without_the_path() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let database = database(temporary.path()).await;
        let root_path = temporary.path().join("storage");
        let root = database
            .create_storage_root(StorageRootId::new(), new_root("Storage", &root_path, true))
            .await
            .expect("root");
        database
            .create_category(default_category(root.id, "../escape"))
            .await
            .expect("category");

        let failure = resolve_destination(&database, None)
            .await
            .expect_err("a folder outside its root is refused");
        let refusal = destination_unresolved(failure);
        assert_eq!(refusal.code(), "category.destination_unresolved");
        let root_text = root_path.to_string_lossy();
        assert!(
            !refusal.message().contains(root_text.as_ref()),
            "the answer names no storage path: {}",
            refusal.message()
        );
    }

    #[tokio::test]
    async fn a_root_the_filesystem_refuses_is_a_500_without_the_cause() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let database = database(temporary.path()).await;
        // A file where the root's directory should be: creating the root fails with an I/O
        // error, which is no setting the category could fix.
        let occupied = temporary.path().join("occupied");
        std::fs::write(&occupied, b"not a directory").expect("file");
        database
            .create_storage_root(StorageRootId::new(), new_root("Storage", &occupied, true))
            .await
            .expect("root");

        let failure = resolve_destination(&database, None)
            .await
            .expect_err("a root that cannot be created fails");
        let refusal = destination_unresolved(failure);
        assert_eq!(refusal.code(), crate::error_codes::INTERNAL_ERROR);
        let occupied_text = occupied.to_string_lossy();
        assert!(!refusal.message().contains(occupied_text.as_ref()));
    }

    #[test]
    fn a_missing_root_is_the_categorys_and_a_store_failure_is_internal() {
        let missing = destination_unresolved(anyhow::Error::new(Unresolvable::RootMissing));
        assert_eq!(missing.code(), "category.destination_unresolved");

        let store = destination_unresolved(anyhow::anyhow!(
            "error returned from database: no such table: categories"
        ));
        assert_eq!(store.code(), crate::error_codes::INTERNAL_ERROR);
        assert!(!store.message().contains("categories"));
    }
}
