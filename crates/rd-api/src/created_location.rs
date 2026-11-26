//! `Location` on a `201` whose resource has a single read (RD-1480-10, API-16).
//!
//! One layer rather than a header in every create handler: whether the resource can be read by
//! its id is the scope table's knowledge, so a new `GET <collection>/{id}` brings the header with
//! it, and no handler (the MCP tools call the same ones) changes what it returns. A collection
//! without one keeps its bare `201`: a `Location` that answers `405` says nothing.

use axum::{
    body::Body,
    extract::{MatchedPath, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rd_api_core::scope_policy;
use utoipa::openapi::{HeaderBuilder, Object, OpenApi, RefOr, Type};

use crate::{ApiError, AppState};

/// Whether `POST` to `collection` creates what `GET <collection>/{id}` reads.
fn has_single_read(collection: &str) -> bool {
    scope_policy::requirement(&format!("{collection}/{{id}}"), &Method::GET).is_some()
}

/// The id a create answered: its own, or the token's beside the bearer it is shown once with.
fn created_id(body: &[u8]) -> Option<String> {
    let body: serde_json::Value = serde_json::from_slice(body).ok()?;
    body.get("id")
        .or_else(|| body.pointer("/token/id"))?
        .as_str()
        .map(ToOwned::to_owned)
}

/// Adds the header to a `201` of a collection whose rows can be read one by one.
pub(crate) async fn add(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let collection = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .filter(|path| *request.method() == Method::POST && has_single_read(path));
    let response = next.run(request).await;
    match collection {
        Some(collection) if response.status() == StatusCode::CREATED => {
            located(&state, &collection, response).await
        }
        _ => response,
    }
}

async fn located(state: &AppState, collection: &str, response: Response) -> Response {
    let (mut parts, body) = response.into_parts();
    // A handler's `Json`, so it is in memory already and reading it waits for nothing.
    let body = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(error) => return ApiError::from(anyhow::Error::new(error)).into_response(),
    };
    if let Some(id) = created_id(&body) {
        // Under the mount point the service is reached by, which it accepts with and without.
        let base = state.access.proxy.read().await.base_path().to_owned();
        if let Ok(value) = HeaderValue::from_str(&format!("{base}{collection}/{id}")) {
            parts.headers.insert(header::LOCATION, value);
        }
    }
    Response::from_parts(parts, Body::from(body))
}

/// Says in the document which `201` carries the header, from the same table [`add`] asks.
pub(crate) fn declare(document: &mut OpenApi) {
    for (path, item) in &mut document.paths.paths {
        if !has_single_read(path) {
            continue;
        }
        let Some(operation) = &mut item.post else {
            continue;
        };
        if let Some(RefOr::T(response)) = operation.responses.responses.get_mut("201") {
            let header = HeaderBuilder::new()
                .schema(Some(Object::with_type(Type::String)))
                .description(Some("Where the created resource is read"))
                .build();
            response
                .headers
                .insert("location".to_owned(), RefOr::T(header));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{created_id, has_single_read};

    #[test]
    fn the_scope_table_says_which_collections_have_a_single_read() {
        assert!(has_single_read("/api/v1/downloads"));
        assert!(has_single_read("/api/v1/notifications/targets"));
        assert!(!has_single_read("/api/v1/backups/restore/uploads"));
        assert!(!has_single_read("/api/v1/downloads/bulk"));
    }

    #[test]
    fn the_id_is_the_rows_own_or_the_tokens() {
        assert_eq!(
            created_id(br#"{"id":"a","token":{"id":"b"}}"#).as_deref(),
            Some("a")
        );
        assert_eq!(
            created_id(br#"{"bearer":"x","token":{"id":"b"}}"#).as_deref(),
            Some("b")
        );
        assert_eq!(created_id(br#"{"code":"x"}"#), None);
        assert_eq!(created_id(b"no json"), None);
    }

    /// Every collection with a create and a single read says so in the document, and no other.
    #[test]
    fn the_document_declares_the_header_where_it_is_sent() {
        let document = serde_json::to_value(crate::openapi_document()).expect("serialise");
        let paths = document["paths"].as_object().expect("paths");
        let mut declared = 0;
        for (path, item) in paths {
            let Some(created) = item.pointer("/post/responses/201") else {
                continue;
            };
            let header = created.pointer("/headers/location").is_some();
            let single = paths
                .get(&format!("{path}/{{id}}"))
                .is_some_and(|item| item.get("get").is_some());
            assert_eq!(header, single, "POST {path}");
            declared += usize::from(header);
        }
        assert!(declared >= 20, "only {declared} creates declare Location");
    }
}
