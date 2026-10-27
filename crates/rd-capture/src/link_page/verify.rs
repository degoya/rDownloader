//! The link page's one decision: whether an address and a token pair the agent with a service.
//!
//! The service is asked first, with the token, for the agent's settings: the request the agent
//! makes every few seconds once it runs, so a token the service answers here is a token the agent
//! can work with. Only then is anything stored, and storing is the caller's (`config::save`), so
//! the page and `configure` keep one way of keeping a token.

use reqwest::StatusCode;
use serde::Deserialize;
use url::Url;

use crate::{
    client::{CaptureClient, ForeignListener, ServiceRefusal},
    config,
};

/// The shortest token `config::save` takes; a shorter one is refused before any request.
const MIN_TOKEN_CHARS: usize = 32;

/// What the page sends. No `Debug`: the token must not reach a log line by accident.
#[derive(Deserialize)]
pub(crate) struct LinkRequest {
    pub service: String,
    pub token: String,
    /// "Connect unencrypted" was ticked: plain `http` to an address off this machine.
    #[serde(default)]
    pub allow_insecure: bool,
}

/// Why a link was not made: one stable code each, which the page words in the agent's language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkRefusal {
    /// Not an `http` or `https` address with a host.
    InvalidAddress,
    /// Plain `http` off this machine without the ticked confirmation.
    InsecureNotConfirmed,
    /// Nothing answered, or not in a way the agent can use.
    Unreachable,
    /// Something answered that is not rDownloader; the token was not sent to it.
    NotRdownloader,
    /// The service does not know the token, or it is too short to be one.
    TokenRejected,
    /// The token is known but lacks the capture rights.
    MissingScope,
    /// The service took the token, but it could not be kept.
    NotStored,
}

impl LinkRefusal {
    /// The code the page receives and words.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::InvalidAddress => "link_invalid_address",
            Self::InsecureNotConfirmed => "link_insecure_unconfirmed",
            Self::Unreachable => "link_unreachable",
            Self::NotRdownloader => "link_not_rdownloader",
            Self::TokenRejected => "link_token_rejected",
            Self::MissingScope => "link_missing_scope",
            Self::NotStored => "link_not_stored",
        }
    }
}

/// A link that was verified and stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Linked {
    pub service: Url,
    /// Plain `http` off this machine: the token travels unencrypted, and the tray says so.
    pub insecure: bool,
}

/// Verifies `request` against the service and, only when the service accepted the token, hands
/// address, trimmed token and confirmation to `store`.
pub(crate) async fn verify_and_save(
    request: LinkRequest,
    store: impl FnOnce(&Url, &str, bool) -> anyhow::Result<()>,
) -> Result<Linked, LinkRefusal> {
    let service = parse_service(&request.service)?;
    // Pasted from a page or a password manager, a token often carries a line break.
    let token = request.token.trim();
    if token.chars().count() < MIN_TOKEN_CHARS {
        return Err(LinkRefusal::TokenRejected);
    }
    if config::ensure_transport_is_safe(&service, request.allow_insecure).is_err() {
        return Err(LinkRefusal::InsecureNotConfirmed);
    }
    let client = CaptureClient::new(service.clone(), token.to_owned()).map_err(|error| {
        tracing::warn!(%error, "the link page could not build its client");
        LinkRefusal::Unreachable
    })?;
    if let Err(error) = client.agent_settings().await {
        let refusal = refusal_of(&error);
        tracing::info!(%service, code = refusal.code(), %error, "the link was not made");
        return Err(refusal);
    }
    if let Err(error) = store(&service, token, request.allow_insecure) {
        tracing::warn!(%service, error = format!("{error:#}"), "the link could not be stored");
        return Err(LinkRefusal::NotStored);
    }
    let insecure = config::ensure_transport_is_safe(&service, false).is_err();
    tracing::info!(%service, insecure, "linked from the link page");
    Ok(Linked { service, insecure })
}

/// The address as typed, with its spaces gone; `192.168.1.10:8710` without a scheme is not one.
fn parse_service(text: &str) -> Result<Url, LinkRefusal> {
    let service = Url::parse(text.trim()).map_err(|_| LinkRefusal::InvalidAddress)?;
    if !matches!(service.scheme(), "http" | "https") || service.host().is_none() {
        return Err(LinkRefusal::InvalidAddress);
    }
    Ok(service)
}

/// What the service's answer means for the person at the page.
fn refusal_of(error: &anyhow::Error) -> LinkRefusal {
    if error.downcast_ref::<ForeignListener>().is_some() {
        return LinkRefusal::NotRdownloader;
    }
    match error
        .downcast_ref::<ServiceRefusal>()
        .map(ServiceRefusal::status)
    {
        Some(StatusCode::UNAUTHORIZED) => LinkRefusal::TokenRejected,
        Some(StatusCode::FORBIDDEN) => LinkRefusal::MissingScope,
        _ => LinkRefusal::Unreachable,
    }
}
