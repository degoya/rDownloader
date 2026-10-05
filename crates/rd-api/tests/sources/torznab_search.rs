//! RD-1100-03: a Torznab indexer (Jackett, Prowlarr) searched from the LinkGrabber, its torrent
//! hits taken in as LinkGrabber packages, and the TV and film searches with their ids.
//!
//! A fake Jackett on loopback answers everything; nothing here reaches a real indexer. What is
//! held: the hit is read as a torrent with its swarm, the ids reach the wire under their Newznab
//! names, a `.torrent`, a magnet and a download that only redirects to a magnet all arrive in
//! the LinkGrabber -- and **the API key is never in an answer**.

use crate::common;

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use common::{get_json, post_json, test_router};
use serde_json::{Value, json};

const API_KEY: &str = "rd-1100-03-jackett-key-9c1e";
const MAGNET: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Some.Show";
const LONE_MAGNET: &str = "magnet:?xt=urn:btih:89abcdef0123456789abcdef0123456789abcdef";

const CAPS: &str = r#"<?xml version="1.0"?>
<caps>
  <server title="Jackett"/>
  <searching>
    <search available="yes" supportedParams="q"/>
    <tv-search available="yes" supportedParams="q,season,ep,tvdbid,imdbid"/>
    <movie-search available="yes" supportedParams="q,imdbid,tmdbid"/>
  </searching>
  <categories><category id="5000" name="TV"/></categories>
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

/// A single-file torrent, the smallest one the parser takes.
fn torrent_bytes() -> Vec<u8> {
    let mut torrent =
        b"d8:announce31:http://tracker.example/announce4:infod6:lengthi32e4:name7:release12:piece lengthi16384e6:pieces20:"
            .to_vec();
    torrent.extend_from_slice(&[0_u8; 20]);
    torrent.extend_from_slice(b"ee");
    torrent
}

/// Jackett's answer: the key in the download address the way Jackett hands it out, a magnet
/// beside it, and a hit that is a magnet and nothing else.
fn results(base: &str, key: &str) -> String {
    let magnet = MAGNET.replace('&', "&amp;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <item>
      <title>Some.Show.S01E02.1080p</title>
      <guid>{base}/details/1</guid>
      <pubDate>Tue, 29 Sep 2026 10:00:00 GMT</pubDate>
      <enclosure url="{base}/dl?jackett_apikey={key}&amp;file=a" length="1500000"
                 type="application/x-bittorrent"/>
      <torznab:attr name="category" value="5040"/>
      <torznab:attr name="seeders" value="42"/>
      <torznab:attr name="peers" value="50"/>
      <torznab:attr name="magneturl" value="{magnet}"/>
    </item>
    <item>
      <title>Only.A.Magnet</title>
      <enclosure url="{LONE_MAGNET}" type="application/x-bittorrent"/>
      <torznab:attr name="seeders" value="3"/>
      <torznab:attr name="leechers" value="9"/>
    </item>
  </channel>
</rss>"#
    )
}

async fn serve_api(
    State(fake): State<Fake>,
    Query(params): Query<Vec<(String, String)>>,
) -> impl IntoResponse {
    fake.queries.lock().expect("lock").push(params.clone());
    let key = value(&params, "apikey").unwrap_or_default().to_owned();
    let body = if key != API_KEY {
        r#"<?xml version="1.0"?><error code="100" description="Invalid API Key"/>"#.to_owned()
    } else if value(&params, "t") == Some("caps") {
        CAPS.to_owned()
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

async fn serve_torrent(
    State(fake): State<Fake>,
    Query(params): Query<Vec<(String, String)>>,
) -> Response {
    fake.downloads.lock().expect("lock").push(params.clone());
    if value(&params, "jackett_apikey") != Some(API_KEY) {
        return (StatusCode::UNAUTHORIZED, "no").into_response();
    }
    (
        [("content-type", "application/x-bittorrent")],
        torrent_bytes(),
    )
        .into_response()
}

/// What Prowlarr answers for a tracker that has nothing but magnets.
async fn serve_redirect() -> Redirect {
    Redirect::to(LONE_MAGNET)
}

async fn fake_jackett() -> (String, Fake) {
    let fake = Fake {
        base: Arc::new(Mutex::new(String::new())),
        queries: Arc::new(Mutex::new(Vec::new())),
        downloads: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/api", get(serve_api))
        .route("/dl", get(serve_torrent))
        .route("/redirect", get(serve_redirect))
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
        json!({ "name": "Jackett", "url": format!("{base}/api"), "api_key": API_KEY }),
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
async fn a_torznab_indexer_answers_torrent_hits_with_their_swarm_and_without_the_key() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, _) = fake_jackett().await;
    let indexer = create_indexer(&router, &base).await;
    let id = indexer["id"].as_str().expect("id");

    // The search mask offers only what the indexer says it takes.
    let (status, caps) =
        post_json(&router, &format!("/api/v1/indexers/{id}/caps"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{caps}");
    assert_eq!(
        caps["supported_params"]["tv-search"],
        json!(["q", "season", "ep", "tvdbid", "imdbid"])
    );
    assert_no_key(&caps);

    let (status, answer) = post_json(
        &router,
        "/api/v1/indexers/search",
        json!({ "query": "some show" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_no_key(&answer);
    let hits = answer["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 2, "{answer}");
    let first = &hits[0];
    assert_eq!(first["kind"], "torrent");
    assert_eq!(first["seeders"], 42);
    assert_eq!(first["leechers"], 8);
    assert_eq!(first["size_bytes"], 1_500_000);
    assert_eq!(first["magnet"], MAGNET);
    assert!(
        first["download"]
            .as_str()
            .expect("download")
            .contains("jackett_apikey=rdownloader-indexer-key"),
        "{first}"
    );
    let second = &hits[1];
    assert_eq!(second["kind"], "torrent");
    assert_eq!(second["download"], LONE_MAGNET);
    assert_eq!(second["leechers"], 9);
}

#[tokio::test]
async fn a_typed_search_sends_its_function_and_its_ids_and_refuses_a_foreign_one() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, fake) = fake_jackett().await;
    create_indexer(&router, &base).await;

    let (status, answer) = post_json(
        &router,
        "/api/v1/indexers/search",
        json!({
            "search_type": "tv",
            "season": 1,
            "episode": 2,
            "tvdb_id": 81189,
            "imdb_id": "tt0903747",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let (status, answer) = post_json(
        &router,
        "/api/v1/indexers/search",
        json!({ "search_type": "movie", "query": "the matrix", "tmdb_id": 603 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    let queries = fake.queries.lock().expect("lock").clone();
    assert_eq!(queries.len(), 2, "{queries:?}");
    for (name, expected) in [
        ("t", "tvsearch"),
        ("season", "1"),
        ("ep", "2"),
        ("tvdbid", "81189"),
        ("imdbid", "0903747"),
        ("apikey", API_KEY),
    ] {
        assert_eq!(
            value(&queries[0], name),
            Some(expected),
            "{name} in {queries:?}"
        );
    }
    for (name, expected) in [("t", "movie"), ("q", "the matrix"), ("tmdbid", "603")] {
        assert_eq!(
            value(&queries[1], name),
            Some(expected),
            "{name} in {queries:?}"
        );
    }
    assert_eq!(value(&queries[1], "season"), None);

    for (body, code) in [
        (
            json!({ "search_type": "movie", "season": 1 }),
            "indexer.search_field_unsupported",
        ),
        (
            json!({ "search_type": "tv", "episode": 3 }),
            "indexer.episode_without_season",
        ),
        (
            json!({ "search_type": "movie", "imdb_id": "nm0000206" }),
            "indexer.search_id_invalid",
        ),
    ] {
        let (status, refused) = post_json(&router, "/api/v1/indexers/search", body.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(refused["code"], code, "{body}");
    }
    assert_eq!(
        fake.queries.lock().expect("lock").len(),
        2,
        "a refused search must cost the indexer nothing"
    );
}

#[tokio::test]
async fn grabbed_torrent_hits_arrive_in_the_linkgrabber_and_the_key_goes_only_to_the_indexer() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (base, fake) = fake_jackett().await;
    let indexer = create_indexer(&router, &base).await;
    let (_, answer) = post_json(
        &router,
        "/api/v1/indexers/search",
        json!({ "query": "some show" }),
    )
    .await;
    let file_hit = answer["hits"][0].clone();
    let magnet_hit = answer["hits"][1].clone();

    let (status, grabbed) = post_json(
        &router,
        "/api/v1/indexers/grab",
        json!({
            "items": [
                {
                    "indexer_id": file_hit["indexer_id"],
                    "download": file_hit["download"],
                    "title": file_hit["title"],
                    "magnet": file_hit["magnet"],
                },
                {
                    "indexer_id": magnet_hit["indexer_id"],
                    "download": magnet_hit["download"],
                    "title": magnet_hit["title"],
                },
                // A download that only redirects to a magnet: the hit's magnet is taken.
                {
                    "indexer_id": indexer["id"],
                    "download": format!("{base}/redirect"),
                    "title": "Redirected.Release",
                    "magnet": LONE_MAGNET,
                },
                {
                    "indexer_id": indexer["id"],
                    "download": "magnet:?dn=no-topic",
                    "title": "Broken.Magnet",
                },
            ],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{grabbed}");
    assert_no_key(&grabbed);
    assert_eq!(grabbed["imports"].as_array().map(Vec::len), Some(0));
    let names: Vec<&str> = grabbed["torrents"]
        .as_array()
        .expect("torrents")
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Some.Show.S01E02.1080p",
            "Only.A.Magnet",
            "Redirected.Release"
        ],
        "{grabbed}"
    );
    assert_eq!(grabbed["failed"][0]["title"], "Broken.Magnet");
    assert_eq!(
        grabbed["failed"][0]["error"]["code"],
        "indexer.magnet_invalid"
    );

    // The `.torrent` was fetched once, with the key, from the indexer's own server.
    let downloads = fake.downloads.lock().expect("lock").clone();
    assert_eq!(downloads.len(), 1, "{downloads:?}");
    assert_eq!(value(&downloads[0], "jackett_apikey"), Some(API_KEY));

    // The same LinkGrabber list a pasted magnet or an uploaded `.torrent` lands in.
    let (_, packages) = get_json(&router, "/api/v1/collector/packages").await;
    assert_no_key(&packages);
    for name in names {
        assert!(
            packages
                .as_array()
                .expect("packages")
                .iter()
                .any(|package| package["name"] == name),
            "{name} missing from {packages}"
        );
    }
}
