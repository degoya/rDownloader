//! RD-170-16: every archive set unpacked into a folder of its own, named after the archive.
//!
//! Off, the package folder is the destination as it always was; the tests in `tests.rs` hold
//! that half unchanged.

use std::{path::Path, time::Duration};

use rd_core::{CategoryId, PackageId, PostprocessKind, PostprocessState};
use rd_db::{Database, NewPackage};

use crate::{
    ExtractionConfig, ExtractionService, ExtractionTrigger,
    tests::{
        run_extraction, seed_completed_file, seed_nested_package, wait_until_finished, zip_bytes,
    },
    unpack_job::UnpackTarget,
};

async fn database_with(temp: &Path, settings: serde_json::Value) -> Database {
    let database = Database::open(temp.join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting("service.settings".to_owned(), settings)
        .await
        .expect("settings");
    database
}

/// A completed package holding the given archives, each one a finished download.
async fn seed_archives(
    database: &Database,
    destination: &Path,
    category_id: Option<CategoryId>,
    archives: &[(&str, Vec<u8>)],
) -> PackageId {
    std::fs::create_dir_all(destination).expect("destination");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "archives".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    for (name, bytes) in archives {
        std::fs::write(destination.join(name), bytes).expect("archive");
        seed_completed_file(database, package.id, name).await;
    }
    package.id
}

/// A manual run of a package that is already `Completed`, waited on by its unpack step: the
/// package state cannot tell this run from the last one (see
/// `a_renamed_package_folder_does_not_give_a_second_run_a_second_set_of_steps`).
async fn unpack_again(database: &Database, temp: &Path, package_id: PackageId) {
    let mark = chrono::Utc::now();
    let service = ExtractionService::start(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: std::env::temp_dir().join("rd-scripts-test"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
    );
    service
        .request(package_id, ExtractionTrigger::Manual)
        .await
        .expect("request");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let done = database
                .list_postprocess_steps(&package_id.to_string())
                .await
                .expect("steps")
                .iter()
                .any(|step| {
                    step.kind == PostprocessKind::ExtractZip
                        && step.state == PostprocessState::Completed
                        && step.updated_at > mark
                });
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the second unpack finished");
    wait_until_finished(&service, package_id).await;
    service.shutdown().await;
}

#[test]
fn a_folder_is_named_after_its_set_without_volume_or_extension() {
    let package = tempfile::tempdir().expect("tempdir");
    let directory = package.path();
    let sets = rd_postprocess::group_archive_sets(&[
        directory.join("Film.part1.rar"),
        directory.join("Film.part2.rar"),
        directory.join("Extras.zip"),
        directory.join("x.7z.001"),
        directory.join("x.7z.002"),
    ]);
    let mut folders = UnpackTarget::OwnFolder.destinations(directory, &sets);
    folders.sort();
    assert_eq!(
        folders,
        vec![
            directory.join("Extras"),
            directory.join("Film"),
            directory.join("x")
        ]
    );
    // Off: the package folder itself, as before.
    assert!(
        UnpackTarget::Package
            .destinations(directory, &sets)
            .iter()
            .all(|folder| folder == directory)
    );
}

#[test]
fn two_sets_of_one_name_get_a_folder_each_and_the_same_one_every_time() {
    // RD-190-06: `Film.zip` and `Film.rar` used to share `Film/`. The sets come sorted, zip
    // before rar, so the zip keeps the plain name and the rar takes the next free one.
    let package = tempfile::tempdir().expect("tempdir");
    let directory = package.path();
    let sets = rd_postprocess::group_archive_sets(&[
        directory.join("Film.rar"),
        directory.join("Film.zip"),
    ]);
    assert_eq!(sets.len(), 2, "{sets:?}");
    let folders = UnpackTarget::OwnFolder.destinations(directory, &sets);
    let named: Vec<_> = sets
        .iter()
        .map(|set| {
            set.first()
                .file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned()
        })
        .zip(folders.clone())
        .collect();
    assert_eq!(
        named,
        vec![
            ("Film.zip".to_owned(), directory.join("Film")),
            ("Film.rar".to_owned(), directory.join("Film (1)")),
        ]
    );
    for folder in &folders {
        std::fs::create_dir(folder).expect("folder");
    }
    assert_eq!(
        UnpackTarget::OwnFolder.destinations(directory, &sets),
        folders,
        "a rerun lands in the same folders"
    );
}

#[test]
fn a_nested_set_stays_in_the_folder_its_parent_went_into() {
    let package = tempfile::tempdir().expect("tempdir");
    let directory = package.path();
    let sets = rd_postprocess::group_archive_sets(&[
        directory.join("Film").join("inner.zip"),
        directory.join("Film").join("deeper").join("more.zip"),
        directory.join("loose.zip"),
    ]);
    assert_eq!(sets.len(), 3, "{sets:?}");
    let folders = UnpackTarget::EnclosingFolder.destinations(directory, &sets);
    for (set, folder) in sets.iter().zip(folders) {
        let expected = if set.first().starts_with(directory.join("Film")) {
            directory.join("Film")
        } else {
            // Nothing encloses a set in the package root, so it gets a folder of its own.
            directory.join("loose")
        };
        assert_eq!(folder, expected, "{set:?}");
    }
}

#[tokio::test]
async fn every_archive_set_is_unpacked_into_a_folder_of_its_own() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(
        temp.path(),
        serde_json::json!({ "unpack_to_subfolder": true }),
    )
    .await;
    let destination = temp.path().join("dl");
    let package_id = seed_archives(
        &database,
        &destination,
        None,
        &[
            ("Film.zip", zip_bytes("film.mkv", b"film")),
            ("Extras.zip", zip_bytes("extra.txt", b"extra")),
        ],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;

    assert_eq!(
        std::fs::read(destination.join("Film").join("film.mkv")).ok(),
        Some(b"film".to_vec())
    );
    assert_eq!(
        std::fs::read(destination.join("Extras").join("extra.txt")).ok(),
        Some(b"extra".to_vec())
    );
    assert!(!destination.join("film.mkv").exists());
    assert!(!destination.join("extra.txt").exists());
    // The archives stay where they are; the package level (Unpack) deletes nothing.
    assert!(destination.join("Film.zip").is_file());
    assert!(destination.join("Extras.zip").is_file());
    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let film = steps
        .iter()
        .find(|step| {
            step.kind == PostprocessKind::ExtractZip && step.source_path.ends_with("Film.zip")
        })
        .expect("Film step");
    assert_eq!(
        film.output_path.as_deref(),
        destination.join("Film").to_str(),
        "the step names the folder the set went into"
    );
}

#[tokio::test]
async fn two_archives_whose_names_make_one_folder_unpack_into_two() {
    // `Film .zip` sanitises to the same folder name as `Film.zip` without being the same set,
    // the case `Film.rar` beside `Film.zip` is too (RD-190-06) — with a zip a test can build.
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(
        temp.path(),
        serde_json::json!({ "unpack_to_subfolder": true }),
    )
    .await;
    let destination = temp.path().join("dl");
    let package_id = seed_archives(
        &database,
        &destination,
        None,
        &[
            ("Film.zip", zip_bytes("film.mkv", b"first")),
            ("Film .zip", zip_bytes("film.mkv", b"second")),
        ],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;

    assert_eq!(
        std::fs::read(destination.join("Film").join("film.mkv")).ok(),
        Some(b"first".to_vec())
    );
    assert_eq!(
        std::fs::read(destination.join("Film (1)").join("film.mkv")).ok(),
        Some(b"second".to_vec()),
        "the second set did not overwrite the first"
    );
}

#[tokio::test]
async fn an_archive_inside_an_archive_is_unpacked_inside_the_same_folder() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(
        temp.path(),
        serde_json::json!({ "recursive_unpack": true, "unpack_to_subfolder": true }),
    )
    .await;
    let destination = temp.path().join("dl");
    let package_id = seed_nested_package(&database, &destination, 2).await;

    run_extraction(&database, temp.path(), package_id).await;

    assert_eq!(
        std::fs::read(destination.join("level1").join("payload.txt")).ok(),
        Some(b"payload".to_vec())
    );
    // The intermediate is deleted as always, and nothing lands in the package root.
    assert!(!destination.join("level1").join("level2.zip").exists());
    assert!(!destination.join("payload.txt").exists());
    assert!(!destination.join("level2").exists());
    assert!(!destination.join("level1").join("level2").exists());
    assert!(destination.join("level1.zip").is_file());
}

#[tokio::test]
async fn a_file_in_the_way_moves_the_folder_on_and_a_rerun_finds_it_again() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(
        temp.path(),
        serde_json::json!({ "unpack_to_subfolder": true }),
    )
    .await;
    let destination = temp.path().join("dl");
    std::fs::create_dir_all(&destination).expect("destination");
    std::fs::write(destination.join("Extras"), b"a file, not a folder").expect("file");
    let package_id = seed_archives(
        &database,
        &destination,
        None,
        &[("Extras.zip", zip_bytes("extra.txt", b"extra"))],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;
    // A manual run always extracts again; it has to merge into the same folder.
    std::fs::remove_file(destination.join("Extras (1)").join("extra.txt")).expect("remove");
    unpack_again(&database, temp.path(), package_id).await;

    assert_eq!(
        std::fs::read(destination.join("Extras")).ok(),
        Some(b"a file, not a folder".to_vec()),
        "the file in the way is left alone"
    );
    assert_eq!(
        std::fs::read(destination.join("Extras (1)").join("extra.txt")).ok(),
        Some(b"extra".to_vec())
    );
    assert!(!destination.join("Extras (2)").exists());
}

/// A category whose only post-processing override is `unpack_to_subfolder`.
async fn category_with(database: &Database, temp: &Path, folders: Option<bool>) -> CategoryId {
    let root = database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            rd_db::NewStorageRoot {
                name: "Downloads".to_owned(),
                path: temp.to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root");
    database
        .create_category(rd_db::NewCategory {
            name: "Foldered".to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root.id,
            relative_path: "foldered".to_owned(),
            is_default: false,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            unpack_to_subfolder: folders,
            direct_unpack: None,
            malware_scan: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category")
        .id
}

/// Runs one `Film.zip` package of a category with `folders` as its override under the global
/// setting `global`, and says whether the film ended up in `Film/` (else in the package root).
async fn unpacked_into_a_folder(global: bool, folders: Option<bool>) -> bool {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(
        temp.path(),
        serde_json::json!({ "unpack_to_subfolder": global }),
    )
    .await;
    let category = category_with(&database, temp.path(), folders).await;
    let destination = temp.path().join("dl");
    let package_id = seed_archives(
        &database,
        &destination,
        Some(category),
        &[("Film.zip", zip_bytes("film.mkv", b"film"))],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;

    let in_folder = destination.join("Film").join("film.mkv").is_file();
    let in_root = destination.join("film.mkv").is_file();
    assert!(in_folder != in_root, "the film is in exactly one place");
    in_folder
}

/// The category value wins over the global one; a category that sets nothing inherits it.
#[tokio::test]
async fn a_category_override_wins_and_an_unset_one_inherits_the_global_setting() {
    assert!(
        unpacked_into_a_folder(false, Some(true)).await,
        "category on, global off"
    );
    assert!(
        !unpacked_into_a_folder(true, Some(false)).await,
        "category off, global on"
    );
    assert!(
        unpacked_into_a_folder(true, None).await,
        "category unset, global on"
    );
    assert!(
        !unpacked_into_a_folder(false, None).await,
        "category unset, global off"
    );
}
