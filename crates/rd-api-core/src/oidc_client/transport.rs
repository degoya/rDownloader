//! The HTTP the client speaks to an identity provider, and how its failures read.

use super::*;

/// A discovery or key-set failure of the library, as the provider failure it is.
pub(super) fn discovery_failure(
    error: openidconnect::DiscoveryError<SendError>,
) -> ProviderFailure {
    match error {
        openidconnect::DiscoveryError::Validation(_) => {
            ProviderFailure::Discovery(DiscoveryError::IssuerMismatch)
        }
        openidconnect::DiscoveryError::Request(SendError::Refused) => {
            ProviderFailure::Discovery(DiscoveryError::InsecureEndpoint)
        }
        openidconnect::DiscoveryError::Request(_) => ProviderFailure::Unreachable,
        _ => ProviderFailure::Unreadable,
    }
}

/// Why [`send`] gave no answer. Never carries a body or an address.
#[derive(Debug)]
pub enum SendError {
    /// The address is neither `https` nor on this machine.
    Refused,
    /// No answer: refused connection, TLS failure, timeout.
    Unreachable,
    /// An answer larger than [`oidc::RESPONSE_LIMIT_BYTES`].
    TooLarge,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Refused => "the address is neither https nor on this machine",
            Self::Unreachable => "the identity provider did not answer",
            Self::TooLarge => "the identity provider's answer is too large",
        })
    }
}

impl std::error::Error for SendError {}

/// A client for one exchange with the provider: no proxy, no redirect, a timeout, and the custom
/// CA material of the installation beside the system's roots.
pub(super) async fn http_client(state: &AppState) -> Result<reqwest::Client, ProviderFailure> {
    let custom_ca_pem = state
        .scheduler
        .network_defaults()
        .read()
        .await
        .custom_ca_pem
        .clone();
    let mut builder = reqwest::Client::builder()
        .timeout(oidc::REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    for pem in &custom_ca_pem {
        match reqwest::Certificate::from_pem(pem) {
            Ok(certificate) => builder = builder.add_root_certificate(certificate),
            Err(error) => {
                tracing::warn!(%error, "ignoring an unparsable custom CA for the identity provider");
            }
        }
    }
    builder.build().map_err(|error| {
        tracing::warn!(%error, "could not build the identity provider client");
        ProviderFailure::Unreachable
    })
}

/// The library's HTTP requests, sent with the workspace's reqwest: only to an allowed endpoint,
/// and an answer read up to [`oidc::RESPONSE_LIMIT_BYTES`] — a larger one is refused, not cut.
pub async fn send(
    client: reqwest::Client,
    request: HttpRequest,
) -> Result<HttpResponse, SendError> {
    let (parts, body) = request.into_parts();
    let url = parts.uri.to_string();
    if !oidc::endpoint_allowed(&url) {
        return Err(SendError::Refused);
    }
    let response = client
        .request(parts.method, url)
        .headers(parts.headers)
        .body(body)
        .send()
        .await
        .map_err(|_| SendError::Unreachable)?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = read_bounded_body(response, oidc::RESPONSE_LIMIT_BYTES)
        .await
        .map_err(|error| match error {
            BodyError::TooLarge => SendError::TooLarge,
            BodyError::Interrupted(_) => SendError::Unreachable,
        })?;
    if !status.is_success() {
        tracing::warn!(%status, "the identity provider answered with a failure");
    }
    let mut answer = HttpResponse::new(body);
    *answer.status_mut() = status;
    *answer.headers_mut() = headers;
    Ok(answer)
}
