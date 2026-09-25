//! RD-080-11: Newznab and Torznab indexers as a subscription source.
//!
//! The query building and redaction are unit-tested in `rd-subscription`; what runs here is
//! the whole path — the stored key reaches the indexer, results become review items, an
//! indexer that refuses the query fails visibly rather than quietly, and **the key never
//! appears in anything a client or a log can see**.

mod common;

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use common::{get_json, post_json, put_json, test_router};
use serde_json::{Value, json};

const API_KEY: &str = "super-secret-indexer-key";

const RESULTS: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
  <channel>
    <title>Indexer</title>
    <item>
      <title>Example.Release.1080p</title>
      <guid>abc123</guid>
      <pubDate>Wed, 04 Feb 2026 13:00:00 GMT</pubDate>
      <!-- The shape a real indexer hands out: an API call with no telling extension, whose
           identity is stated in the enclosure type and nowhere else. -->
      <enclosure url="https://indexer.test/api?t=get&amp;id=abc123&amp;apikey=SECRET"
                 length="1073741824" type="application/x-nzb"/>
      <newznab:attr name="size" value="1073741824"/>
      <newznab:attr name="category" value="5030"/>
    </item>
  </channel>
</rss>"#;

const NZB: &str = r#"<?xml version="1.0"?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <head><meta type="password">from-nzb-head</meta></head>
  <file poster="poster" subject="Example.Release.1080p &quot;example.r00&quot; yEnc (1/1)">
    <groups><group>alt.binaries.test</group></groups>
    <segments><segment bytes="500000" number="1">msg-1@example</segment></segments>
  </file>
</nzb>"#;

const CAPS: &str = r#"<?xml version="1.0"?>
<caps>
  <server title="Example Indexer"/>
  <limits max="100" default="50"/>
  <searching>
    <search available="yes"/>
    <tv-search available="yes"/>
    <movie-search available="no"/>
  </searching>
  <categories>
    <category id="5000" name="TV">
      <subcat id="5040" name="HD"/>
    </category>
  </categories>
</caps>"#;

/// The release name the fake indexer states in `X-DNZB-Name`.
const HEADER_RELEASE: &str = "Header.Release.2160p-GRP";

const REFUSED: &str =
    r#"<?xml version="1.0"?><error code="100" description="Incorrect user credentials"/>"#;

/// One request's query parameters, in the order they arrived.
type RecordedQuery = Vec<(String, String)>;

/// Records every query it was sent, so what actually reached the wire can be asserted on.
#[derive(Clone)]
struct IndexerState {
    queries: Arc<Mutex<Vec<RecordedQuery>>>,
    refuse: bool,
}

async fn serve_api(
    State(state): State<IndexerState>,
    Query(params): Query<Vec<(String, String)>>,
) -> impl IntoResponse {
    let is_caps = params.iter().any(|(k, v)| k == "t" && v == "caps");
    state.queries.lock().expect("lock").push(params);
    let body = if state.refuse {
        REFUSED
    } else if is_caps {
        CAPS
    } else {
        RESULTS
    };
    (
        StatusCode::OK,
        [("content-type", "application/rss+xml")],
        body.to_owned(),
    )
}

async fn indexer_server(refuse: bool) -> (String, IndexerState) {
    let state = IndexerState {
        queries: Arc::new(Mutex::new(Vec::new())),
        refuse,
    };
    let app = Router::new()
        .route("/api", get(serve_api))
        // No `.nzb` extension, and the type is what identifies it — exactly how NZBHydra
        // and Prowlarr hand out download links.
        .route(
            "/getnzb",
            get(|| async { ([("content-type", "application/x-nzb")], NZB) }),
        )
        .route(
            // What plenty of indexers actually answer with. The document is an NZB; the
            // header says nothing useful about it.
            "/getnzb-untyped",
            get(|| async { ([("content-type", "application/octet-stream")], NZB) }),
        )
        .route(
            // What omgwtfnzbs and every other Newznab indexer answer with: the release name
            // in the headers SABnzbd established, and nothing in the address.
            "/getnzb-named",
            get(|| async {
                (
                    // No `Content-Disposition`: that one already names the link during the
                    // online check, and this is about the header that does not.
                    [
                        ("content-type", "application/x-nzb"),
                        ("x-dnzb-name", HEADER_RELEASE),
                    ],
                    NZB,
                )
            }),
        )
        .route(
            // A refusal dressed up as `200 OK`: the daily API limit, an expired key. The
            // body is not an NZB, and the headers are the only place the reason appears.
            "/getnzb-refused",
            get(|| async {
                (
                    [
                        ("content-type", "application/x-nzb"),
                        ("x-dnzb-rcode", "450"),
                        ("x-dnzb-rtext", "Request limit reached"),
                    ],
                    "<error code=\"450\" description=\"Request limit reached\"/>",
                )
            }),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}/api?t=search&cat=5030"), state)
}

fn body(url: &str) -> Value {
    json!({
        "name": "My Indexer",
        "url": url,
        "kind": "indexer",
        "enabled": true,
        "mode": "review",
        "interval_seconds": 3_600,
        "filters": {},
        "backlog": { "mode": "review_all" },
        "api_key": API_KEY
    })
}

async fn create_and_poll(router: &Router, url: &str) -> String {
    create_and_poll_with(router, body(url)).await
}

/// The same, for a subscription that needs more than the shared fixture says.
async fn create_and_poll_with(router: &Router, request: Value) -> String {
    let (status, created) = post_json(router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    let (status, _) = post_json(
        router,
        &format!("/api/v1/subscriptions/{id}/poll"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    for _ in 0..200 {
        let (_, runs) = get_json(router, &format!("/api/v1/subscriptions/{id}/runs")).await;
        if !runs.as_array().map(Vec::is_empty).unwrap_or(true) {
            return id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("poll never finished");
}

#[tokio::test]
async fn results_become_review_items_and_download_the_nzb() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, state) = indexer_server(false).await;
    let id = create_and_poll(&router, &url).await;

    let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{id}/items")).await;
    let items = items.as_array().expect("array");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["title"], "Example.Release.1080p");
    // The enclosure, which is the NZB, not the indexer's details page.
    assert_eq!(
        items[0]["url"],
        "https://indexer.test/api?t=get&id=abc123&apikey=SECRET"
    );
    assert_eq!(
        items[0]["media_type"], "application/x-nzb",
        "what the feed says it is, kept for the queue: {items:?}"
    );
    assert_eq!(items[0]["state"], "pending");

    // The saved search survived and the stored key was sent.
    let queries = state.queries.lock().expect("lock");
    let first = &queries[0];
    assert!(
        first.iter().any(|(k, v)| k == "cat" && v == "5030"),
        "{first:?}"
    );
    assert!(
        first.iter().any(|(k, v)| k == "t" && v == "search"),
        "{first:?}"
    );
    assert!(
        first.iter().any(|(k, v)| k == "apikey" && v == API_KEY),
        "the stored key should have reached the indexer"
    );
}

#[tokio::test]
async fn the_api_key_is_never_returned_by_the_api() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, _) = indexer_server(false).await;
    let id = create_and_poll(&router, &url).await;

    let (_, listed) = get_json(&router, "/api/v1/subscriptions").await;
    let (_, runs) = get_json(&router, &format!("/api/v1/subscriptions/{id}/runs")).await;
    let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{id}/items")).await;
    for payload in [&listed, &runs, &items] {
        let text = payload.to_string();
        assert!(!text.contains(API_KEY), "API key leaked: {text}");
        assert!(!text.contains("vault://"), "vault reference leaked: {text}");
    }
    // Only whether a key exists is disclosed.
    assert_eq!(listed[0]["has_secret"], true);
}

#[tokio::test]
async fn an_indexer_that_refuses_the_query_fails_visibly_and_without_the_key() {
    // A wrong key answers 200 with an error *document*. Without that check it would look
    // like an indexer that simply had nothing new — the worst possible failure mode.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, _) = indexer_server(true).await;
    let id = create_and_poll(&router, &url).await;

    let (_, listed) = get_json(&router, "/api/v1/subscriptions").await;
    let row = listed
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == id.as_str())
        .expect("row");
    let error = row["last_error"].as_str().unwrap_or_default();
    assert!(
        error.contains("Incorrect user credentials"),
        "expected the indexer's own wording, got {row}"
    );
    assert_eq!(row["consecutive_failures"], 1);
    // And the stored error must not carry the credential it was made with.
    assert!(
        !error.contains(API_KEY),
        "API key leaked into the error: {error}"
    );
}

#[tokio::test]
async fn an_unreachable_indexer_reports_a_redacted_address() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let id = create_and_poll(&router, "http://127.0.0.1:1/api?t=search").await;

    let (_, listed) = get_json(&router, "/api/v1/subscriptions").await;
    let row = listed
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == id.as_str())
        .expect("row");
    let error = row["last_error"].as_str().unwrap_or_default();
    assert!(!error.is_empty(), "{row}");
    // The address is in the message so the failure is diagnosable; the key is not.
    assert!(
        !error.contains(API_KEY),
        "API key leaked into the error: {error}"
    );
}

#[tokio::test]
async fn an_indexer_nzb_link_is_imported_rather_than_saved_to_disk() {
    // The failure this test exists for: routed as an ordinary link, the NZB *document*
    // lands in the download folder and nothing is ever fetched from Usenet. It also covers
    // the NZBHydra/Prowlarr shape — no `.nzb` extension, identified by content type alone.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, _) = indexer_server(false).await;
    let origin = base.split("/api").next().expect("origin").to_owned();

    let (status, _) = post_json(
        &router,
        "/api/v1/collector/batches",
        json!({
            "text": format!("{origin}/getnzb"),
            "source": "api",
            "source_label": null,
            "package_name": null,
            "password": null
        }),
    )
    .await;
    assert!(status.is_success());

    // Wait for the check to settle and re-route the link.
    let mut provider = String::new();
    for _ in 0..200 {
        let (_, candidates) = get_json(&router, "/api/v1/collector/candidates").await;
        if let Some(item) = candidates.as_array().and_then(|items| items.first()) {
            provider = item["provider"].as_str().unwrap_or_default().to_owned();
            if item["state"] != "checking" && item["state"] != "resolving" {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(
        provider, "nzb",
        "a link served as application/x-nzb must reach the import path"
    );

    // Enqueue the package and confirm an NZB import exists rather than an HTTP download.
    let (_, packages) = get_json(&router, "/api/v1/collector/packages").await;
    let package_id = packages[0]["id"].as_str().expect("package id").to_owned();
    let (status, response) = post_json(
        &router,
        &format!("/api/v1/collector/packages/{package_id}/enqueue"),
        json!({}),
    )
    .await;
    assert!(status.is_success(), "{response}");

    let (_, imports) = get_json(&router, "/api/v1/nzb/imports").await;
    let imports = imports.as_array().expect("imports");
    assert_eq!(imports.len(), 1, "expected one NZB import, got {imports:?}");
    assert_eq!(
        imports[0]["password"], "from-nzb-head",
        "the archive password embedded in the NZB must survive the import: {imports:?}"
    );
    let (_, packages) = get_json(&router, "/api/v1/packages").await;
    assert_eq!(
        packages[0]["password"], "from-nzb-head",
        "the extractor reads the password from the download package: {packages}"
    );
    // And the stored source must not carry the credential the link was fetched with.
    let source = imports[0]["source_path"].as_str().unwrap_or_default();
    assert!(!source.contains(API_KEY), "{source}");
}

#[tokio::test]
async fn capabilities_are_discovered_and_double_as_the_test_action() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, state) = indexer_server(false).await;
    let (status, created) = post_json(&router, "/api/v1/subscriptions", body(&url)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id");

    let (status, caps) = post_json(
        &router,
        &format!("/api/v1/subscriptions/{id}/caps"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{caps}");
    assert_eq!(caps["server"], "Example Indexer");
    assert_eq!(caps["limit_max"], 100);
    // Only what the indexer actually offers; `movie-search` is available="no".
    let searching: Vec<&str> = caps["searching"]
        .as_array()
        .expect("searching")
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert!(searching.contains(&"search"));
    assert!(searching.contains(&"tv-search"));
    assert!(!searching.contains(&"movie-search"));
    // The tree, flattened with parents so the UI can label "TV / HD".
    let categories = caps["categories"].as_array().expect("categories");
    assert_eq!(categories.len(), 2);
    let hd = categories
        .iter()
        .find(|category| category["id"] == "5040")
        .expect("5040");
    assert_eq!(hd["parent_id"], "5000");

    // A caps request carries the key and nothing else: a saved search's cat/q are dropped.
    let queries = state.queries.lock().expect("lock");
    let caps_query = queries
        .iter()
        .find(|query| query.iter().any(|(k, v)| k == "t" && v == "caps"))
        .expect("caps query");
    assert!(
        caps_query
            .iter()
            .any(|(k, v)| k == "apikey" && v == API_KEY)
    );
    assert!(!caps_query.iter().any(|(k, _)| k == "cat"));
}

#[tokio::test]
async fn asking_a_non_indexer_for_capabilities_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let mut request = body("https://example.test/feed.xml");
    request["kind"] = json!("feed");
    let (status, created) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id");

    let (status, response) = post_json(
        &router,
        &format!("/api/v1/subscriptions/{id}/caps"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    assert_eq!(response["code"], "subscription.not_an_indexer");
}

#[tokio::test]
async fn a_mapped_indexer_category_decides_where_a_release_lands() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, _) = indexer_server(false).await;

    // Two categories, so the mapping has something to choose between.
    let library = temp.path().join("library");
    std::fs::create_dir_all(&library).expect("library");
    let (status, root) = post_json(
        &router,
        "/api/v1/storage-roots",
        json!({ "name": "library", "path": library.to_string_lossy(), "is_default": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{root}");
    let root_id = root["id"].as_str().expect("root id").to_owned();

    let make_category = async |name: &str| -> String {
        let (status, created) = post_json(
            &router,
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
    };
    let tv_id = make_category("TV").await;
    let other_id = make_category("Other").await;

    let mut request = body(&url);
    request["mode"] = json!("auto_queue");
    // `review_all` deliberately never auto-queues on a first poll, so a cutoff old enough to
    // admit the fixture is what actually exercises the queueing path.
    request["backlog"] = json!({ "mode": "since", "since": "2020-01-01T00:00:00Z" });
    request["category_id"] = json!(other_id);
    // The fixture's release is filed under 5030.
    request["category_map"] = json!([{ "source_category": "5030", "category_id": tv_id }]);
    let (status, created) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["category_map"][0]["source_category"], "5030");

    let id = created["id"].as_str().expect("id").to_owned();
    let (status, _) = post_json(
        &router,
        &format!("/api/v1/subscriptions/{id}/poll"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    for _ in 0..200 {
        let (_, runs) = get_json(&router, &format!("/api/v1/subscriptions/{id}/runs")).await;
        if !runs.as_array().map(Vec::is_empty).unwrap_or(true) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{id}/items")).await;
    let items = items.as_array().expect("items");
    assert_eq!(items[0]["source_category"], "5030");

    // The queued package landed in the mapped category, not the subscription's default.
    for _ in 0..200 {
        let (_, packages) = get_json(&router, "/api/v1/collector/packages").await;
        if let Some(package) = packages.as_array().and_then(|list| list.first()) {
            assert_eq!(
                package["category_id"],
                tv_id.as_str(),
                "release should have gone to the mapped category, not the default: {package}"
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("nothing reached the collector");
}

/// Queueing a reviewed item puts it in the LinkGrabber.
///
/// Review mode is the form's default, so this is the ordinary way an indexer hit becomes a
/// download. Marking the row queued is not enough on its own: until the item is handed to the
/// intake it exists only in the subscription's own table, and the LinkGrabber stays empty.
#[tokio::test]
async fn queueing_a_reviewed_item_hands_it_to_the_collector() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, _) = indexer_server(false).await;
    let id = create_and_poll(&router, &url).await;

    let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{id}/items")).await;
    let item_id = items[0]["id"].as_str().expect("item id").to_owned();
    assert_eq!(items[0]["state"], "pending");

    let (status, body) = put_json(
        &router,
        &format!("/api/v1/subscriptions/items/{item_id}"),
        json!({ "state": "queued" }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    for _ in 0..200 {
        let (_, packages) = get_json(&router, "/api/v1/collector/packages").await;
        if packages.as_array().is_some_and(|list| !list.is_empty()) {
            let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{id}/items")).await;
            assert_eq!(items[0]["state"], "queued", "{items}");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the queued item never reached the collector");
}

/// An indexer hit is imported as an NZB, not fetched as a file.
///
/// The address is an API call: it carries no extension, and an indexer that rate-limits
/// unauthenticated calls answers a HEAD with an error rather than a content type. Routed as an
/// ordinary link it is downloaded as a document — or, as reported, fails mid-response — and
/// nothing is ever fetched from Usenet. The feed said what it was; that is what decides.
#[tokio::test]
async fn a_queued_indexer_hit_is_routed_by_what_the_feed_declared() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, _) = indexer_server(false).await;
    let id = create_and_poll(&router, &url).await;

    let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{id}/items")).await;
    let item_id = items[0]["id"].as_str().expect("item id").to_owned();
    let (status, body) = put_json(
        &router,
        &format!("/api/v1/subscriptions/items/{item_id}"),
        json!({ "state": "queued" }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    for _ in 0..200 {
        let (_, candidates) = get_json(&router, "/api/v1/collector/candidates").await;
        if let Some(candidate) = candidates.as_array().and_then(|list| list.first()) {
            assert_eq!(
                candidate["provider"], "nzb",
                "the link is imported into the Usenet queue, not downloaded: {candidate}"
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the queued item never reached the collector");
}

/// A container is recognised from the document when the server does not say what it is.
///
/// The content type is the better signal and is tried first, but it belongs to the server, and
/// an indexer that answers `application/octet-stream` used to leave the link to be downloaded
/// as a document into the download folder — the exact outcome this path exists to prevent.
#[tokio::test]
async fn an_nzb_is_recognised_even_when_the_server_does_not_say_so() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, _) = indexer_server(false).await;
    let origin = base.split("/api").next().expect("origin").to_owned();

    let (status, _) = post_json(
        &router,
        "/api/v1/collector/batches",
        json!({
            "text": format!("{origin}/getnzb-untyped"),
            "source": "api",
            "source_label": null,
            "package_name": null,
            "password": null
        }),
    )
    .await;
    assert!(status.is_success());

    for _ in 0..200 {
        let (_, candidates) = get_json(&router, "/api/v1/collector/candidates").await;
        if let Some(item) = candidates.as_array().and_then(|list| list.first())
            && item["state"] != "checking"
            && item["state"] != "resolving"
        {
            assert_eq!(
                item["provider"], "nzb",
                "recognised from the document itself: {item}"
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the check never settled");
}

/// A hit keeps the name the feed gave it, all the way into the queue.
///
/// Reported from a live instance: two omgwtfnzbs hits arrived as packages both called
/// `api.omgwtfnzbs.org` holding one link called `api`. Every hit of an indexer is fetched
/// from the same `…/api` address, so the address can name none of them — and the feed had
/// said what each one was all along.
#[tokio::test]
async fn a_queued_indexer_hit_keeps_the_release_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, _) = indexer_server(false).await;
    let id = create_and_poll(&router, &url).await;

    let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{id}/items")).await;
    let item_id = items[0]["id"].as_str().expect("item id").to_owned();
    let (status, body) = put_json(
        &router,
        &format!("/api/v1/subscriptions/items/{item_id}"),
        json!({ "state": "queued" }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    for _ in 0..200 {
        let (_, candidates) = get_json(&router, "/api/v1/collector/candidates").await;
        if let Some(candidate) = candidates.as_array().and_then(|list| list.first()) {
            assert_eq!(
                candidate["file_name"], "Example.Release.1080p",
                "the link is named after the release, not the API endpoint: {candidate}"
            );
            let (_, packages) = get_json(&router, "/api/v1/collector/packages").await;
            assert_eq!(
                packages[0]["name"], "Example.Release.1080p",
                "and so is its package: {packages}"
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the queued item never reached the collector");
}

/// Without a feed to name it, the import takes the name the indexer answers with.
///
/// A link pasted by hand has only its address, which for an indexer is `…/api`. A
/// `Content-Disposition` already names the link during the online check; `X-DNZB-Name` is
/// not sent on a HEAD-shaped probe and is only seen when the document is fetched, which is
/// why the import path reads it for itself — as SABnzbd does.
#[tokio::test]
async fn an_import_is_named_by_the_headers_the_indexer_answers_with() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, _) = indexer_server(false).await;
    let origin = base.split("/api").next().expect("origin").to_owned();

    let (status, _) = post_json(
        &router,
        "/api/v1/collector/batches",
        json!({
            "text": format!("{origin}/getnzb-named"),
            "source": "api",
            "source_label": null,
            "package_name": null,
            "password": null
        }),
    )
    .await;
    assert!(status.is_success());
    let package_id = settle(&router).await;
    let (status, response) = post_json(
        &router,
        &format!("/api/v1/collector/packages/{package_id}/enqueue"),
        json!({}),
    )
    .await;
    assert!(status.is_success(), "{response}");

    let (_, imports) = get_json(&router, "/api/v1/nzb/imports").await;
    assert_eq!(
        imports[0]["name"], HEADER_RELEASE,
        "the import is named after the release the indexer stated: {imports}"
    );
}

/// An indexer refusing inside a `200 OK` is reported as the refusal it is.
///
/// The daily API limit is the usual one. The body is an error document, so parsing it used
/// to fail with "that NZB is invalid" — which points at the release rather than at the
/// indexer, and sends the user looking in the wrong place.
#[tokio::test]
async fn a_refusal_dressed_up_as_success_is_reported_as_one() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, _) = indexer_server(false).await;
    let origin = base.split("/api").next().expect("origin").to_owned();

    let (status, _) = post_json(
        &router,
        "/api/v1/collector/batches",
        json!({
            "text": format!("{origin}/getnzb-refused"),
            "source": "api",
            "source_label": null,
            "package_name": null,
            "password": null
        }),
    )
    .await;
    assert!(status.is_success());
    let package_id = settle(&router).await;
    let (status, response) = post_json(
        &router,
        &format!("/api/v1/collector/packages/{package_id}/enqueue"),
        json!({}),
    )
    .await;
    assert!(!status.is_success(), "{response}");
    assert_eq!(response["code"], "collector.nzb_rejected", "{response}");
    assert!(
        response["error"]
            .as_str()
            .unwrap_or_default()
            .contains("Request limit reached"),
        "the indexer's own wording reaches the user: {response}"
    );
}

/// Waits for the online check of the one submitted link to settle, and answers with the
/// package it ended up in — which the check itself can still move once it learns the name.
async fn settle(router: &Router) -> String {
    for _ in 0..200 {
        let (_, candidates) = get_json(router, "/api/v1/collector/candidates").await;
        if let Some(item) = candidates.as_array().and_then(|list| list.first())
            && item["state"] != "checking"
            && item["state"] != "resolving"
            && let Some(package_id) = item["package_id"].as_str()
        {
            // The regrouping that follows the check runs after the candidate is recorded,
            // so read the package back until it holds the link.
            let (_, packages) = get_json(router, "/api/v1/collector/packages").await;
            if packages
                .as_array()
                .is_some_and(|list| list.iter().any(|package| package["id"] == package_id))
            {
                return package_id.to_owned();
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the check never settled");
}

/// Categories can be asked for before there is a subscription to ask for.
///
/// The `{id}` route resolves the key out of the vault, so mapping categories used to mean
/// saving the indexer and reopening it — reported from use as exactly that annoyance.
#[tokio::test]
async fn capabilities_can_be_probed_before_the_subscription_exists() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, state) = indexer_server(false).await;

    let (status, caps) = post_json(
        &router,
        "/api/v1/subscriptions/caps",
        json!({ "url": url, "api_key": API_KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{caps}");
    assert_eq!(caps["server"], "Example Indexer");
    assert_eq!(caps["categories"].as_array().expect("categories").len(), 2);

    // Nothing was stored on the way: no subscription, and therefore no key in the vault.
    let (status, listed) = get_json(&router, "/api/v1/subscriptions").await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed.as_array().expect("array").is_empty());

    let queries = state.queries.lock().expect("lock");
    assert!(
        queries
            .iter()
            .any(|query| query.iter().any(|(k, v)| k == "t" && v == "caps"))
    );
}

#[tokio::test]
async fn probing_without_a_key_is_refused_before_anything_is_requested() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, state) = indexer_server(false).await;

    let (status, problem) = post_json(
        &router,
        "/api/v1/subscriptions/caps",
        json!({ "url": url, "api_key": "   " }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "subscription.api_key_missing");
    assert!(state.queries.lock().expect("lock").is_empty());
}

/// The chosen categories reach the indexer as `cat`, rather than only sorting what came back.
#[tokio::test]
async fn a_subscription_asks_only_for_the_categories_it_wants() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, state) = indexer_server(false).await;

    // The shared fixture's address already carries `cat=5030` — a saved search, which wins.
    // This subscription is written with a bare address, the way somebody pasting the indexer's
    // API root would have it.
    let bare = url.split('?').next().expect("address").to_owned();
    let mut request = body(&bare);
    request["source_categories"] = json!(["5000", "5040"]);
    let (status, created) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["source_categories"], json!(["5000", "5040"]));
    let id = created["id"].as_str().expect("id");

    let (status, _) = post_json(
        &router,
        &format!("/api/v1/subscriptions/{id}/poll"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let queries = state.queries.lock().expect("lock");
    let search = queries
        .iter()
        .find(|query| query.iter().any(|(k, v)| k == "t" && v == "search"))
        .expect("a search query");
    assert!(
        search.iter().any(|(k, v)| k == "cat" && v == "5000,5040"),
        "the poll asked for everything: {search:?}"
    );
}

/// Duplicates and blank entries are dropped; the stored list is what gets sent.
#[tokio::test]
async fn requested_categories_are_trimmed_and_deduplicated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, _state) = indexer_server(false).await;

    let mut request = body(&url);
    request["source_categories"] = json!([" 5040 ", "5040", "", "5000"]);
    let (status, created) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["source_categories"], json!(["5040", "5000"]));
}

/// A pasted search that already names its categories keeps them.
///
/// The fixture's address carries `cat=5030`, which is how an indexer's own RSS button hands one
/// out. Whoever pasted that meant it, so a stored choice must not quietly widen or narrow it.
#[tokio::test]
async fn an_address_that_already_names_categories_is_left_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, state) = indexer_server(false).await;

    let mut request = body(&url);
    request["source_categories"] = json!(["5000"]);
    let (status, created) = post_json(&router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id");

    let (status, _) = post_json(
        &router,
        &format!("/api/v1/subscriptions/{id}/poll"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let queries = state.queries.lock().expect("lock");
    let search = queries
        .iter()
        .find(|query| query.iter().any(|(k, v)| k == "t" && v == "search"))
        .expect("a search query");
    let cats: Vec<&str> = search
        .iter()
        .filter(|(k, _)| k == "cat")
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(
        cats,
        vec!["5030"],
        "the address's own categories were overwritten"
    );
}

/// RD-106-10: a title filter applies to indexer hits, and the rejected ones keep their reason.
///
/// Reported as "the list is no longer filtered by what I typed". It is filtered: `evaluate`
/// runs in the poll service, above the adapter trait, so an indexer poll passes through the
/// same filter a feed does — this test is the answer to that question rather than a repeat of
/// it. What the interface did not do was say so, because an accepted and a rejected hit stood
/// in one list with nothing between them but a small grey label.
#[tokio::test]
async fn a_title_filter_applies_on_the_indexer_path_and_keeps_its_reason() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, state) = indexer_server(false).await;

    // The fixture's one hit is `Example.Release.1080p`.
    let mut wanted = body(&url);
    wanted["name"] = json!("Matching filter");
    wanted["filters"] = json!({ "title_contains": ["example"] });
    let matching = create_and_poll_with(&router, wanted).await;

    let mut unwanted = body(&url);
    unwanted["name"] = json!("Filter that excludes it");
    unwanted["filters"] = json!({ "title_contains": ["german"] });
    let rejecting = create_and_poll_with(&router, unwanted).await;

    let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{matching}/items")).await;
    let items = items.as_array().expect("array");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["state"], "pending", "{items:?}");

    let (_, items) = get_json(&router, &format!("/api/v1/subscriptions/{rejecting}/items")).await;
    let items = items.as_array().expect("array");
    // Stored rather than dropped: an unwritten hit would be rediscovered on every poll, and
    // the reason is what makes an over-strict filter visible instead of an empty channel.
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["state"], "skipped", "{items:?}");
    assert_eq!(items[0]["reason"], "title_not_included", "{items:?}");

    // The run counts the same decision, which is what the interface reads back.
    let (_, runs) = get_json(&router, &format!("/api/v1/subscriptions/{rejecting}/runs")).await;
    let runs = runs.as_array().expect("array");
    assert_eq!(runs[0]["found"], 1, "{runs:?}");
    assert_eq!(runs[0]["accepted"], 0, "{runs:?}");
    assert_eq!(runs[0]["skipped"], 1, "{runs:?}");

    // And the filter stayed here: no pattern was handed to the indexer as a search term.
    let queries = state.queries.lock().expect("lock");
    for query in queries.iter() {
        assert!(
            query.iter().all(|(key, _)| key != "q"),
            "a filter pattern reached the indexer: {query:?}"
        );
    }
}
