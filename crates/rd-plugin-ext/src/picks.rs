//! Pages whose entries wait for a choice (RD-1170-03).
//!
//! A rule with `groups.pick` answers a series page with a list -- thirty releases with their
//! season, episode, resolution, language and hoster -- and resolves none of them, because the
//! links of each sit behind a captcha. This module keeps that list until somebody chooses, and
//! then resolves what was chosen one entry after the other: one captcha at a time, in front of
//! the person who asked for it, with a count they can watch ("3 of 8") and a button that stops
//! it.
//!
//! **In memory, on purpose.** A list is the answer to one paste, and the paste can be repeated
//! at any time; what it resolves lands in the LinkGrabber, which is stored. Keeping the list in
//! the database would buy a list that survives a restart at the price of a migration, a crash
//! point and a clean-up for state nobody needs back. The board holds at most [`MAX_PAGES`].
//!
//! **A list keeps its id while its page is listed again** (RD-1190-17). The capture agent used
//! to repeat a clipboard paste the intake answered with `site_rules.pick_waiting`, every retry
//! listed the page again, and each listing replaced the list under a new id -- so the drawer's
//! list vanished "after a while" and *Fetch* answered `site_rules.pick_not_found`. The same
//! page listed again by the same rule now refreshes the list it already has, under the same id,
//! and a page that does leave the board is remembered with the reason ([`PickGone`]).
//!
//! **An unanswered captcha is not a failure.** Nobody solved it in time, or somebody declined
//! it: the entry goes back to `pending` with the code that says so, and can be picked again.
//! Only a refusal that is a statement about the page -- it changed, it is gone, it is guarded
//! -- marks an entry `failed`.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use chrono::{DateTime, Utc};
use rd_siterules::{Crawl, PickList, Rule, Step};
use tokio::sync::Notify;
use url::Url;

mod worker;

pub use worker::PickDelivery;

/// Most pages the board keeps. The oldest one that is not resolving makes room.
pub const MAX_PAGES: usize = 20;

/// How many departed pages the board remembers the reason for.
const GONE_KEPT: usize = 64;

/// The code an entry carries when resolving it was stopped.
pub(crate) const CANCELLED: &str = "site_rules.pick_cancelled";

/// Where one entry stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryState {
    /// Nothing fetched for it yet: never picked, stopped, or its captcha went unanswered.
    Pending,
    /// Picked, waiting for its turn.
    Queued,
    /// Its links are being fetched.
    Resolving,
    /// Its links are being fetched, and the rule's steps for it hold a captcha: what it waits
    /// for is a person.
    Captcha,
    /// Its links are in the LinkGrabber.
    Done,
    /// The page refused it for a reason a second try does not change.
    Failed,
}

impl EntryState {
    /// The stable word the interface and the tools read.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Queued => "queued",
            Self::Resolving => "resolving",
            Self::Captcha => "captcha",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// One entry's state, beside the entry the rule listed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntryProgress {
    pub state: EntryState,
    /// Why it came back `pending` or ended `failed`, as a stable code.
    pub code: Option<String>,
    /// How many links it put into the LinkGrabber, once `done`.
    pub links: u32,
}

/// One page and its entries.
#[derive(Clone, Debug)]
pub struct PickPage {
    pub id: String,
    /// The rule that listed it, as it was then: the entries are resolved by the rule that
    /// found them, even when it is edited meanwhile.
    pub rule: Rule,
    /// The address that was crawled.
    pub address: Url,
    /// The page's own name, which the rule's `package` read.
    pub package_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub list: PickList,
    /// One per entry of `list`, in the same order.
    pub progress: Vec<EntryProgress>,
    /// Whether entries are being resolved right now.
    pub running: bool,
    /// The entries of the current round, and how many of them have finished: "3 of 8".
    pub total: u32,
    pub finished: u32,
    /// Raised by every stop, so a worker of an earlier round never takes an entry of a later
    /// one.
    round: u64,
    cancel: Arc<Notify>,
}

/// What a listing produced, in short.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickSummary {
    pub id: String,
    pub entries: usize,
}

/// Why picking was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PickError {
    /// No page with this id; it was discarded, or the service restarted.
    NotFound,
    /// The page has no entry with this index.
    NoEntry(usize),
}

impl PickError {
    /// The stable code the interface translates.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "site_rules.pick_not_found",
            Self::NoEntry(_) => "site_rules.no_entry",
        }
    }
}

/// Why a page is no longer on the board (RD-1190-17). A page the board never held -- or held
/// before the service restarted -- has no reason at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickGone {
    /// Somebody discarded it.
    Discarded,
    /// The board was full and it was the oldest page not resolving.
    Evicted,
}

impl PickGone {
    /// The stable word the interface and the tools read.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discarded => "discarded",
            Self::Evicted => "evicted",
        }
    }
}

/// How resolving one entry ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EntryOutcome {
    /// So many links reached the LinkGrabber.
    Done(u32),
    /// Back to `pending`, with the reason.
    Pending(String),
    /// `failed`, with the reason.
    Failed(String),
}

/// One entry handed to the worker.
#[derive(Clone, Debug)]
pub struct PickJob {
    pub page: String,
    pub index: usize,
    pub rule: Rule,
    pub address: Url,
    pub list: PickList,
    /// The entry's own name, for the package its links land in.
    pub label: Option<String>,
}

/// Every page waiting for a choice.
#[derive(Debug, Default)]
pub struct PickBoard {
    pages: Mutex<VecDeque<PickPage>>,
    counter: AtomicU64,
    /// The last pages that left the board, newest last, with the reason.
    gone: Mutex<VecDeque<(String, PickGone)>>,
}

impl PickBoard {
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<PickPage>> {
        // A poisoned lock still holds the lists; a panic elsewhere is no reason to lose them.
        self.pages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Keeps what a two-stage rule listed. `None` when the crawl lists nothing to choose.
    ///
    /// The same page listed again by the same rule keeps its list and its id: a page that is
    /// resolving is left exactly as it is, any other gets the fresh entries, each keeping the
    /// state of the identical entry before it, and moves to the newest place. Pasting a page
    /// twice leaves one list behind, and a drawer showing it never loses it to the paste.
    pub fn add(&self, rule: &Rule, crawl: Crawl) -> Option<PickSummary> {
        let list = crawl.pick?;
        let mut pages = self.lock();
        let same = |kept: &PickPage| kept.rule.id == rule.id && kept.address == crawl.address;
        if let Some(position) = pages.iter().position(same) {
            let mut page = pages.remove(position)?;
            if !page.running {
                page.progress = carried(&page, &list);
                page.list = list;
                page.rule = rule.clone();
                page.package_name = crawl.package_name;
                page.total = 0;
                page.finished = 0;
            }
            let summary = PickSummary {
                id: page.id.clone(),
                entries: page.list.entries.len(),
            };
            pages.push_back(page);
            return Some(summary);
        }
        let entries = list.entries.len();
        let number = self.counter.fetch_add(1, Ordering::Relaxed);
        let created_at = Utc::now();
        let id = format!("{:x}-{number}", created_at.timestamp_millis());
        let page = PickPage {
            id: id.clone(),
            rule: rule.clone(),
            address: crawl.address,
            package_name: crawl.package_name,
            created_at,
            progress: vec![pending(); entries],
            list,
            running: false,
            total: 0,
            finished: 0,
            round: 0,
            cancel: Arc::new(Notify::new()),
        };
        while pages.len() >= MAX_PAGES {
            let Some(oldest) = pages.iter().position(|kept| !kept.running) else {
                break;
            };
            if let Some(evicted) = pages.remove(oldest) {
                self.left(evicted.id, PickGone::Evicted);
            }
        }
        pages.push_back(page);
        Some(PickSummary { id, entries })
    }

    /// Remembers why a page left the board.
    fn left(&self, id: String, reason: PickGone) {
        let mut gone = self
            .gone
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if gone.len() >= GONE_KEPT {
            gone.pop_front();
        }
        gone.push_back((id, reason));
    }

    /// Why a page the board no longer holds left it; `None` for a page it holds, never held,
    /// or held before the service restarted.
    #[must_use]
    pub fn gone(&self, id: &str) -> Option<PickGone> {
        self.gone
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .rev()
            .find(|(gone, _)| gone == id)
            .map(|(_, reason)| *reason)
    }

    /// Every page, oldest first.
    #[must_use]
    pub fn pages(&self) -> Vec<PickPage> {
        self.lock().iter().cloned().collect()
    }

    /// One page.
    #[must_use]
    pub fn page(&self, id: &str) -> Option<PickPage> {
        self.lock().iter().find(|page| page.id == id).cloned()
    }

    /// Discards a page, stopping whatever it is resolving. Whether there was one.
    pub fn remove(&self, id: &str) -> bool {
        let mut pages = self.lock();
        let Some(position) = pages.iter().position(|page| page.id == id) else {
            return false;
        };
        if let Some(page) = pages.remove(position) {
            page.cancel.notify_waiters();
            self.left(page.id, PickGone::Discarded);
        }
        true
    }

    /// Queues the chosen entries, and names the round a worker has to be started for when
    /// none is running ([`crate::SiteRules::work_picks`]).
    ///
    /// An entry already queued, being resolved or done is left as it is; a pending or failed
    /// one is queued again. A round that is running grows by what was added.
    pub fn queue(&self, id: &str, indices: &[usize]) -> Result<(PickPage, Option<u64>), PickError> {
        let mut pages = self.lock();
        let page = pages
            .iter_mut()
            .find(|page| page.id == id)
            .ok_or(PickError::NotFound)?;
        if let Some(missing) = indices.iter().find(|index| **index >= page.progress.len()) {
            return Err(PickError::NoEntry(*missing));
        }
        if !page.running {
            page.total = 0;
            page.finished = 0;
        }
        for index in indices {
            let entry = &mut page.progress[*index];
            if matches!(entry.state, EntryState::Pending | EntryState::Failed) {
                entry.state = EntryState::Queued;
                entry.code = None;
                page.total = page.total.saturating_add(1);
            }
        }
        let queued = page
            .progress
            .iter()
            .any(|entry| entry.state == EntryState::Queued);
        let start = queued && !page.running;
        if start {
            page.running = true;
        }
        Ok((page.clone(), start.then_some(page.round)))
    }

    /// Takes the next queued entry of a page's `round` and marks it as being resolved. `None`
    /// ends the round: nothing is queued any more, the page was stopped or it is gone.
    pub fn next(&self, id: &str, round: u64) -> Option<PickJob> {
        let mut pages = self.lock();
        let page = pages.iter_mut().find(|page| page.id == id)?;
        if page.round != round {
            return None;
        }
        let Some(index) = page
            .progress
            .iter()
            .position(|entry| entry.state == EntryState::Queued)
        else {
            page.running = false;
            return None;
        };
        page.progress[index].state = if needs_captcha(&page.rule) {
            EntryState::Captcha
        } else {
            EntryState::Resolving
        };
        Some(PickJob {
            page: page.id.clone(),
            index,
            rule: page.rule.clone(),
            address: page.address.clone(),
            list: page.list.clone(),
            label: page
                .list
                .entries
                .get(index)
                .and_then(|entry| entry.label.clone()),
        })
    }

    /// Records how one entry ended.
    pub fn finish(&self, id: &str, index: usize, outcome: EntryOutcome) {
        let mut pages = self.lock();
        let Some(page) = pages.iter_mut().find(|page| page.id == id) else {
            return;
        };
        let Some(entry) = page.progress.get_mut(index) else {
            return;
        };
        *entry = match outcome {
            EntryOutcome::Done(links) => EntryProgress {
                state: EntryState::Done,
                code: None,
                links,
            },
            EntryOutcome::Pending(code) => EntryProgress {
                state: EntryState::Pending,
                code: Some(code),
                links: 0,
            },
            EntryOutcome::Failed(code) => EntryProgress {
                state: EntryState::Failed,
                code: Some(code),
                links: 0,
            },
        };
        page.finished = page.finished.saturating_add(1).min(page.total);
    }

    /// Stops a page's round: what is queued goes back to `pending`, and the entry being
    /// resolved is given up -- its captcha leaves the broker with it.
    pub fn cancel(&self, id: &str) -> Option<PickPage> {
        let mut pages = self.lock();
        let page = pages.iter_mut().find(|page| page.id == id)?;
        for entry in &mut page.progress {
            if matches!(
                entry.state,
                EntryState::Queued | EntryState::Resolving | EntryState::Captcha
            ) {
                entry.state = EntryState::Pending;
                entry.code = Some(CANCELLED.to_owned());
            }
        }
        page.running = false;
        page.round = page.round.wrapping_add(1);
        page.cancel.notify_waiters();
        Some(page.clone())
    }

    /// What wakes a worker of this page when the page is stopped or discarded.
    fn stopper(&self, id: &str) -> Option<Arc<Notify>> {
        self.lock()
            .iter()
            .find(|page| page.id == id)
            .map(|page| Arc::clone(&page.cancel))
    }
}

/// An entry nothing has happened to yet.
fn pending() -> EntryProgress {
    EntryProgress {
        state: EntryState::Pending,
        code: None,
        links: 0,
    }
}

/// The progress of a fresh list of a page the board already holds: an entry identical to one
/// listed before keeps what became of it, so a release that is done stays done.
fn carried(page: &PickPage, list: &PickList) -> Vec<EntryProgress> {
    list.entries
        .iter()
        .map(|entry| {
            page.list
                .entries
                .iter()
                .position(|before| before.text == entry.text)
                .and_then(|index| page.progress.get(index).cloned())
                .unwrap_or_else(pending)
        })
        .collect()
}

/// Whether an entry of this rule waits for a person: its group's steps hold a captcha.
fn needs_captcha(rule: &Rule) -> bool {
    rule.groups.as_ref().is_some_and(|groups| {
        groups
            .steps
            .iter()
            .any(|step| matches!(step, Step::Captcha { .. }))
    })
}

#[cfg(test)]
#[path = "picks_tests.rs"]
mod tests;
