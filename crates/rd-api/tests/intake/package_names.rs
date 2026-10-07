//! RD-1140-05: the package-name rules — "Tidy file names" for package names, global and per
//! category — on every way a package is created here: the LinkGrabber, an NZB import and a
//! direct download. A name the application derived is tidied, one somebody stated stays.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, patch_json, post_json, put_json, test_harness};
use serde_json::json;

/// Creates a storage root and a category below it over REST, returning the category's id.
async fn category(harness: &common::Harness, directory: &std::path::Path, name: &str) -> String {
    let root = directory.join("library");
    std::fs::create_dir_all(&root).expect("library directory");
    let root = dunce::canonicalize(&root).expect("canonical library directory");
    let roots = harness.database.list_storage_roots().await.expect("roots");
    let root_id = match roots.first() {
        Some(existing) => existing.id.to_string(),
        None => {
            let (status, created) = post_json(
                &harness.router,
                "/api/v1/storage-roots",
                json!({ "name": "names-library", "path": root.to_string_lossy(), "is_default": true }),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{created}");
            created["id"].as_str().expect("root id").to_owned()
        }
    };
    let (status, created) = post_json(
        &harness.router,
        "/api/v1/categories",
        json!({
            "name": name,
            "color": "#336699",
            "storage_root_id": root_id,
            "relative_path": name,
            "is_default": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("category id").to_owned()
}

/// Switches the global rules on over the settings document, the way the settings page does.
async fn global_rules(harness: &common::Harness, rules: serde_json::Value) {
    let (status, mut settings) = get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(
        settings["package_name_rules"],
        json!({
            "spaces_to_dots": false,
            "collapse_separators": false,
            "strip_bracket_tags": false,
            "lowercase": false
        }),
        "every rule is off until somebody switches it on"
    );
    settings["admin_login_disabled"] = json!(true);
    settings["package_name_rules"] = rules.clone();
    let (status, body) = put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, stored) = get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(stored["package_name_rules"], rules);
}

/// A LinkGrabber batch of two volumes of one release, named by nothing but the files.
fn release_batch(
    host: &str,
    category_id: Option<rd_core::CategoryId>,
    package_name: Option<&str>,
) -> rd_db::NewCollectorBatch {
    let files = ["Big Buck Bunny.part1.rar", "Big Buck Bunny.part2.rar"];
    rd_db::NewCollectorBatch {
        package_hints: Vec::new(),
        mirror_hints: Vec::new(),
        source: rd_core::IngressSource::Manual,
        source_label: None,
        package_name: package_name.map(str::to_owned),
        password: None,
        passwords: Vec::new(),
        category_id,
        priority: None,
        providers: vec![None; 2],
        file_names: files.iter().map(|file| Some((*file).to_owned())).collect(),
        sizes: vec![None; 2],
        requests: vec![None; 2],
        body_refs: vec![None; 2],
        urls: (1..=2)
            .map(|part| {
                format!("https://{host}/release.part{part}.rar")
                    .parse()
                    .expect("url")
            })
            .collect(),
        auto_check: false,
        source_attributes: Vec::new(),
    }
}

#[tokio::test]
async fn the_linkgrabber_shows_and_queues_the_tidied_name_of_each_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let plain = category(&harness, directory.path(), "names-plain").await;
    let lower = category(&harness, directory.path(), "names-lower").await;
    global_rules(
        &harness,
        json!({
            "spaces_to_dots": true,
            "collapse_separators": true,
            "strip_bracket_tags": false,
            "lowercase": false
        }),
    )
    .await;
    let (status, updated) = patch_json(
        &harness.router,
        &format!("/api/v1/categories/{lower}/postprocess"),
        json!({ "package_name_rules": { "lowercase": true } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["package_name_rules"]["lowercase"], true);
    assert_eq!(updated["package_name_rules"]["spaces_to_dots"], json!(null));

    let id = |value: &str| value.parse::<rd_core::CategoryId>().expect("category id");
    for (host, category, name) in [
        ("plain.example", id(&plain), None),
        ("lower.example", id(&lower), None),
        ("stated.example", id(&plain), Some("Hand Made Name")),
    ] {
        harness
            .database
            .add_collector_batch(release_batch(host, Some(category), name))
            .await
            .expect("batch");
    }

    let (status, packages) = get_json(&harness.router, "/api/v1/collector/packages").await;
    assert_eq!(status, StatusCode::OK, "{packages}");
    let packages = packages.as_array().expect("packages");
    assert_eq!(packages.len(), 3, "{packages:?}");
    let mut expected = Vec::new();
    for package in packages {
        let name = package["name"].as_str().expect("name");
        let queue_name = package["queue_name"].as_str();
        let wanted = if package["category_id"] == json!(lower) {
            assert!(name.contains(' '), "{package}");
            name.replace(' ', ".").to_lowercase()
        } else if package["auto_named"] == json!(true) {
            assert!(name.contains(' '), "{package}");
            name.replace(' ', ".")
        } else {
            // Stated: no queue name, and the name goes into the queue as it is.
            assert_eq!(name, "Hand Made Name");
            assert_eq!(queue_name, None, "{package}");
            name.to_owned()
        };
        if package["auto_named"] == json!(true) {
            assert_eq!(queue_name, Some(wanted.as_str()), "{package}");
        }
        expected.push(wanted);
    }

    let ids: Vec<&str> = packages
        .iter()
        .filter_map(|package| package["id"].as_str())
        .collect();
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/packages/enqueue",
        json!({ "ids": ids, "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let queued = harness.database.list_packages().await.expect("packages");
    let mut names: Vec<String> = queued.iter().map(|package| package.name.clone()).collect();
    names.sort();
    expected.sort();
    assert_eq!(names, expected);
    // The folder follows the name: it is created under the tidied one, never renamed later.
    for package in &queued {
        assert!(
            package.destination.ends_with(&package.name),
            "{} is not in a folder of its name: {}",
            package.name,
            package.destination
        );
    }
}

#[tokio::test]
async fn an_nzb_and_a_direct_download_are_tidied_unless_the_name_was_stated() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let lower = category(&harness, directory.path(), "names-nzb").await;
    global_rules(
        &harness,
        json!({
            "spaces_to_dots": true,
            "collapse_separators": true,
            "strip_bracket_tags": true,
            "lowercase": false
        }),
    )
    .await;
    let (status, body) = patch_json(
        &harness.router,
        &format!("/api/v1/categories/{lower}/postprocess"),
        json!({ "package_name_rules": { "lowercase": true } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let import = harness
        .database
        .add_nzb_import(rd_db::NewNzbImport {
            name: "Big Buck Bunny [1080p].nzb".to_owned(),
            sha256: "c5".repeat(32),
            category_id: Some(lower.parse().expect("category id")),
            priority: None,
            import_mode: rd_core::ImportMode::Review,
            source: rd_core::IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![rd_db::NewNzbFile {
                subject: "bunny.bin".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![rd_db::NewNzbSegment {
                    number: 1,
                    bytes: 128,
                    message_id: "bunny-1@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");
    let (status, package) = post_json(
        &harness.router,
        &format!("/api/v1/nzb/imports/{}/enqueue", import.id),
        json!({ "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{package}");
    assert_eq!(package["name"], "big.buck.bunny", "{package}");

    // A direct link without a name: derived from the address, and tidied.
    let (status, created) = post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": "https://example.invalid/Big_Buck__Bunny.mkv", "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    // The same with a name somebody typed: it stays.
    let (status, stated) = post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({
            "url": "https://example.invalid/other.mkv",
            "package_name": "Typed  Name_here",
            "paused": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{stated}");
    let packages = harness.database.list_packages().await.expect("packages");
    let name_of = |id: &serde_json::Value| {
        packages
            .iter()
            .find(|package| json!(package.id) == *id)
            .map(|package| package.name.clone())
            .expect("package")
    };
    assert_eq!(name_of(&created["package_id"]), "Big.Buck.Bunny");
    assert_eq!(name_of(&stated["package_id"]), "Typed  Name_here");
}

#[tokio::test]
async fn the_preview_layers_a_category_override_over_the_saved_switches() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    global_rules(
        &harness,
        json!({
            "spaces_to_dots": true,
            "collapse_separators": false,
            "strip_bracket_tags": false,
            "lowercase": false
        }),
    )
    .await;
    let (status, preview) = post_json(
        &harness.router,
        "/api/v1/postprocess/package-name-preview",
        json!({ "name": "Big Buck Bunny [1080p]", "rules": { "strip_bracket_tags": true } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["name"], "Big.Buck.Bunny.");
    // The folder rules then drop the trailing dot Windows cannot keep.
    assert_eq!(preview["folder"], "Big.Buck.Bunny");
    assert_eq!(preview["rules"]["spaces_to_dots"], true);
    assert_eq!(preview["rules"]["strip_bracket_tags"], true);

    let (_, unchanged) = post_json(
        &harness.router,
        "/api/v1/postprocess/package-name-preview",
        json!({ "name": "Big Buck Bunny", "rules": { "spaces_to_dots": false } }),
    )
    .await;
    assert_eq!(unchanged["name"], "Big Buck Bunny");
}

/// The regex pairs: refused with a stable code when they cannot run, applied after the switches
/// when they can, and a category's list replaces the global one.
#[tokio::test]
async fn regex_pairs_are_checked_on_save_and_run_after_the_switches() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let own = category(&harness, directory.path(), "names-regex").await;

    let (_, mut settings) = get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(settings["package_name_regex"], json!([]));
    settings["admin_login_disabled"] = json!(true);
    settings["package_name_rules"]["spaces_to_dots"] = json!(true);
    settings["package_name_regex"] = json!([{ "pattern": "(unclosed", "replacement": "" }]);
    let (status, refused) = put_json(&harness.router, "/api/v1/settings", settings.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "settings.package_name_regex_invalid");
    let many: Vec<_> = (0..11)
        .map(|_| json!({ "pattern": "a", "replacement": "" }))
        .collect();
    settings["package_name_regex"] = json!(many);
    let (status, refused) = put_json(&harness.router, "/api/v1/settings", settings.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "settings.package_name_regex_too_many");
    // A blank pattern is dropped rather than refused.
    settings["package_name_regex"] = json!([
        { "pattern": "\\.\\[1080p\\]$", "replacement": "" },
        { "pattern": "  ", "replacement": "x" }
    ]);
    let (status, saved) = put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved["package_name_regex"].as_array().map(Vec::len),
        Some(1)
    );

    let (status, refused) = patch_json(
        &harness.router,
        &format!("/api/v1/categories/{own}/postprocess"),
        json!({ "package_name_regex": [{ "pattern": "(?=x)", "replacement": "" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "settings.package_name_regex_invalid");
    let (status, updated) = patch_json(
        &harness.router,
        &format!("/api/v1/categories/{own}/postprocess"),
        json!({ "package_name_regex": [{ "pattern": "^(\\w+)\\.(\\w+)", "replacement": "${2}.${1}" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");

    // Global: dots, then the pair drops the tag. The category's own list replaces that pair.
    let (_, global) = post_json(
        &harness.router,
        "/api/v1/postprocess/package-name-preview",
        json!({ "name": "Big Buck Bunny [1080p]" }),
    )
    .await;
    assert_eq!(global["name"], "Big.Buck.Bunny");
    let (status, created) = post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({
            "url": "https://example.invalid/Big.Buck.Bunny.mkv",
            "category_id": own,
            "paused": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let packages = harness.database.list_packages().await.expect("packages");
    let package = packages
        .iter()
        .find(|package| json!(package.id) == created["package_id"])
        .expect("package");
    // The category's pair, not the global one: the first two words swap.
    assert_eq!(package.name, "Buck.Big.Bunny");
    let (_, preview) = post_json(
        &harness.router,
        "/api/v1/postprocess/package-name-preview",
        json!({
            "name": "Big Buck Bunny [1080p]",
            "regex": [{ "pattern": "^(\\w+)\\.(\\w+)", "replacement": "${2}.${1}" }]
        }),
    )
    .await;
    assert_eq!(preview["name"], "Buck.Big.Bunny.[1080p]");
}
