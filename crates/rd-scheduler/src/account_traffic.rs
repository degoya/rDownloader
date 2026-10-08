//! Accounts whose traffic their hoster reports used up (RD-1190-13, RD-1190-14).
//!
//! The file that ran into the limit waits like any other rate limit (`retry.rs`): `RetryWait`
//! with the hoster's wait, no attempt spent. What this adds is the account: its other files are
//! refused the same way until the quota frees up, so the setting decides whether they find out
//! one by one (`nothing`), wait with it (`pause_account`, the default) or hold the whole queue
//! (`pause_queue`).
//!
//! Two things end the wait early or on time. The hoster's wait ends: the hold lapses with the
//! waiting file's due time, and the file tries again. Or a check of the account reports traffic
//! again — the periodic one below, or the one somebody starts from the account list — and every
//! file of the account that waits for its traffic is queued at once. Neither ever touches a file
//! somebody paused, nor the queue's own timed pause: they release only their own hold.
//!
//! In memory, like the hoster blocks of an IP limit (`hostblock.rs`); a start rebuilds the holds
//! from the files that still wait for their account's traffic, which carry the code and the due
//! time in their row.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rd_core::{AccountId, AccountTrafficAction, DownloadFile, DownloadState, Failure};

use crate::{HoldSource, SchedulerHandle};

/// Minutes between two checks of an account whose traffic is used up.
///
/// A rolling window ("200000 Mb for last 1 days") frees traffic a little at a time as old
/// downloads fall out of it, so the hour the hoster's wait lasts can be shortened: a quarter of
/// it answers within minutes of the quota coming back, and four account-page requests an hour
/// are nothing a hoster notices — a download attempt per waiting file would be.
pub const TRAFFIC_CHECK_INTERVAL_MINUTES: i64 = 15;

/// The reason the queue's hold carries while an account with `pause_queue` waits.
const HOLD_REASON: &str = "account_traffic_exhausted";

/// One account whose traffic is used up, as the interface, the tray and MCP report it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountTrafficHold {
    pub account_id: AccountId,
    /// What the setting makes of it right now.
    pub action: AccountTrafficAction,
    /// When the hoster's wait ends and the waiting files try again.
    pub until: DateTime<Utc>,
    /// When the account is checked for traffic next.
    pub next_check_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug)]
struct Held {
    until: DateTime<Utc>,
    next_check_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
struct State {
    held: HashMap<AccountId, Held>,
    action: AccountTrafficAction,
    overrides: BTreeMap<AccountId, AccountTrafficAction>,
    /// Whether the queue's hold was last set, so a tick writes it only on a change.
    queue_held: bool,
}

impl State {
    fn action_for(&self, account: AccountId) -> AccountTrafficAction {
        self.overrides.get(&account).copied().unwrap_or(self.action)
    }
}

/// The accounts held for their traffic, and the setting that says what that means.
#[derive(Clone, Debug, Default)]
pub(crate) struct TrafficHolds {
    state: Arc<Mutex<State>>,
}

impl TrafficHolds {
    fn with<T>(&self, apply: impl FnOnce(&mut State) -> T) -> T {
        // A poisoned lock only means a panic elsewhere held it; the map itself is intact.
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        apply(&mut state)
    }

    /// Takes over the setting and its per-account overrides.
    pub(crate) fn configure(
        &self,
        action: AccountTrafficAction,
        overrides: BTreeMap<AccountId, AccountTrafficAction>,
    ) {
        self.with(|state| {
            state.action = action;
            state.overrides = overrides;
        });
    }

    /// What the setting makes of `account`'s used-up traffic.
    pub(crate) fn action_for(&self, account: AccountId) -> AccountTrafficAction {
        self.with(|state| state.action_for(account))
    }

    /// Holds `account` until `until`, keeping a later end already set; a new hold is checked
    /// first after one interval.
    pub(crate) fn hold(&self, account: AccountId, until: DateTime<Utc>, now: DateTime<Utc>) {
        self.with(|state| {
            state
                .held
                .entry(account)
                .and_modify(|held| held.until = held.until.max(until))
                .or_insert(Held {
                    until,
                    next_check_at: now + Duration::minutes(TRAFFIC_CHECK_INTERVAL_MINUTES),
                });
        });
    }

    /// Whether a file of `account` waits for the account's traffic in this pass: only under
    /// `pause_account`, and only until the hoster's wait ends.
    pub(crate) fn holds_back(&self, account: AccountId, now: DateTime<Utc>) -> bool {
        self.with(|state| {
            state
                .held
                .get(&account)
                .is_some_and(|held| held.until > now)
                && state.action_for(account) == AccountTrafficAction::PauseAccount
        })
    }

    /// Drops the holds whose wait has ended, and answers whether the queue's hold has to be
    /// set (`Some(true)`), cleared (`Some(false)`) or left as it is (`None`).
    pub(crate) fn settle(&self, now: DateTime<Utc>) -> Option<bool> {
        self.with(|state| {
            state.held.retain(|_, held| held.until > now);
            let wanted = state
                .held
                .keys()
                .any(|account| state.action_for(*account) == AccountTrafficAction::PauseQueue);
            (wanted != state.queue_held).then(|| {
                state.queue_held = wanted;
                wanted
            })
        })
    }

    /// The accounts whose next check is due, each moved one interval on.
    pub(crate) fn due_checks(&self, now: DateTime<Utc>) -> Vec<AccountId> {
        self.with(|state| {
            state
                .held
                .iter_mut()
                .filter(|(_, held)| held.next_check_at <= now)
                .map(|(account, held)| {
                    held.next_check_at = now + Duration::minutes(TRAFFIC_CHECK_INTERVAL_MINUTES);
                    *account
                })
                .collect()
        })
    }

    /// Releases `account`; whether it was held.
    pub(crate) fn release(&self, account: AccountId) -> bool {
        self.with(|state| state.held.remove(&account).is_some())
    }

    /// Releases every account.
    pub(crate) fn release_all(&self) {
        self.with(|state| state.held.clear());
    }

    /// The accounts held at `now`, the soonest to end first.
    pub(crate) fn list(&self, now: DateTime<Utc>) -> Vec<AccountTrafficHold> {
        let mut holds = self.with(|state| {
            state
                .held
                .iter()
                .filter(|(_, held)| held.until > now)
                .map(|(account, held)| AccountTrafficHold {
                    account_id: *account,
                    action: state.action_for(*account),
                    until: held.until,
                    next_check_at: held.next_check_at.min(held.until),
                })
                .collect::<Vec<_>>()
        });
        holds.sort_by(|left, right| {
            left.until
                .cmp(&right.until)
                .then_with(|| left.account_id.cmp(&right.account_id))
        });
        holds
    }
}

/// Whether `file` waits for its account's traffic: `RetryWait` with the code that says so.
fn waits_for_traffic(file: &DownloadFile) -> bool {
    file.state == DownloadState::RetryWait
        && file
            .last_error
            .as_ref()
            .is_some_and(rd_core::is_account_traffic_exhausted)
}

impl SchedulerHandle {
    /// Records that `file`'s account ran out of traffic, when `failure` says so and the file
    /// waits for it (`retry_at`); called for every failure the queue records.
    pub(crate) async fn note_account_traffic(
        &self,
        file: &DownloadFile,
        failure: &Failure,
        retry_at: Option<DateTime<Utc>>,
    ) {
        let (Some(account), Some(until)) = (file.account_id, retry_at) else {
            return;
        };
        if !rd_core::is_account_traffic_exhausted(failure) {
            return;
        }
        self.traffic_holds.hold(account, until, Utc::now());
        tracing::info!(
            account_id = %account,
            %until,
            action = ?self.traffic_holds.action_for(account),
            "the account's traffic is used up; its downloads wait"
        );
        self.settle_account_traffic(Utc::now()).await;
    }

    /// Whether `file` waits in this pass because its account's traffic is used up.
    pub(crate) fn held_for_account_traffic(&self, file: &DownloadFile, now: DateTime<Utc>) -> bool {
        file.account_id
            .is_some_and(|account| self.traffic_holds.holds_back(account, now))
    }

    /// One supervision step: ends the holds whose wait has passed, sets or clears the queue's
    /// hold, and checks the accounts whose check is due — each in a task of its own, so a slow
    /// hoster never stalls the dispatch.
    pub(crate) async fn supervise_account_traffic(&self) {
        let now = Utc::now();
        self.settle_account_traffic(now).await;
        for account in self.traffic_holds.due_checks(now) {
            let scheduler = self.clone();
            tokio::spawn(async move {
                match scheduler.resolvers.check_account(account).await {
                    Ok(status) => {
                        let left = status.traffic_left.map(rd_core::ByteCount::get);
                        if let Err(error) = scheduler.account_checked(account, left).await {
                            tracing::warn!(%error, account_id = %account, "the account's waiting downloads were not released");
                        }
                    }
                    Err(failure) => {
                        tracing::debug!(account_id = %account, error = %failure, "the traffic check of a held account failed");
                    }
                }
            });
        }
    }

    async fn settle_account_traffic(&self, now: DateTime<Utc>) {
        if let Some(held) = self.traffic_holds.settle(now) {
            self.network_hold
                .set(HoldSource::AccountTraffic, held.then_some(HOLD_REASON))
                .await;
        }
    }

    /// What a check of `account` found: traffic left, in bytes, when it said. Traffic above
    /// zero releases the account and queues every file of it that waits for its traffic;
    /// anything else changes nothing. Answers how many files were queued.
    ///
    /// Also for a check nobody here asked for — the account list's "test" — so a quota that
    /// came back is used at once rather than at the end of the hoster's wait.
    pub async fn account_checked(
        &self,
        account: AccountId,
        traffic_left: Option<u64>,
    ) -> Result<usize> {
        if !traffic_left.is_some_and(|left| left > 0) {
            return Ok(0);
        }
        let released = self.traffic_holds.release(account);
        self.settle_account_traffic(Utc::now()).await;
        let mut queued = 0;
        for file in self.database.startable_downloads().await? {
            if file.account_id != Some(account) || !waits_for_traffic(&file) {
                continue;
            }
            match self
                .database
                .transition_download(file.id, DownloadState::Queued)
                .await
            {
                Ok(_) => queued += 1,
                Err(error) => {
                    tracing::debug!(%error, download = %file.id, "a file waiting for its account's traffic was not queued");
                }
            }
        }
        if released || queued > 0 {
            tracing::info!(account_id = %account, queued, "the account has traffic again; its downloads continue");
        }
        Ok(queued)
    }

    /// The accounts whose traffic is used up, the soonest to end first.
    #[must_use]
    pub fn account_traffic(&self) -> Vec<AccountTrafficHold> {
        self.traffic_holds.list(Utc::now())
    }

    /// Lets every account go: somebody started the queue by hand, which outranks an automatic
    /// hold. The files that wait for their traffic keep their own due times.
    pub async fn release_account_traffic(&self) {
        self.traffic_holds.release_all();
        self.settle_account_traffic(Utc::now()).await;
    }

    /// Rebuilds the holds from the files that still wait for their account's traffic, before
    /// the first dispatch, so a start does not send the account's other files into the limit.
    pub(crate) async fn restore_account_traffic(&self) -> Result<()> {
        let now = Utc::now();
        for file in self.database.startable_downloads().await? {
            if let (Some(account), Some(until)) = (file.account_id, file.next_retry_at)
                && waits_for_traffic(&file)
                && until > now
            {
                self.traffic_holds.hold(account, until, now);
            }
        }
        self.settle_account_traffic(now).await;
        Ok(())
    }
}

#[cfg(test)]
#[path = "account_traffic_tests.rs"]
mod tests;
