//! Jobs that run at the provider (RD-107-06): the host side of `world remote-job-plugin`.
//!
//! The eleventh type, and the first whose work outlives the call that started it. Every
//! function here is short and returns on the provider's next answer; nothing waits, because
//! nothing in a sandbox may. The waiting, the remote identifier, the clock, the person's
//! answer and the restart belong to the caller, which writes them down.
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md` records why that line is where it is.
//!
//! Like the crawler wrapper, almost everything this file does beyond calling the guest is
//! *refusing* something the guest said: a list is bounded, a percentage is clamped, a path is
//! stripped of anything that would let it out of where it belongs. The answers describe an
//! account at a third party, so they are a proposal from an untrusted party and not a fact.

use std::sync::Arc;

use anyhow::Result;
use rd_core::AccountId;
use rd_plugin_api::ResolverHost;

use super::{ExtensionRuntime, bindings::remote_job as bindings};
use crate::{PluginManifest, runtime::PluginStoreState};

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

/// A compiled remote-job plugin, pinned to one installed manifest version.
pub struct RemoteJobPlugin {
    runtime: ExtensionRuntime,
    pre: bindings::RemoteJobPluginPre<PluginStoreState>,
}

impl RemoteJobPlugin {
    /// Compiles a verified package and links only what its manifest grants.
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        let (runtime, pre) = ExtensionRuntime::build(manifest, component_bytes, host)?;
        Ok(Self {
            runtime,
            pre: bindings::RemoteJobPluginPre::new(pre)?,
        })
    }

    /// The manifest this plugin was built from.
    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        self.runtime.manifest()
    }

    /// The kinds of source this plugin can ask its provider's cache about (RD-130-11).
    ///
    /// Reaches nothing. A kind named twice counts once, and the answer is in a fixed order,
    /// so a caller that stores it can compare two loads without sorting.
    pub async fn cache_kinds(&self) -> Result<Vec<CacheKind>> {
        let mut store = self.runtime.store(None)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let kinds = instance
            .rdownloader_plugin_remote_job()
            .call_cache_kinds(&mut store)
            .await?;
        let mut kinds: Vec<CacheKind> = kinds.into_iter().map(kind_from).collect();
        kinds.sort_unstable();
        kinds.dedup();
        Ok(kinds)
    }

    /// Whether the provider holds each source ready right now (RD-130-11).
    ///
    /// Exactly one answer per query, in the order given. A batch longer than
    /// [`MAX_CACHE_QUERIES`] or heavier than [`MAX_CACHE_CONTAINER_BYTES`] is the caller's
    /// mistake and fails as a whole; the caller splits. A source carrying a marker
    /// (RD-120-66) never reaches the guest and answers `Unknown` in its place, and an answer
    /// list of the wrong length is refused whole — a shifted list would put one link's cache
    /// on another.
    pub async fn check_cached(
        &self,
        account: AccountId,
        queries: &[CacheQuery],
    ) -> Result<Result<Vec<CacheAnswer>, RemoteJobRefusal>> {
        anyhow::ensure!(
            queries.len() <= MAX_CACHE_QUERIES,
            "{} cache queries in one call, at most {MAX_CACHE_QUERIES}",
            queries.len()
        );
        let container_bytes: usize = queries
            .iter()
            .map(|query| match &query.source {
                RemoteJobSource::Container(bytes) => bytes.len(),
                RemoteJobSource::Magnet(_) | RemoteJobSource::Address(_) => 0,
            })
            .sum();
        anyhow::ensure!(
            container_bytes <= MAX_CACHE_CONTAINER_BYTES,
            "{container_bytes} container bytes in one cache check, at most {MAX_CACHE_CONTAINER_BYTES}"
        );
        let asked: Vec<usize> = queries
            .iter()
            .enumerate()
            .filter(|(_, query)| !marked(&query.source))
            .map(|(index, _)| index)
            .collect();
        let mut answers = vec![CacheAnswer::unknown(); queries.len()];
        if asked.is_empty() {
            return Ok(Ok(answers));
        }
        let wit_queries: Vec<bindings::exports::rdownloader::plugin::remote_job::CacheQuery> =
            asked
                .iter()
                .map(
                    |&index| bindings::exports::rdownloader::plugin::remote_job::CacheQuery {
                        source: to_wit_source(&queries[index].source),
                        kind: to_wit_kind(queries[index].kind),
                    },
                )
                .collect();
        let mut store = self.runtime.store(Some(account))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let answer = instance
            .rdownloader_plugin_remote_job()
            .call_check_cached(&mut store, &account.to_string(), &wit_queries)
            .await?;
        let received = match answer {
            Ok(received) => received,
            Err(failure) => return Ok(Err(refusal(failure))),
        };
        if received.len() != asked.len() {
            return Ok(Err(RemoteJobRefusal {
                code: Some("remote_job.cache_answer_misaligned".to_owned()),
                message: format!(
                    "the plugin answered {} cache queries with {} answers",
                    asked.len(),
                    received.len()
                ),
                category: rd_core::FailureKind::Permanent,
            }));
        }
        for (index, answer) in asked.into_iter().zip(received) {
            answers[index] = answer_from(answer);
        }
        Ok(Ok(answers))
    }

    /// Whether the plugin takes this source at all.
    ///
    /// Reaches nothing: asked of a magnet before it is handed to anybody, and answered from
    /// the source alone.
    pub async fn claims(&self, source: &RemoteJobSource) -> Result<bool> {
        if marked(source) {
            return Ok(false);
        }
        let mut store = self.runtime.store(None)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        Ok(instance
            .rdownloader_plugin_remote_job()
            .call_claims(&mut store, &to_wit_source(source))
            .await?)
    }

    /// The content key of a source, derived without a request.
    ///
    /// The value the duplicate guard is built on, so it is checked rather than trusted: an
    /// empty key, or one long enough to be a payload rather than a key, is refused here. A
    /// key the host cannot store is worse than no key at all — it would look like a guard
    /// and hold nothing.
    pub async fn identify(
        &self,
        source: &RemoteJobSource,
    ) -> Result<Result<String, RemoteJobRefusal>> {
        if marked(source) {
            return Ok(Err(marked_refusal()));
        }
        let mut store = self.runtime.store(None)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let answer = instance
            .rdownloader_plugin_remote_job()
            .call_identify(&mut store, &to_wit_source(source))
            .await?;
        Ok(match answer {
            Ok(key) if is_usable_key(&key) => Ok(key),
            Ok(_) => Err(RemoteJobRefusal {
                code: Some("remote_job.unusable_content_key".to_owned()),
                message: "the plugin answered with a content key the host cannot store".to_owned(),
                category: rd_core::FailureKind::Permanent,
            }),
            Err(failure) => Err(refusal(failure)),
        })
    }

    /// Hands the source to the provider.
    ///
    /// Assumed **not** idempotent. Everything that keeps it from being called twice for one
    /// source is on the caller's side — the content key, the unique row, the attempt ceiling
    /// — and this wrapper adds nothing of its own, deliberately: a retry hidden in here would
    /// be exactly the duplicate the whole design exists to prevent.
    pub async fn submit(
        &self,
        account: AccountId,
        source: &RemoteJobSource,
        content_key: &str,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
        if marked(source) {
            return Ok(Err(marked_refusal()));
        }
        let mut store = self.runtime.store(Some(account))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let request = bindings::exports::rdownloader::plugin::remote_job::SubmitRequest {
            source: to_wit_source(source),
            account_id: account.to_string(),
            content_key: content_key.to_owned(),
        };
        let answer = instance
            .rdownloader_plugin_remote_job()
            .call_submit(&mut store, &request)
            .await?;
        Ok(match answer {
            Ok(handle) => handle_from(handle, account),
            Err(failure) => Err(refusal(failure)),
        })
    }

    /// The job the provider already holds for `content_key`, if any.
    ///
    /// The crash window the row cannot close, asked of the provider instead of guessed at.
    pub async fn adopt(
        &self,
        account: AccountId,
        content_key: &str,
    ) -> Result<Result<Option<RemoteJobHandle>, RemoteJobRefusal>> {
        let mut store = self.runtime.store(Some(account))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let answer = instance
            .rdownloader_plugin_remote_job()
            .call_adopt(&mut store, &account.to_string(), content_key)
            .await?;
        Ok(match answer {
            Ok(None) => Ok(None),
            Ok(Some(handle)) => handle_from(handle, account).map(Some),
            Err(failure) => Err(refusal(failure)),
        })
    }

    /// Where the job stands now.
    pub async fn poll(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<RemoteJobProgress, RemoteJobRefusal>> {
        let mut store = self.runtime.store(Some(account))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let answer = instance
            .rdownloader_plugin_remote_job()
            .call_poll(&mut store, &to_wit_handle(handle))
            .await?;
        Ok(match answer {
            Ok(progress) => Ok(progress_from(progress)),
            Err(failure) => Err(refusal(failure)),
        })
    }

    /// Answers the question `AwaitingChoice` asked.
    ///
    /// An empty choice never reaches the guest. At one provider it is an error and at another
    /// it silently means "all of them", and neither is an answer anybody gave — so it is
    /// refused here, once, rather than in every plugin.
    pub async fn choose(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
        chosen: &[u32],
    ) -> Result<Result<(), RemoteJobRefusal>> {
        if chosen.is_empty() {
            return Ok(Err(RemoteJobRefusal {
                code: Some("remote_job.empty_choice".to_owned()),
                message: "a selection has to name at least one entry".to_owned(),
                category: rd_core::FailureKind::Permanent,
            }));
        }
        let mut store = self.runtime.store(Some(account))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let answer = instance
            .rdownloader_plugin_remote_job()
            .call_choose(&mut store, &to_wit_handle(handle), chosen)
            .await?;
        Ok(answer.map_err(refusal))
    }

    /// Removes the job at the provider.
    ///
    /// Called from one explicit, confirmed request and from no other path. Nothing in this
    /// file calls it, which is the point.
    pub async fn discard(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<(), RemoteJobRefusal>> {
        let mut store = self.runtime.store(Some(account))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let answer = instance
            .rdownloader_plugin_remote_job()
            .call_discard(&mut store, &to_wit_handle(handle))
            .await?;
        Ok(answer.map_err(refusal))
    }
}

/// Longest content key the host will store. A key is a digest or an identifier; anything
/// longer is a payload wearing a key's name.
const MAX_CONTENT_KEY: usize = 256;

fn is_usable_key(key: &str) -> bool {
    !key.trim().is_empty() && key.len() <= MAX_CONTENT_KEY && !key.contains(char::is_control)
}

/// Longest remote identifier the host will store, on the same reasoning.
const MAX_REMOTE_ID: usize = 256;

/// Accepts a handle only when the provider actually named the job.
///
/// The identifier is the whole point of the handle: without it there is nothing to poll,
/// nothing to choose against and nothing to delete, and a row carrying an empty one would be
/// a job the person can see and nobody can reach.
fn handle_from(
    handle: bindings::exports::rdownloader::plugin::remote_job::RemoteHandle,
    account: AccountId,
) -> Result<RemoteJobHandle, RemoteJobRefusal> {
    let remote_id = handle.remote_id.trim().to_owned();
    if remote_id.is_empty() || remote_id.len() > MAX_REMOTE_ID {
        return Err(RemoteJobRefusal {
            code: Some("remote_job.missing_remote_id".to_owned()),
            message: "the provider did not name the job it created".to_owned(),
            category: rd_core::FailureKind::Permanent,
        });
    }
    Ok(RemoteJobHandle {
        remote_id,
        // The account is the host's own, never the guest's answer: a plugin that named
        // another account here would be asking for a credential it was not started for.
        account_id: account.to_string(),
        job_state: handle.job_state,
    })
}

fn to_wit_source(
    source: &RemoteJobSource,
) -> bindings::exports::rdownloader::plugin::remote_job::JobSource {
    use bindings::exports::rdownloader::plugin::remote_job::JobSource as Wit;
    match source {
        RemoteJobSource::Magnet(address) => Wit::Magnet(address.clone()),
        RemoteJobSource::Container(bytes) => Wit::Container(bytes.clone()),
        RemoteJobSource::Address(address) => Wit::Address(address.clone()),
    }
}

fn to_wit_handle(
    handle: &RemoteJobHandle,
) -> bindings::exports::rdownloader::plugin::remote_job::RemoteHandle {
    bindings::exports::rdownloader::plugin::remote_job::RemoteHandle {
        remote_id: handle.remote_id.clone(),
        account_id: handle.account_id.clone(),
        job_state: handle.job_state.clone(),
    }
}

fn kind_from(kind: bindings::exports::rdownloader::plugin::remote_job::CacheKind) -> CacheKind {
    use bindings::exports::rdownloader::plugin::remote_job::CacheKind as Wit;
    match kind {
        Wit::Torrent => CacheKind::Torrent,
        Wit::Usenet => CacheKind::Usenet,
        Wit::Hoster => CacheKind::Hoster,
    }
}

fn to_wit_kind(kind: CacheKind) -> bindings::exports::rdownloader::plugin::remote_job::CacheKind {
    use bindings::exports::rdownloader::plugin::remote_job::CacheKind as Wit;
    match kind {
        CacheKind::Torrent => Wit::Torrent,
        CacheKind::Usenet => Wit::Usenet,
        CacheKind::Hoster => Wit::Hoster,
    }
}

fn answer_from(
    answer: bindings::exports::rdownloader::plugin::remote_job::CacheAnswer,
) -> CacheAnswer {
    use bindings::exports::rdownloader::plugin::remote_job::CacheState as Wit;
    CacheAnswer {
        state: match answer.state {
            Wit::Cached => CacheState::Cached,
            Wit::Known => CacheState::Known,
            Wit::Unknown => CacheState::Unknown,
        },
        file_name: answer
            .file_name
            .as_deref()
            .map(safe_name)
            .filter(|name| !name.is_empty()),
        size: answer.size,
    }
}

fn progress_from(
    progress: bindings::exports::rdownloader::plugin::remote_job::RemoteProgress,
) -> RemoteJobProgress {
    use bindings::exports::rdownloader::plugin::remote_job::RemoteProgress as Wit;
    match progress {
        Wit::Preparing(seconds) => RemoteJobProgress::Preparing {
            retry_after_seconds: seconds,
        },
        Wit::AwaitingChoice(entries) => RemoteJobProgress::AwaitingChoice {
            entries: entries
                .into_iter()
                .take(MAX_JOB_ENTRIES)
                .map(|entry| RemoteJobEntry {
                    id: entry.id,
                    path: safe_path(&entry.path),
                    size: entry.size,
                    selected: entry.selected,
                })
                .collect(),
        },
        Wit::Working(work) => RemoteJobProgress::Working(RemoteJobWork {
            progress_permille: work.progress_permille.map(|value| value.min(1_000)),
            speed_bytes_per_second: work.speed_bytes_per_second,
            seconds_remaining: work.seconds_remaining,
        }),
        Wit::Ready(artifacts) => RemoteJobProgress::Ready {
            artifacts: artifacts
                .into_iter()
                .take(MAX_JOB_ARTIFACTS)
                .map(|artifact| RemoteJobArtifact {
                    url: artifact.url,
                    file_name: artifact.file_name.as_deref().map(safe_name),
                    size: artifact.size,
                    package_hint: artifact
                        .package_hint
                        .as_deref()
                        .map(safe_path)
                        .filter(|hint| !hint.is_empty()),
                })
                .collect(),
        },
        Wit::Failed(failure) => RemoteJobProgress::Failed(refusal(failure)),
    }
}

/// Whether an address source carries a marker (RD-120-66). A container is bytes, not an
/// address; the host does not expand markers inside an upload, which is where it goes.
fn marked(source: &RemoteJobSource) -> bool {
    match source {
        RemoteJobSource::Magnet(address) | RemoteJobSource::Address(address) => {
            crate::foreign_address::carries_marker(address)
        }
        RemoteJobSource::Container(_) => false,
    }
}

fn marked_refusal() -> RemoteJobRefusal {
    let failure = crate::foreign_address::refused();
    RemoteJobRefusal {
        code: failure.code,
        message: failure.message,
        category: failure.category,
    }
}

fn refusal(failure: crate::component::rdownloader::plugin::types::Failure) -> RemoteJobRefusal {
    let failure = crate::component::from_wit_failure(failure);
    RemoteJobRefusal {
        code: failure.code,
        message: failure.message,
        category: failure.category,
    }
}

/// Reduces a path from a stranger's data structure to something that can only mean a place
/// inside this job.
///
/// The same rule the crawler's `package-hint` is held to, stated once here. A remote entry's
/// path becomes a file name and a folder under somebody's download directory, so a `..`, an
/// absolute path or a drive letter in it is not a path — it is an attempt to leave.
fn safe_path(path: &str) -> String {
    path.split(['/', '\\'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .map(safe_name)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// One path segment, reduced to something that can stand as a name.
fn safe_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_control() && !matches!(character, '/' | '\\' | ':'))
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .trim()
        .chars()
        .take(200)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{is_usable_key, safe_name, safe_path};

    /// A path out of a torrent is a stranger's data structure, and the one thing it may never
    /// do is name a place outside the job it came from.
    #[test]
    fn a_remote_path_cannot_leave_the_job_it_came_from() {
        assert_eq!(safe_path("Show/Season 1/ep.mkv"), "Show/Season 1/ep.mkv");
        assert_eq!(safe_path("../../etc/passwd"), "etc/passwd");
        assert_eq!(safe_path("/absolute/file.bin"), "absolute/file.bin");
        // A backslash is a separator here too, so a drive letter loses its colon and becomes
        // an ordinary relative folder rather than a root.
        assert_eq!(
            safe_path("C:\\Windows\\system32\\x"),
            "C/Windows/system32/x"
        );
        assert_eq!(safe_path("./a/./b"), "a/b");
        assert_eq!(safe_path("   "), "");
    }

    /// Control characters in a name reach a log line and a file system; both are reasons to
    /// drop them here rather than to hope.
    #[test]
    fn a_remote_name_keeps_nothing_that_is_not_a_name() {
        assert_eq!(safe_name("ep\u{0}01.mkv"), "ep01.mkv");
        assert_eq!(safe_name("  spaced.mkv  "), "spaced.mkv");
        assert_eq!(safe_name("...hidden..."), "hidden");
        assert_eq!(safe_name(&"x".repeat(400)).len(), 200);
    }

    /// The content key is what the duplicate guard is built on, so a key the host could not
    /// store is refused rather than written down in a shortened form nobody can match again.
    #[test]
    fn a_content_key_the_host_cannot_store_is_refused() {
        assert!(is_usable_key("c8f1a0b2"));
        assert!(!is_usable_key(""));
        assert!(!is_usable_key("   "));
        assert!(!is_usable_key(&"a".repeat(257)));
        assert!(!is_usable_key("has\na newline"));
    }
}
