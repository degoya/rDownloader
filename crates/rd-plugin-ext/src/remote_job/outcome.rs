//! What the sweep reads back: a finished job's addresses, a refusal in the shape the interface
//! translates, and the outcome of a start and of a poll.
//!
//! Split out of `remote_job.rs` (PLUG-21).

use rd_core::{FailureKind, RemoteJobFile};
use rd_plugin_host::extension::{RemoteJobRefusal, RemoteJobWork};

use super::{NO_PLUGIN, UNSPECIFIED};

/// One address a finished job produced, in the shape the LinkGrabber takes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadyArtifact {
    pub url: url::Url,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    /// The folder the address sat in inside the job; addresses sharing one become one
    /// package.
    pub package_hint: Option<String>,
}

/// Why a call produced no answer, and whether asking again could change that.
///
/// One shape for every refusal the sweep sees, because the sweep asks every refusal the same
/// two questions: what code does the interface translate, and does the row wait or end.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobRefusal {
    /// Stable translation code; never empty.
    pub code: String,
    /// English, redaction-safe text; the fallback when no catalogue carries the code.
    pub message: String,
    /// Whether waiting could plausibly change the answer: an outage, a rate limit, a blocked
    /// address. Everything else ends the job.
    pub retryable: bool,
    /// The wait the provider suggested, when it suggested one. A suggestion: the host clamps
    /// it into its own bounds.
    pub retry_after_seconds: Option<u64>,
}

impl JobRefusal {
    pub(super) fn permanent(code: &str, message: String) -> Self {
        Self {
            code: code.to_owned(),
            message,
            retryable: false,
            retry_after_seconds: None,
        }
    }

    pub(super) fn no_plugin(plugin_id: &str) -> Self {
        Self::permanent(
            NO_PLUGIN,
            format!("no installed plugin can run remote job plugin {plugin_id}"),
        )
    }
}

impl From<RemoteJobRefusal> for JobRefusal {
    fn from(refusal: RemoteJobRefusal) -> Self {
        let retry_after_seconds = match refusal.category {
            FailureKind::Transient {
                retry_after_seconds,
            }
            | FailureKind::RateLimited {
                retry_after_seconds,
            }
            | FailureKind::IpBlocked {
                retry_after_seconds,
            } => retry_after_seconds,
            _ => None,
        };
        Self {
            retryable: refusal.is_worth_retrying(),
            retry_after_seconds,
            code: refusal
                .code
                .filter(|code| !code.trim().is_empty())
                .unwrap_or_else(|| UNSPECIFIED.to_owned()),
            message: refusal.message,
        }
    }
}

/// What asking the installed plugins to identify one source produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartOutcome {
    /// The plugin for this provider takes no such source; nothing was written.
    NotClaimed,
    /// The source is one of the plugin's, and this is the key the row is claimed under.
    /// Derived without a request, which is what lets the duplicate guard fire before one.
    Identified {
        plugin_id: String,
        content_key: String,
    },
    /// Nothing can start: no plugin claims the provider, or the plugin refused the source.
    Refused(JobRefusal),
}

/// Where a job stands, as one poll described it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PollOutcome {
    /// Working on something that needs nobody, with the wait the plugin suggested.
    Preparing { retry_after_seconds: Option<u64> },
    /// Nothing moves until a person has chosen. Never empty.
    AwaitingChoice(Vec<RemoteJobFile>),
    /// The provider is fetching.
    Working(RemoteJobWork),
    /// Finished, with the addresses the LinkGrabber may take. Never empty.
    Ready(Vec<ReadyArtifact>),
    /// The provider ended it, the call failed, or the answer held nothing usable.
    Refused(JobRefusal),
}
