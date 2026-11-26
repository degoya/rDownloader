//! The refusals every operation shares, and the one body all of them have (RD-1480-09).
//!
//! What a handler decides stays declared at the handler. What is decided in front of it -- the
//! credential, the permission area, the token's call limit, whether the request reads at all --
//! is the same for every operation and is added here, as references to one response each, where
//! the handler declares nothing more precise under that status.

use rd_api_core::scope_policy::{Requirement, requirement};
use rd_core::Scope;
use utoipa::openapi::{
    Content, HeaderBuilder, Object, OpenApi, Ref, RefOr, Response, ResponseBuilder, Type,
    path::{Operation, ParameterIn},
};

use super::{access, operations};

const JSON: &str = "application/json";

/// The shared refusals: status, component name, what it means.
const SHARED: &[(&str, &str, &str)] = &[
    (
        "400",
        "BadRequest",
        "The request does not read or a value in it is refused: `request.body_invalid`, \
         `request.invalid_id`, `request.query_invalid` or a code of the operation's own",
    ),
    (
        "401",
        "Unauthorized",
        "No credential, or one that does not stand: `auth.session_required`, \
         `auth.setup_pending` or `capture.token_required`",
    ),
    (
        "403",
        "Forbidden",
        "The credential does not hold the permission area of the operation: \
         `auth.scope_insufficient` with `scope`",
    ),
    (
        "413",
        "PayloadTooLarge",
        "The body is larger than the operation takes: `request.body_too_large`",
    ),
    (
        "415",
        "UnsupportedMediaType",
        "The body is not declared as the type the operation reads: \
         `request.content_type_unsupported` with `expected`",
    ),
    (
        "422",
        "UnprocessableContent",
        "The JSON does not have the expected shape: `request.body_invalid`",
    ),
    (
        "429",
        "TooManyRequests",
        "The API token made more calls this minute than its limit allows: \
         `api.token_rate_limited` with `limit` and `seconds`",
    ),
];

/// The operations that take a JSON document as raw bytes and parse it themselves: a document of
/// another shape is their own `400`.
const PARSED_BY_HAND: &[&str] = &["preview_site_rule_import", "import_site_rules"];

/// Adds the shared refusals and gives every refusal without a schema the error body.
pub(super) fn describe(doc: &mut OpenApi) {
    let components = doc.components.get_or_insert_with(Default::default);
    for (status, name, description) in SHARED {
        let mut response = ResponseBuilder::new()
            .description(*description)
            .content(JSON, error_body());
        if *status == "429" {
            response = response.header(
                "Retry-After",
                HeaderBuilder::new()
                    .schema(Some(Object::with_type(Type::Integer)))
                    .description(Some("Seconds until the next call is admitted"))
                    .build(),
            );
        }
        components
            .responses
            .insert((*name).to_owned(), RefOr::T(response.build()));
    }
    operations::each(doc, |path, method, operation| {
        for status in shared_by(path, method, operation) {
            let Some((_, name, _)) = SHARED.iter().find(|(shared, _, _)| *shared == status) else {
                continue;
            };
            operation
                .responses
                .responses
                .entry(status.to_owned())
                .or_insert_with(|| RefOr::Ref(Ref::from_response_name(*name)));
        }
        for (status, response) in &mut operation.responses.responses {
            if let RefOr::T(response) = response
                && is_refusal(status)
                && response.content.is_empty()
            {
                with_error_body(response);
            }
        }
    });
}

/// The statuses decided in front of the operation's handler.
fn shared_by(path: &str, method: &axum::http::Method, operation: &Operation) -> Vec<&'static str> {
    let mut statuses = Vec::new();
    if let Some(Requirement::Scope(scope)) = requirement(path, method) {
        statuses.push("401");
        // Every capture token holds `capture:*`, so that area refuses nobody who has one; and a
        // capture token has no call limit.
        if scope != Scope::Capture {
            statuses.push("403");
        }
        if !access::is_capture(scope) {
            statuses.push("429");
        }
    }
    let body = operations::body(operation);
    let reads_a_parameter =
        operation
            .parameters
            .iter()
            .flatten()
            .any(|parameter| match parameter {
                RefOr::T(parameter) => {
                    matches!(
                        parameter.parameter_in,
                        ParameterIn::Path | ParameterIn::Query
                    )
                }
                RefOr::Ref(_) => true,
            });
    if body.is_some() || reads_a_parameter {
        statuses.push("400");
    }
    // Only a JSON request's oversized body is answered in the error format
    // (`container_upload::code_oversized_json`); an upload's is the limit layer's bare `413`.
    if let Some(body) = body.filter(|body| body.content.contains_key(JSON)) {
        statuses.push("413");
        if body.content.len() == 1 {
            statuses.push("415");
            let parsed_by_hand = operation
                .operation_id
                .as_deref()
                .is_some_and(|id| PARSED_BY_HAND.contains(&id));
            if !parsed_by_hand {
                statuses.push("422");
            }
        }
    }
    statuses
}

/// Whether a response declared under this status is a refusal.
pub(super) fn is_refusal(status: &str) -> bool {
    status.starts_with(['4', '5'])
}

fn error_body() -> Content {
    Content::new(Some(Ref::from_schema_name("ErrorBody")))
}

fn with_error_body(response: &mut Response) {
    response
        .content
        .insert(JSON.to_owned(), RefOr::T(error_body()));
}
