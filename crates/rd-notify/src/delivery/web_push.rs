//! The Web Push transport (RD-1240-13): one encrypted `POST` to a browser's push service.
//!
//! The browser hands its push service's address over when it subscribes, so the address is a
//! stranger's word like a webhook's: it has to be `https`, it may reach public addresses only
//! (a push service is never on the person's own network), no redirect is followed and only the
//! head of a failed answer is read. The service decides which subscriptions a message goes to;
//! this module sends one and says how it went, including the 404/410 that means the browser has
//! dropped the subscription.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use super::{Message, webhook};
use crate::model::{NotificationEvent, Severity};

#[path = "web_push_crypto.rs"]
mod crypto;

pub use crypto::VapidKey;

/// How long a push service keeps a message for a browser that is offline: a day. A finished
/// download announced two days later is no news any more.
const TIME_TO_LIVE_SECONDS: u32 = 24 * 60 * 60;

/// How long one call may take, connection and answer together.
const TIMEOUT: Duration = Duration::from_secs(30);

/// The longest title and body a message carries; the rest is cut, so the encrypted body always
/// fits the 4096 bytes a push service takes. Counted in characters, the worst case is six bytes
/// each (a control character JSON writes as `\u001f`): 3600 bytes, and the tag and the event
/// stay well within the 393 left of the 3993 one message may hold.
const MAX_TITLE_CHARS: usize = 100;
const MAX_BODY_CHARS: usize = 500;

/// What the history says about a push address the address rule refuses.
const REFUSED: &str = "the push address is not a public https address";

/// One browser that receives push messages.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WebPushSubscription {
    pub id: String,
    /// The push service's address for this browser; it names the subscription.
    pub endpoint: String,
    /// What the person called the device, or what the browser said it is.
    pub device_name: String,
    /// The events this browser wants; empty means every event.
    pub events: Vec<NotificationEvent>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The browser's P-256 key, URL-safe base64. Never returned by the API.
    #[serde(skip)]
    pub p256dh: String,
    /// The browser's authentication secret, URL-safe base64. Never returned by the API.
    #[serde(skip)]
    pub auth: String,
}

impl WebPushSubscription {
    /// Whether this browser wants to hear about `event`.
    #[must_use]
    pub fn wants(&self, event: NotificationEvent) -> bool {
        self.events.is_empty() || self.events.contains(&event)
    }
}

/// How one push went.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PushOutcome {
    /// The push service took the message.
    Delivered,
    /// The push service no longer knows the subscription (404 or 410): the browser unsubscribed,
    /// was reset or its subscription expired. The service deletes it.
    Gone,
    Failed {
        status: Option<u16>,
        detail: String,
        retryable: bool,
    },
}

/// Whether `endpoint` is an address a subscription may name: an `https` URL with a host.
#[must_use]
pub fn is_push_address(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint.trim()).is_ok_and(|url| url.scheme() == "https" && url.has_host())
}

/// Whether a push message may go to `endpoint`: a push address ([`is_push_address`]) whose host
/// is, or resolves to, a public address. One rule for both ends: [`send_push`] checks it before
/// it sends, and the subscription route before it stores, so an address that could never be
/// delivered to is refused when it is handed over rather than at every message (RD-1240-28).
/// A name that does not resolve is not refused -- nothing was, and the push service may only be
/// out of reach for a moment.
pub async fn is_deliverable_push_address(endpoint: &str, lookup: &dyn rd_http::HostLookup) -> bool {
    if !is_push_address(endpoint) {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(endpoint.trim()) else {
        return false;
    };
    !matches!(
        rd_http::check_target(&push_reach(), lookup, &url).await,
        Err(rd_http::TargetRefusal::Refused(_))
    )
}

/// What a push may reach: public addresses only, as a webhook without the private switch.
fn push_reach() -> rd_http::AddressPolicy {
    rd_http::AddressPolicy::new(false)
}

/// Whether `p256dh` and `auth` are keys a browser hands out: a P-256 point and 16 bytes.
#[must_use]
pub fn are_push_keys(p256dh: &str, auth: &str) -> bool {
    crypto::decode(p256dh).is_ok_and(|key| key.len() == 65 && key[0] == 4)
        && crypto::decode(auth).is_ok_and(|secret| secret.len() == 16)
}

/// What the service worker receives: the text, the event it opens the matching view for, and a
/// tag, so a delivery tried twice replaces its first notification instead of adding a second.
#[must_use]
pub fn push_payload(message: &Message) -> Vec<u8> {
    let cut = |text: &str, limit: usize| text.chars().take(limit).collect::<String>();
    serde_json::to_vec(&serde_json::json!({
        "title": cut(&message.title, MAX_TITLE_CHARS),
        "body": cut(&message.body, MAX_BODY_CHARS),
        "event": message.event,
        "tag": message.idempotency_key,
    }))
    .unwrap_or_default()
}

/// Sends `payload` to one browser, signed with `key`.
pub async fn send_push(
    key: &VapidKey,
    subscription: &WebPushSubscription,
    payload: &[u8],
    severity: Severity,
) -> PushOutcome {
    match try_send(key, subscription, payload, severity).await {
        Ok(outcome) => outcome,
        // A transport error (DNS, TLS, connection refused) is worth another attempt.
        Err(error) => failed(None, error.to_string(), true),
    }
}

async fn try_send(
    key: &VapidKey,
    subscription: &WebPushSubscription,
    payload: &[u8],
    severity: Severity,
) -> anyhow::Result<PushOutcome> {
    if !is_deliverable_push_address(&subscription.endpoint, &rd_http::SystemLookup).await {
        return Ok(failed(None, REFUSED, false));
    }
    let Ok(url) = reqwest::Url::parse(subscription.endpoint.trim()) else {
        return Ok(failed(None, REFUSED, false));
    };
    let reach = push_reach();
    let body = match crypto::encrypt(&subscription.p256dh, &subscription.auth, payload) {
        Ok(body) => body,
        // Keys that do not encrypt never will; the browser has to subscribe again.
        Err(error) => return Ok(failed(None, error.to_string(), false)),
    };
    let authorization = key.authorization(&url, Utc::now())?;
    let urgency = if severity == Severity::Info {
        "normal"
    } else {
        "high"
    };
    let response = match webhook::client(&reach, TIMEOUT)?
        .post(url)
        .header(reqwest::header::AUTHORIZATION, authorization)
        .header(reqwest::header::CONTENT_ENCODING, "aes128gcm")
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header("TTL", TIME_TO_LIVE_SECONDS.to_string())
        .header("Urgency", urgency)
        .body(body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) if rd_http::is_refusal(&error) => return Ok(failed(None, REFUSED, false)),
        Err(error) => return Err(error.into()),
    };
    Ok(judge(response).await)
}

/// What an answer of the push service means for the subscription and the delivery.
async fn judge(response: reqwest::Response) -> PushOutcome {
    let status = response.status();
    if status.is_success() {
        return PushOutcome::Delivered;
    }
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
        return PushOutcome::Gone;
    }
    let text = webhook::answer_head(response).await;
    failed(Some(status.as_u16()), text, outcome_is_retryable(status))
}

/// A push service's own trouble is retried; a refusal of the request — a token it does not
/// accept, a key the browser did not subscribe with (403), a message too large (413) — is not.
fn outcome_is_retryable(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

fn failed(status: Option<u16>, detail: impl Into<String>, retryable: bool) -> PushOutcome {
    PushOutcome::Failed {
        status,
        detail: super::excerpt(detail.into()),
        retryable,
    }
}

#[cfg(test)]
#[path = "web_push_tests.rs"]
mod tests;
