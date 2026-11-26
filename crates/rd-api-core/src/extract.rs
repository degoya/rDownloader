//! The request extractors of the REST surface (RD-1480-08).
//!
//! axum's own `Json`, `Path` and `Query` refuse a malformed request with plain text, while every
//! other REST error is an [`ErrorBody`](crate::error::ErrorBody) whose stable `code` a client
//! translates. These three read exactly what axum's read, refuse with exactly the status axum
//! refuses with, and answer in the one error format:
//!
//! * [`Json`]: `request.body_invalid` (`400` for text that is no JSON, `422` for JSON of another
//!   shape), `request.content_type_unsupported` (`415`), `request.body_too_large` (`413`),
//! * [`Path`] answers `request.invalid_id`,
//! * [`Query`] answers `request.query_invalid`.
//!
//! A route that answers a malformed request with a code of its own keeps its own extractor
//! ([`Page`](crate::list_bounds::Page), [`UploadBody`](crate::container_upload::UploadBody)).
//! The compatibility adapters keep axum's: their errors are in the format of the program they
//! stand in for. `repo_lints/rest_extractors.rs` holds both.

use std::ops::{Deref, DerefMut};

use axum::{
    extract::{
        FromRequest, FromRequestParts, OptionalFromRequest, Request,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{StatusCode, Uri, request::Parts},
    response::{IntoResponse, Response},
};
use serde::{Serialize, de::DeserializeOwned};

use crate::ApiError;

/// A JSON request body, or a JSON answer.
#[derive(Clone, Copy, Debug, Default)]
#[must_use]
pub struct Json<T>(pub T);

/// The typed parameters of the matched route.
#[derive(Debug)]
pub struct Path<T>(pub T);

/// The typed query string.
#[derive(Clone, Copy, Debug, Default)]
pub struct Query<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match <axum::Json<T> as FromRequest<S>>::from_request(request, state).await {
            Ok(axum::Json(body)) => Ok(Self(body)),
            Err(rejection) => Err(body_refused(&rejection)),
        }
    }
}

/// `None` for a request without a `Content-Type`, as axum reads an optional body.
impl<T, S> OptionalFromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Option<Self>, Self::Rejection> {
        match <axum::Json<T> as OptionalFromRequest<S>>::from_request(request, state).await {
            Ok(body) => Ok(body.map(|axum::Json(body)| Self(body))),
            Err(rejection) => Err(body_refused(&rejection)),
        }
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

impl<T, S> FromRequestParts<S> for Path<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match <axum::extract::Path<T> as FromRequestParts<S>>::from_request_parts(parts, state)
            .await
        {
            Ok(axum::extract::Path(parameters)) => Ok(Self(parameters)),
            Err(rejection) => Err(path_refused(&rejection)),
        }
    }
}

impl<T, S> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Self::try_from_uri(&parts.uri)
    }
}

impl<T: DeserializeOwned> Query<T> {
    /// The query string of `uri`, for an extractor that refuses with a code of its own.
    ///
    /// # Errors
    ///
    /// `request.query_invalid` for a query string of another shape.
    pub fn try_from_uri(uri: &Uri) -> Result<Self, ApiError> {
        match axum::extract::Query::<T>::try_from_uri(uri) {
            Ok(axum::extract::Query(query)) => Ok(Self(query)),
            Err(rejection) => Err(query_refused(&rejection)),
        }
    }
}

impl<T> From<T> for Json<T> {
    fn from(body: T) -> Self {
        Self(body)
    }
}

impl<T> Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Json<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> Deref for Path<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Deref for Query<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn body_refused(rejection: &JsonRejection) -> ApiError {
    match rejection.status() {
        StatusCode::PAYLOAD_TOO_LARGE => crate::container_upload::body_too_large(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => ApiError::unsupported_media_type(
            "request.content_type_unsupported",
            rejection.body_text(),
        )
        .with_param("expected", "application/json"),
        status => refused(status, "request.body_invalid", rejection.body_text()),
    }
}

fn path_refused(rejection: &PathRejection) -> ApiError {
    refused(
        rejection.status(),
        "request.invalid_id",
        rejection.body_text(),
    )
}

fn query_refused(rejection: &QueryRejection) -> ApiError {
    refused(
        rejection.status(),
        "request.query_invalid",
        rejection.body_text(),
    )
}

/// axum's status and text under a stable code. A status of the service's own -- a handler that
/// names more parameters than its route has -- is the coded `500` every other fault is.
fn refused(status: StatusCode, code: &'static str, text: String) -> ApiError {
    if status.is_server_error() {
        return ApiError::from(anyhow::anyhow!(text));
    }
    ApiError::with_status(status, code, text)
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
