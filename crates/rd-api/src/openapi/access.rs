//! What a client must present, said by the document (RD-1480-09).
//!
//! Read out of [`ROUTE_POLICY`](rd_api_core::scope_policy::ROUTE_POLICY), the table the service
//! decides with, so the document cannot claim another price than the one that is charged: the
//! `security` of every operation and, because OpenAPI has no field for it outside OAuth, the
//! permission area as `x-required-scope`.

use rd_api_core::scope_policy::{Requirement, requirement};
use rd_core::Scope;
use utoipa::openapi::{
    OpenApi, SecurityRequirement,
    extensions::ExtensionsBuilder,
    security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme},
};

use super::operations;

/// The browser session, as `rd_api_core::auth` sets the cookie.
const SESSION: &str = "session";
const API_TOKEN: &str = "apiToken";
const CAPTURE_TOKEN: &str = "captureToken";

/// The operation's permission area, machine-readable.
pub(super) const REQUIRED_SCOPE: &str = "x-required-scope";

/// Whether a route of this price takes a capture token, and nothing else.
pub(super) fn is_capture(scope: Scope) -> bool {
    Scope::CAPTURE.contains(&scope)
}

/// Declares the credentials and, on every operation, which of them it takes.
pub(super) fn describe(doc: &mut OpenApi) {
    let components = doc.components.get_or_insert_with(Default::default);
    components.add_security_scheme(
        SESSION,
        SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
            "rd_session",
            "The web interface's session after a sign-in; it holds every `api:` permission area.",
        ))),
    );
    components.add_security_scheme(
        API_TOKEN,
        bearer(
            "An API token (Settings > Clients & API). It needs the permission area the \
             operation names in `x-required-scope`.",
        ),
    );
    components.add_security_scheme(
        CAPTURE_TOKEN,
        bearer(
            "The token of a paired capture agent or browser extension; no API token works here.",
        ),
    );
    operations::each(doc, |path, method, operation| {
        let Some(requirement) = requirement(path, method) else {
            return;
        };
        let Requirement::Scope(scope) = requirement else {
            // An empty list rather than none: "no credential", not "nothing said".
            operation.security = Some(Vec::new());
            return;
        };
        let area = scope.as_str();
        operation.security = Some(if is_capture(scope) {
            vec![SecurityRequirement::new(CAPTURE_TOKEN, [area])]
        } else {
            vec![
                SecurityRequirement::new(API_TOKEN, [area]),
                SecurityRequirement::new(SESSION, [area]),
            ]
        });
        let required = ExtensionsBuilder::new().add(REQUIRED_SCOPE, area).build();
        match &mut operation.extensions {
            Some(extensions) => extensions.merge(required),
            None => operation.extensions = Some(required),
        }
    });
}

fn bearer(description: &str) -> SecurityScheme {
    SecurityScheme::Http(
        HttpBuilder::new()
            .scheme(HttpAuthScheme::Bearer)
            .description(Some(description))
            .build(),
    )
}
