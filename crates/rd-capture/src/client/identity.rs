//! Who answers at the service address, asked before the capture token goes there (RD-1200-03,
//! `docs/security/capture-agent.md` finding 4).
//!
//! On a machine with several accounts, any of them can take a loopback port while it is free —
//! the service's 8710 while the service is down, for one, which is where a keyring token without
//! `capture.json` goes. The service's health route answers without a credential and names the
//! product, so the agent asks it first and sends the token only to a listener that answers as
//! rDownloader; a listener that answers otherwise is reported and gets nothing. Once confirmed,
//! the answer holds until a connection to the address fails, and whoever listens there then is
//! asked again.
//!
//! What this tells apart is rDownloader from another program — another account's server, a
//! development server, a mistyped port. A program written to imitate the health answer is not
//! told apart; that needs a credential of the service's own and is a residual risk in the model.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use reqwest::{Client, StatusCode};
use url::Url;

/// The name the service's health route answers with (`rd-api/src/handlers.rs::health`).
pub(crate) const SERVICE_NAME: &str = "rDownloader";

/// Most of the health answer read; the real one is well under a hundred bytes.
const MAX_ANSWER_BYTES: usize = 16 * 1024;

/// A listener at the service address that does not answer as rDownloader.
#[derive(Debug)]
pub(crate) struct ForeignListener {
    pub(crate) service: Url,
}

impl std::fmt::Display for ForeignListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the program answering at {} is not rDownloader; the capture token was not sent to it",
            self.service
        )
    }
}

impl std::error::Error for ForeignListener {}

#[derive(Clone, Default)]
pub(crate) struct Identity {
    confirmed: Arc<AtomicBool>,
    /// A foreign listener was logged and has not been replaced by the service since; the log
    /// says so once, not on every poll.
    reported: Arc<AtomicBool>,
}

impl Identity {
    /// Confirms who listens at `service`, unless that is known since the last lost connection.
    ///
    /// An address that does not answer at all is an ordinary error, not a foreign listener: the
    /// service may still be starting.
    pub(crate) async fn confirm(&self, http: &Client, service: &Url) -> Result<()> {
        if self.confirmed.load(Ordering::Acquire) {
            return Ok(());
        }
        let response = http.get(service.join("api/v1/health")?).send().await?;
        let status = response.status();
        let body = read_at_most(response, MAX_ANSWER_BYTES).await;
        if answers_as_rdownloader(status, &body) {
            self.confirmed.store(true, Ordering::Release);
            self.reported.store(false, Ordering::Release);
            return Ok(());
        }
        let foreign = ForeignListener {
            service: service.clone(),
        };
        if !self.reported.swap(true, Ordering::AcqRel) {
            tracing::error!(%status, "{foreign}");
        }
        Err(foreign.into())
    }

    /// A connection to the address failed: the next request asks again.
    pub(crate) fn forget(&self) {
        self.confirmed.store(false, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn assume_confirmed(&self) {
        self.confirmed.store(true, Ordering::Release);
    }
}

/// Whether one health answer is the service's: a success whose JSON names rDownloader.
pub(crate) fn answers_as_rdownloader(status: StatusCode, body: &[u8]) -> bool {
    status.is_success()
        && serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .is_some_and(|answer| {
                answer.get("service").and_then(serde_json::Value::as_str) == Some(SERVICE_NAME)
            })
}

/// The service's version from one health answer, for the tray's server line (RD-1240-06).
///
/// Only from an answer that names rDownloader, and only a version that looks like one: up to 32
/// letters, digits, dots, dashes and plus signs. Whatever else the listener sends never reaches
/// the menu.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn health_version(body: &[u8]) -> Option<String> {
    let answer = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    if answer.get("service").and_then(serde_json::Value::as_str) != Some(SERVICE_NAME) {
        return None;
    }
    let version = answer.get("version")?.as_str()?;
    let plausible = !version.is_empty()
        && version.len() <= 32
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    plausible.then(|| version.to_owned())
}

/// A health answer's body, read no further than this module reads it for the identity check.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) async fn read_health_answer(response: reqwest::Response) -> Vec<u8> {
    read_at_most(response, MAX_ANSWER_BYTES).await
}

/// The body, cut off past `limit`: a foreign listener decides how much it sends.
async fn read_at_most(mut response: reqwest::Response, limit: usize) -> Vec<u8> {
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        body.extend_from_slice(&chunk);
        if body.len() > limit {
            break;
        }
    }
    body
}

#[cfg(test)]
mod tests;
