//! Packages: queue order, folder renames, postprocess steps and single lookups.

use rd_core::{
    AuthProfileSelection, DownloadId, ImportMode, IngressSource, PackageId, PostprocessKind,
    PostprocessState,
};

use crate::{
    Database, NewAccount, NewDownload, NewNzbFile, NewNzbImport, NewNzbSegment, NewPackage,
};

#[tokio::test]
async fn packages_and_downloads_follow_priority_then_manual_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("queue-order.sqlite"))
        .await
        .expect("database");
    let mut ids = Vec::new();
    for (name, priority) in [
        ("first-normal", rd_core::DownloadPriority::Normal),
        ("high", rd_core::DownloadPriority::High),
        ("second-normal", rd_core::DownloadPriority::Normal),
        ("low", rd_core::DownloadPriority::Low),
    ] {
        let package = database
            .create_package(NewPackage {
                id: PackageId::new(),
                name: name.to_owned(),
                destination: directory.path().to_string_lossy().into_owned(),
                category_id: None,
                priority,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        database
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id: package.id,
                source: format!("https://example.test/{name}").parse().expect("URL"),
                file_name: name.to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: AuthProfileSelection::Auto,
                initial_state: rd_core::DownloadState::Queued,
                kind: rd_core::DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                replay: None,
                mirror_group: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download");
        ids.push(package.id);
    }
    let names = |packages: Vec<rd_core::DownloadPackage>| {
        packages
            .into_iter()
            .map(|package| package.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(database.list_packages().await.expect("packages")),
        ["high", "first-normal", "second-normal", "low"]
    );
    let files = database.list_downloads().await.expect("downloads");
    assert_eq!(
        files
            .iter()
            .map(|file| file.file_name.as_str())
            .collect::<Vec<_>>(),
        ["high", "first-normal", "second-normal", "low"]
    );

    // Manual order inside the normal tier: second-normal before first-normal.
    database
        .reorder_packages(vec![ids[1], ids[2], ids[0], ids[3]])
        .await
        .expect("reorder");
    assert_eq!(
        names(database.list_packages().await.expect("packages")),
        ["high", "second-normal", "first-normal", "low"]
    );

    // Priority beats manual position; a category change writes the destination the caller
    // resolved for that one package and remembers where its data used to be.
    let previous = database.list_packages().await.expect("packages");
    let previous = previous
        .iter()
        .find(|package| package.id == ids[3])
        .expect("package")
        .destination
        .clone();
    let updated = database
        .update_packages(
            vec![ids[3]],
            crate::PackageChange {
                category: Some(crate::CategoryAssignment {
                    category_id: None,
                    destinations: std::collections::HashMap::from([(
                        ids[3],
                        "/tmp/elsewhere/low".to_owned(),
                    )]),
                }),
                priority: Some(rd_core::DownloadPriority::High),
                name: None,
                password: None,
                postprocess_level: None,
                script: None,
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].priority, rd_core::DownloadPriority::High);
    assert_eq!(updated[0].destination, "/tmp/elsewhere/low");
    assert_eq!(
        database
            .package_previous_destination(ids[3])
            .await
            .expect("previous destination"),
        Some(previous),
        "the former directory is kept until it has been swept"
    );
    database
        .clear_package_previous_destination(ids[3])
        .await
        .expect("clear");
    assert_eq!(
        database
            .package_previous_destination(ids[3])
            .await
            .expect("previous destination"),
        None,
        "and is forgotten once the sweep has run"
    );
    assert_eq!(
        names(database.list_packages().await.expect("packages")),
        ["high", "low", "second-normal", "first-normal"]
    );
}

/// The manual file order inside a package is written, read back, and still there after the
/// service is restarted — the whole point of storing it instead of keeping it in the view.
#[tokio::test]
async fn download_order_inside_a_package_survives_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("queue.sqlite");
    let package_id = PackageId::new();
    let ordered: Vec<DownloadId>;

    {
        let database = Database::open(&path).await.expect("database");
        database
            .create_package(NewPackage {
                id: package_id,
                name: "Release".to_owned(),
                destination: directory.path().to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let mut created = Vec::new();
        for name in ["one.bin", "two.bin", "three.bin"] {
            let file = database
                .create_download(NewDownload {
                    id: DownloadId::new(),
                    package_id,
                    source: format!("https://example.test/{name}").parse().expect("URL"),
                    file_name: name.to_owned(),
                    total_bytes: None,
                    expected_checksum: None,
                    account_id: None,
                    proxy_profile_id: None,
                    auth_profile: AuthProfileSelection::Auto,
                    initial_state: rd_core::DownloadState::Queued,
                    kind: rd_core::DownloadKind::Http,
                    media: None,
                    remote_credential_id: None,
                    replay: None,
                    mirror_group: None,
                    enrichment: Vec::new(),
                    secret_fragment: None,
                })
                .await
                .expect("download");
            created.push(file.id);
        }
        let names = |files: Vec<rd_core::DownloadFile>| {
            files
                .into_iter()
                .map(|file| file.file_name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(database.list_downloads().await.expect("downloads")),
            ["one.bin", "two.bin", "three.bin"]
        );

        ordered = vec![created[2], created[0], created[1]];
        database
            .reorder_downloads(package_id, ordered.clone())
            .await
            .expect("reorder");
        assert_eq!(
            names(database.list_downloads().await.expect("downloads")),
            ["three.bin", "one.bin", "two.bin"]
        );
    }

    // A second `Database::open` on the same file is the restart: nothing of the first instance
    // survives except what it wrote.
    let restarted = Database::open(&path).await.expect("reopened database");
    let files = restarted.list_downloads().await.expect("downloads");
    assert_eq!(
        files
            .iter()
            .map(|file| file.file_name.as_str())
            .collect::<Vec<_>>(),
        ["three.bin", "one.bin", "two.bin"]
    );
    assert_eq!(
        files.iter().map(|file| file.id).collect::<Vec<_>>(),
        ordered
    );
    assert_eq!(
        files.iter().map(|file| file.position).collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

/// The prefix rewrite a folder rename does, and the one thing it must not do.
///
/// `postprocess_steps` is keyed by `(owner_id, kind, source_path)`, so a stored path is an
/// identity and not a note. It has to follow the folder. A *sibling* folder whose name merely
/// starts with the same characters must not, which is why the match is on whole path
/// components rather than on a plain string prefix.
#[tokio::test]
async fn renaming_a_package_folder_carries_its_stored_paths_and_leaves_its_siblings_alone() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("rename.sqlite3"))
        .await
        .expect("database");
    let base = directory.path().join("library");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "Show S01".to_owned(),
            destination: base.join("Show S01").to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let owner = package.id.to_string();
    let old = base.join("Show S01");
    // Three shapes that have to survive the rewrite: a file below the folder, the folder
    // itself as an output, and a path in a folder that only looks like a prefix match.
    for (kind, source, output) in [
        (
            rd_core::PostprocessKind::ExtractZip,
            old.join("archive.zip").to_string_lossy().into_owned(),
            Some(old.to_string_lossy().into_owned()),
        ),
        (
            rd_core::PostprocessKind::Cleanup,
            base.join("Show S01 Extras")
                .join("archive.zip")
                .to_string_lossy()
                .into_owned(),
            None,
        ),
    ] {
        database
            .checkpoint_postprocess(
                owner.clone(),
                kind,
                source,
                rd_core::PostprocessState::Completed,
                output,
                None,
            )
            .await
            .expect("checkpoint");
    }

    let renamed = base.join("Show S01 Complete");
    let updated = database
        .rename_package_directory(
            package.id,
            "Show S01 Complete".to_owned(),
            renamed.to_string_lossy().into_owned(),
        )
        .await
        .expect("rename")
        .expect("package");

    assert_eq!(updated.name, "Show S01 Complete");
    assert_eq!(updated.destination, renamed.to_string_lossy());
    assert_eq!(
        database
            .package_previous_destination(package.id)
            .await
            .expect("previous destination")
            .as_deref(),
        Some(old.to_string_lossy().as_ref()),
        "the disk move has to stay outstanding until it has actually run"
    );

    let steps = database
        .list_postprocess_steps(&owner)
        .await
        .expect("steps");
    let moved = steps
        .iter()
        .find(|step| step.kind == rd_core::PostprocessKind::ExtractZip)
        .expect("the extract step");
    assert_eq!(
        moved.source_path,
        renamed.join("archive.zip").to_string_lossy(),
        "the step's identity did not follow its folder"
    );
    assert_eq!(
        moved.output_path.as_deref(),
        Some(renamed.to_string_lossy().as_ref()),
        "the folder itself, stored as an output, did not follow"
    );
    let untouched = steps
        .iter()
        .find(|step| step.kind == rd_core::PostprocessKind::Cleanup)
        .expect("the cleanup step");
    assert_eq!(
        untouched.source_path,
        base.join("Show S01 Extras")
            .join("archive.zip")
            .to_string_lossy(),
        "a sibling folder that starts with the same characters was dragged along"
    );
}

/// A rename of a package that does not exist is a `None`, not an error and not a silent write.
#[tokio::test]
async fn renaming_a_package_that_is_gone_reports_nothing_rather_than_writing() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("rename.sqlite3"))
        .await
        .expect("database");
    assert!(
        database
            .rename_package_directory(
                PackageId::new(),
                "New Name".to_owned(),
                "/tmp/new".to_owned()
            )
            .await
            .expect("rename")
            .is_none()
    );
}

/// A Usenet package stores where each assembled file landed, on the import's rows rather than
/// on its own; the rename has to reach them through the import or they keep naming a folder
/// that no longer exists.
#[tokio::test]
async fn renaming_a_usenet_package_folder_carries_the_assembled_file_paths_too() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("rename-usenet.sqlite3"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "Show S01.nzb".to_owned(),
            sha256: "ab".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "archive.zip".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: 42,
                    message_id: "usenet-rename@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");
    let base = directory.path().join("library");
    let package = database
        .enqueue_nzb_import(
            import.id,
            base.clone(),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    let old = std::path::PathBuf::from(&package.destination);
    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    database
        .checkpoint_nzb_file_output(
            file.id,
            old.join("archive.zip").to_string_lossy().into_owned(),
        )
        .await
        .expect("file checkpoint");

    let renamed = base.join("Show S01 Complete");
    database
        .rename_package_directory(
            package.id,
            "Show S01 Complete".to_owned(),
            renamed.to_string_lossy().into_owned(),
        )
        .await
        .expect("rename")
        .expect("package");

    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    assert_eq!(
        file.output_path.as_deref(),
        Some(renamed.join("archive.zip").to_string_lossy().as_ref()),
        "the assembled file still names the folder the package no longer has"
    );
}

/// RD-107-04: a step's stable code and its parameters survive the round trip.
#[tokio::test]
async fn a_postprocess_step_keeps_its_code_and_parameters() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("coded.sqlite"))
        .await
        .expect("database");
    let owner = PackageId::new().to_string();

    database
        .checkpoint_postprocess_coded(
            owner.clone(),
            PostprocessKind::Par2,
            "/tmp/release.par2".to_owned(),
            PostprocessState::Failed,
            None,
            Some("PAR2 repair needs 9 blocks but only 3 are available".to_owned()),
            Some("postprocess.par2_not_enough_blocks".to_owned()),
            [
                ("needed".to_owned(), "9".to_owned()),
                ("available".to_owned(), "3".to_owned()),
            ]
            .into_iter()
            .collect(),
        )
        .await
        .expect("checkpoint");

    let steps = database
        .list_postprocess_steps(&owner)
        .await
        .expect("steps");
    let step = steps.first().expect("one step");
    assert_eq!(
        step.code.as_deref(),
        Some("postprocess.par2_not_enough_blocks")
    );
    assert_eq!(step.params.get("needed").map(String::as_str), Some("9"));
    assert_eq!(step.params.get("available").map(String::as_str), Some("3"));

    // A later outcome without a code must clear the old one rather than leave it standing.
    database
        .checkpoint_postprocess(
            owner.clone(),
            PostprocessKind::Par2,
            "/tmp/release.par2".to_owned(),
            PostprocessState::Completed,
            None,
            Some("repaired=true".to_owned()),
        )
        .await
        .expect("checkpoint");
    let steps = database
        .list_postprocess_steps(&owner)
        .await
        .expect("steps");
    let step = steps.first().expect("one step");
    assert_eq!(step.code, None);
    assert!(step.params.is_empty());
}

/// The single getters answer like the lists they stand in for (audit 1.9.1, DB-04).
#[tokio::test]
async fn one_package_and_one_account_read_like_their_lists() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("getters.sqlite"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "one".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let account = database
        .create_account(NewAccount {
            provider: "premiumize".to_owned(),
            label: "Premiumize".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: Some("secret://premiumize/api-key".to_owned()),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");

    let listed = database.list_packages().await.expect("packages");
    let one = database
        .get_package(package_id)
        .await
        .expect("get")
        .expect("the package");
    assert_eq!(
        serde_json::to_value(&one).expect("json"),
        serde_json::to_value(&listed[0]).expect("json")
    );
    assert!(
        database
            .get_package(PackageId::new())
            .await
            .expect("get")
            .is_none()
    );

    let accounts = database.list_accounts().await.expect("accounts");
    let found = database
        .get_account(account.id)
        .await
        .expect("get")
        .expect("the account");
    assert_eq!(
        serde_json::to_value(&found).expect("json"),
        serde_json::to_value(&accounts[0]).expect("json")
    );
    assert!(
        database
            .get_account(rd_core::AccountId::new())
            .await
            .expect("get")
            .is_none()
    );
}

/// The commit of a torrent move (RD-1100-10): it switches only from the folder it was told, so
/// a repeated commit after a restart is harmless, and it records no sweep for the scheduler.
#[tokio::test]
async fn a_torrent_move_switches_the_package_folder_only_from_where_it_started() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("switch.sqlite"))
        .await
        .expect("database");
    let old = directory.path().join("downloads").join("Release");
    let new = directory.path().join("archive").join("Release");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "Release".to_owned(),
            destination: old.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");

    let switched = database
        .switch_package_destination(
            package.id,
            old.to_string_lossy().into_owned(),
            new.to_string_lossy().into_owned(),
        )
        .await
        .expect("switch");
    assert!(switched);
    let stored = database
        .get_package(package.id)
        .await
        .expect("read")
        .expect("package");
    assert_eq!(stored.destination, new.to_string_lossy());
    assert_eq!(
        database
            .package_previous_destination(package.id)
            .await
            .expect("previous destination"),
        None,
        "the torrent move owns its files; the scheduler's sweep must not run over them"
    );

    let again = database
        .switch_package_destination(
            package.id,
            old.to_string_lossy().into_owned(),
            directory
                .path()
                .join("elsewhere")
                .to_string_lossy()
                .into_owned(),
        )
        .await
        .expect("second switch");
    assert!(
        !again,
        "a package that no longer names `from` is left alone"
    );
    let stored = database
        .get_package(package.id)
        .await
        .expect("read")
        .expect("package");
    assert_eq!(stored.destination, new.to_string_lossy());
}
