//! RD-1140-01: a single folder named like the package is dissolved into the package folder.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use rd_core::{CategoryId, PackageId, PostprocessKind, PostprocessState};
use rd_db::{Database, NewPackage};
use zip::write::SimpleFileOptions;

use crate::{
    tests::{run_extraction, seed_completed_file},
    unwrap_job::{SOURCE, STAGING},
};

pub(crate) async fn database_with(temp: &Path, settings: serde_json::Value) -> Database {
    let database = Database::open(temp.join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting("service.settings".to_owned(), settings)
        .await
        .expect("settings");
    database
}

/// A ZIP holding each member's text, folders spelled with `/`.
pub(crate) fn zip_of(members: &[(&str, &str)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, content) in members {
        zip.start_file(*name, SimpleFileOptions::default())
            .expect("start member");
        zip.write_all(content.as_bytes()).expect("write member");
    }
    zip.finish().expect("finish ZIP").into_inner()
}

/// A completed package named `name` in `<temp>/library/<name>`, holding each file as a finished
/// download (a name with `/` lands in a folder, as a multi-file NZB or torrent puts it).
pub(crate) async fn seed(
    database: &Database,
    temp: &Path,
    name: &str,
    category_id: Option<CategoryId>,
    files: &[(&str, Vec<u8>)],
) -> (PackageId, PathBuf) {
    let destination = rd_files::package_directory(&temp.join("library"), name);
    std::fs::create_dir_all(&destination).expect("destination");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: name.to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    for (file, bytes) in files {
        let path = destination.join(file);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("folder");
        std::fs::write(path, bytes).expect("file");
        seed_completed_file(database, package.id, file).await;
    }
    (package.id, destination)
}

/// The names in `folder`, sorted.
fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .expect("folder")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// The problem the dissolve recorded, as its code.
async fn reported(database: &Database, package_id: PackageId) -> Option<String> {
    database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps")
        .into_iter()
        .find(|step| step.kind == PostprocessKind::Cleanup && step.source_path == SOURCE)
        .map(|step| {
            assert_eq!(step.state, PostprocessState::Skipped);
            step.code.unwrap_or_default()
        })
}

fn unwrapping() -> serde_json::Value {
    serde_json::json!({ "default_level": "delete", "unwrap_package_folder": true })
}

#[tokio::test]
async fn a_single_folder_named_like_the_package_is_dissolved() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(temp.path(), unwrapping()).await;
    // Case differs on purpose: the comparison ignores it.
    let archive = zip_of(&[
        ("release.name-GRP/film.r00", "inner volume"),
        ("release.name-GRP/Sub/extra.txt", "extra"),
    ]);
    let (package_id, directory) = seed(
        &database,
        temp.path(),
        "Release.Name-GRP",
        None,
        &[("Release.Name-GRP.zip", archive)],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;

    assert_eq!(names(&directory), ["Sub", "film.r00"]);
    assert_eq!(
        std::fs::read(directory.join("Sub").join("extra.txt")).ok(),
        Some(b"extra".to_vec())
    );
    assert_eq!(reported(&database, package_id).await, None);
}

#[tokio::test]
async fn another_name_or_a_second_entry_keeps_the_folder() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(temp.path(), unwrapping()).await;
    let other = zip_of(&[("Something.Else/film.mkv", "film")]);
    let (other_id, other_dir) = seed(
        &database,
        temp.path(),
        "Release",
        None,
        &[("Release.zip", other)],
    )
    .await;
    let beside = zip_of(&[("Twin/film.mkv", "film"), ("readme.txt", "read me")]);
    let (beside_id, beside_dir) = seed(
        &database,
        temp.path(),
        "Twin",
        None,
        &[("Twin.zip", beside)],
    )
    .await;

    run_extraction(&database, temp.path(), other_id).await;
    run_extraction(&database, temp.path(), beside_id).await;

    assert_eq!(names(&other_dir), ["Something.Else"]);
    assert_eq!(names(&beside_dir), ["Twin", "readme.txt"]);
    assert!(beside_dir.join("Twin").join("film.mkv").is_file());
}

#[tokio::test]
async fn a_package_without_archives_is_dissolved_too() {
    // An NZB or a torrent that brings its own folder along: nothing to unpack, the cleanup runs.
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(temp.path(), unwrapping()).await;
    let (package_id, directory) = seed(
        &database,
        temp.path(),
        "Show.S01",
        None,
        &[
            ("Show.S01/e01.mkv", b"one".to_vec()),
            ("Show.S01/e02.mkv", b"two".to_vec()),
        ],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;

    assert_eq!(names(&directory), ["e01.mkv", "e02.mkv"]);
}

#[tokio::test]
async fn off_by_default_the_folder_stays() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(
        temp.path(),
        serde_json::json!({ "default_level": "delete" }),
    )
    .await;
    let archive = zip_of(&[("Release/film.mkv", "film")]);
    let (package_id, directory) = seed(
        &database,
        temp.path(),
        "Release",
        None,
        &[("Release.zip", archive)],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;

    assert_eq!(names(&directory), ["Release"]);
}

#[tokio::test]
async fn a_taken_name_leaves_everything_as_it_was_and_says_so() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(temp.path(), unwrapping()).await;
    // The one name a move up could collide with in a fresh pass: the folder's working name.
    let archive = zip_of(&[
        ("Release/film.mkv", "film"),
        (format!("Release/{STAGING}/note.txt").as_str(), "note"),
    ]);
    let (package_id, directory) = seed(
        &database,
        temp.path(),
        "Release",
        None,
        &[("Release.zip", archive)],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;

    assert_eq!(names(&directory), ["Release"]);
    assert_eq!(names(&directory.join("Release")), [STAGING, "film.mkv"]);
    assert_eq!(
        reported(&database, package_id).await.as_deref(),
        Some(crate::steps::codes::UNWRAP_CONFLICT)
    );
}

#[tokio::test]
async fn a_dissolve_left_half_done_is_finished_without_overwriting_anything() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(temp.path(), unwrapping()).await;
    let (package_id, directory) = seed(
        &database,
        temp.path(),
        "Release",
        None,
        &[("moved.mkv", b"already up".to_vec())],
    )
    .await;
    // What a stop between two moves leaves: one entry up, the rest still in the working folder,
    // one of them under a name that is now taken.
    let staging = directory.join(STAGING);
    std::fs::create_dir_all(&staging).expect("staging");
    std::fs::write(staging.join("rest.mkv"), b"rest").expect("rest");
    std::fs::write(staging.join("moved.mkv"), b"the other one").expect("taken");

    run_extraction(&database, temp.path(), package_id).await;

    assert_eq!(
        std::fs::read(directory.join("moved.mkv")).ok(),
        Some(b"already up".to_vec()),
        "nothing is overwritten"
    );
    assert_eq!(
        std::fs::read(directory.join("rest.mkv")).ok(),
        Some(b"rest".to_vec())
    );
    assert_eq!(names(&staging), ["moved.mkv"]);
    assert_eq!(
        reported(&database, package_id).await.as_deref(),
        Some(crate::steps::codes::UNWRAP_CONFLICT)
    );
}

#[tokio::test]
async fn with_a_folder_per_archive_each_archive_folder_is_dissolved_and_kept() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(
        temp.path(),
        serde_json::json!({
            "default_level": "delete",
            "unwrap_package_folder": true,
            "unpack_to_subfolder": true,
        }),
    )
    .await;
    // Named like the package: its folder `Release/` is what a folder per archive asked for, so
    // only the archive's own `Release/` inside it goes.
    let archive = zip_of(&[("Release/film.mkv", "film")]);
    let (package_id, directory) = seed(
        &database,
        temp.path(),
        "Release",
        None,
        &[("Release.zip", archive)],
    )
    .await;
    let extras = zip_of(&[("Extras/extra.txt", "extra"), ("other.txt", "other")]);
    let (pack_id, pack) = seed(
        &database,
        temp.path(),
        "Pack",
        None,
        &[("Extras.zip", extras)],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;
    run_extraction(&database, temp.path(), pack_id).await;

    assert_eq!(names(&directory), ["Release"]);
    assert_eq!(names(&directory.join("Release")), ["film.mkv"]);
    // Two entries in the archive's folder: it stays as it is.
    assert_eq!(names(&pack), ["Extras"]);
    assert_eq!(names(&pack.join("Extras")), ["Extras", "other.txt"]);
}

/// A category whose only post-processing override is `unwrap_package_folder`.
async fn category_with(database: &Database, temp: &Path, unwrap: Option<bool>) -> CategoryId {
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
            name: "Unwrapping".to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root.id,
            relative_path: "unwrapping".to_owned(),
            is_default: false,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            unpack_to_subfolder: None,
            unwrap_package_folder: unwrap,
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

/// Runs one `Release` package of a category with `unwrap` as its override under the global
/// setting `global`, and says whether the folder named like it was dissolved.
async fn dissolved(global: bool, unwrap: Option<bool>) -> bool {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = database_with(
        temp.path(),
        serde_json::json!({ "default_level": "delete", "unwrap_package_folder": global }),
    )
    .await;
    let category = category_with(&database, temp.path(), unwrap).await;
    let archive = zip_of(&[("Release/film.mkv", "film")]);
    let (package_id, directory) = seed(
        &database,
        temp.path(),
        "Release",
        Some(category),
        &[("Release.zip", archive)],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;

    let up = directory.join("film.mkv").is_file();
    let inside = directory.join("Release").join("film.mkv").is_file();
    assert!(up != inside, "the film is in exactly one place");
    up
}

/// The category value wins over the global one in both directions; unset inherits.
#[tokio::test]
async fn a_category_override_wins_and_an_unset_one_inherits_the_global_setting() {
    assert!(
        dissolved(false, Some(true)).await,
        "category on, global off"
    );
    assert!(
        !dissolved(true, Some(false)).await,
        "category off, global on"
    );
    assert!(dissolved(true, None).await, "category unset, global on");
    assert!(!dissolved(false, None).await, "category unset, global off");
}
