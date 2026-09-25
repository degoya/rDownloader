//! Handing a browser's session at a provider over to one account (RD-120-45).
//!
//! The service cannot read a browser's cookies; only the extension can, and only with the
//! person's consent. So a handover has two halves that never meet in one place:
//!
//! 1. A person asks for it in the web interface, for **one account** — an authenticated
//!    session with the secrets scope, the same one that may type cookies into the account.
//!    The scope is the provider's `cookie_scope`, read from the installed plugin's manifest;
//!    nobody chooses a domain.
//! 2. The extension, polling with its capture token, sees the waiting request, asks the person
//!    in its popup, reads that one origin's cookies and hands them to
//!    [`crate::browser_session_handlers::deliver_capture_browser_session`].
//!
//! The split is what keeps the rule `docs/auth-profiles.md` states for captured sessions: a
//! capture token lives in a browser, so it must not be able to mint a usable credential. Here
//! it cannot choose the account, the provider or the domain, and cannot start a handover at
//! all — it can only answer one a person started, within [`WAIT`], once.
//!
//! Purely in memory, like the captcha queue: a restart drops every request, and the person
//! asks again.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Duration, Utc};
use rd_core::AccountId;
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;
use uuid::Uuid;

/// How long a request waits for the extension. Long enough to switch to the browser, open
/// the popup and answer the browser's own permission prompt; short enough that a forgotten
/// request does not stand open for a token that turns up tomorrow.
pub(crate) const WAIT: Duration = Duration::minutes(5);

/// How long a finished request stays readable, so the interface polling it sees how it ended.
const KEEP: Duration = Duration::minutes(10);

/// Where a handover stands.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSessionState {
    /// Waiting for the person to confirm it in the extension.
    Waiting,
    /// The cookies arrived and are stored on the account.
    Delivered,
    /// The person declined it in the extension.
    Declined,
    /// Nobody answered within the waiting time.
    Expired,
}

/// A handover as the web interface sees it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct BrowserSessionResponse {
    pub id: Uuid,
    pub state: BrowserSessionState,
    /// The host whose cookies are asked for, from the provider's `cookie_scope`.
    pub host: String,
    pub expires_at: DateTime<Utc>,
}

/// A waiting handover as the extension sees it: what it needs to ask the person, and nothing
/// else. The scope is the service's, never the page's — the extension reads exactly this.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CaptureBrowserSession {
    pub id: Uuid,
    pub provider: String,
    /// The provider's display name, for the question the popup asks.
    pub provider_name: String,
    /// Which account the session is for, so the person can tell two requests apart.
    pub account_label: String,
    /// The provider's `cookie_scope`: an `https` URL whose host is the only one read.
    pub scope: String,
    pub host: String,
    pub expires_at: DateTime<Utc>,
}

/// One request, keyed by its account: asking again replaces the earlier one.
#[derive(Clone, Debug)]
pub(crate) struct Handover {
    pub id: Uuid,
    pub account_id: AccountId,
    pub account_label: String,
    pub provider: String,
    pub provider_name: String,
    pub scope: Url,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub state: BrowserSessionState,
    /// Set while a delivery is being stored, so a second delivery cannot race the first.
    claimed: bool,
}

impl Handover {
    pub(crate) fn host(&self) -> &str {
        self.scope.host_str().unwrap_or_default()
    }

    /// The state as of `now`: a waiting request past its time has expired.
    fn state_at(&self, now: DateTime<Utc>) -> BrowserSessionState {
        if self.state == BrowserSessionState::Waiting && now >= self.expires_at {
            BrowserSessionState::Expired
        } else {
            self.state
        }
    }

    fn response(&self, now: DateTime<Utc>) -> BrowserSessionResponse {
        BrowserSessionResponse {
            id: self.id,
            state: self.state_at(now),
            host: self.host().to_owned(),
            expires_at: self.expires_at,
        }
    }
}

/// What the account and its provider contribute to a new request.
pub(crate) struct Request {
    pub account_id: AccountId,
    pub account_label: String,
    pub provider: String,
    pub provider_name: String,
    pub scope: Url,
}

/// Why a delivery or a decline was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Refusal {
    /// No request with that id, or it ended already.
    NotWaiting,
}

/// The requests currently open, shared by the web and the capture handlers.
#[derive(Clone, Default)]
pub struct BrowserSessions {
    inner: Arc<Mutex<HashMap<AccountId, Handover>>>,
}

impl BrowserSessions {
    /// Opens a request for one account, replacing any earlier one for it.
    pub(crate) fn begin(&self, request: Request, now: DateTime<Utc>) -> BrowserSessionResponse {
        let handover = Handover {
            id: Uuid::now_v7(),
            account_id: request.account_id,
            account_label: request.account_label,
            provider: request.provider,
            provider_name: request.provider_name,
            scope: request.scope,
            created_at: now,
            expires_at: now + WAIT,
            state: BrowserSessionState::Waiting,
            claimed: false,
        };
        let response = handover.response(now);
        if let Ok(mut open) = self.inner.lock() {
            prune(&mut open, now);
            open.insert(handover.account_id, handover);
        }
        response
    }

    /// The account's request, if one was made recently enough to still be told about.
    pub(crate) fn status(
        &self,
        account_id: AccountId,
        now: DateTime<Utc>,
    ) -> Option<BrowserSessionResponse> {
        let mut open = self.inner.lock().ok()?;
        prune(&mut open, now);
        open.get(&account_id).map(|handover| handover.response(now))
    }

    /// Withdraws the account's request. Reports whether there was one.
    pub(crate) fn cancel(&self, account_id: AccountId) -> bool {
        self.inner
            .lock()
            .is_ok_and(|mut open| open.remove(&account_id).is_some())
    }

    /// Every request the extension may still answer, oldest first.
    pub(crate) fn waiting(&self, now: DateTime<Utc>) -> Vec<CaptureBrowserSession> {
        let Ok(mut open) = self.inner.lock() else {
            return Vec::new();
        };
        prune(&mut open, now);
        let mut waiting: Vec<&Handover> = open
            .values()
            .filter(|handover| {
                !handover.claimed && handover.state_at(now) == BrowserSessionState::Waiting
            })
            .collect();
        waiting.sort_by_key(|handover| handover.created_at);
        waiting
            .into_iter()
            .map(|handover| CaptureBrowserSession {
                id: handover.id,
                provider: handover.provider.clone(),
                provider_name: handover.provider_name.clone(),
                account_label: handover.account_label.clone(),
                scope: handover.scope.to_string(),
                host: handover.host().to_owned(),
                expires_at: handover.expires_at,
            })
            .collect()
    }

    /// Takes a waiting request for delivery. Only one delivery can hold it at a time; it ends
    /// with [`Self::finish`].
    pub(crate) fn claim(&self, id: Uuid, now: DateTime<Utc>) -> Result<Handover, Refusal> {
        let mut open = self.inner.lock().map_err(|_| Refusal::NotWaiting)?;
        let handover = open
            .values_mut()
            .find(|handover| handover.id == id)
            .filter(|handover| {
                !handover.claimed && handover.state_at(now) == BrowserSessionState::Waiting
            })
            .ok_or(Refusal::NotWaiting)?;
        handover.claimed = true;
        Ok(handover.clone())
    }

    /// Ends a claimed delivery: delivered when it was stored, waiting again when it was not,
    /// so a refused upload can be retried within the same request.
    pub(crate) fn finish(&self, id: Uuid, stored: bool) {
        if let Ok(mut open) = self.inner.lock()
            && let Some(handover) = open.values_mut().find(|handover| handover.id == id)
        {
            handover.claimed = false;
            if stored {
                handover.state = BrowserSessionState::Delivered;
            }
        }
    }

    /// The person declined in the extension.
    pub(crate) fn decline(&self, id: Uuid, now: DateTime<Utc>) -> Result<(), Refusal> {
        let mut open = self.inner.lock().map_err(|_| Refusal::NotWaiting)?;
        let handover = open
            .values_mut()
            .find(|handover| handover.id == id)
            .filter(|handover| {
                !handover.claimed && handover.state_at(now) == BrowserSessionState::Waiting
            })
            .ok_or(Refusal::NotWaiting)?;
        handover.state = BrowserSessionState::Declined;
        Ok(())
    }
}

/// Drops requests nobody needs to read any more.
fn prune(open: &mut HashMap<AccountId, Handover>, now: DateTime<Utc>) {
    open.retain(|_, handover| now < handover.created_at + KEEP);
}

#[cfg(test)]
#[path = "browser_session_tests.rs"]
mod tests;
