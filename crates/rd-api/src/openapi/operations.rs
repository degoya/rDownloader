//! Every operation of the assembled document with its path and method, for what is said about
//! all of them in one place rather than at each handler.

use axum::http::Method;
use utoipa::openapi::{OpenApi, RefOr, path::Operation, request_body::RequestBody};

/// Calls `visit` on every operation.
pub(super) fn each(doc: &mut OpenApi, mut visit: impl FnMut(&str, &Method, &mut Operation)) {
    for (path, item) in &mut doc.paths.paths {
        for (method, operation) in [
            (Method::GET, &mut item.get),
            (Method::PUT, &mut item.put),
            (Method::POST, &mut item.post),
            (Method::DELETE, &mut item.delete),
            (Method::PATCH, &mut item.patch),
            (Method::HEAD, &mut item.head),
            (Method::OPTIONS, &mut item.options),
        ] {
            if let Some(operation) = operation {
                visit(path, &method, operation);
            }
        }
    }
}

/// The request body an operation declares. No handler declares one by reference.
pub(super) fn body(operation: &Operation) -> Option<&RequestBody> {
    match operation.request_body.as_ref()? {
        RefOr::T(body) => Some(body),
        RefOr::Ref(_) => None,
    }
}
