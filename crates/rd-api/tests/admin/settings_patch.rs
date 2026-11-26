//! `PATCH /api/v1/settings`, the document's `ETag` and `If-Match`, and the event a change
//! announces (RD-1480-07).

use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
};
use rd_core::{EventEnvelope, EventKind};
use serde_json::json;

use crate::common::{self, API_BEARER, Harness, get_with_bearer, patch_with_bearer};

const SETTINGS: &str = "/api/v1/settings";
/// A token that configures, without the administration scope.
const CONFIG_BEARER: &str = "test-config-bearer-token";

/// A service with the login switched on, so a scope is what decides.
async fn harness(directory: &std::path::Path) -> Harness {
    common::harness(
        directory,
        common::Options::default()
            .login()
            .token(CONFIG_BEARER, rd_core::API_CONFIG_SCOPE),
    )
    .await
}

/// One request as `bearer` with an optional `If-Match`; status, headers and the JSON body.
async fn write(
    harness: &Harness,
    method: &str,
    bearer: &str,
    if_match: Option<&str>,
    body: &serde_json::Value,
) -> (StatusCode, HeaderMap, serde_json::Value) {
    let mut request = common::request_to(method, SETTINGS)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(tag) = if_match {
        request = request.header(header::IF_MATCH, tag);
    }
    let request = request.body(Body::from(body.to_string())).expect("request");
    let (status, headers, bytes) = common::send_raw(&harness.router, request).await;
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, headers, body)
}

fn etag(headers: &HeaderMap) -> String {
    headers
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .expect("an ETag")
        .to_owned()
}

async fn read_etag(harness: &Harness) -> String {
    let request = common::request_to("GET", SETTINGS)
        .header(header::AUTHORIZATION, format!("Bearer {API_BEARER}"))
        .body(Body::empty())
        .expect("request");
    let (status, headers, _) = common::send_raw(&harness.router, request).await;
    assert_eq!(status, StatusCode::OK);
    etag(&headers)
}

async fn stored(harness: &Harness) -> serde_json::Value {
    let (status, settings) = get_with_bearer(&harness.router, SETTINGS, API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    settings
}

/// What a `settings.changed` in the stream so far carried.
fn announced(
    events: &mut tokio::sync::broadcast::Receiver<EventEnvelope>,
) -> Vec<serde_json::Value> {
    std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| event.kind == EventKind::SettingsChanged)
        .map(|event| event.payload)
        .collect()
}

/// The finding itself: a body naming one field changes that field, where `PUT` -- unchanged --
/// takes every other one for its default.
#[tokio::test]
async fn a_patch_keeps_the_fields_it_does_not_name_and_put_still_replaces() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let (status, _, body) = write(
        &harness,
        "PATCH",
        API_BEARER,
        None,
        &json!({ "max_retries": 7, "media_output_template": "%(title)s" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, headers, body) = write(
        &harness,
        "PATCH",
        API_BEARER,
        None,
        &json!({ "max_active_files": 9 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["max_active_files"], 9);
    let after = stored(&harness).await;
    assert_eq!(after["max_active_files"], 9);
    assert_eq!(after["max_retries"], 7);
    assert_eq!(after["media_output_template"], "%(title)s");
    assert_eq!(etag(&headers), read_etag(&harness).await);

    // `null` empties a field that can be empty, the one the document then leaves out included,
    // and is refused for one that cannot.
    let (status, _, body) = write(
        &harness,
        "PATCH",
        API_BEARER,
        None,
        &json!({ "media_output_template": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["media_output_template"], json!(null));
    let (status, _, body) = write(
        &harness,
        "PATCH",
        API_BEARER,
        None,
        &json!({ "max_retries": null }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "settings.invalid");
    assert_eq!(body["params"]["key"], "max_retries");
    for (patch, code) in [
        (json!({}), "settings.patch_empty"),
        (json!({ "max_retires": 1 }), "settings.unknown_key"),
        (
            json!({ "max_retries": 500 }),
            "settings.max_retries_too_high",
        ),
    ] {
        let (status, _, body) = write(&harness, "PATCH", API_BEARER, None, &patch).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{patch}: {body}");
        assert_eq!(body["code"], code, "{patch}");
    }
    assert_eq!(stored(&harness).await["max_retries"], 7);

    let (status, _, body) = write(
        &harness,
        "PUT",
        API_BEARER,
        None,
        &json!({ "max_active_files": 4 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = stored(&harness).await;
    assert_eq!(after["max_active_files"], 4);
    assert_ne!(after["max_retries"], 7, "PUT replaces the whole document");
}

/// A stale tag saves nothing, on both writes; the stored one, a weakened one and a star pass.
#[tokio::test]
async fn a_write_naming_another_document_is_refused_and_changes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let first = read_etag(&harness).await;
    assert_eq!(first, read_etag(&harness).await, "unchanged, same tag");
    let (status, headers, body) = write(
        &harness,
        "PATCH",
        API_BEARER,
        Some(&first),
        &json!({ "max_retries": 7 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let second = etag(&headers);
    assert_ne!(first, second);

    let whole = stored(&harness).await;
    for (method, body) in [("PATCH", json!({ "max_retries": 1 })), ("PUT", whole)] {
        let (status, _, refused) = write(&harness, method, API_BEARER, Some(&first), &body).await;
        assert_eq!(
            status,
            StatusCode::PRECONDITION_FAILED,
            "{method}: {refused}"
        );
        assert_eq!(refused["code"], "settings.document_changed", "{method}");
    }
    assert_eq!(stored(&harness).await["max_retries"], 7);

    for tag in ["*".to_owned(), format!("W/{second}"), second.clone()] {
        let (status, _, body) = write(
            &harness,
            "PATCH",
            API_BEARER,
            Some(&tag),
            &json!({ "max_retries": 7 }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{tag}: {body}");
    }
}

/// The administrator's fields stay the administrator's: by value, by `null`, hidden among others
/// and with a matching condition. Each refusal is audited like the one of `PUT`.
#[tokio::test]
async fn a_configuration_token_cannot_patch_a_privileged_field() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let (status, _, body) = write(
        &harness,
        "PATCH",
        API_BEARER,
        None,
        &json!({ "external_url": "https://downloads.example" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = stored(&harness).await;
    let tag = read_etag(&harness).await;

    for (field, patch) in [
        (
            "admin_login_disabled",
            json!({ "admin_login_disabled": true }),
        ),
        ("external_url", json!({ "external_url": null })),
        (
            "allowed_hosts",
            json!({ "max_retries": 3, "allowed_hosts": ["attacker.example"] }),
        ),
        (
            "completion_script",
            json!({ "completion_script": "/tmp/run.sh" }),
        ),
        (
            "mcp_scripts_allowed",
            json!({ "mcp_scripts_allowed": true }),
        ),
    ] {
        for condition in [None, Some(tag.as_str())] {
            let (status, _, body) =
                write(&harness, "PATCH", CONFIG_BEARER, condition, &patch).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{field}: {body}");
            assert_eq!(body["code"], "auth.scope_insufficient", "{field}");
            assert_eq!(body["params"]["setting"], field);
        }
        let records = settings_records(&harness).await;
        let newest = records.first().expect("an audit record");
        assert_eq!(newest["outcome"], "failure", "{field}: {newest}");
        assert_eq!(newest["details"]["refused_field"], field, "{newest}");
    }
    assert_eq!(
        stored(&harness).await,
        before,
        "a refused patch saved something"
    );

    // An ordinary field passes for the same token, and the record names it without its value.
    let (status, body) = patch_with_bearer(
        &harness.router,
        SETTINGS,
        CONFIG_BEARER,
        json!({ "record_default_quality": "720p60-canary" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let records = settings_records(&harness).await;
    let newest = records.first().expect("an audit record");
    assert_eq!(newest["outcome"], "success", "{newest}");
    assert_eq!(newest["details"]["fields"], "record_default_quality");
    assert!(!newest.to_string().contains("720p60-canary"), "{newest}");
}

/// Reading the document costs `api:config`, and so does patching it.
#[tokio::test]
async fn a_read_only_token_patches_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let (status, _, body) = write(
        &harness,
        "PATCH",
        common::READ_BEARER,
        None,
        &json!({ "max_retries": 7 }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// Every write of the document announces the fields it changed and no value; one that changed
/// nothing announces nothing.
#[tokio::test]
async fn a_change_announces_the_names_of_its_fields() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let mut events = harness.database.subscribe();

    let change = json!({ "max_retries": 7, "record_default_quality": "720p60-canary" });
    for expected in [
        vec![json!({ "fields": ["max_retries", "record_default_quality"] })],
        // The same patch again.
        Vec::new(),
    ] {
        let (status, _, body) = write(&harness, "PATCH", API_BEARER, None, &change).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(announced(&mut events), expected);
    }

    let mut whole = stored(&harness).await;
    whole["max_retries"] = json!(6);
    let (status, _, body) = write(&harness, "PUT", API_BEARER, None, &whole).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        announced(&mut events),
        [json!({ "fields": ["max_retries"] })]
    );

    // The two writers beside the settings routes.
    let (status, body) = common::put_with_bearer(
        &harness.router,
        "/api/v1/bandwidth/schedule",
        API_BEARER,
        json!({ "timezone": "Pacific/Auckland", "default_profile_id": null, "windows": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        announced(&mut events),
        [json!({ "fields": ["bandwidth_timezone"] })]
    );

    let (status, body) = patch_with_bearer(
        &harness.router,
        "/api/v1/plugins/example.plugin",
        API_BEARER,
        json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        announced(&mut events),
        [json!({ "fields": ["disabled_plugins"] })]
    );
}

/// Two writers side by side: both changes are there afterwards.
#[tokio::test]
async fn patches_side_by_side_lose_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let patches = [
        json!({ "max_retries": 7 }),
        json!({ "max_active_files": 9 }),
        json!({ "audit_retention_days": 90 }),
        json!({ "hotfolder_poll_seconds": 42 }),
    ];
    let answers = tokio::join!(
        write(&harness, "PATCH", API_BEARER, None, &patches[0]),
        write(&harness, "PATCH", API_BEARER, None, &patches[1]),
        write(&harness, "PATCH", API_BEARER, None, &patches[2]),
        write(&harness, "PATCH", API_BEARER, None, &patches[3]),
    );
    for (status, _, body) in [answers.0, answers.1, answers.2, answers.3] {
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let after = stored(&harness).await;
    for patch in patches {
        for (field, value) in patch.as_object().expect("object") {
            assert_eq!(&after[field], value, "{field}");
        }
    }
}

/// The `settings_changed` audit records, newest first.
async fn settings_records(harness: &Harness) -> Vec<serde_json::Value> {
    harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 100,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit")
        .into_iter()
        .map(|record| serde_json::to_value(record).expect("json"))
        .filter(|record| record["action"] == "settings_changed")
        .collect()
}
