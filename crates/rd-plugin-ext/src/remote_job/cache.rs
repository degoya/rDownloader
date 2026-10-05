//! The cache questions (RD-130-11): which providers can ask a cache and about what, and the
//! batched check of whether a provider holds each source ready.
//!
//! Split out of `remote_job.rs` (PLUG-21).

use anyhow::Result;
use rd_core::AccountId;
use rd_plugin_host::extension::{
    CacheAnswer, CacheKind, CacheQuery, MAX_CACHE_CONTAINER_BYTES, MAX_CACHE_QUERIES,
    RemoteJobSource,
};

use super::{JobRefusal, NO_PLUGIN, RemoteJobRunners, Runner};

impl RemoteJobRunners {
    /// Every provider whose plugin can ask a cache, with the kinds it asks about, sorted by
    /// slug (RD-130-11). A provider two plugins claim is the one `by_slug` routes to, the same
    /// rule a new job follows.
    pub async fn cache_providers(&self) -> Vec<(String, Vec<CacheKind>)> {
        let mut slugs: Vec<(&String, &usize)> = self.by_slug.iter().collect();
        slugs.sort_unstable_by(|left, right| left.0.cmp(right.0));
        let mut providers = Vec::new();
        for (slug, index) in slugs {
            let Some(runner) = self.plugins.get(*index) else {
                continue;
            };
            let kinds = Self::kinds_of(runner).await;
            if !kinds.is_empty() {
                providers.push((slug.clone(), kinds.to_vec()));
            }
        }
        providers
    }

    /// Whether the provider behind `provider_slug` holds each source ready (RD-130-11).
    ///
    /// One answer per query, in the order given. A query of a kind the plugin did not name
    /// answers `Unknown` without reaching it; the rest go in batches the host wrapper accepts
    /// -- at most [`MAX_CACHE_QUERIES`] and [`MAX_CACHE_CONTAINER_BYTES`] each -- and the
    /// answers are put back where their queries stood. Any refusal or trap drops the whole
    /// call: a half-answered check is not something the caller could tell from a whole one.
    pub async fn check_cached(
        &self,
        provider_slug: &str,
        account: AccountId,
        queries: &[CacheQuery],
    ) -> Result<Vec<CacheAnswer>, JobRefusal> {
        let runner = self
            .by_slug
            .get(&provider_slug.to_ascii_lowercase())
            .and_then(|index| self.plugins.get(*index))
            .ok_or_else(|| {
                JobRefusal::permanent(
                    NO_PLUGIN,
                    format!("no installed plugin asks a cache for {provider_slug}"),
                )
            })?;
        let kinds = Self::kinds_of(runner).await;
        let mut answers = vec![CacheAnswer::unknown(); queries.len()];
        let asked: Vec<usize> = queries
            .iter()
            .enumerate()
            .filter(|(_, query)| kinds.contains(&query.kind))
            .map(|(index, _)| index)
            .collect();
        for batch in cache_batches(queries, &asked) {
            let batch_queries: Vec<CacheQuery> =
                batch.iter().map(|&index| queries[index].clone()).collect();
            let received = Self::settle(
                runner,
                runner.plugin.check_cached(account, &batch_queries).await,
            )?;
            if received.len() != batch.len() {
                // The host wrapper refuses a misaligned list already; a driver that is not
                // the wrapper is held to the same rule here.
                return Err(JobRefusal::permanent(
                    "remote_job.cache_answer_misaligned",
                    format!("{} answered a cache check out of step", runner.info.name),
                ));
            }
            for (index, answer) in batch.into_iter().zip(received) {
                answers[index] = answer;
            }
        }
        Ok(answers)
    }

    /// What `cache-kinds` answered for this runner, asked once.
    async fn kinds_of(runner: &Runner) -> &[CacheKind] {
        runner
            .kinds
            .get_or_init(|| async {
                match runner.plugin.cache_kinds().await {
                    Ok(mut kinds) => {
                        kinds.sort_unstable();
                        kinds.dedup();
                        kinds
                    }
                    Err(error) => {
                        tracing::warn!(
                            plugin = %runner.info.name,
                            %error,
                            "remote-job plugin could not name its cache kinds"
                        );
                        Vec::new()
                    }
                }
            })
            .await
            .as_slice()
    }
}

/// Splits the positions in `asked` into batches the host wrapper accepts: at most
/// [`MAX_CACHE_QUERIES`] queries and [`MAX_CACHE_CONTAINER_BYTES`] container bytes each. A
/// single container above the byte bound is left out -- it would be refused in any batch --
/// and so answers `Unknown`.
fn cache_batches(queries: &[CacheQuery], asked: &[usize]) -> Vec<Vec<usize>> {
    let mut batches = Vec::new();
    let mut batch: Vec<usize> = Vec::new();
    let mut bytes = 0_usize;
    for &index in asked {
        let weight = match &queries[index].source {
            RemoteJobSource::Container(content) => content.len(),
            RemoteJobSource::Magnet(_) | RemoteJobSource::Address(_) => 0,
        };
        if weight > MAX_CACHE_CONTAINER_BYTES {
            continue;
        }
        if batch.len() == MAX_CACHE_QUERIES || bytes + weight > MAX_CACHE_CONTAINER_BYTES {
            batches.push(std::mem::take(&mut batch));
            bytes = 0;
        }
        batch.push(index);
        bytes += weight;
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    batches
}
