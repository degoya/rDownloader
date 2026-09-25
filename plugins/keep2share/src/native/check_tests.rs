//! `check()` and `hosters()` coverage, split out of `native/tests.rs` to keep both files within
//! the crate layout's 500-line convention. Uses that module's `MockHost` and fixtures.

use std::sync::Arc;

use rd_core::{AccountId, LinkStatus};
use rd_plugin_api::{CheckRequest, ClientIdentity, HostHttpRequest, Resolver, ResolverHost};
use url::Url;

use super::super::Keep2ShareResolver;
use super::{
    FILE_ID, FILE_URL, MockHost, OFFLINE_ID, body_str, client_identity, getfilesinfo_response,
};

#[tokio::test]
async fn check_maps_online_offline_and_unparseable_batch() {
    // JD's `checkLinks` calls `/getfilesinfo` with `account = null` — no login, no secret gate.
    let host = MockHost::with_responses(
        vec![getfilesinfo_response(&format!(
            r#"{{"status":"success","code":200,"files":[{{"id":"{FILE_ID}","name":"a.rar","size":10,"is_available":true}}]}}"#
        ))],
        false,
    );
    let resolver = Keep2ShareResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let results = resolver
        .check(CheckRequest {
            urls: vec![
                format!("https://k2s.cc/file/{FILE_ID}")
                    .parse::<Url>()
                    .expect("URL"),
                format!("https://k2s.cc/file/{OFFLINE_ID}")
                    .parse::<Url>()
                    .expect("URL"),
                "https://k2s.cc/folder/abcdefghijklm"
                    .parse::<Url>()
                    .expect("URL"),
            ],
            client: client_identity(),
        })
        .await
        .expect("results");

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status, LinkStatus::Online);
    assert_eq!(results[0].file_name.as_deref(), Some("a.rar"));
    assert_eq!(results[0].size.map(|size| size.get()), Some(10));
    // Absent from the batch response -> offline (JD: `fileInfo == null` -> unavailable).
    assert_eq!(results[1].status, LinkStatus::Offline);
    // Not a `/file/`-shaped link -> never enters the batch at all.
    assert_eq!(results[2].status, LinkStatus::Unknown);

    let requests = host.requests.lock().expect("mock lock");
    // One batched getfilesinfo call, no login (unparseable links never enter the batch).
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        requests[0].url.as_str(),
        "https://k2s.cc/api/v2/getfilesinfo"
    );
    assert_eq!(
        body_str(&requests[0]),
        format!(r#"{{"ids":["{FILE_ID}","{OFFLINE_ID}"]}}"#)
    );
}

/// JD's `checkLinks` chunks at 100 fileIDs per `/getfilesinfo` call and loops
/// (`K2SApi.java:476-485`, "Check up to 100 fileIDs with one request"). This test drives 150
/// links through `check()` and asserts: exactly 2 `/getfilesinfo` requests (100 ids, then 50),
/// every result lands at the same index as its input URL (input order preserved end to end), and
/// a chunk whose request itself fails (the second chunk here, given a `network_restricted` error)
/// degrades only that chunk's links to `Unknown` rather than misreporting them `Offline` — the
/// exact failure mode Finding 2 flagged (ids past a truncated/failed batch would otherwise be
/// indistinguishable from "not in the response").
#[tokio::test]
async fn check_chunks_at_100_ids_preserves_order_and_isolates_a_failed_chunk() {
    let ids: Vec<String> = (0..150).map(|i| format!("id{i:011}")).collect();
    let urls: Vec<Url> = ids
        .iter()
        .map(|id| {
            format!("https://k2s.cc/file/{id}")
                .parse::<Url>()
                .expect("URL")
        })
        .collect();

    let first_chunk_files: String = ids[..100]
        .iter()
        .map(|id| format!(r#"{{"id":"{id}","name":"{id}.rar","size":1,"is_available":true}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let host = MockHost::with_responses(
        vec![
            getfilesinfo_response(&format!(
                r#"{{"status":"success","code":200,"files":[{first_chunk_files}]}}"#
            )),
            // The second chunk's request itself fails -> its 50 links must degrade to `Unknown`,
            // not `Offline` (they are simply missing from any successfully-parsed `files` list).
            getfilesinfo_response(
                r#"{"status":"error","code":406,"errorCode":73,"message":"Network restricted"}"#,
            ),
        ],
        false,
    );
    let resolver = Keep2ShareResolver::new(Arc::clone(&host) as Arc<dyn ResolverHost>);
    let results = resolver
        .check(CheckRequest {
            urls: urls.clone(),
            client: client_identity(),
        })
        .await
        .expect("results");

    assert_eq!(results.len(), 150);
    for (index, result) in results.iter().enumerate() {
        assert_eq!(result.url, urls[index], "result {index} out of input order");
        if index < 100 {
            assert_eq!(result.status, LinkStatus::Online, "result {index}");
            assert_eq!(
                result.file_name.as_deref(),
                Some(format!("{}.rar", ids[index]).as_str())
            );
        } else {
            // In the failed second chunk: `Unknown`, not `Offline`.
            assert_eq!(result.status, LinkStatus::Unknown, "result {index}");
        }
    }

    let requests = host.requests.lock().expect("mock lock");
    assert_eq!(requests.len(), 2, "one /getfilesinfo call per 100-id chunk");
    let sent_ids = |request: &HostHttpRequest| -> Vec<String> {
        let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json body");
        body["ids"]
            .as_array()
            .expect("ids array")
            .iter()
            .map(|value| value.as_str().expect("id string").to_owned())
            .collect()
    };
    assert_eq!(sent_ids(&requests[0]), ids[..100].to_vec());
    assert_eq!(sent_ids(&requests[1]), ids[100..].to_vec());
}

#[tokio::test]
async fn check_needs_no_account_or_secret() {
    // `check()` never gates on the secret and never requires `client.account_id` — `/getfilesinfo`
    // is unauthenticated in JD (see the `api` module doc's IMPL-VERIFY note).
    let host = MockHost::with_responses(
        vec![getfilesinfo_response(
            r#"{"status":"success","code":200,"files":[]}"#,
        )],
        false,
    );
    let resolver = Keep2ShareResolver::new(host as Arc<dyn ResolverHost>);
    let results = resolver
        .check(CheckRequest {
            urls: vec![FILE_URL.parse::<Url>().expect("URL")],
            client: ClientIdentity {
                account_id: None,
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect("no account/secret required");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, LinkStatus::Offline);
}

#[tokio::test]
async fn hosters_returns_the_match_host_catalogue() {
    let host = MockHost::with_responses(Vec::new(), true);
    let resolver = Keep2ShareResolver::new(host as Arc<dyn ResolverHost>);
    let hosters = resolver.hosters(AccountId::new()).await.expect("hosters");
    assert_eq!(
        hosters,
        vec!["k2s.cc", "keep2share.cc", "k2share.cc", "keep2s.cc"]
    );
}
