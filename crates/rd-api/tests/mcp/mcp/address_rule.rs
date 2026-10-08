//! RD-1190-18: an address an agent hands to `add_downloads` keeps to the address rule a
//! stranger's link keeps to (RD-150-03). A literal address in the local network is refused with
//! `mirror.internal_address`; a name is written with its address as its one source row, held to
//! the public internet, so the transfer refuses it too should it resolve inward.

use super::{API_BEARER, everything::installation_parts, everything::ok, handshake};

#[tokio::test]
async fn an_agent_cannot_queue_an_address_in_the_local_network() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (router, database) = installation_parts(directory.path()).await;
    let session = handshake(&router, API_BEARER).await;
    let added = ok(
        &router,
        &session,
        "add_downloads",
        serde_json::json!({
            "urls": ["http://192.168.1.10/release.bin", "https://files.example.invalid/release.bin"],
            "start_paused": true
        }),
    )
    .await;
    let failed = added["failed"].as_array().expect("failed");
    assert_eq!(failed.len(), 1, "{added}");
    assert_eq!(
        failed[0]["url"], "http://192.168.1.10/release.bin",
        "{added}"
    );
    assert_eq!(failed[0]["code"], rd_core::CODE_INTERNAL_ADDRESS, "{added}");
    let created = added["created"].as_array().expect("created");
    assert_eq!(created.len(), 1, "{added}");
    let id: rd_core::DownloadId = created[0]["download"]["id"]
        .as_str()
        .expect("download id")
        .parse()
        .expect("an id");
    let rows = database.download_sources(id).await.expect("sources");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(!rows[0].local_network, "{rows:?}");
}
