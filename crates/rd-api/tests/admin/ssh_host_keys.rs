//! Forgetting a confirmed SSH host key (audit 2026-11-25, TST-7): the one named, by host, port and
//! algorithm, and no other.

use crate::common;

use axum::http::StatusCode;
use common::{delete_json, get_json, post_json, test_router};
use serde_json::json;

const HOSTS: &str = "/api/v1/remote-credentials/ssh-hosts";

async fn trust(router: &axum::Router, host: &str, port: u16, algorithm: &str) {
    let (status, body) = post_json(
        router,
        HOSTS,
        json!({
            "host": host,
            "port": port,
            "algorithm": algorithm,
            "fingerprint": "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn known(router: &axum::Router) -> Vec<(String, u64, String)> {
    let (status, body) = get_json(router, HOSTS).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut keys: Vec<_> = body
        .as_array()
        .expect("a list")
        .iter()
        .map(|key| {
            (
                key["host"].as_str().expect("host").to_owned(),
                key["port"].as_u64().expect("port"),
                key["algorithm"].as_str().expect("algorithm").to_owned(),
            )
        })
        .collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn only_the_named_key_is_forgotten() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    trust(&router, "sftp.example.com", 22, "ssh-ed25519").await;
    trust(&router, "sftp.example.com", 22, "rsa-sha2-512").await;
    trust(&router, "sftp.example.com", 2222, "ssh-ed25519").await;
    trust(&router, "other.example.com", 22, "ssh-ed25519").await;
    assert_eq!(known(&router).await.len(), 4);

    let (status, body) = delete_json(
        &router,
        "/api/v1/remote-credentials/ssh-hosts/sftp.example.com/22/ssh-ed25519",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "remote.host_key_forgotten", "{body}");
    let left = |host: &str, port, algorithm: &str| (host.to_owned(), port, algorithm.to_owned());
    assert_eq!(
        known(&router).await,
        [
            left("other.example.com", 22, "ssh-ed25519"),
            left("sftp.example.com", 22, "rsa-sha2-512"),
            left("sftp.example.com", 2222, "ssh-ed25519"),
        ]
    );
}
