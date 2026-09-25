//! Exactly one storage root is the default, at all times.
//!
//! Without the invariant a fresh install could end up with no default at all — the first root
//! created without ticking the switch — and destination resolution then fell back to whichever
//! root happened to sort first alphabetically.

use rd_core::StorageRootId;
use rd_db::{Database, NewStorageRoot};
use tempfile::TempDir;

fn root(name: &str, path: &str, is_default: bool) -> NewStorageRoot {
    NewStorageRoot {
        name: name.to_owned(),
        path: path.to_owned(),
        is_default,
        minimum_free_bytes: None,
    }
}

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("roots.sqlite"))
        .await
        .expect("database")
}

/// Names of the roots currently marked default, in list order.
async fn defaults(database: &Database) -> Vec<String> {
    database
        .list_storage_roots()
        .await
        .expect("roots")
        .into_iter()
        .filter(|root| root.is_default)
        .map(|root| root.name)
        .collect()
}

#[tokio::test]
async fn first_root_becomes_default_even_when_not_requested() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;

    let created = database
        .create_storage_root(StorageRootId::new(), root("Downloads", "/downloads", false))
        .await
        .expect("create");

    assert!(
        created.is_default,
        "the returned root must already say it is the default"
    );
    assert_eq!(defaults(&database).await, vec!["Downloads".to_owned()]);
}

#[tokio::test]
async fn a_second_root_does_not_steal_the_default() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    database
        .create_storage_root(StorageRootId::new(), root("Downloads", "/downloads", false))
        .await
        .expect("first");

    let second = database
        .create_storage_root(StorageRootId::new(), root("Movies", "/movies", false))
        .await
        .expect("second");

    assert!(!second.is_default, "only the first root is promoted");
    assert_eq!(defaults(&database).await, vec!["Downloads".to_owned()]);
}

#[tokio::test]
async fn a_second_root_may_take_the_default_when_asked() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    database
        .create_storage_root(StorageRootId::new(), root("Downloads", "/downloads", false))
        .await
        .expect("first");

    database
        .create_storage_root(StorageRootId::new(), root("Movies", "/movies", true))
        .await
        .expect("second");

    assert_eq!(defaults(&database).await, vec!["Movies".to_owned()]);
}

#[tokio::test]
async fn unticking_the_only_default_is_coerced_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let only = database
        .create_storage_root(StorageRootId::new(), root("Downloads", "/downloads", false))
        .await
        .expect("create");

    let updated = database
        .update_storage_root(only.id, root("Downloads", "/downloads", false))
        .await
        .expect("update");

    assert!(
        updated.is_default,
        "the last default cannot be given up; the response must say so"
    );
    assert_eq!(defaults(&database).await, vec!["Downloads".to_owned()]);
}

#[tokio::test]
async fn unticking_the_default_is_coerced_back_even_with_other_roots() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let first = database
        .create_storage_root(StorageRootId::new(), root("Downloads", "/downloads", false))
        .await
        .expect("first");
    database
        .create_storage_root(StorageRootId::new(), root("Movies", "/movies", false))
        .await
        .expect("second");

    let updated = database
        .update_storage_root(first.id, root("Downloads", "/downloads", false))
        .await
        .expect("update");

    assert!(updated.is_default, "another root existing does not help");
    assert_eq!(defaults(&database).await, vec!["Downloads".to_owned()]);
}

#[tokio::test]
async fn editing_a_non_default_root_leaves_the_default_alone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    database
        .create_storage_root(StorageRootId::new(), root("Downloads", "/downloads", false))
        .await
        .expect("first");
    let second = database
        .create_storage_root(StorageRootId::new(), root("Movies", "/movies", false))
        .await
        .expect("second");

    let updated = database
        .update_storage_root(second.id, root("Films", "/movies", false))
        .await
        .expect("update");

    assert!(!updated.is_default);
    assert_eq!(defaults(&database).await, vec!["Downloads".to_owned()]);
}

#[tokio::test]
async fn deleting_the_default_promotes_the_alphabetically_first_survivor() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let bravo = database
        .create_storage_root(StorageRootId::new(), root("Bravo", "/bravo", false))
        .await
        .expect("bravo");
    database
        .create_storage_root(StorageRootId::new(), root("Charlie", "/charlie", false))
        .await
        .expect("charlie");
    database
        .create_storage_root(StorageRootId::new(), root("Alpha", "/alpha", false))
        .await
        .expect("alpha");
    assert_eq!(defaults(&database).await, vec!["Bravo".to_owned()]);

    database
        .delete_storage_root(bravo.id)
        .await
        .expect("delete");

    assert_eq!(defaults(&database).await, vec!["Alpha".to_owned()]);
}

#[tokio::test]
async fn deleting_a_non_default_root_does_not_move_the_default() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    database
        .create_storage_root(StorageRootId::new(), root("Bravo", "/bravo", false))
        .await
        .expect("bravo");
    let alpha = database
        .create_storage_root(StorageRootId::new(), root("Alpha", "/alpha", false))
        .await
        .expect("alpha");

    database
        .delete_storage_root(alpha.id)
        .await
        .expect("delete");

    assert_eq!(defaults(&database).await, vec!["Bravo".to_owned()]);
}

#[tokio::test]
async fn deleting_the_last_root_leaves_an_empty_table() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let only = database
        .create_storage_root(StorageRootId::new(), root("Downloads", "/downloads", false))
        .await
        .expect("create");

    database.delete_storage_root(only.id).await.expect("delete");

    assert!(
        database
            .list_storage_roots()
            .await
            .expect("roots")
            .is_empty()
    );
}

#[tokio::test]
async fn default_storage_root_returns_the_marked_root() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    assert!(
        database
            .default_storage_root()
            .await
            .expect("empty lookup")
            .is_none(),
        "no roots, no default"
    );
    database
        .create_storage_root(StorageRootId::new(), root("Zulu", "/zulu", false))
        .await
        .expect("zulu");
    database
        .create_storage_root(StorageRootId::new(), root("Alpha", "/alpha", false))
        .await
        .expect("alpha");

    let default = database
        .default_storage_root()
        .await
        .expect("lookup")
        .expect("a default exists");

    assert_eq!(
        default.name, "Zulu",
        "the marked root wins, not the alphabetically first"
    );
}
