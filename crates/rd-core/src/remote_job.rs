//! A job that runs at the provider (RD-107-06).
//!
//! The durable half of `world remote-job-plugin`. The plugin knows the provider's API and
//! remembers nothing between calls; everything that has to *last* is here and in the row this
//! type describes — the remote identifier, the content key, the person's answer, the clock and
//! the count of how often the provider has been asked to create something.
//!
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md` is where the split is argued. What this
//! file adds is the part of it that can be decided without a network: given a row, what is the
//! next thing that may happen to it, and when.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AccountId, CollectorPackageId, RemoteJobId};

/// How often the provider may be asked to create the same job before the host stops asking.
///
/// Two, and the reason is the whole idempotency argument: at every provider this world was
/// designed for, submitting is *not* idempotent, so an unbounded retry is an unbounded number
/// of torrents in somebody's account. Two attempts with an adoption check between them can be
/// reasoned about — the first may have been lost in flight, the second was not — and a third
/// says nothing a person could not find out faster by looking at their account.
pub const MAX_SUBMIT_ATTEMPTS: u32 = 2;

/// Shortest gap between two polls of one job.
///
/// Real-Debrid's API allows 250 requests a minute for the whole account, and the resolver
/// unrestricting this very job's links spends from the same budget. A floor here is what stops
/// a provider's own "ask again in 1 second" from becoming that.
pub const MIN_POLL_SECONDS: u64 = 5;

/// Longest gap between two polls of one job. A provider that asked for a day would otherwise
/// make a finished download invisible until tomorrow.
pub const MAX_POLL_SECONDS: u64 = 900;

/// What kind of thing was handed over. Kept so a restart can re-offer the same source.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteJobSourceKind {
    /// A `magnet:` address.
    Magnet,
    /// The bytes of a container the provider accepts — a `.torrent` file, or an `.nzb`.
    Container,
    /// A plain address the provider fetches for itself (RD-120-20).
    Address,
}

impl RemoteJobSourceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Magnet => "magnet",
            Self::Container => "container",
            Self::Address => "address",
        }
    }

    /// Parses the value stored in the row.
    #[must_use]
    pub fn from_str_value(value: &str) -> Option<Self> {
        match value {
            "magnet" => Some(Self::Magnet),
            "container" => Some(Self::Container),
            "address" => Some(Self::Address),
            _ => None,
        }
    }
}

/// Where a remote job has got to, in the host's own vocabulary.
///
/// Deliberately not the provider's list. Real-Debrid names ten states, Premiumize five and
/// AllDebrid a different five; what they share is these six, and a plugin's job is to say
/// which of the six a provider's word means.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteJobState {
    /// The row exists and the provider has not confirmed a job for it yet. The one state in
    /// which a `submit` may still happen, and the one a crash can leave behind.
    Submitting,
    /// The provider has it and is doing something that needs nobody.
    Preparing,
    /// Nothing moves until a person has chosen which entries they want.
    AwaitingChoice,
    /// The provider is fetching.
    Working,
    /// Finished; the addresses it produced have been taken.
    Ready,
    /// Ended by the provider, or given up on by the host.
    Failed,
    /// Removed at the provider, on an explicit confirmed request.
    Discarded,
}

impl RemoteJobState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitting => "submitting",
            Self::Preparing => "preparing",
            Self::AwaitingChoice => "awaiting_choice",
            Self::Working => "working",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Discarded => "discarded",
        }
    }

    /// Parses the value stored in the row.
    #[must_use]
    pub fn from_str_value(value: &str) -> Option<Self> {
        match value {
            "submitting" => Some(Self::Submitting),
            "preparing" => Some(Self::Preparing),
            "awaiting_choice" => Some(Self::AwaitingChoice),
            "working" => Some(Self::Working),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            "discarded" => Some(Self::Discarded),
            _ => None,
        }
    }

    /// Whether anything more can happen to this job on its own.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Ready | Self::Failed | Self::Discarded)
    }

    /// Whether the sweep should still be asking the provider about it.
    ///
    /// `AwaitingChoice` is open but not asked about: nothing at the provider changes until a
    /// person answers, and polling in the meantime spends an account's request budget on
    /// re-reading a question nobody has got to yet.
    #[must_use]
    pub fn is_polled(self) -> bool {
        matches!(self, Self::Submitting | Self::Preparing | Self::Working)
    }

    /// Whether a job in this state may move to `next`.
    ///
    /// Three rules, and each is a defect that would otherwise be invisible:
    ///
    /// - **Nothing ever goes back to `Submitting`.** The source was handed over once; a second
    ///   submit is the duplicate the whole design exists to prevent, and a state machine that
    ///   allowed the state would eventually find a path to the call.
    /// - **A job that ended stays ended.** A late poll answer arriving after the person
    ///   deleted the job must not resurrect it.
    /// - **What ended may still be deleted at the provider.** That is the one thing left to do
    ///   with a finished job, and it is why `Discarded` is reachable from `Ready` and `Failed`.
    #[must_use]
    pub fn may_advance_to(self, next: Self) -> bool {
        match (self, next) {
            (_, Self::Submitting) | (Self::Discarded, _) => false,
            (Self::Ready | Self::Failed, next) => next == Self::Discarded,
            _ => true,
        }
    }

    /// How long to wait before asking again, or `None` when this state is not polled.
    ///
    /// `hint` is what the plugin suggested, and it is a suggestion: the host clamps it into
    /// its own bounds, because a plugin that could set the interval could spend the whole
    /// account's request budget on one job.
    #[must_use]
    pub fn poll_delay_seconds(self, hint: Option<u64>) -> Option<u64> {
        if !self.is_polled() {
            return None;
        }
        let default = match self {
            // A submit that left no identifier is re-examined quickly: the adoption check
            // that follows is one request and it unblocks everything behind it.
            Self::Submitting => 10,
            Self::Preparing => 15,
            _ => 30,
        };
        Some(
            hint.unwrap_or(default)
                .clamp(MIN_POLL_SECONDS, MAX_POLL_SECONDS),
        )
    }
}

/// The next thing the host may do about a job that has no remote identifier yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmitStep {
    /// Hand the source over.
    Submit,
    /// Ask the provider what it already holds for this content key, before handing anything
    /// over a second time.
    Adopt,
    /// The job exists; there is nothing to submit and it should be polled.
    Poll,
    /// The ceiling is reached and the provider still named no job. Stop asking and say so.
    GiveUp,
}

/// One entry inside a remote job, as a person is shown it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct RemoteJobFile {
    /// The provider's own identifier for the entry. What a choice names.
    pub id: u32,
    /// Where it sits inside the job, already reduced to a path that cannot leave it.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Whether the provider preselected it — a default, not an answer.
    pub selected: bool,
}

/// One job running at a provider, as the row holds it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct RemoteJob {
    pub id: RemoteJobId,
    pub account_id: AccountId,
    /// Which plugin runs it. Read from the row rather than guessed: only one plugin claims a
    /// provider in this world, and which one a due row belongs to is not a search.
    pub plugin_id: String,
    /// What `identify` answered for the source. Unique per account, and the reason a magnet
    /// submitted twice cannot become two jobs.
    pub content_key: String,
    /// The provider's own identifier, written the moment `submit` answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    pub state: RemoteJobState,
    pub source_kind: RemoteJobSourceKind,
    /// How often the provider has been asked to create this job.
    pub submit_attempts: u32,
    /// Whether the provider has already been asked what it holds for this content key.
    ///
    /// Separate from the attempt count because it answers a different question: the count says
    /// how often something was created, this says whether the crash window has been looked
    /// into. Asking twice would be harmless and not asking at all would not.
    pub adoption_checked: bool,
    /// The LinkGrabber package the finished addresses go to, once there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<CollectorPackageId>,
    /// What the person is being asked, for a job in `AwaitingChoice`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<RemoteJobFile>,
    /// What they answered. Empty until they do; never filled in on their behalf.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chosen: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_permille: Option<u16>,
    /// Redaction-safe English text, for a job that failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Stable translation code the interface switches on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_poll_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The plugin's own bookkeeping for the next call.
    ///
    /// Never serialised, for the same reason `AuthFlow::flow_state` is not: it is the plugin's
    /// business, of no use to a client, and an API that returned it would be publishing a
    /// short-lived handle for nothing.
    #[serde(skip)]
    #[schema(ignore)]
    pub job_state: Option<String>,
}

impl RemoteJob {
    /// The next thing the host may do about this job.
    ///
    /// The whole restart rule, in one place and without a network call, so it can be read and
    /// tested rather than inferred from the order of statements in a sweep loop.
    #[must_use]
    pub fn submit_step(&self) -> SubmitStep {
        if self.remote_id.is_some() {
            return SubmitStep::Poll;
        }
        if self.submit_attempts == 0 {
            return SubmitStep::Submit;
        }
        // Something was sent and no identifier came back. Before sending a second time, ask
        // the provider what it already holds: this is the one window the unique row cannot
        // close, and guessing it wrong leaves a stranger's torrent in somebody's account.
        if !self.adoption_checked {
            return SubmitStep::Adopt;
        }
        if self.submit_attempts < MAX_SUBMIT_ATTEMPTS {
            SubmitStep::Submit
        } else {
            SubmitStep::GiveUp
        }
    }

    /// Whether a person still has to answer something about this job.
    #[must_use]
    pub fn needs_a_person(&self) -> bool {
        self.state == RemoteJobState::AwaitingChoice
    }

    /// The entries a person chose, kept to the ones the job actually offered.
    ///
    /// A selection arrives from a client and names provider ids, so it is filtered against
    /// what was shown rather than forwarded: an id the job never offered would be somebody
    /// else's entry at best and an error at worst. Deduplicated and ordered, so the same
    /// choice always produces the same call.
    #[must_use]
    pub fn accept_choice(&self, requested: &[u32]) -> Vec<u32> {
        let mut chosen: Vec<u32> = requested
            .iter()
            .copied()
            .filter(|id| self.entries.iter().any(|entry| entry.id == *id))
            .collect();
        chosen.sort_unstable();
        chosen.dedup();
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_POLL_SECONDS, MAX_SUBMIT_ATTEMPTS, MIN_POLL_SECONDS, RemoteJob, RemoteJobFile,
        RemoteJobSourceKind, RemoteJobState, SubmitStep,
    };
    use crate::{AccountId, RemoteJobId};

    fn job() -> RemoteJob {
        let now = chrono::Utc::now();
        RemoteJob {
            id: RemoteJobId::new(),
            account_id: AccountId::new(),
            plugin_id: "019d0000-0000-7000-8000-00000000011d".to_owned(),
            content_key: "c8f1a0b2".to_owned(),
            remote_id: None,
            state: RemoteJobState::Submitting,
            source_kind: RemoteJobSourceKind::Magnet,
            submit_attempts: 0,
            adoption_checked: false,
            package_id: None,
            entries: Vec::new(),
            chosen: Vec::new(),
            progress_permille: None,
            message: None,
            code: None,
            next_poll_at: Some(now),
            created_at: now,
            updated_at: now,
            job_state: None,
        }
    }

    /// The first thing that happens to a fresh row: the source goes over.
    #[test]
    fn a_fresh_row_submits_once() {
        assert_eq!(job().submit_step(), SubmitStep::Submit);
    }

    /// RD-107-06's third acceptance criterion, at the level it is actually decided.
    ///
    /// A crash between the request going out and the identifier coming back leaves a row with
    /// an attempt and no id. What must *not* happen then is a second `addMagnet`, because it
    /// is not idempotent and would leave a second torrent in the account. So the step is
    /// `Adopt` — ask the provider what it already holds — and only what that answers decides
    /// whether anything is sent again.
    #[test]
    fn a_restart_inside_the_submit_window_adopts_before_it_submits_again() {
        let mut job = job();
        job.submit_attempts = 1;
        assert_eq!(job.submit_step(), SubmitStep::Adopt);
    }

    /// Once the provider has named the job, there is nothing left to create.
    #[test]
    fn a_row_that_already_has_a_remote_id_is_never_submitted_again() {
        let mut job = job();
        job.remote_id = Some("XKCD123".to_owned());
        assert_eq!(job.submit_step(), SubmitStep::Poll);
        // Not even one that came back from a restart mid-flight with an attempt recorded.
        job.submit_attempts = MAX_SUBMIT_ATTEMPTS;
        job.adoption_checked = true;
        assert_eq!(job.submit_step(), SubmitStep::Poll);
    }

    /// An adoption that found nothing permits exactly one more attempt, and then stops.
    #[test]
    fn two_attempts_are_the_ceiling_and_the_third_is_a_refusal() {
        let mut job = job();
        job.submit_attempts = 1;
        job.adoption_checked = true;
        assert_eq!(job.submit_step(), SubmitStep::Submit);
        job.submit_attempts = MAX_SUBMIT_ATTEMPTS;
        assert_eq!(job.submit_step(), SubmitStep::GiveUp);
    }

    /// Nothing ever goes back to submitting, and nothing comes back from the dead.
    #[test]
    fn a_finished_job_only_moves_on_to_being_deleted() {
        for state in [
            RemoteJobState::Submitting,
            RemoteJobState::Preparing,
            RemoteJobState::AwaitingChoice,
            RemoteJobState::Working,
            RemoteJobState::Ready,
            RemoteJobState::Failed,
            RemoteJobState::Discarded,
        ] {
            assert!(
                !state.may_advance_to(RemoteJobState::Submitting),
                "{state:?} must not go back to submitting"
            );
        }
        // A late poll answer must not resurrect a job somebody deleted.
        assert!(!RemoteJobState::Discarded.may_advance_to(RemoteJobState::Working));
        assert!(!RemoteJobState::Ready.may_advance_to(RemoteJobState::Working));
        assert!(!RemoteJobState::Failed.may_advance_to(RemoteJobState::Ready));
        // The one thing left to do with a finished job.
        assert!(RemoteJobState::Ready.may_advance_to(RemoteJobState::Discarded));
        assert!(RemoteJobState::Failed.may_advance_to(RemoteJobState::Discarded));
    }

    /// A question nobody has answered is not a reason to keep asking the provider.
    #[test]
    fn a_job_waiting_for_a_person_is_not_polled() {
        assert_eq!(
            RemoteJobState::AwaitingChoice.poll_delay_seconds(Some(30)),
            None
        );
        for done in [
            RemoteJobState::Ready,
            RemoteJobState::Failed,
            RemoteJobState::Discarded,
        ] {
            assert_eq!(done.poll_delay_seconds(None), None, "{done:?}");
        }
        assert!(RemoteJobState::Working.poll_delay_seconds(None).is_some());
    }

    /// The plugin suggests and the host decides: a provider asking to be polled every second
    /// would spend an account's whole request budget on one job.
    #[test]
    fn a_plugins_suggested_wait_is_clamped_into_the_hosts_own_bounds() {
        assert_eq!(
            RemoteJobState::Working.poll_delay_seconds(Some(1)),
            Some(MIN_POLL_SECONDS)
        );
        assert_eq!(
            RemoteJobState::Working.poll_delay_seconds(Some(86_400)),
            Some(MAX_POLL_SECONDS)
        );
        assert_eq!(
            RemoteJobState::Working.poll_delay_seconds(Some(60)),
            Some(60)
        );
    }

    /// A choice arrives from a client and names the provider's own ids, so it is kept to what
    /// the job actually offered instead of being forwarded.
    #[test]
    fn a_choice_is_kept_to_the_entries_the_job_offered() {
        let mut job = job();
        job.entries = vec![
            RemoteJobFile {
                id: 3,
                path: "a.mkv".to_owned(),
                size: Some(10),
                selected: false,
            },
            RemoteJobFile {
                id: 7,
                path: "b.mkv".to_owned(),
                size: None,
                selected: true,
            },
        ];
        // An id nobody was shown is dropped, duplicates collapse, and the order is stable.
        assert_eq!(job.accept_choice(&[7, 99, 3, 7]), vec![3, 7]);
        // A choice of nothing stays a choice of nothing: the host does not fill one in.
        assert!(job.accept_choice(&[99]).is_empty());
    }
}
