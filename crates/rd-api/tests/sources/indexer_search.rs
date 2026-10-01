//! RD-180-19 and RD-180-20: indexers defined once, searched from the LinkGrabber, their hits
//! grabbed into NZB imports, and an indexer subscription that takes one over with a search term.
//!
//! A fake Newznab server on loopback answers everything; nothing here reaches a real indexer.
//! What is held: the parameters that reach the wire, a refusal document turned into a stable
//! code, and -- above all -- **the API key never in an answer**: not in the indexer list, not
//! in a hit's address, not in an error.

use crate::common;

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use common::{WAIT, delete_json, eventually, get_json, post_json, put_json, test_router};
use serde_json::{Value, json};

const API_KEY: &str = "rd-180-19-indexer-key-5f3a";

const NZB: &str = r#"<?xml version="1.0"?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <file poster="poster" subject="Some.Show.S01E01.1080p &quot;show.r00&quot; yEnc (1/1)">
    <groups><group>alt.binaries.test</group></groups>
    <segments><segment bytes="500000" number="1">msg-1@example</segment></segments>
  </file>
</nzb>"#;

const CAPS: &str = r#"<?xml version="1.0"?>
<caps>
  <server title="Fake Indexer"/>
  <searching><search available="yes"/></searching>
  <categories><category id="5000" name="TV"><subcat id="5040" name="HD"/></category></categories>
</caps>"#;

type RecordedQuery = Vec<(String, String)>;

#[derive(Clone)]
struct Fake {
    base: Arc<Mutex<String>>,
    queries: Arc<Mutex<Vec<RecordedQuery>>>,
    downloads: Arc<Mutex<Vec<RecordedQuery>>>,
}

fn value<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// The search answer: the key the request carried is echoed into the enclosure address and the
/// guid, which is exactly how an indexer hands one out.
fn results(base: &str, key: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
  <channel>
    <newznab:response offset="0" total="2"/>
    <item>
      <title>Some.Show.S01E01.1080p</title>
      <guid>{base}/details/abc?apikey={key}</guid>
      <pubDate>Tue, 29 Sep 2026 10:00:00 GMT</pubDate>
      <enclosure url="{base}/getnzb?id=abc&amp;apikey={key}" length="1500000"
                 type="application/x-nzb"/>
      <newznab:attr name="category" value="5040"/>
      <newznab:attr name="grabs" value="12"/>
      <newznab:attr name="password" value="1"/>
    </item>
    <item>
      <title>Some.Show.S01E02.720p</title>
      <guid>def</guid>
      <enclosure url="{base}/getnzb?id=def&amp;apikey={key}" length="700000"
                 type="application/x-nzb"/>
      <newznab:attr name="category" value="5030"/>
    </item>
  </channel>
</rss>"#
    )
}

fn refusal(code: &str, description: &str) -> String {
    format!(r#"<?xml version="1.0"?><error code="{code}" description="{description}"/>"#)
}

async fn serve_api(
    State(fake): State<Fake>,
    Query(params): Query<Vec<(String, String)>>,
) -> impl IntoResponse {
    fake.queries.lock().expect("lock").push(params.clone());
    let key = value(&params, "apikey").unwrap_or_default().to_owned();
    let body = if value(&params, "t") == Some("caps") {
        CAPS.to_owned()
    } else if key != API_KEY {
        refusal("100", "Incorrect user credentials")
    } else if value(&params, "q") == Some("refuse-201") {
        refusal("201", "Incorrect parameter (search too short)")
    } else if value(&params, "q") == Some("refuse-501") {
        refusal("501", "Download limit reached")
    } else {
        let base = fake.base.lock().expect("lock").clone();
        results(&base, &key)
    };
    (
        StatusCode::OK,
        [("content-type", "application/rss+xml")],
        body,
    )
}

async fn serve_nzb(
    State(fake): State<Fake>,
    Query(params): Query<Vec<(String, String)>>,
) -> impl IntoResponse {
    fake.downloads.lock().expect("lock").push(params.clone());
    if value(&params, "apikey") != Some(API_KEY) {
        return (
            [("content-type", "application/xml")],
            refusal("100", "Incorrect user credentials"),
        );
    }
    ([("content-type", "application/x-nzb")], NZB.to_owned())
}

/// A fake indexer on loopback; answers its base address (`http://127.0.0.1:<port>`).
async fn fake_indexer() -> (String, Fake) {
    let fake = Fake {
        base: Arc::new(Mutex::new(String::new())),
        queries: Arc::new(Mutex::new(Vec::new())),
        downloads: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/api", get(serve_api))
        .route("/getnzb", get(serve_nzb))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let base = format!("http://{address}");
    fake.base.lock().expect("lock").clone_from(&base);
    (base, fake)
}

async fn create_indexer(router: &Router, base: &str) -> Value {
    let (status, created) = post_json(
        router,
        "/api/v1/indexers",
        json!({
            "name": "Fake",
            "url": format!("{base}/api"),
            "api_key": API_KEY,
            "categories": ["5040"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created
}

fn assert_no_key(payload: &Value) {
    let text = payload.to_string();
    assert!(!text.contains(API_KEY), "API key leaked: {text}");
    assert!(!text.contains("vault://"), "vault reference leaked: {text}");
}

#[tokio::test]
async fn an_indexer_is_defined_edited_and_removed_without_its_key_ever_coming_back() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, _) = fake_indexer().await;

    let (status, refused) = post_json(
        &router,
        "/api/v1/indexers",
        json!({ "name": "No key", "url": format!("{base}/api") }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused["code"], "indexer.api_key_missing");
    let (status, refused) = post_json(
        &router,
        "/api/v1/indexers",
        json!({ "name": "Ftp", "url": "ftp://indexer.test/api", "api_key": API_KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["code"], "indexer.url_scheme");

    let created = create_indexer(&router, &base).await;
    assert_no_key(&created);
    assert_eq!(created["has_secret"], true);
    assert_eq!(created["categories"], json!(["5040"]));
    let id = created["id"].as_str().expect("id");

    let (status, duplicate) = post_json(
        &router,
        "/api/v1/indexers",
        json!({ "name": "Fake", "url": format!("{base}/api"), "api_key": "other" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(duplicate["code"], "indexer.name_taken");

    // An edit without a key keeps the stored one: the test still passes afterwards.
    let (status, edited) = put_json(
        &router,
        &format!("/api/v1/indexers/{id}"),
        json!({ "name": "Renamed", "url": format!("{base}/api"), "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["name"], "Renamed");
    assert_eq!(edited["enabled"], false);
    assert_eq!(edited["has_secret"], true);
    let (status, caps) =
        post_json(&router, &format!("/api/v1/indexers/{id}/caps"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{caps}");
    assert_eq!(caps["server"], "Fake Indexer");

    let (_, listed) = get_json(&router, "/api/v1/indexers").await;
    assert_no_key(&listed);
    assert_eq!(listed.as_array().expect("list").len(), 1);

    let (status, _) = delete_json(&router, &format!("/api/v1/indexers/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, missing) = delete_json(&router, &format!("/api/v1/indexers/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["code"], "indexer.not_found");
}

#[tokio::test]
async fn a_search_sends_its_parameters_once_and_answers_hits_without_the_key() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, fake) = fake_indexer().await;
    let indexer = create_indexer(&router, &base).await;

    let (status, answer) = post_json(
        &router,
        "/api/v1/indexers/search",
        json!({
            "query": "some show !cam",
            "max_age_days": 30,
            "hide_passworded": true,
            "limit": 50,
            "offset": 50,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_no_key(&answer);

    let queries = fake.queries.lock().expect("lock").clone();
    assert_eq!(queries.len(), 1, "one request per search: {queries:?}");
    let sent = &queries[0];
    for (name, expected) in [
        ("t", "search"),
        ("q", "some show !cam"),
        ("maxage", "30"),
        ("pw", "2"),
        ("limit", "50"),
        ("offset", "50"),
        ("extended", "1"),
        // The indexer's own default, as no category was named.
        ("cat", "5040"),
        ("apikey", API_KEY),
    ] {
        assert_eq!(value(sent, name), Some(expected), "{name} in {sent:?}");
    }

    let hits = answer["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 2, "{answer}");
    let first = &hits[0];
    assert_eq!(first["title"], "Some.Show.S01E01.1080p");
    assert_eq!(first["indexer_id"], indexer["id"]);
    assert_eq!(first["indexer_name"], "Fake");
    assert_eq!(first["size_bytes"], 1_500_000);
    assert_eq!(first["category"], "5040");
    assert_eq!(first["grabs"], 12);
    assert_eq!(first["passworded"], true);
    assert!(
        first["download"]
            .as_str()
            .expect("download")
            .contains("rdownloader-indexer-key"),
        "{first}"
    );
    assert_eq!(hits[1]["passworded"], false);
    let outcome = &answer["indexers"][0];
    assert_eq!(outcome["returned"], 2);
    assert_eq!(outcome["total"], 2);
    assert_eq!(outcome["more"], false);
    assert!(outcome.get("error").is_none_or(Value::is_null), "{outcome}");
}

#[tokio::test]
async fn a_search_the_indexer_would_refuse_is_refused_before_it_is_sent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, fake) = fake_indexer().await;

    let (status, refused) = post_json(
        &router,
        "/api/v1/indexers/search",
        json!({ "query": "ubuntu" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused["code"], "indexer.none_enabled");

    create_indexer(&router, &base).await;
    for (body, code) in [
        (json!({ "query": "ab" }), "indexer.query_too_short"),
        (json!({ "limit": 501 }), "indexer.limit_invalid"),
        (json!({ "max_age_days": 0 }), "indexer.max_age_invalid"),
    ] {
        let (status, refused) = post_json(&router, "/api/v1/indexers/search", body.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(refused["code"], code, "{body}");
    }
    assert!(
        fake.queries.lock().expect("lock").is_empty(),
        "a refused search must cost the indexer nothing"
    );
}

#[tokio::test]
async fn an_indexer_refusal_becomes_a_stable_code_in_its_outcome() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, _) = fake_indexer().await;
    create_indexer(&router, &base).await;
    // A second indexer on the same server with a wrong key: its refusal must not cost the
    // first one its hits.
    let (status, _) = post_json(
        &router,
        "/api/v1/indexers",
        json!({ "name": "Wrong key", "url": format!("{base}/api"), "api_key": "wrong" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, answer) = post_json(
        &router,
        "/api/v1/indexers/search",
        json!({ "query": "show" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_no_key(&answer);
    let outcomes = answer["indexers"].as_array().expect("outcomes");
    let wrong = outcomes
        .iter()
        .find(|outcome| outcome["indexer_name"] == "Wrong key")
        .expect("the wrong-key indexer");
    assert_eq!(wrong["error"]["code"], "indexer.credentials_refused");
    assert_eq!(wrong["error"]["params"]["code"], "100");
    assert_eq!(answer["hits"].as_array().expect("hits").len(), 2);

    for (query, code) in [
        ("refuse-201", "indexer.query_rejected"),
        ("refuse-501", "indexer.limit_reached"),
    ] {
        let (_, answer) = post_json(
            &router,
            "/api/v1/indexers/search",
            json!({ "query": query, "indexer_ids": [outcomes[0]["indexer_id"]] }),
        )
        .await;
        assert_eq!(answer["indexers"][0]["error"]["code"], code, "{answer}");
    }
}

#[tokio::test]
async fn grabbed_hits_arrive_as_nzb_imports_and_the_key_goes_only_to_the_indexer() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, fake) = fake_indexer().await;
    let indexer = create_indexer(&router, &base).await;
    let (_, answer) = post_json(
        &router,
        "/api/v1/indexers/search",
        json!({ "query": "show" }),
    )
    .await;
    let hit = answer["hits"][0].clone();

    let (status, grabbed) = post_json(
        &router,
        "/api/v1/indexers/grab",
        json!({
            "items": [
                { "indexer_id": hit["indexer_id"], "download": hit["download"], "title": hit["title"] },
                // The placeholder on another server: the key is not sent there, nor is anything.
                {
                    "indexer_id": indexer["id"],
                    "download": "https://elsewhere.invalid/getnzb?apikey=rdownloader-indexer-key",
                    "title": "Elsewhere",
                },
            ],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{grabbed}");
    assert_no_key(&grabbed);
    let imports = grabbed["imports"].as_array().expect("imports");
    assert_eq!(imports.len(), 1, "{grabbed}");
    // Named the way an uploaded file is: after the file, which is named after the hit.
    assert_eq!(imports[0]["name"], "Some.Show.S01E01.1080p.nzb");
    assert_eq!(grabbed["failed"][0]["title"], "Elsewhere");
    assert_eq!(
        grabbed["failed"][0]["error"]["code"],
        "indexer.download_foreign"
    );

    let downloads = fake.downloads.lock().expect("lock").clone();
    assert_eq!(downloads.len(), 1, "{downloads:?}");
    assert_eq!(value(&downloads[0], "apikey"), Some(API_KEY));
    assert_eq!(value(&downloads[0], "id"), Some("abc"));

    // The same review list an uploaded file lands in.
    let (_, listed) = get_json(&router, "/api/v1/nzb/imports").await;
    assert!(
        listed
            .as_array()
            .expect("imports")
            .iter()
            .any(|import| import["name"] == "Some.Show.S01E01.1080p.nzb"),
        "{listed}"
    );
}

/// RD-180-20: a subscription takes a defined indexer over and sends its own search term.
#[tokio::test]
async fn a_subscription_takes_an_indexer_over_and_sends_its_search_term() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, fake) = fake_indexer().await;
    let indexer = create_indexer(&router, &base).await;

    let request = |url: &str, kind: &str| {
        json!({
            "name": format!("Show {kind} {url}"),
            "url": url,
            "kind": kind,
            "indexer_id": indexer["id"],
            "interval_seconds": 3_600,
            "backlog": { "mode": "review_all" },
            "indexer_search": { "query": "some show", "max_age_days": 7, "hide_passworded": true },
        })
    };
    let (status, refused) = post_json(
        &router,
        "/api/v1/subscriptions",
        request("https://elsewhere.invalid/api", "indexer"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["code"], "subscription.indexer_url_mismatch");
    let (status, refused) = post_json(
        &router,
        "/api/v1/subscriptions",
        request("https://feed.invalid/rss", "feed"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["code"], "subscription.indexer_kind");

    let (status, created) =
        post_json(&router, "/api/v1/subscriptions", request("", "indexer")).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_no_key(&created);
    assert_eq!(created["has_secret"], true);
    assert_eq!(created["url"], format!("{base}/api"));
    assert_eq!(created["source_categories"], json!(["5040"]));
    assert_eq!(created["indexer_search"]["query"], "some show");
    let id = created["id"].as_str().expect("id").to_owned();

    let (status, _) = post_json(
        &router,
        &format!("/api/v1/subscriptions/{id}/poll"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let fake = &fake;
    let sent = eventually(WAIT, "the poll never reached the indexer", || async move {
        fake.queries
            .lock()
            .expect("lock")
            .iter()
            .find(|query| value(query, "q").is_some())
            .cloned()
    })
    .await;
    for (name, expected) in [
        ("q", "some show"),
        ("maxage", "7"),
        ("pw", "2"),
        ("cat", "5040"),
        ("apikey", API_KEY),
    ] {
        assert_eq!(value(&sent, name), Some(expected), "{name} in {sent:?}");
    }

    // Search parameters belong to an indexer subscription alone.
    let (status, refused) = post_json(
        &router,
        "/api/v1/subscriptions",
        json!({
            "name": "A feed",
            "url": "https://feed.invalid/rss",
            "kind": "feed",
            "indexer_search": { "query": "some show" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["code"], "subscription.search_kind");
}
