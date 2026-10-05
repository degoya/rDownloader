//! Writing a check result, with what the provider caches and the metadata enrichers add to it.

use super::*;

impl LinkCheckService {
    /// Stores a failure on the candidate.
    ///
    /// A candidate carries a plain message rather than a code, so the parameters a person
    /// actually needs are folded into the text: an unknown SSH host key is useless without
    /// the fingerprint to compare, which is public data, not a secret.
    pub(super) async fn record_failure(
        &self,
        candidate: &LinkCandidate,
        failure: &rd_core::Failure,
    ) {
        let mut message = rd_core::CandidateMessage::from(failure);
        if let Some(fingerprint) = failure.params.get("fingerprint") {
            message.text.push_str(" (");
            message.text.push_str(fingerprint);
            if let Some(stored) = failure.params.get("stored_fingerprint") {
                message.text.push_str(", previously ");
                message.text.push_str(stored);
            }
            message.text.push(')');
            // The fingerprint is in the text and in no catalogue, so the translated sentence
            // would drop exactly the part this message exists for.
            message.code = None;
        }
        self.record(candidate, None, Some(message)).await;
    }

    pub(super) async fn record(
        &self,
        candidate: &LinkCandidate,
        result: Option<LinkCheckResult>,
        error: Option<rd_core::CandidateMessage>,
    ) {
        self.record_with_cache(candidate, result, error, None, None)
            .await;
    }

    /// Like [`Self::record`], with what a provider's cache said about the link merged in
    /// first (RD-130-11). `checked_by` is the slug of the account whose resolver checked it,
    /// which a resolver's own `cached` answer is stamped with.
    pub(super) async fn record_with_cache(
        &self,
        candidate: &LinkCandidate,
        result: Option<LinkCheckResult>,
        error: Option<rd_core::CandidateMessage>,
        hint: Option<&link_check_cache::CacheHint>,
        checked_by: Option<&str>,
    ) {
        let (result, cached_by) = link_check_cache::merge(result, hint, checked_by);
        let error = if result.is_some() {
            error
        } else {
            error.or_else(|| {
                Some(rd_core::CandidateMessage::coded(
                    "collector.check_failed",
                    "The check failed",
                ))
            })
        };
        // `candidate` carries the pre-claim state: a duplicate keeps its warning but still
        // gains the probed file name and media metadata.
        let was_duplicate = candidate.state == rd_core::LinkCandidateState::Duplicate;
        let online = result
            .as_ref()
            .is_some_and(|result| matches!(result.status, LinkStatus::Online | LinkStatus::Cached));
        let known = result
            .as_ref()
            .and_then(|result| result.media.as_ref())
            .and_then(|media| serde_json::to_string(media).ok());
        let file_name = result.as_ref().and_then(|result| result.file_name.clone());
        if let Err(problem) = self
            .inner
            .database
            .record_candidate_check(candidate.id, result, error, was_duplicate, cached_by)
            .await
        {
            tracing::warn!(candidate_id = %candidate.id, %problem, "could not store link check");
        }
        if online {
            self.enrich(candidate, file_name.as_deref(), known.as_deref())
                .await;
        }
    }

    /// Asks the installed enrichers about a link that resolved.
    ///
    /// Only after the check succeeded, and only when somebody switched enrichment on: an
    /// enricher reaches a service outside this machine, and doing that for every media link
    /// on the strength of having installed a plugin would be a decision nobody made.
    async fn enrich(
        &self,
        candidate: &LinkCandidate,
        file_name: Option<&str>,
        known: Option<&str>,
    ) {
        // The switch is checked before the plugins are compiled: an installation that never
        // asked for enrichment should not pay for building an enricher it will not call.
        if !self.enrichment_enabled().await {
            return;
        }
        let enrichers = self.enrichers().await;
        if enrichers.is_empty() {
            return;
        }
        // What the indexer already declared about this hit, read back and gated once more
        // before it leaves the process (RD-107-02).
        let declared = match self
            .inner
            .database
            .candidate_source_attributes(candidate.id)
            .await
        {
            Ok(attributes) => attributes,
            Err(error) => {
                tracing::warn!(
                    candidate_id = %candidate.id,
                    %error,
                    "declared attributes unreadable"
                );
                std::collections::BTreeMap::new()
            }
        };
        let known = known_for(known, &declared);
        let fields = enrichers
            .enrich(&candidate.url, file_name, known.as_deref())
            .await;
        if fields.is_empty() {
            return;
        }
        if let Err(error) = self
            .inner
            .database
            .set_candidate_enrichment(candidate.id, fields)
            .await
        {
            tracing::warn!(candidate_id = %candidate.id, %error, "enrichment could not be stored");
        }
    }

    /// The installed enrichers, compiled on first use.
    async fn enrichers(&self) -> Arc<rd_plugin_ext::MetadataEnrichers> {
        Arc::clone(
            self.inner
                .enrichers
                .get_or_init(|| async {
                    match rd_plugin_ext::MetadataEnrichers::load(
                        &self.inner.plugins,
                        Some(Arc::clone(&self.inner.plugin_host)),
                    )
                    .await
                    {
                        Ok(enrichers) => Arc::new(enrichers),
                        Err(error) => {
                            tracing::warn!(%error, "could not load metadata enricher plugins");
                            Arc::new(rd_plugin_ext::MetadataEnrichers::none())
                        }
                    }
                })
                .await,
        )
    }

    /// Whether the person switched metadata enrichment on. Read per check rather than cached:
    /// switching it off has to take effect on the next link, not after a restart.
    async fn enrichment_enabled(&self) -> bool {
        // A database error still reads as "off", as before: enrichment is an optional extra
        // and must not fail a check. A field stored with the wrong type is reported.
        self.inner
            .database
            .service_setting_field::<bool>("metadata_enrichment_enabled")
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
    }
}
