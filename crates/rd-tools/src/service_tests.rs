use crate::ToolError;

/// RA-TR-04: a manifest that cannot be fetched says why, not only reqwest's top line
/// "error sending request for url (…)".
#[tokio::test]
async fn an_unreachable_manifest_reports_the_cause() {
    // A port nothing listens on: bound and closed again, so the connect is refused.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("free port")
        .port();
    let error = super::fetch_manifest(
        &reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client"),
        &format!("http://127.0.0.1:{port}/manifest.json"),
    )
    .await
    .expect_err("nothing answers");
    let ToolError::DownloadFailed { reason, .. } = error else {
        panic!("an unexpected error: {error}");
    };
    assert!(
        reason.to_lowercase().contains("connect"),
        "the cause was lost: {reason}"
    );
}
