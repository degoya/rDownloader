//! English texts every plugin builds the same way (RD-1190-10, audit PL-07).
//!
//! The code and its translations stay the plugin's own; only the English text the backend sends
//! beside the code is written once here, so its wording cannot drift between plugins.

/// The English text of an HTTP status nothing more specific explains, under the provider's
/// name: `AllDebrid HTTP status 503`. Each plugin's `{status}` translation mirrors it.
#[must_use]
pub fn http_error(provider: &str, status: u16) -> String {
    format!("{provider} HTTP status {status}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_http_error_names_the_provider_and_the_status() {
        assert_eq!(super::http_error("Put.io", 503), "Put.io HTTP status 503");
    }
}
