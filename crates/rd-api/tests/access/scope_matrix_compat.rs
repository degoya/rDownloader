//! The aria2 adapter's prices in the scope matrix (RD-1240-11), apart from `scope_matrix.rs` so
//! that file stays under the length limit.

use crate::common;
use crate::common::auth_harness;
use crate::scope_matrix::bearer_holding;
use axum::http::StatusCode;
use rd_core::Scope;

/// The aria2 adapter (RD-1240-11) is priced like the other two compatibility doors: a secret
/// lacking any one of `api:intake`, `api:queue` and `api:read` is refused, however much else it
/// holds; the three together pass.
#[tokio::test]
async fn the_aria2_adapter_wants_all_three_compatibility_scopes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let (database, key) = (&harness.database, rd_db::SERVICE_SETTINGS_KEY);
    // A null document indexed mutably becomes an object holding the one switch.
    let mut on = database
        .get_setting(key)
        .await
        .expect("read")
        .unwrap_or_default();
    on["aria2_rpc_enabled"] = true.into();
    database.set_setting(key.to_owned(), on).await.expect("on");

    let compat = [Scope::Intake, Scope::Queue, Scope::Read];
    for missing in compat.iter().copied().map(Some).chain([None]) {
        let held: Vec<&str> = Scope::API
            .iter()
            .filter(|scope| missing.is_none_or(|missing| !scope.satisfies(missing)))
            .filter(|scope| missing.is_some() || compat.contains(scope))
            .map(|scope| scope.as_str())
            .collect();
        let label = missing.map_or("aria2-all".to_owned(), |scope| {
            format!("aria2-without-{}", scope.as_str().replace(':', "-"))
        });
        let bearer = bearer_holding(database, &label, &held).await;
        let body = serde_json::json!({
            "jsonrpc": "2.0", "method": "aria2.getVersion", "id": 1,
            "params": [format!("token:{bearer}")],
        });
        let request = common::request_to("POST", "/jsonrpc")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("request");
        let (status, answer) = common::send(&harness.router, request).await;
        let expected = [StatusCode::OK, StatusCode::FORBIDDEN][usize::from(missing.is_some())];
        assert_eq!(status, expected, "{label}: {answer}");
    }
}
