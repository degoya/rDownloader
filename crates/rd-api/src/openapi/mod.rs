//! Assembles the OpenAPI document from the per-area operation sets.

mod access;
mod events;
mod operations;
mod refusals;
mod schemas;
mod schemas_capture;
mod schemas_diagnostics;
mod schemas_intake;
mod schemas_queue;

use utoipa::OpenApi;

use crate::routes;

/// Document metadata and tags; operations and schemas are merged in. Every tag an operation
/// carries has its description here (`completeness_tests`).
#[derive(OpenApi)]
#[openapi(
    components(schemas(rd_core::EventEnvelope, rd_core::EventKind)),
    tags(
        (name = "api-tokens", description = "API tokens: issue, re-scope, limit and revoke"),
        (name = "audit", description = "The audit log: who did what, from where"),
        (name = "auth", description = "Signing in: first setup, password, passkeys and the identity provider"),
        (name = "automations", description = "Rules that act on events, and their runs"),
        (name = "bandwidth", description = "Bandwidth profiles, the weekly schedule and the limit in force"),
        (name = "captcha", description = "Captchas waiting to be solved"),
        (name = "capture", description = "The desktop agent and the browser extension: pairing, settings, and what they call with a capture token"),
        (name = "collector", description = "LinkGrabber intake"),
        (name = "configuration", description = "Accounts, categories, folders, proxies, stored sign-ins, site rules and their import and export"),
        (name = "diagnostics", description = "Structured log store and diagnostic bundle"),
        (name = "downloads", description = "Persistent download queue"),
        (name = "history", description = "What was downloaded, after it left the queue"),
        (name = "indexers", description = "Newznab and Torznab indexers: configuration and search"),
        (name = "notifications", description = "Notification targets, rules, templates and deliveries"),
        (name = "plugins", description = "Installed plugins, their keys, repositories and updates"),
        (name = "security", description = "The password, second factors, sessions and the identity provider's settings"),
        (name = "stats", description = "Persistent transfer statistics"),
        (name = "streams", description = "Stream channels, their schedules and recordings"),
        (name = "subscriptions", description = "Followed channels, feeds and indexers, and what they brought"),
        (name = "system", description = "Service health, settings, backups, storage, tools, updates and restarts"),
        (name = "usenet", description = "NNTP servers and NZB jobs")
    )
)]
struct Base;

/// `info.description`: what a reader needs before the first call.
const ABOUT: &str = "The REST API of rDownloader, a local-first download manager. The web \
interface uses nothing else.\n\n\
**Credentials.** An API token in `Authorization: Bearer`, or the session cookie of the web \
interface; every operation names the permission area it costs in `security` and \
`x-required-scope`. One that names a `capture:` area takes a capture token and nothing else, one \
with an empty `security` no credential at all.\n\n\
**Errors.** Every refusal is an `ErrorBody`: a stable `code` to act on, an English `error` to \
show, and the `params` the code's sentence names. A `429` that knows its wait says it in \
`Retry-After` as well. An address no operation has answers `404` with `web.route_not_found`.\n\n\
**Paging.** The growing lists take `limit` and `offset` and then name the whole list's length in \
`X-Total-Count`; without them the whole list comes back.\n\n\
**Live changes.** `GET /api/v1/events` announces what changed as Server-Sent Events.";

/// The complete document served at `/api/v1/openapi.json`.
pub(crate) fn document() -> utoipa::openapi::OpenApi {
    let mut doc = Base::openapi();
    doc.merge(schemas::Schemas::openapi());
    doc.merge(schemas_capture::CaptureSchemas::openapi());
    doc.merge(schemas_diagnostics::DiagnosticsSchemas::openapi());
    doc.merge(schemas_intake::IntakeSchemas::openapi());
    doc.merge(schemas_queue::QueueSchemas::openapi());
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
    doc.info.description = Some(ABOUT.to_owned());
    access::describe(&mut doc);
    refusals::describe(&mut doc);
    events::describe(&mut doc);
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
mod completeness_tests;

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
