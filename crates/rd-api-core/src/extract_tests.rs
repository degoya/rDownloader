//! A refused body, path or query string carries axum's status and a stable code (RD-1480-08).

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::DefaultBodyLimit,
    http::{Request, StatusCode, header},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower::ServiceExt as _;

use super::{Json, Path, Query};

#[derive(Debug, Deserialize, Serialize)]
struct Named {
    name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Counted {
    count: Option<u32>,
}

fn router() -> Router {
    Router::new()
        .route(
            "/body",
            post(|Json(body): Json<Named>| async move { Json(body) })
                .layer(DefaultBodyLimit::max(64)),
        )
        .route(
            "/optional",
            post(|body: Option<Json<Named>>| async move { Json(body.map(|Json(body)| body)) }),
        )
        .route(
            "/path/{id}",
            get(|Path(id): Path<uuid::Uuid>| async move { Json(id) }),
        )
        .route(
            "/short/{id}",
            get(|Path(pair): Path<(String, String)>| async move { Json(pair) }),
        )
        .route(
            "/query",
            get(|Query(query): Query<Counted>| async move { Json(query) }),
        )
}

async fn send(request: Request<Body>) -> (StatusCode, Value) {
    let response = router().oneshot(request).await.expect("response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = serde_json::from_slice(&body).expect("every answer is JSON");
    (status, body)
}

fn posted(uri: &str, content_type: Option<&str>, body: &str) -> Request<Body> {
    let mut request = Request::post(uri);
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    request.body(Body::from(body.to_owned())).expect("request")
}

fn asked(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).expect("request")
}

const JSON: Option<&str> = Some("application/json");

#[tokio::test]
async fn a_refused_body_keeps_its_status_and_gains_a_code() {
    for (content_type, body, status, code) in [
        (JSON, "{", StatusCode::BAD_REQUEST, "request.body_invalid"),
        (JSON, "", StatusCode::BAD_REQUEST, "request.body_invalid"),
        (
            JSON,
            "{\"name\":7}",
            StatusCode::UNPROCESSABLE_ENTITY,
            "request.body_invalid",
        ),
        (
            Some("text/plain"),
            "{\"name\":\"a\"}",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "request.content_type_unsupported",
        ),
        (
            None,
            "{\"name\":\"a\"}",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "request.content_type_unsupported",
        ),
    ] {
        let (answered, refusal) = send(posted("/body", content_type, body)).await;
        assert_eq!(answered, status, "{body}: {refusal}");
        assert_eq!(refusal["code"], code, "{body}: {refusal}");
        assert!(refusal["error"].is_string(), "{body}: {refusal}");
    }

    let long = format!("{{\"name\":\"{}\"}}", "a".repeat(80));
    let (status, refusal) = send(posted("/body", JSON, &long)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{refusal}");
    assert_eq!(refusal["code"], "request.body_too_large", "{refusal}");

    let (_, refusal) = send(posted("/body", None, "{}")).await;
    assert_eq!(refusal["params"]["expected"], "application/json");
}

#[tokio::test]
async fn an_accepted_body_reads_as_before() {
    let (status, body) = send(posted(
        "/body",
        Some("application/vnd.x+json; charset=utf-8"),
        "{\"name\":\"a\"}",
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "a");

    // An optional body is the one thing a missing Content-Type means nothing for.
    let (status, body) = send(posted("/optional", None, "")).await;
    assert_eq!((status, body), (StatusCode::OK, Value::Null));
    let (status, body) = send(posted("/optional", JSON, "{\"name\":\"a\"}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "a");
    let (status, body) = send(posted("/optional", JSON, "[]")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "request.body_invalid");
}

#[tokio::test]
async fn a_refused_path_or_query_string_is_coded() {
    let (status, body) = send(asked("/path/not-an-id")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "request.invalid_id");

    let (status, body) = send(asked("/query?count=many")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "request.query_invalid");

    let (status, body) = send(asked("/query?count=3")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 3);
}

/// A handler that asks for more parameters than its route has is a fault of the service.
#[tokio::test]
async fn a_route_fault_stays_a_500_without_its_reason() {
    let (status, body) = send(asked("/short/one")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["code"], crate::error_codes::INTERNAL_ERROR);
}
