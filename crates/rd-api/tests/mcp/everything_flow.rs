//! The listing tools and the tools that take their ids, used together (RD-120-32).
//!
//! RD-120-29 left mirror handling out because pinning names a candidate and no tool listed
//! candidates: a tool whose argument nothing can supply. Each step below takes its id from a
//! listing tool's answer — never from the database behind it — so a listing that stopped
//! handing out the id a later tool needs fails here.

use super::{API_BEARER, handshake, installation, ok, refused_with};

fn ids_of(list: &serde_json::Value) -> Vec<String> {
    list["items"]
        .as_array()
        .expect("a paged list")
        .iter()
        .map(|row| row["id"].as_str().expect("an id").to_owned())
        .collect()
}

#[tokio::test]
async fn the_linkgrabber_is_handled_link_by_link() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    ok(
        &router,
        &session,
        "collect_links",
        serde_json::json!({
            "text": "https://example.invalid/one.bin\nhttps://example.invalid/two.bin"
        }),
    )
    .await;
    let listed = ok(&router, &session, "list_candidates", serde_json::json!({})).await;
    let ids = ids_of(&listed);
    assert_eq!(ids.len(), 2, "{listed}");

    let renamed = ok(
        &router,
        &session,
        "update_candidate",
        serde_json::json!({ "id": ids[0], "file_name": "renamed.bin" }),
    )
    .await;
    assert_eq!(renamed["file_name"], "renamed.bin", "{renamed}");

    let package = ok(
        &router,
        &session,
        "move_candidates",
        serde_json::json!({ "ids": ids, "new_package_name": "Together" }),
    )
    .await;
    assert!(package.get("password").is_none(), "{package}");
    let package_id = package["id"].as_str().expect("package id").to_owned();

    let reversed = vec![ids[1].clone(), ids[0].clone()];
    ok(
        &router,
        &session,
        "reorder_candidates",
        serde_json::json!({ "package_id": package_id, "ids": reversed }),
    )
    .await;
    let members = ok(
        &router,
        &session,
        "list_candidates",
        serde_json::json!({ "package_id": package_id }),
    )
    .await;
    assert_eq!(ids_of(&members), reversed, "{members}");

    // Neither link is part of a mirror group, and the route says so by the stable code — which
    // is the proof that the id reached it.
    assert_eq!(
        refused_with(
            &router,
            &session,
            "set_candidate_mirror",
            serde_json::json!({ "id": ids[0], "action": "pin" }),
        )
        .await,
        "collector.mirror_not_grouped"
    );

    let changed = ok(
        &router,
        &session,
        "update_collector_package",
        serde_json::json!({ "id": package_id, "body": { "priority": "high" } }),
    )
    .await;
    assert_eq!(changed["priority"], "high", "{changed}");
    ok(
        &router,
        &session,
        "reorder_collector",
        serde_json::json!({ "entries": [{ "kind": "collector", "id": package_id }] }),
    )
    .await;

    let removed = ok(
        &router,
        &session,
        "delete_candidates",
        serde_json::json!({ "ids": ids }),
    )
    .await;
    assert_eq!(
        removed["done"].as_array().map(Vec::len),
        Some(2),
        "{removed}"
    );
}

#[tokio::test]
async fn an_nzb_is_reviewed_queued_and_its_package_handled() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    const NZB: &[u8] = include_bytes!("../../../../testfile/sabnzbd-test-download-100MB.nzb");

    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    ok(
        &router,
        &session,
        "import_nzb",
        serde_json::json!({ "file_name": "test.nzb", "content": STANDARD.encode(NZB) }),
    )
    .await;
    let imports = ok(&router, &session, "list_nzb_imports", serde_json::json!({})).await;
    let import_id = imports[0]["id"].as_str().expect("an import").to_owned();
    assert!(imports[0].get("password").is_none(), "{imports}");

    let files = ok(
        &router,
        &session,
        "get_nzb_import",
        serde_json::json!({ "id": import_id, "view": "files" }),
    )
    .await;
    assert!(
        files.as_array().is_some_and(|files| !files.is_empty()),
        "{files}"
    );
    let changed = ok(
        &router,
        &session,
        "update_nzb_import",
        serde_json::json!({ "id": import_id, "body": { "priority": "low" } }),
    )
    .await;
    assert_eq!(changed["priority"], "low", "{changed}");
    ok(
        &router,
        &session,
        "reorder_collector",
        serde_json::json!({ "entries": [{ "kind": "nzb", "id": import_id }] }),
    )
    .await;

    let package = ok(
        &router,
        &session,
        "enqueue_nzb_import",
        serde_json::json!({ "id": import_id, "paused": true }),
    )
    .await;
    let package_id = package["id"].as_str().expect("package id").to_owned();

    let packages = ok(&router, &session, "list_packages", serde_json::json!({})).await;
    assert!(
        packages["items"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["id"] == package_id.as_str())),
        "{packages}"
    );
    ok(
        &router,
        &session,
        "reorder_packages",
        serde_json::json!({ "ids": [package_id] }),
    )
    .await;
    let changed = ok(
        &router,
        &session,
        "update_package",
        serde_json::json!({ "id": package_id, "body": { "priority": "high" } }),
    )
    .await;
    assert_eq!(changed["priority"], "high", "{changed}");
    ok(
        &router,
        &session,
        "get_package_postprocess",
        serde_json::json!({ "id": package_id }),
    )
    .await;

    let downloads = ok(
        &router,
        &session,
        "list_downloads",
        serde_json::json!({ "package_id": package_id, "limit": 200 }),
    )
    .await;
    let mut members: Vec<String> = downloads["items"]
        .as_array()
        .expect("downloads")
        .iter()
        .filter(|row| row["package_id"] == package_id.as_str())
        .map(|row| row["id"].as_str().expect("id").to_owned())
        .collect();
    assert!(!members.is_empty(), "{downloads}");
    members.reverse();
    ok(
        &router,
        &session,
        "reorder_downloads",
        serde_json::json!({ "package_id": package_id, "ids": members }),
    )
    .await;
}

#[tokio::test]
async fn a_torrent_a_category_a_site_rule_and_a_storage_target_are_reached_by_listed_ids() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    const TORRENT: &[u8] = include_bytes!("../../../../testfile/big-buck-bunny.torrent");

    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;

    // A torrent's candidate announces its view, and the view answers for that id.
    ok(
        &router,
        &session,
        "import_torrent",
        serde_json::json!({ "file_name": "bbb.torrent", "content": STANDARD.encode(TORRENT) }),
    )
    .await;
    let listed = ok(&router, &session, "list_candidates", serde_json::json!({})).await;
    let torrent = listed["items"]
        .as_array()
        .and_then(|rows| {
            rows.iter().find(|row| {
                row["details"]
                    .as_array()
                    .is_some_and(|d| d.contains(&"torrent".into()))
            })
        })
        .unwrap_or_else(|| panic!("no torrent candidate in {listed}"));
    let detail = ok(
        &router,
        &session,
        "get_candidate_details",
        serde_json::json!({ "id": torrent["id"], "view": "torrent" }),
    )
    .await;
    assert!(detail.is_object(), "{detail}");

    // A category's post-processing and seeding, by the id the configuration list hands out.
    let root = ok(
        &router,
        &session,
        "create_storage_root",
        serde_json::json!({ "name": "Downloads", "path": directory.path().join("dl") }),
    )
    .await;
    ok(
        &router,
        &session,
        "create_category",
        serde_json::json!({
            "name": "Films", "color": "#336699",
            "storage_root_id": root["id"], "relative_path": "films"
        }),
    )
    .await;
    let categories = ok(
        &router,
        &session,
        "list_configuration",
        serde_json::json!({ "section": "categories" }),
    )
    .await;
    let category = categories
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["name"] == "Films"))
        .unwrap_or_else(|| panic!("no category in {categories}"));
    ok(
        &router,
        &session,
        "list_postprocess_options",
        serde_json::json!({ "kind": "scripts" }),
    )
    .await;
    let changed = ok(
        &router,
        &session,
        "update_category_postprocess",
        serde_json::json!({ "id": category["id"], "body": { "recursive_unpack": true } }),
    )
    .await;
    assert_eq!(changed["recursive_unpack"], true, "{changed}");
    ok(
        &router,
        &session,
        "set_category_seeding",
        serde_json::json!({ "id": category["id"], "body": { "ratio": 2.0 } }),
    )
    .await;

    // A site rule written, found in the list, rewritten and removed.
    let rule = |name: &str| {
        serde_json::json!({
            "id": "mcp-board", "name": name, "group": "board", "version": 1,
            "match": { "hosts": ["example.org"], "paths": ["^/release/"] },
            "steps": [
                { "kind": "fetch" },
                { "kind": "regex", "pattern": "href=\"(https?://[^\"]+)\"", "into": "links", "all": true }
            ],
            "package": { "from": "title" },
            "probe": "https://example.org/release/1",
            "checked": "2026-09-23"
        })
    };
    ok(
        &router,
        &session,
        "create_site_rule",
        serde_json::json!({ "rule": rule("Board"), "enabled": true }),
    )
    .await;
    let rules = ok(&router, &session, "list_site_rules", serde_json::json!({})).await;
    let own = rules["rules"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == "mcp-board"))
        .unwrap_or_else(|| panic!("the rule is not listed: {rules}"));
    ok(
        &router,
        &session,
        "update_site_rule",
        serde_json::json!({ "id": own["id"], "rule": rule("Board, renamed"), "enabled": true }),
    )
    .await;
    ok(
        &router,
        &session,
        "delete_site_rule",
        serde_json::json!({ "id": own["id"] }),
    )
    .await;

    // A storage target, by the name the capacity list gives it.
    let capacity = ok(
        &router,
        &session,
        "get_storage_capacity",
        serde_json::json!({}),
    )
    .await;
    let target = capacity["roots"][0]["target"]
        .as_str()
        .unwrap_or_else(|| panic!("no storage target in {capacity}"))
        .to_owned();
    let resumed = ok(
        &router,
        &session,
        "resume_storage_target",
        serde_json::json!({ "target": target }),
    )
    .await;
    assert_eq!(resumed["code"], "storage.not_blocked", "{resumed}");
}
