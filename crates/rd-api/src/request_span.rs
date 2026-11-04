//! The request span every traced request runs in, with credential material redacted.

/// Builds the request span with credential-bearing query parameters redacted.
///
/// The default span records the URI as it arrived, and the SABnzbd compatibility surface accepts
/// its API key in the query string — not by choice, but because that is the shape every real
/// SABnzbd client sends, so refusing it would break all of them. The key would therefore be
/// written into this service's own log, which is copied, shipped and read by people who have no
/// business holding an API token. Redacting it here covers the span before any subscriber sees
/// it; a reverse proxy in front still logs its own access line, which this cannot reach.
///
/// The names come from `rd_core::is_secret_parameter`, so the one list the project keeps of
/// credential-bearing parameters governs this too, rather than a second list that drifts.
pub(crate) fn request_span(request: &axum::http::Request<axum::body::Body>) -> tracing::Span {
    tracing::info_span!(
        "request",
        method = %request.method(),
        uri = %redact_uri(request.uri()),
        version = ?request.version(),
    )
}

/// The authorization-code callbacks, whose `code` and `state` are single-use sign-in material.
///
/// Matched as a path suffix so a reverse-proxy base path still counts, and only here: a
/// `code` parameter anywhere else (a hoster's share code, say) stays readable in the log.
const CALLBACK_PATHS: &[&str] = &["/api/v1/auth/oidc/callback", "/api/v1/oauth/callback"];

/// The query parameters redacted on a [`CALLBACK_PATHS`] request.
const CALLBACK_PARAMETERS: &[&str] = &["code", "state", "iss"];

fn is_sign_in_callback(path: &str) -> bool {
    CALLBACK_PATHS
        .iter()
        .any(|callback| path.ends_with(callback))
}

/// The request target with every credential-bearing query value replaced.
///
/// A URI with nothing to hide is rendered byte-for-byte, so ordinary log lines keep their exact
/// text and only the ones carrying a secret change shape.
fn redact_uri(uri: &axum::http::Uri) -> String {
    let Some(query) = uri.query() else {
        return uri.to_string();
    };
    let callback = is_sign_in_callback(uri.path());
    let secret = |name: &str| {
        rd_core::is_secret_parameter(name)
            || (callback
                && CALLBACK_PARAMETERS
                    .iter()
                    .any(|parameter| parameter.eq_ignore_ascii_case(name)))
    };
    if !query
        .split('&')
        .any(|pair| secret(pair.split('=').next().unwrap_or(pair)))
    {
        return uri.to_string();
    }
    let redacted = query
        .split('&')
        .map(|pair| {
            let name = pair.split('=').next().unwrap_or(pair);
            if secret(name) {
                format!("{name}={}", rd_core::REDACTION_PLACEHOLDER)
            } else {
                pair.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{redacted}", uri.path())
}

#[cfg(test)]
mod redaction_tests {
    use super::redact_uri;

    #[test]
    fn a_traced_uri_keeps_everything_except_the_credential() {
        let plain = "/api?mode=queue&cat=tv".parse().expect("uri");
        assert_eq!(redact_uri(&plain), "/api?mode=queue&cat=tv");

        // The shape every SABnzbd client sends. Only the value goes; the mode still has to be
        // readable, or the log stops being useful for the thing logs are kept for.
        let keyed = "/sabnzbd/api?mode=addurl&apikey=7f3b&name=x"
            .parse()
            .expect("uri");
        assert_eq!(
            redact_uri(&keyed),
            "/sabnzbd/api?mode=addurl&apikey=[redacted]&name=x"
        );

        let bare = "/api".parse().expect("uri");
        assert_eq!(redact_uri(&bare), "/api");
    }

    #[test]
    fn a_sign_in_callback_keeps_its_code_and_state_out_of_the_span() {
        for path in [
            "/api/v1/auth/oidc/callback",
            "/api/v1/oauth/callback",
            "/rd/api/v1/oauth/callback",
        ] {
            let uri = format!("{path}?code=abc123&state=xyz&iss=https%3A%2F%2Fidp&scope=openid")
                .parse()
                .expect("uri");
            assert_eq!(
                redact_uri(&uri),
                format!("{path}?code=[redacted]&state=[redacted]&iss=[redacted]&scope=openid")
            );
        }
    }

    #[test]
    fn a_code_parameter_elsewhere_is_left_alone() {
        let uri = "/api/v1/links?code=share42&state=open"
            .parse()
            .expect("uri");
        assert_eq!(redact_uri(&uri), "/api/v1/links?code=share42&state=open");
    }
}
