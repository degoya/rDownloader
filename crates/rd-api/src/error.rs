use std::{borrow::Cow, collections::BTreeMap};

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

/// Flat string parameters referenced by a translated message.
pub type MessageParams = BTreeMap<String, String>;

/// Redaction-safe REST error: English text plus a stable code clients translate.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    /// Borrowed for the codes this crate owns, owned for the ones it does not.
    ///
    /// Almost every code here is a literal in this crate, and a `&'static str` says so. A
    /// plugin's is not: a crawler reports `premiumize_crawler.folder_empty` from a catalogue
    /// that arrived with the package, and the alternatives to an owned string were leaking it
    /// or throwing the code away and leaving the interface with English prose it cannot
    /// translate.
    code: Cow<'static, str>,
    message: String,
    params: MessageParams,
}

impl ApiError {
    fn with_status(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: Cow::Borrowed(code),
            message: message.into(),
            params: BTreeMap::new(),
        }
    }

    /// Creates a client error.
    #[must_use]
    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::BAD_REQUEST, code, message)
    }

    /// Creates a client error whose code was not known at compile time.
    ///
    /// The only source of one is a plugin: what a crawler or a resolver refuses with carries
    /// its own stable code, translated by the catalogue its package shipped.
    #[must_use]
    pub fn bad_request_owned(code: String, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: Cow::Owned(code),
            message: message.into(),
            params: BTreeMap::new(),
        }
    }

    /// Creates an authentication error.
    #[must_use]
    pub fn unauthorized(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::UNAUTHORIZED, code, message)
    }

    /// Creates an authorization error: authenticated, but not permitted.
    #[must_use]
    pub fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::FORBIDDEN, code, message)
    }

    /// Creates a missing-resource error.
    #[must_use]
    pub fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::NOT_FOUND, code, message)
    }

    /// Creates a conflict error.
    #[must_use]
    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::CONFLICT, code, message)
    }

    /// Creates a "understood but cannot be fulfilled" error.
    ///
    /// Used where the request is well-formed and the resource exists, but the combination
    /// asked for has no answer — a filter set that matches no format, for instance.
    #[must_use]
    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }

    /// Creates a "the body is larger than this route takes" error.
    #[must_use]
    pub fn payload_too_large(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::PAYLOAD_TOO_LARGE, code, message)
    }

    /// Creates a rate-limit error.
    #[must_use]
    pub fn too_many_requests(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::TOO_MANY_REQUESTS, code, message)
    }

    /// Creates an upstream connectivity error.
    #[must_use]
    pub fn bad_gateway(code: &'static str, message: impl Into<String>) -> Self {
        Self::with_status(StatusCode::BAD_GATEWAY, code, message)
    }

    /// Creates an upstream connectivity error whose code was not known at compile time.
    ///
    /// The counterpart of [`bad_request_owned`] for the other direction: a remote-job plugin
    /// refuses with its own stable code, and a provider that said no is not a client mistake.
    /// Without this the choice was between reporting it as a 400 and throwing the code away.
    ///
    /// [`bad_request_owned`]: ApiError::bad_request_owned
    #[must_use]
    pub fn bad_gateway_owned(code: String, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: Cow::Owned(code),
            message: message.into(),
            params: BTreeMap::new(),
        }
    }

    /// Attaches a translation parameter (counts, names, limits).
    ///
    /// Parameters routinely carry a URL or a host, so the value is redacted on the way in
    /// rather than trusting every call site to remember.
    #[must_use]
    pub fn with_param(mut self, key: &str, value: impl ToString) -> Self {
        self.params
            .insert(key.to_owned(), rd_core::redact_text(&value.to_string()));
        self
    }

    /// Human-readable message (for logging without leaking the response type).
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Stable translation code.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        tracing::error!(error = %rd_core::redact_text(&error.to_string()), "request failed");
        Self::with_status(
            StatusCode::INTERNAL_SERVER_ERROR,
            crate::error_codes::INTERNAL_ERROR,
            "Internal service error",
        )
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        anyhow::Error::new(error).into()
    }
}

/// JSON body of every REST error.
#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    /// English, redaction-safe text.
    pub error: String,
    /// Stable code such as `package.not_found`.
    pub code: String,
    /// Flat parameters referenced by the translated text.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: MessageParams,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                // Total, retroactive cover: every handler's message passes here, so a new
                // endpoint cannot leak a signed URL by forgetting to redact.
                error: rd_core::redact_text(&self.message),
                code: self.code.into_owned(),
                params: self.params,
            }),
        )
            .into_response()
    }
}
