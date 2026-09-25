//! Category → storage-root destination resolution shared by handlers and services.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rd_core::CategoryId;
use rd_db::Database;

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
        .context("Category references a storage root that does not exist")?;
    let allowlist =
        rd_files::StorageRoot::create(root.id, root.name, PathBuf::from(root.path)).await?;
    Ok(Some(allowlist.resolve(Path::new(&category.relative_path))?))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use rd_core::StorageRootId;
    use rd_db::{Database, NewCategory, NewStorageRoot};

    use super::resolve_destination;

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
        // a symlink (`/var` -> `/private/var`), so the expectation is built from the same.
        let base = temporary.path().canonicalize().expect("canonical tempdir");
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
        // a symlink (`/var` -> `/private/var`), so the expectation is built from the same.
        let base = temporary.path().canonicalize().expect("canonical tempdir");
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
        // a symlink (`/var` -> `/private/var`), so the expectation is built from the same.
        let base = temporary.path().canonicalize().expect("canonical tempdir");
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
}
