//! The route policy held against the assembled OpenAPI document: every documented operation has
//! exactly one entry, and every entry is a documented operation or a named exception. Here rather
//! than beside the table since RD-160-06, because the document is assembled in this crate; the
//! tests that read only the table stay with it in `rd-api-core`.

use std::collections::BTreeSet;

use axum::http::Method;
use rd_api_core::scope_policy::ROUTE_POLICY;

/// Routes the router registers that utoipa does not document, with the reason.
///
/// The exhaustiveness test allows exactly these four to be absent from the OpenAPI document
/// and nothing else, so an endpoint that quietly stops being documented is still caught.
const UNDOCUMENTED: &[(&str, &str)] = &[
    (
        "/api/v1/events",
        "server-sent events; utoipa has no stream response type",
    ),
    (
        "/api/v1/capture/events",
        "server-sent events, capture surface",
    ),
    (
        "/api/v1/capture/nzb",
        "multipart upload; documented in the capture contract instead",
    ),
    (
        "/api/v1/openapi.json",
        "serves the document, so it cannot appear inside it",
    ),
];

/// Every `(path, method)` in the document, as the router registered them.
///
/// Read out of the serialised JSON rather than utoipa's own types on purpose: the JSON is
/// the contract — it is what `scripts/api-contract.sh` writes and what the frontend types
/// are generated from — and it does not move under us when utoipa reshapes `PathItem`.
fn documented() -> BTreeSet<(String, Method)> {
    let document = serde_json::to_value(crate::openapi_document()).expect("serialise");
    let paths = document
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .expect("the document has paths");
    let mut operations = BTreeSet::new();
    for (path, item) in paths {
        let item = item.as_object().expect("path item");
        for method in item.keys() {
            let Ok(method) = Method::from_bytes(method.to_uppercase().as_bytes()) else {
                continue;
            };
            operations.insert((path.clone(), method));
        }
    }
    assert!(
        operations.len() > 200,
        "only {} operations were found; the document did not serialise as expected",
        operations.len()
    );
    operations
}

fn policy_keys() -> BTreeSet<(String, Method)> {
    ROUTE_POLICY
        .iter()
        .map(|entry| (entry.path.to_owned(), entry.method.clone()))
        .collect()
}

/// Adding a route without deciding what it costs must not be possible.
///
/// This is the whole reason the policy is a table. A per-handler check that someone
/// forgets leaves a route with no check at all, and nothing says so; a missing table row
/// fails here, before the route can ship.
#[test]
fn every_documented_operation_has_a_policy_entry() {
    let missing: Vec<_> = documented()
        .difference(&policy_keys())
        .map(|(path, method)| format!("{method} {path}"))
        .collect();
    assert!(
        missing.is_empty(),
        "these operations have no scope decision in ROUTE_POLICY:\n  {}",
        missing.join("\n  ")
    );
}

/// And the table must not describe routes that no longer exist.
///
/// A stale entry is not merely untidy: it is a scope decision that reads as coverage of
/// something, and the something is gone. The four exceptions are routes the router really
/// does serve and utoipa cannot document; each carries its reason in [`UNDOCUMENTED`].
#[test]
fn every_policy_entry_is_a_documented_operation_or_a_named_exception() {
    let documented = documented();
    let allowed: BTreeSet<&str> = UNDOCUMENTED.iter().map(|(path, _)| *path).collect();
    let stale: Vec<_> = policy_keys()
        .into_iter()
        .filter(|key| !documented.contains(key))
        .filter(|(path, _)| !allowed.contains(path.as_str()))
        .map(|(path, method)| format!("{method} {path}"))
        .collect();
    assert!(
        stale.is_empty(),
        "ROUTE_POLICY describes operations the API does not have:\n  {}",
        stale.join("\n  ")
    );
}

/// Every documented exception has to actually be in the table, or it is just a comment.
#[test]
fn every_undocumented_route_is_still_in_the_table() {
    let paths: BTreeSet<&str> = ROUTE_POLICY.iter().map(|entry| entry.path).collect();
    for (path, reason) in UNDOCUMENTED {
        assert!(paths.contains(path), "{path} is exempted but has no entry");
        assert!(!reason.is_empty(), "{path} is exempted without a reason");
    }
}

/// The two endpoints RD-108-19 withdrew are gone from both the document and the table.
///
/// Neither had a client. `plan/preview` duplicated the `PUT` that already reports which
/// files a pattern drops, and `replay-restart` duplicated `downloads/{id}/reset` — which
/// does strictly more and, unlike it, clears the scheduler's stop reason. Named here so
/// re-adding either is a decision somebody makes on purpose.
#[test]
fn the_endpoints_withdrawn_for_having_no_caller_stay_withdrawn() {
    let withdrawn = [
        (
            "/api/v1/collector/candidates/{id}/torrent/plan/preview",
            Method::POST,
        ),
        ("/api/v1/downloads/{id}/replay-restart", Method::POST),
    ];
    let documented = documented();
    let policy = policy_keys();
    for (path, method) in withdrawn {
        let key = (path.to_owned(), method);
        assert!(!documented.contains(&key), "{path} is documented again");
        assert!(!policy.contains(&key), "{path} is in ROUTE_POLICY again");
    }
}

/// One decision per route, or the lookup silently picks whichever came first.
#[test]
fn no_route_is_listed_twice() {
    assert_eq!(
        policy_keys().len(),
        ROUTE_POLICY.len(),
        "ROUTE_POLICY contains a duplicate (path, method)"
    );
}
