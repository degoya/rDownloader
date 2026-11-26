//! What a client already has and what it takes, for the answers that stay the same for a whole
//! run: the embedded interface and the OpenAPI document (RD-1480-19).

use axum::http::{HeaderMap, HeaderName, header};

/// Whether `If-None-Match` names `etag`, so the client's copy is the current one. Weak tags match
/// as well: a proxy that compresses an answer weakens its tag.
pub(crate) fn has_current(headers: &HeaderMap, etag: &str) -> bool {
    list(headers, header::IF_NONE_MATCH)
        .any(|tag| tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == etag)
}

/// Whether the client takes a gzip body: named in `Accept-Encoding` and not refused with `q=0`.
pub(crate) fn accepts_gzip(headers: &HeaderMap) -> bool {
    list(headers, header::ACCEPT_ENCODING).any(|entry| {
        let mut parts = entry.split(';').map(str::trim);
        parts
            .next()
            .is_some_and(|coding| coding.eq_ignore_ascii_case("gzip"))
            && !parts.any(|parameter| {
                parameter
                    .strip_prefix("q=")
                    .and_then(|quality| quality.trim().parse::<f32>().ok())
                    .is_some_and(|quality| quality <= 0.0)
            })
    })
}

/// The entries of a comma-separated header, over every line of it.
fn list(headers: &HeaderMap, name: HeaderName) -> impl Iterator<Item = &str> {
    headers
        .get_all(name)
        .into_iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderName, header};

    use super::{accepts_gzip, has_current};

    fn headers(name: HeaderName, values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(name.clone(), value.parse().expect("header value"));
        }
        headers
    }

    #[test]
    fn a_tag_is_found_in_a_list_weakened_or_as_a_star() {
        let current =
            |values: &[&str]| has_current(&headers(header::IF_NONE_MATCH, values), "\"a\"");
        assert!(current(&["\"a\""]));
        assert!(current(&["\"b\", W/\"a\""]));
        assert!(current(&["\"b\"", "\"a\""]));
        assert!(current(&["*"]));
        assert!(!current(&["\"b\""]));
        assert!(!current(&[]));
    }

    #[test]
    fn gzip_is_taken_when_named_and_not_refused() {
        let takes = |values: &[&str]| accepts_gzip(&headers(header::ACCEPT_ENCODING, values));
        assert!(takes(&["gzip, deflate, br"]));
        assert!(takes(&["br", "GZIP;q=0.5"]));
        assert!(!takes(&["gzip;q=0, br"]));
        assert!(!takes(&["gzip; q=0.0"]));
        assert!(!takes(&["br, xgzip"]));
        assert!(!takes(&["identity"]));
        assert!(!takes(&[]));
    }
}
