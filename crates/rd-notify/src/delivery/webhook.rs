//! The webhook transport: one signed `POST`, held to the address rule (audit 2026-10-05, S2).
//!
//! A webhook's address is configuration, and the answer to a failed call ends up in the
//! delivery history anyone with `api:config` reads. Without a rule the service would post to its
//! own API, to a cloud's metadata endpoint, or to whatever a public receiver redirected it to,
//! and hand back what came out. So the call keeps to the rule every other request to an entered
//! address keeps to: the address is checked before the request, the client resolves names only
//! through the guard, no redirect is followed, and only the head of an answer is read.
//!
//! Behind the proxy the environment names (`HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`) the call
//! pins too (RD-1330-01, owner 2026-10-16): the proxy is handed the address checked when the
//! connection is made, never the name ([`rd_http::EnvironmentRoute`]). A name only the proxy
//! could resolve fails as `proxy.name_unresolved`, a tunnel the proxy refuses as
//! `proxy.tunnel_refused` with its status.

use std::time::Duration;

use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit as _, Mac};
use secrecy::ExposeSecret;
use sha2::Sha256;

use super::{Attempt, IDEMPOTENCY_HEADER, Message, SIGNATURE_HEADER};
use crate::model::NotificationTarget;

/// How long one call may take, connection and answer together.
const TIMEOUT: Duration = Duration::from_secs(30);

/// How much of a failed call's answer is read. The history keeps 500 characters of it; a
/// receiver that keeps sending must not fill the service's memory.
const MAX_ANSWER_BYTES: usize = 8 * 1024;

/// What the history says about a target the address rule refuses.
const REFUSED: &str = "the webhook address points at a link-local address, at an address this \
     service may not reach or at one of rDownloader's own services";

pub(super) async fn send(
    reach: &rd_http::AddressPolicy,
    target: &NotificationTarget,
    message: &Message,
    secret: Option<&secrecy::SecretString>,
) -> Result<Attempt> {
    let route = rd_http::EnvironmentRoute::system();
    send_via(reach, &route, target, message, secret).await
}

/// [`send`] the way `route` says: its lookup and the proxy it names. A test hands in its own.
pub(super) async fn send_via(
    reach: &rd_http::AddressPolicy,
    route: &rd_http::EnvironmentRoute,
    target: &NotificationTarget,
    message: &Message,
    secret: Option<&secrecy::SecretString>,
) -> Result<Attempt> {
    let url = reqwest::Url::parse(target.endpoint.trim()).context("webhook address")?;
    // A literal address never reaches the guarded resolver; it is judged here, and a name with
    // it. A name without an address is left to fail as unreachable.
    if let Err(rd_http::TargetRefusal::Refused(_)) =
        rd_http::check_target(reach, route.lookup(), &url).await
    {
        return Ok(refused());
    }
    let body = serde_json::to_vec(&message.payload)?;
    let mut request = client(reach, route, &url, TIMEOUT)?
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(IDEMPOTENCY_HEADER, &message.idempotency_key);
    if let Some(secret) = secret {
        request = request.header(SIGNATURE_HEADER, sign(secret.expose_secret(), &body));
    }
    let response = match request.body(body).send().await {
        Ok(response) => response,
        Err(error) if rd_http::is_refusal(&error) => return Ok(refused()),
        Err(error) => return proxy_failure(&error).ok_or_else(|| error.into()),
    };
    let status = response.status();
    if status.is_success() {
        return Ok(Attempt::ok(Some(status.as_u16())));
    }
    let text = answer_head(response).await;
    // 4xx other than 408/429 means the request itself is wrong; repeating it will not help. A
    // redirect is not followed, and the same call would only be redirected again.
    let retryable = status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
    Ok(Attempt::failed(Some(status.as_u16()), text, retryable))
}

/// A client for one call to `target`: names resolved through the guard at connect time, so a
/// name that passed the check above cannot point inside a moment later — behind the proxy the
/// environment names as well, which is handed the checked address — and no redirect followed:
/// a receiver that answers `302` with an inner address would otherwise be the way around it.
/// The Web Push transport sends through the same kind of client (RD-1240-13).
pub(super) fn client(
    reach: &rd_http::AddressPolicy,
    route: &rd_http::EnvironmentRoute,
    target: &reqwest::Url,
    timeout: Duration,
) -> Result<reqwest::Client> {
    let builder = rd_http::client_builder(rd_http::ClientPurpose::Service)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout);
    route
        .guarded(builder, target, reach)
        .context("webhook client")?
        .build()
        .context("webhook client")
}

/// A refused target is refused again on every retry.
fn refused() -> Attempt {
    Attempt::failed(None, REFUSED, false)
}

fn proxy_failure(error: &reqwest::Error) -> Option<Attempt> {
    let (detail, retryable) = proxy_detail(error)?;
    Some(Attempt::failed(None, detail, retryable))
}

/// What the proxy made of the call, by its stable code, and whether it is worth another
/// attempt: a tunnel it refused (retried unless the status says the proxy forbids it), or a
/// name only it could have resolved. `None` for any other failure. The Web Push transport
/// reports it the same way.
pub(super) fn proxy_detail(error: &reqwest::Error) -> Option<(String, bool)> {
    let failure = rd_http::tunnel_failure(error).or_else(|| rd_http::unresolved_failure(error))?;
    let code = failure.code.as_deref().unwrap_or_default();
    Some((
        format!("{code}: {}", failure.message),
        failure.category.is_retryable(),
    ))
}

/// The first [`MAX_ANSWER_BYTES`] of an answer; the rest is never read. A broken transfer
/// keeps what arrived.
pub(super) async fn answer_head(mut response: reqwest::Response) -> String {
    let mut head = Vec::new();
    while head.len() < MAX_ANSWER_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => head.extend_from_slice(&chunk),
            Ok(None) | Err(_) => break,
        }
    }
    head.truncate(MAX_ANSWER_BYTES);
    String::from_utf8_lossy(&head).into_owned()
}

/// `sha256=<hex>` over the exact body that is sent, the shape most receivers expect.
pub(super) fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        <Hmac<Sha256>>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
#[path = "webhook_tests.rs"]
mod tests;
