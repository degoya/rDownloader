//! Categories and their write paths.

use crate::{Database, NewCategory, NewStorageRoot};

/// Every column a category write names must have a value bound to it.
///
/// This is not a hypothetical: adding `delete_par2` left the update statement with one more
/// placeholder than value, so *every* category edit failed at runtime while the code compiled
/// and every existing test still passed. Exercising both write paths and reading the result
/// back is what makes the next such column a test failure instead of a support ticket.
#[tokio::test]
async fn a_category_survives_both_write_paths_with_every_field_intact() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("categories.sqlite"))
        .await
        .expect("database");
    let root = database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            NewStorageRoot {
                name: "Downloads".to_owned(),
                path: directory.path().to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root");
    let created = database
        .create_category(NewCategory {
            name: "Movies".to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root.id,
            relative_path: "movies".to_owned(),
            is_default: false,
            postprocess_level: Some(rd_core::PostprocessLevel::Delete),
            script: Some("done.sh".to_owned()),
            cleanup_extensions: Some(vec!["nfo".to_owned()]),
            recursive_unpack: Some(true),
            unpack_to_subfolder: Some(true),
            direct_unpack: Some(true),
            malware_scan: Some(true),
            sfv_verify: Some(false),
            safe_postproc: Some(false),
            delete_par2: Some(true),
            upload_enabled: Some(true),
            upload_remote: Some("archive:movies".to_owned()),
        })
        .await
        .expect("category");
    assert_eq!(created.delete_par2, Some(true));
    assert_eq!(created.unpack_to_subfolder, Some(true));
    assert_eq!(created.direct_unpack, Some(true));
    assert_eq!(created.malware_scan, Some(true));
    assert_eq!(created.safe_postproc, Some(false));

    // The general update path, which is the one that was broken.
    let updated = database
        .update_category(
            created.id,
            NewCategory {
                name: "Films".to_owned(),
                color: "#F87171".to_owned(),
                storage_root_id: root.id,
                relative_path: "films".to_owned(),
                is_default: false,
                postprocess_level: Some(rd_core::PostprocessLevel::Unpack),
                script: None,
                cleanup_extensions: None,
                recursive_unpack: Some(false),
                unpack_to_subfolder: Some(false),
                direct_unpack: Some(false),
                malware_scan: Some(false),
                sfv_verify: Some(true),
                safe_postproc: Some(true),
                delete_par2: Some(false),
                upload_enabled: Some(false),
                upload_remote: None,
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.name, "Films");
    assert_eq!(updated.delete_par2, Some(false));
    assert_eq!(updated.safe_postproc, Some(true));
    assert_eq!(updated.unpack_to_subfolder, Some(false));
    assert_eq!(updated.direct_unpack, Some(false));
    assert_eq!(updated.malware_scan, Some(false));

    // And the post-processing path, which carries the plugin steps.
    database
        .update_category_postprocess(
            created.id,
            crate::CategoryPostprocess {
                level: Some(rd_core::PostprocessLevel::Delete),
                script: Some("after.sh".to_owned()),
                cleanup_extensions: Some(vec!["sfv".to_owned()]),
                recursive_unpack: Some(true),
                unpack_to_subfolder: Some(true),
                direct_unpack: Some(true),
                malware_scan: Some(true),
                sfv_verify: Some(false),
                safe_postproc: Some(false),
                delete_par2: Some(true),
                plugin_steps: Some(vec!["019d0000-0000-7000-8000-000000000106".to_owned()]),
                upload_enabled: Some(true),
                upload_remote: Some("archive:films".to_owned()),
                // A blank template is dropped on the way in, the other one kept (RD-1100-08).
                sorting: Some(rd_core::SortTemplates {
                    series: Some("  ".to_owned()),
                    dated: None,
                    movie: Some(" {movie} ({year})/{movie} ({year}) ".to_owned()),
                }),
            },
        )
        .await
        .expect("postprocess update");

    let stored = database
        .list_categories()
        .await
        .expect("categories")
        .into_iter()
        .find(|category| category.id == created.id)
        .expect("category is still there");
    assert_eq!(stored.delete_par2, Some(true));
    assert_eq!(stored.safe_postproc, Some(false));
    assert_eq!(stored.unpack_to_subfolder, Some(true));
    assert_eq!(stored.direct_unpack, Some(true));
    assert_eq!(stored.malware_scan, Some(true));
    assert_eq!(stored.script.as_deref(), Some("after.sh"));
    assert_eq!(
        stored.plugin_steps.as_deref(),
        Some(["019d0000-0000-7000-8000-000000000106".to_owned()].as_slice())
    );
    assert_eq!(stored.upload_remote.as_deref(), Some("archive:films"));
    assert_eq!(
        stored.sorting,
        Some(rd_core::SortTemplates {
            series: None,
            dated: None,
            movie: Some("{movie} ({year})/{movie} ({year})".to_owned()),
        })
    );
}
