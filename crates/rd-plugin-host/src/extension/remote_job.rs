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

mod convert;
mod types;

use convert::{
    answer_from, handle_from, is_usable_key, kind_from, marked, marked_refusal, progress_from,
    refusal, to_wit_handle, to_wit_kind, to_wit_source,
};
pub use types::{
    CacheAnswer, CacheKind, CacheQuery, CacheState, MAX_CACHE_CONTAINER_BYTES, MAX_CACHE_QUERIES,
    MAX_JOB_ARTIFACTS, MAX_JOB_ENTRIES, RemoteJobArtifact, RemoteJobEntry, RemoteJobHandle,
    RemoteJobProgress, RemoteJobRefusal, RemoteJobSource, RemoteJobWork,
};

/// `job-context`: the one label the host chose for the call in progress.
///
/// Answers what [`RemoteJobPlugin::submit_named`] put in the store and nothing else, so a
/// guest cannot ask about another job, and every call but `submit` answers `none`.
impl bindings::rdownloader::plugin::job_context::Host for PluginStoreState {
    async fn source_name(&mut self) -> Option<String> {
        self.job_source_name.clone()
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
        self.submit_named(account, source, content_key, None).await
    }

    /// [`Self::submit`], with the name the source was added under. The guest reads it through
    /// `job-context.source-name` during this call and in no other.
    pub async fn submit_named(
        &self,
        account: AccountId,
        source: &RemoteJobSource,
        content_key: &str,
        source_name: Option<&str>,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
        if marked(source) {
            return Ok(Err(marked_refusal()));
        }
        let mut store = self.runtime.store(Some(account))?;
        store.data_mut().job_source_name = source_name.map(str::to_owned);
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
