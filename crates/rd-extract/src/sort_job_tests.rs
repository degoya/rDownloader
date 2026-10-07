//! The sort step through the whole pipeline (RD-1100-08): files land by their category's
//! templates, companions follow, the unrecognised stay, and nothing leaves the category's folder.

use std::path::{Path, PathBuf};

use rd_core::{
    CategoryId, CollisionPolicy, PackageId, PostprocessKind, PostprocessState, SortTemplates,
};
use rd_db::{Database, NewPackage};

use crate::{
    sort_job::SOURCE,
    tests::{run_extraction, seed_completed_file},
};

const SERIES: &str = "{show}/Season {season:00}/{show} - S{season:00}E{episode:00} - {title}";
const MOVIE: &str = "{movie} ({year})/{movie} ({year})";

/// A category whose packages sort by the two templates above.
async fn sorting_category(database: &Database, temp: &Path) -> CategoryId {
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
    let category = database
        .create_category(rd_db::NewCategory {
            name: "Library".to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root.id,
            relative_path: "library".to_owned(),
            is_default: false,
            postprocess_level: None,
            direct_unpack: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            unpack_to_subfolder: None,
            unwrap_package_folder: None,
            malware_scan: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category");
    database
        .update_category_postprocess(
            category.id,
            rd_db::CategoryPostprocess {
                // An empty list switches the extension cleanup off: the default list removes
                // the NFOs these tests sort.
                cleanup_extensions: Some(Vec::new()),
                sorting: Some(SortTemplates {
                    series: Some(SERIES.to_owned()),
                    dated: None,
                    movie: Some(MOVIE.to_owned()),
                }),
                ..rd_db::CategoryPostprocess::default()
            },
        )
        .await
        .expect("sort templates");
    category.id
}

/// A finished package `name` below `library`, holding `downloads` (rows and files) and `extra`
/// files that sit in the folder without being downloads of their own.
pub(crate) async fn sorted_package(
    database: &Database,
    library: &Path,
    category_id: CategoryId,
    name: &str,
    downloads: &[&str],
    extra: &[&str],
) -> (PackageId, PathBuf) {
    let directory = library.join(name);
    std::fs::create_dir_all(&directory).expect("package folder");
    for file in downloads.iter().chain(extra) {
        let path = directory.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("folder");
        }
        std::fs::write(&path, file.as_bytes()).expect("file");
    }
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: name.to_owned(),
            destination: directory.to_string_lossy().into_owned(),
            category_id: Some(category_id),
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    for file in downloads {
        seed_completed_file(database, package.id, file).await;
    }
    (package.id, directory)
}

pub(crate) async fn sort_step(
    database: &Database,
    package_id: PackageId,
) -> rd_core::PostprocessStep {
    database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps")
        .into_iter()
        .find(|step| step.kind == PostprocessKind::Sort && step.source_path == SOURCE)
        .expect("sort step")
}

pub(crate) async fn prepared(temp: &Path) -> (Database, CategoryId, PathBuf) {
    let database = Database::open(temp.join("extract.sqlite"))
        .await
        .expect("database");
    let category_id = sorting_category(&database, temp).await;
    let library = temp.join("library");
    std::fs::create_dir_all(&library).expect("library");
    (database, category_id, library)
}

#[tokio::test]
async fn an_episode_and_its_subtitle_land_by_the_series_template() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (database, category_id, library) = prepared(temp.path()).await;
    let (package_id, directory) = sorted_package(
        &database,
        &library,
        category_id,
        "Breaking.Bad.S05E14.Ozymandias.720p.HDTV.x264-IMMERSE",
        &["Breaking.Bad.S05E14.Ozymandias.720p.HDTV.x264-IMMERSE.mkv"],
        &["Breaking.Bad.S05E14.Ozymandias.720p.HDTV.x264-IMMERSE.en.srt"],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;

    let season = library.join("Breaking Bad").join("Season 05");
    assert!(
        season
            .join("Breaking Bad - S05E14 - Ozymandias.mkv")
            .is_file()
    );
    assert!(
        season
            .join("Breaking Bad - S05E14 - Ozymandias.en.srt")
            .is_file()
    );
    assert!(!directory.exists(), "the emptied package folder lingers");
    let step = sort_step(&database, package_id).await;
    assert_eq!(step.state, PostprocessState::Completed, "{step:?}");
    assert_eq!(step.params.get("placed").map(String::as_str), Some("2"));
    assert_eq!(step.params.get("left").map(String::as_str), Some("0"));
}

#[tokio::test]
async fn a_multi_episode_file_and_an_obfuscated_film_are_named_right() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (database, category_id, library) = prepared(temp.path()).await;
    let (episodes, _) = sorted_package(
        &database,
        &library,
        category_id,
        "Friends.S01E16E17.DVDRip.XviD-SAiNTS",
        &["Friends.S01E16E17.DVDRip.XviD-SAiNTS.avi"],
        &[],
    )
    .await;
    let (film, _) = sorted_package(
        &database,
        &library,
        category_id,
        "Inception.2010.1080p.BluRay.x264-SPARKS",
        &["a8f3c2e1.mkv"],
        &["info.nfo"],
    )
    .await;

    run_extraction(&database, temp.path(), episodes).await;
    run_extraction(&database, temp.path(), film).await;

    assert!(
        library
            .join("Friends")
            .join("Season 01")
            .join("Friends - S01E16-E17.avi")
            .is_file()
    );
    let folder = library.join("Inception (2010)");
    assert!(folder.join("Inception (2010).mkv").is_file());
    assert!(folder.join("Inception (2010).nfo").is_file());
}

#[tokio::test]
async fn an_unrecognised_file_stays_where_it_was() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (database, category_id, library) = prepared(temp.path()).await;
    let (package_id, directory) = sorted_package(
        &database,
        &library,
        category_id,
        "Holiday",
        &["holiday_video.mp4"],
        &["notes.txt"],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;

    assert!(directory.join("holiday_video.mp4").is_file());
    assert!(directory.join("notes.txt").is_file());
    let mut entries: Vec<_> = std::fs::read_dir(&library)
        .expect("library")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    entries.sort();
    assert_eq!(entries, vec![std::ffi::OsString::from("Holiday")]);
    let step = sort_step(&database, package_id).await;
    assert_eq!(step.state, PostprocessState::Completed);
    assert_eq!(step.params.get("left").map(String::as_str), Some("1"));
}

#[tokio::test]
async fn a_taken_name_follows_the_collision_policy() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (database, category_id, library) = prepared(temp.path()).await;
    let folder = library.join("Inception (2010)");
    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::write(folder.join("Inception (2010).mkv"), b"the first copy").expect("existing");
    database
        .set_category_collision_policy(category_id, Some(CollisionPolicy::Skip))
        .await
        .expect("policy");
    let (package_id, directory) = sorted_package(
        &database,
        &library,
        category_id,
        "Inception.2010.1080p.BluRay.x264-SPARKS",
        &["Inception.2010.1080p.BluRay.x264-SPARKS.mkv"],
        &[],
    )
    .await;

    run_extraction(&database, temp.path(), package_id).await;

    assert_eq!(
        std::fs::read(folder.join("Inception (2010).mkv")).expect("existing"),
        b"the first copy"
    );
    assert!(
        directory
            .join("Inception.2010.1080p.BluRay.x264-SPARKS.mkv")
            .is_file(),
        "a skipped file stays in its package"
    );

    // `rename` puts the second one beside the first.
    database
        .set_category_collision_policy(category_id, Some(CollisionPolicy::Rename))
        .await
        .expect("policy");
    run_extraction(&database, temp.path(), package_id).await;
    assert!(folder.join("Inception (2010) (1).mkv").is_file());
    assert_eq!(
        std::fs::read(folder.join("Inception (2010).mkv")).expect("existing"),
        b"the first copy"
    );
}

/// A folder the template names that is a link to somewhere else is not followed out of the
/// category's folder: the file stays, the step fails and says why.
#[cfg(unix)]
#[tokio::test]
async fn a_linked_folder_does_not_carry_a_file_out_of_the_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (database, category_id, library) = prepared(temp.path()).await;
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside");
    std::os::unix::fs::symlink(&outside, library.join("Inception (2010)")).expect("link");
    let (package_id, directory) = sorted_package(
        &database,
        &library,
        category_id,
        "Inception.2010.1080p.BluRay.x264-SPARKS",
        &["Inception.2010.1080p.BluRay.x264-SPARKS.mkv"],
        &[],
    )
    .await;

    let state = crate::tests::run_extraction_to_end(&database, temp.path(), package_id).await;

    assert_eq!(state, rd_core::PackageState::Failed);
    assert!(
        directory
            .join("Inception.2010.1080p.BluRay.x264-SPARKS.mkv")
            .is_file()
    );
    assert_eq!(
        std::fs::read_dir(&outside).expect("outside").count(),
        0,
        "a file left the category's folder"
    );
    let step = sort_step(&database, package_id).await;
    assert_eq!(step.state, PostprocessState::Failed, "{step:?}");
}
