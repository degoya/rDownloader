//! The headers every answer carries, and the content security policy of the web interface
//! (audit 2026-10-05, S5).
//!
//! Without them the interface could be framed by any page — with the sign-in switched off it was
//! fully operable inside a foreign frame, and `SameSite=Strict` does not stop a page on another
//! port of the same host name — a response could be sniffed into another type, and the address
//! of the interface went out as the `Referer` of every cover image it loads from elsewhere.
//!
//! One layer outside everything else, the host check included, so no answer leaves without
//! them. A header an answer already carries wins: the host-refusal page keeps its own, stricter
//! policy, and the shell sets the full one below.

use axum::{
    http::{HeaderName, HeaderValue},
    response::Response,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

/// The headers every answer gets unless it set its own.
///
/// `same-origin` rather than `no-referrer`: the interface's own requests keep their `Referer`,
/// and none of the sign-in flows needs one — the identity provider and the OAuth providers come
/// back by their redirect address and `state`, never by where the browser came from. Every other
/// answer gets `frame-ancestors 'none'` as its policy: it restricts nothing that answer loads,
/// only where it may be shown.
const EVERY_ANSWER: [(&str, &str); 4] = [
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "same-origin"),
    ("x-frame-options", "DENY"),
    ("content-security-policy", "frame-ancestors 'none'"),
];

/// Adds [`EVERY_ANSWER`] to a response that does not carry them yet.
pub(crate) async fn add(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (name, value) in EVERY_ANSWER {
        headers
            .entry(HeaderName::from_static(name))
            .or_insert_with(|| HeaderValue::from_static(value));
    }
    response
}

/// The content security policy of the web interface's shell, `index.html`.
///
/// What the built interface needs and nothing more: its scripts, styles, fonts and service
/// worker from this origin; the inline styles Nuxt UI sets and the fonts the stylesheet embeds as
/// `data:`; images from anywhere, because subscription covers, thumbnails and indexer posters
/// come from the sites they describe; requests and the event stream only to this origin. No
/// inline script but the one that hands a mount point to the application, allowed by its hash
/// (`inline_script`).
#[must_use]
pub(crate) fn interface_policy(inline_script: Option<&str>) -> String {
    let script_hash = inline_script
        .map(|script| format!(" 'sha256-{}'", STANDARD.encode(Sha256::digest(script))))
        .unwrap_or_default();
    format!(
        "default-src 'self'; script-src 'self'{script_hash}; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob: https: http:; font-src 'self' data:; connect-src 'self'; \
         worker-src 'self'; manifest-src 'self'; object-src 'none'; base-uri 'self'; \
         form-action 'self'; frame-ancestors 'none'"
    )
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::header, response::IntoResponse};

    use super::*;

    #[tokio::test]
    async fn every_answer_gets_the_headers_and_keeps_its_own_policy() {
        let plain = add(Body::empty().into_response()).await;
        for (name, value) in EVERY_ANSWER {
            assert_eq!(
                plain.headers().get(name).and_then(|v| v.to_str().ok()),
                Some(value),
                "{name}"
            );
        }

        let own = (
            [(header::CONTENT_SECURITY_POLICY, "default-src 'none'")],
            "refused",
        )
            .into_response();
        let own = add(own).await;
        assert_eq!(
            own.headers().get(header::CONTENT_SECURITY_POLICY),
            Some(&HeaderValue::from_static("default-src 'none'"))
        );
        assert_eq!(
            own.headers()
                .get_all(header::CONTENT_SECURITY_POLICY)
                .iter()
                .count(),
            1
        );
    }

    /// The hash of the mount-point script is what lets it run; nothing else inline does.
    #[test]
    fn the_shell_policy_allows_exactly_the_mount_point_script() {
        let root = interface_policy(None);
        assert!(root.contains("script-src 'self';"), "{root}");
        assert!(root.contains("frame-ancestors 'none'"), "{root}");

        // `printf '%s' 'window.__RD_BASE__="/downloads";' | openssl dgst -sha256 -binary | base64`
        let mounted = interface_policy(Some(r#"window.__RD_BASE__="/downloads";"#));
        assert!(
            mounted.contains(
                "script-src 'self' 'sha256-fIz3pBPzUGeRgHq7/mg19xIu0BrYoul8rkIBNifuwJE=';"
            ),
            "{mounted}"
        );
    }
}
