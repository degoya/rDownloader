//! Assembles the OpenAPI document from the per-area operation sets.

mod schemas;

use utoipa::OpenApi;

use crate::routes;

/// Document metadata and tags; operations and schemas are merged in.
#[derive(OpenApi)]
#[openapi(
    tags(
        (name = "system", description = "Service health and settings"),
        (name = "downloads", description = "Persistent download queue"),
        (name = "collector", description = "LinkGrabber intake"),
        (name = "diagnostics", description = "Structured log store and diagnostic bundle")
    )
)]
struct Base;

/// The complete document served at `/api/v1/openapi.json`.
pub fn document() -> utoipa::openapi::OpenApi {
    let mut doc = Base::openapi();
    doc.merge(schemas::Schemas::openapi());
    doc.merge(routes::automations::Doc::openapi());
    doc.merge(routes::collector::Doc::openapi());
    doc.merge(routes::config::Doc::openapi());
    doc.merge(routes::media::Doc::openapi());
    doc.merge(routes::notify::Doc::openapi());
    doc.merge(routes::plugins::Doc::openapi());
    doc.merge(routes::queue::Doc::openapi());
    doc.merge(routes::restore::Doc::openapi());
    doc.merge(routes::security::Doc::openapi());
    doc.merge(routes::site_rules::Doc::openapi());
    doc.merge(routes::system::Doc::openapi());
    doc.merge(routes::torrent::Doc::openapi());
    doc.merge(routes::usenet::Doc::openapi());
    doc.merge(routes::stats::Doc::openapi());
    doc.merge(routes::storage::Doc::openapi());
    doc.merge(routes::diagnostics::Doc::openapi());
    doc.merge(routes::lifecycle::Doc::openapi());
    doc.merge(routes::audit::Doc::openapi());
    doc.merge(routes::history::Doc::openapi());
    describe_bare_responses(&mut doc);
    doc
}

/// Gives every response declared without a description its status's reason phrase.
///
/// OpenAPI 3.1 requires `description` on a response object. utoipa 5 wrote an empty string for
/// a response a handler declared without one; utoipa 6 leaves the empty field out, which would
/// make the document invalid 3.1 for every client generator that checks it.
fn describe_bare_responses(doc: &mut utoipa::openapi::OpenApi) {
    for item in doc.paths.paths.values_mut() {
        let operations = [
            &mut item.get,
            &mut item.put,
            &mut item.post,
            &mut item.delete,
            &mut item.options,
            &mut item.head,
            &mut item.patch,
            &mut item.trace,
            &mut item.query,
        ];
        for operation in operations.into_iter().flatten() {
            for (status, response) in &mut operation.responses.responses {
                if let utoipa::openapi::RefOr::T(response) = response
                    && response.description.is_empty()
                {
                    response.description = axum::http::StatusCode::from_bytes(status.as_bytes())
                        .ok()
                        .and_then(|code| code.canonical_reason())
                        .unwrap_or("Response")
                        .to_owned();
                }
            }
        }
    }
}

#[cfg(test)]
mod secret_boundary_tests {
    /// Property names that carry a credential somewhere in the API surface.
    const SENSITIVE: &[&str] = &[
        "password",
        "secret",
        "cookies",
        "certificate_pem",
        "private_key",
        "passphrase",
        "api_key",
        "client_secret",
        "access_token",
        "refresh_token",
    ];

    /// The only readable credential in the whole document: the archive password of a package.
    ///
    /// It is public anyway — it sits in the release title, in the `{{password}}` marker of a
    /// file name or in the feed — and a person who unpacks by hand needs it (RD-104-04).
    const READABLE: &[(&str, &str)] = &[
        ("DownloadPackage", "password"),
        ("CollectorPackage", "password"),
        ("NzbImport", "password"),
        ("SubscriptionItem", "password"),
        ("CollectorIntakeRequest", "password"),
        ("CollectorPackageUpdateRequest", "password"),
        ("PackageUpdateRequest", "password"),
        // Not an archive password, and the one other deliberate exception: enrolment has to
        // hand the TOTP secret back once, for an authenticator that cannot scan a QR code.
        ("TotpEnrolment", "secret"),
    ];

    /// RD-104-04 opened exactly one door. This test is the doorstop: an account password, an
    /// API key, an NNTP or proxy credential, a private key or a cookie jar must never become
    /// readable by accident, so every other sensitive property stays `writeOnly`.
    #[test]
    fn only_the_archive_password_is_readable() {
        let document = serde_json::to_value(super::document()).expect("serialize OpenAPI");
        let schemas = document["components"]["schemas"]
            .as_object()
            .expect("component schemas");
        let mut leaked: Vec<String> = Vec::new();
        let mut readable: Vec<String> = Vec::new();
        for (name, schema) in schemas {
            let Some(properties) = schema.get("properties").and_then(|value| value.as_object())
            else {
                continue;
            };
            for (property, definition) in properties {
                if !SENSITIVE.contains(&property.as_str()) {
                    continue;
                }
                let write_only = definition
                    .get("writeOnly")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let allowed = READABLE
                    .iter()
                    .any(|(schema_name, field)| schema_name == name && field == property);
                if allowed {
                    readable.push(format!("{name}.{property}"));
                    assert!(
                        !write_only,
                        "{name}.{property} is meant to be readable but is marked writeOnly"
                    );
                } else if !write_only {
                    leaked.push(format!("{name}.{property}"));
                }
            }
        }
        assert!(
            leaked.is_empty(),
            "these credentials would be handed back by the API: {leaked:?}"
        );
        assert_eq!(
            readable.len(),
            READABLE.len(),
            "expected exactly the archive-password fields to be readable, found {readable:?}"
        );
    }
}

#[cfg(test)]
mod paging_tests {
    /// A paged list says `X-Total-Count` in the document, not only on the wire, so a client
    /// generated from it can read the header (audit 1.9.1, RA-API-03). Found by its
    /// `PageQuery` parameters, so a seventh paged list is held to the same.
    #[test]
    fn every_paged_list_declares_its_total_count_header() {
        let document = serde_json::to_value(super::document()).expect("serialize OpenAPI");
        let paths = document["paths"].as_object().expect("paths");
        let mut paged = 0;
        for (path, item) in paths {
            let Some(get) = item.get("get") else {
                continue;
            };
            // `PageQuery` and nothing else: the audit, the logs and a subscription's items page
            // with a cursor or a body of their own.
            let takes_a_window = get["parameters"].as_array().is_some_and(|parameters| {
                let mut names: Vec<&str> = parameters
                    .iter()
                    .filter(|parameter| parameter["in"] == "query")
                    .filter_map(|parameter| parameter["name"].as_str())
                    .collect();
                names.sort_unstable();
                names == ["limit", "offset"]
            });
            if !takes_a_window {
                continue;
            }
            paged += 1;
            assert!(
                get["responses"]["200"]["headers"]
                    .get("x-total-count")
                    .is_some(),
                "GET {path} pages but does not declare X-Total-Count"
            );
        }
        assert_eq!(paged, 6, "the paged lists of API-15");
    }
}
