//! Exactly one category is the default, at all times.
//!
//! The invariant `0052` gave storage roots, for the table routing falls back to: without it the
//! first category created without ticking the switch — or deleting the default — left
//! `SELECT id FROM categories WHERE is_default = 1` with nothing, and every link no rule
//! matched arrived with no category at all.

use rd_core::StorageRootId;
use rd_db::{Database, NewCategory, NewStorageRoot};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("categories.sqlite"))
        .await
        .expect("database")
}

/// The storage root every category needs; its own default flag is not what is under test.
async fn root(database: &Database) -> StorageRootId {
    database
        .create_storage_root(
            StorageRootId::new(),
            NewStorageRoot {
                name: "Downloads".to_owned(),
                path: "/downloads".to_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("storage root")
        .id
}

fn category(name: &str, root_id: StorageRootId, is_default: bool) -> NewCategory {
    NewCategory {
        name: name.to_owned(),
        color: "#38BDF8".to_owned(),
        storage_root_id: root_id,
        relative_path: name.to_lowercase(),
        is_default,
        postprocess_level: None,
        script: None,
        cleanup_extensions: None,
        recursive_unpack: None,
        sfv_verify: None,
        safe_postproc: None,
        delete_par2: None,
        upload_enabled: None,
        upload_remote: None,
    }
}

/// Names of the categories currently marked default, in list order.
async fn defaults(database: &Database) -> Vec<String> {
    database
        .list_categories()
        .await
        .expect("categories")
        .into_iter()
        .filter(|category| category.is_default)
        .map(|category| category.name)
        .collect()
}

#[tokio::test]
async fn first_category_becomes_default_even_when_not_requested() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let root_id = root(&database).await;

    let created = database
        .create_category(category("Movies", root_id, false))
        .await
        .expect("create");

    assert!(
        created.is_default,
        "the returned category must already say it is the default"
    );
    assert_eq!(defaults(&database).await, vec!["Movies".to_owned()]);
}

#[tokio::test]
async fn a_second_category_does_not_steal_the_default() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let root_id = root(&database).await;
    database
        .create_category(category("Movies", root_id, false))
        .await
        .expect("first");

    let second = database
        .create_category(category("Series", root_id, false))
        .await
        .expect("second");

    assert!(!second.is_default, "only the first category is promoted");
    assert_eq!(defaults(&database).await, vec!["Movies".to_owned()]);
}

#[tokio::test]
async fn a_second_category_may_take_the_default_when_asked() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let root_id = root(&database).await;
    database
        .create_category(category("Movies", root_id, false))
        .await
        .expect("first");

    database
        .create_category(category("Series", root_id, true))
        .await
        .expect("second");

    assert_eq!(defaults(&database).await, vec!["Series".to_owned()]);
}

#[tokio::test]
async fn unticking_the_only_default_is_coerced_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let root_id = root(&database).await;
    let only = database
        .create_category(category("Movies", root_id, false))
        .await
        .expect("create");

    let updated = database
        .update_category(only.id, category("Movies", root_id, false))
        .await
        .expect("update");

    assert!(
        updated.is_default,
        "the last default cannot be given up; the response must say so"
    );
    assert_eq!(defaults(&database).await, vec!["Movies".to_owned()]);
}

#[tokio::test]
async fn unticking_the_default_is_coerced_back_even_with_other_categories() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let root_id = root(&database).await;
    let first = database
        .create_category(category("Movies", root_id, false))
        .await
        .expect("first");
    database
        .create_category(category("Series", root_id, false))
        .await
        .expect("second");

    let updated = database
        .update_category(first.id, category("Movies", root_id, false))
        .await
        .expect("update");

    assert!(
        updated.is_default,
        "another category existing does not help"
    );
    assert_eq!(defaults(&database).await, vec!["Movies".to_owned()]);
}

#[tokio::test]
async fn editing_a_non_default_category_leaves_the_default_alone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let root_id = root(&database).await;
    database
        .create_category(category("Movies", root_id, false))
        .await
        .expect("first");
    let second = database
        .create_category(category("Series", root_id, false))
        .await
        .expect("second");

    let updated = database
        .update_category(second.id, category("Shows", root_id, false))
        .await
        .expect("update");

    assert!(!updated.is_default);
    assert_eq!(defaults(&database).await, vec!["Movies".to_owned()]);
}

#[tokio::test]
async fn deleting_the_default_promotes_the_alphabetically_first_survivor() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let root_id = root(&database).await;
    let bravo = database
        .create_category(category("Bravo", root_id, false))
        .await
        .expect("bravo");
    database
        .create_category(category("Charlie", root_id, false))
        .await
        .expect("charlie");
    database
        .create_category(category("Alpha", root_id, false))
        .await
        .expect("alpha");
    assert_eq!(defaults(&database).await, vec!["Bravo".to_owned()]);

    database.delete_category(bravo.id).await.expect("delete");

    assert_eq!(defaults(&database).await, vec!["Alpha".to_owned()]);
}

#[tokio::test]
async fn deleting_a_non_default_category_does_not_move_the_default() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let root_id = root(&database).await;
    database
        .create_category(category("Bravo", root_id, false))
        .await
        .expect("bravo");
    let alpha = database
        .create_category(category("Alpha", root_id, false))
        .await
        .expect("alpha");

    database.delete_category(alpha.id).await.expect("delete");

    assert_eq!(defaults(&database).await, vec!["Bravo".to_owned()]);
}

/// Deleting the last category is allowed and leaves nothing to promote — the invariant is
/// "exactly one while the table is non-empty", not "a category can never be removed".
#[tokio::test]
async fn deleting_the_last_category_leaves_an_empty_table() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let root_id = root(&database).await;
    let only = database
        .create_category(category("Movies", root_id, false))
        .await
        .expect("create");

    database.delete_category(only.id).await.expect("delete");

    assert!(
        database
            .list_categories()
            .await
            .expect("categories")
            .is_empty()
    );
}
