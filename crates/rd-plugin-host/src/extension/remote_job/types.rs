//! The host's own vocabulary for a remote job: the source, the handle, the progress, the
//! refusal and the cache answers, with the bounds a guest's lists are cut to.
//!
//! Split out of `remote_job.rs` (PLUG-21).

/// Most entries one `awaiting-choice` may put in front of a person.
///
/// A torrent with ten thousand files is a real thing and a selection list with ten thousand
/// rows is not a question anybody can answer. Trimmed here rather than obeyed, for the same
/// reason `MAX_CRAWLED_LINKS` exists: the host cannot assume the far end kept its word.
pub const MAX_JOB_ENTRIES: usize = 2_000;

/// Most addresses one finished job may hand back.
pub const MAX_JOB_ARTIFACTS: usize = 2_000;

/// What a person handed over, in the host's own vocabulary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteJobSource {
    /// A `magnet:` address, verbatim.
    Magnet(String),
    /// The bytes of a container the provider accepts.
    Container(Vec<u8>),
    /// A plain address the provider fetches for itself (RD-120-20).
    Address(String),
}

/// The provider's job, once it exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteJobHandle {
    /// The provider's own identifier. Written down before anything else happens.
    pub remote_id: String,
    pub account_id: String,
    /// The plugin's own bookkeeping, stored verbatim and handed back. Never shown.
    pub job_state: Option<String>,
}

/// One thing inside the remote job a person may or may not want.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteJobEntry {
    /// The provider's own identifier for this entry. It is what a choice names, so it is
    /// carried through untouched rather than replaced by a position in this list.
    pub id: u32,
    /// Where it sits inside the job. Stripped of anything that would let it out.
    pub path: String,
    pub size: Option<u64>,
    /// Whether the provider already considers it chosen — a default, not an answer.
    pub selected: bool,
}

/// One address the finished job produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteJobArtifact {
    pub url: String,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    pub package_hint: Option<String>,
}

/// How far a job that needs nobody has got.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RemoteJobWork {
    /// Thousandths, clamped to 1000.
    pub progress_permille: Option<u16>,
    pub speed_bytes_per_second: Option<u64>,
    pub seconds_remaining: Option<u64>,
}

/// Where the job stands, as the provider last described it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteJobProgress {
    /// Working on something that needs nobody. The wait is a suggestion; the host owns the
    /// clock, because a plugin that set the interval could spend the account's whole budget.
    Preparing { retry_after_seconds: Option<u64> },
    /// Nothing moves until a person has chosen.
    AwaitingChoice { entries: Vec<RemoteJobEntry> },
    /// The provider is fetching.
    Working(RemoteJobWork),
    /// Finished, with the addresses it produced.
    Ready { artifacts: Vec<RemoteJobArtifact> },
    /// The provider ended it. Terminal.
    Failed(RemoteJobRefusal),
}

/// Why something was refused, in the shape the interface can translate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteJobRefusal {
    /// Stable translation code, e.g. `realdebrid_torrents.magnet_rejected`.
    pub code: Option<String>,
    /// English, redaction-safe text; the fallback when no catalogue carries the code.
    pub message: String,
    /// What kind of failure it was, kept rather than flattened: "the provider is offline"
    /// and "the provider refused this account" arrive as one variant, and a poll loop has to
    /// wait out the first and stop on the second.
    pub category: rd_core::FailureKind,
}

impl RemoteJobRefusal {
    /// Whether waiting could plausibly change the answer.
    ///
    /// The one question the sweep asks of a refusal. A transient outage, a rate limit and an
    /// IP block all pass; everything else ends the job rather than being retried against an
    /// endpoint that may not be idempotent.
    #[must_use]
    pub fn is_worth_retrying(&self) -> bool {
        matches!(
            self.category,
            rd_core::FailureKind::Transient { .. }
                | rd_core::FailureKind::RateLimited { .. }
                | rd_core::FailureKind::IpBlocked { .. }
        )
    }
}

/// Most queries one `check-cached` call carries (RD-130-11).
///
/// The contract promises the guest this bound, so a caller with more splits them; the
/// wrapper refuses a longer batch rather than trimming it, because a trimmed batch would
/// answer for fewer sources than were asked and pair the rest with nothing. The same number
/// as Premiumize's own `cache/check` chunk and TorBox's documented "around 100 at a time".
pub const MAX_CACHE_QUERIES: usize = 100;

/// Most container bytes one `check-cached` call carries, summed over its queries. Equal to
/// the ceiling `torbox-jobs` puts on a single container; the link check sends none today.
pub const MAX_CACHE_CONTAINER_BYTES: usize = 8 * 1024 * 1024;

/// What the host knows a source to be before it asks a cache about it (RD-130-11).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CacheKind {
    /// A magnet, or the magnet of a `.torrent`'s info hash.
    Torrent,
    /// An address that serves an NZB, or an NZB's bytes.
    Usenet,
    /// A file address one of the installed hoster plugins resolves.
    Hoster,
}

/// One source the host asks a provider's cache about.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheQuery {
    pub source: RemoteJobSource,
    pub kind: CacheKind,
}

/// What a provider said about one source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheState {
    /// Held ready right now.
    Cached,
    /// Known to the provider, not held.
    Known,
    /// Nothing to say, including "not in my cache".
    Unknown,
}

/// The answer for one [`CacheQuery`], checked as far as the host can check it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheAnswer {
    pub state: CacheState,
    /// Reduced to a plain name; an empty one becomes `None`.
    pub file_name: Option<String>,
    pub size: Option<u64>,
}

impl CacheAnswer {
    /// The answer for a query that never reached a provider.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            state: CacheState::Unknown,
            file_name: None,
            size: None,
        }
    }
}
