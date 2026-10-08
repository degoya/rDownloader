//! What an accepted index withdraws, applied to the trust store and persisted (RD-140-01).
//!
//! Digests go into the same store an operator's withdrawal goes into (migration 0073) and the
//! verifier's live set; keys into `plugin_withdrawn_keys` (migration 0097) and the verifier's
//! withdrawn fingerprints. Each is written only when it is new, so a refresh that repeats the
//! list writes nothing and announces nothing. The row comes before the live set, as in the
//! withdrawal route: what the next start reads back must never be less than what ran.

use rd_db::{
    NewPluginDigestRevocation, PluginRepository, PluginRepositoryInstall, PluginWithdrawnKey,
};

use super::PluginRepositoryService;
use crate::{format_package_digest, index::PluginIndex, key_fingerprint, parse_package_digest};

/// How far one repository's withdrawals reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WithdrawalScope {
    /// Any package digest and any plugin signing key: the official repository.
    Everything,
    /// Only package versions installed here from this repository, and no keys: every other
    /// repository. One somebody added for a single plugin must not be able to switch off the
    /// bundled plugins or withdraw the release key.
    DeliveredOnly,
}

/// The scope of `repository`'s withdrawals.
#[must_use]
pub fn withdrawal_scope(repository: &PluginRepository) -> WithdrawalScope {
    if repository.is_official() {
        WithdrawalScope::Everything
    } else {
        WithdrawalScope::DeliveredOnly
    }
}

impl PluginRepositoryService {
    pub(crate) async fn apply_withdrawals(
        &self,
        repository: &PluginRepository,
        index: &PluginIndex,
    ) {
        let scope = withdrawal_scope(repository);
        let Some(delivered) = delivered_by(
            &repository.id,
            scope,
            self.database().list_plugin_repository_installs().await,
        ) else {
            return;
        };
        let verifier = self.installer().verifier();
        for text in &index.revoked.package_digests {
            let Ok(digest) = parse_package_digest(text) else {
                continue;
            };
            let hex = format_package_digest(&digest);
            let from_here = delivered.iter().find(|install| install.digest == hex);
            if scope == WithdrawalScope::DeliveredOnly && from_here.is_none() {
                tracing::debug!(repository = %repository.id, digest = %hex, "ignoring a withdrawal of a package this repository did not deliver");
                continue;
            }
            // A list that cannot be read counts as "not withdrawn yet": recording the
            // withdrawal again is harmless, skipping it is not.
            let known = verifier.is_package_revoked(&digest).unwrap_or_else(|error| {
                tracing::warn!(repository = %repository.id, digest = %hex, %error, "could not read the withdrawn plugin packages; recording the withdrawal anyway");
                false
            });
            if known {
                continue;
            }
            let (plugin_id, plugin_name, version) = self.installed_context(&digest).await;
            let recorded = self
                .database()
                .revoke_plugin_digest(NewPluginDigestRevocation {
                    digest: hex.clone(),
                    plugin_id: plugin_id
                        .or_else(|| from_here.map(|install| install.plugin_id.clone())),
                    plugin_name,
                    version: version.or_else(|| from_here.map(|install| install.version.clone())),
                    reason: None,
                })
                .await;
            match recorded {
                Ok(_) => {
                    if let Err(error) = verifier.revoke_package_digest(digest) {
                        tracing::warn!(repository = %repository.id, digest = %hex, %error, "a withdrawn plugin package is recorded, but this process trusts it until the next start");
                    }
                    tracing::info!(repository = %repository.id, digest = %hex, "a plugin repository withdrew a package");
                }
                Err(error) => {
                    tracing::warn!(repository = %repository.id, digest = %hex, %error, "could not record a withdrawn plugin package");
                }
            }
        }
        if scope != WithdrawalScope::Everything {
            if !index.revoked.keys.is_empty() {
                tracing::debug!(repository = %repository.id, "ignoring key withdrawals from a third-party repository");
            }
            return;
        }
        for key in &index.revoked.keys {
            let known = verifier
                .is_key_withdrawn(&key.fingerprint)
                .unwrap_or_else(|error| {
                    tracing::warn!(repository = %repository.id, key_id = %key.key_id, fingerprint = %key.fingerprint, %error, "could not read the withdrawn plugin signing keys; recording the withdrawal anyway");
                    false
                });
            if known {
                continue;
            }
            let recorded = self
                .database()
                .withdraw_plugin_key(PluginWithdrawnKey {
                    fingerprint: key.fingerprint.clone(),
                    key_id: key.key_id.clone(),
                    repository_id: repository.id.clone(),
                    withdrawn_at: chrono::Utc::now().to_rfc3339(),
                })
                .await;
            if let Err(error) = recorded {
                tracing::warn!(key_id = %key.key_id, %error, "could not record a withdrawn plugin signing key");
                continue;
            }
            if let Err(error) = verifier.withdraw_key(&key.fingerprint) {
                tracing::warn!(repository = %repository.id, key_id = %key.key_id, fingerprint = %key.fingerprint, %error, "a withdrawn plugin signing key is recorded, but this process accepts it until the next start");
            }
            // A trusted key of that id *and* fingerprint stops being trusted in memory too; one
            // with the same id and another key is somebody else's and stays.
            let trusted = verifier
                .trusted_keys
                .key(&key.key_id)
                .unwrap_or_else(|error| {
                    tracing::warn!(repository = %repository.id, key_id = %key.key_id, fingerprint = %key.fingerprint, %error, "could not read the trusted plugin signing keys; a withdrawn key may stay trusted in this process until the next start");
                    None
                });
            if trusted.is_some_and(|trusted| key_fingerprint(&trusted) == key.fingerprint)
                && let Err(error) = verifier.revoke_key(&key.key_id)
            {
                tracing::warn!(repository = %repository.id, key_id = %key.key_id, fingerprint = %key.fingerprint, %error, "a withdrawn plugin signing key stays trusted in this process until the next start");
            }
            tracing::warn!(key_id = %key.key_id, "a plugin repository withdrew a signing key; plugins signed with it are skipped from the next start");
        }
    }

    /// The installed plugin a digest belongs to, so the withdrawal list and the plugin card can
    /// name it. Hashes the installed versions, which is only done for a digest not seen before.
    async fn installed_context(
        &self,
        digest: &[u8; 32],
    ) -> (Option<String>, Option<String>, Option<String>) {
        let installer = self.installer();
        let Ok(installed) = installer.list_installed().await else {
            return (None, None, None);
        };
        for manifest in installed {
            let id = manifest.id.to_string();
            if installer
                .installed_package_digest(&id, &manifest.version)
                .await
                .ok()
                .flatten()
                .as_ref()
                == Some(digest)
            {
                return (
                    Some(id),
                    Some(manifest.name.clone()),
                    Some(manifest.version.clone()),
                );
            }
        }
        (None, None, None)
    }
}

/// What the repository `repository_id` delivered here, from the read of every delivery record.
///
/// A third-party repository may withdraw only what it delivered, so an unreadable record used
/// to turn each of its withdrawals into "not delivered here" and drop it with a debug line
/// (PL-03). Now that is `None` with a warning: its withdrawals wait for the next refresh, which
/// applies the same list again. The official repository needs the record only to name a
/// plugin and goes on without it.
fn delivered_by(
    repository_id: &str,
    scope: WithdrawalScope,
    installs: anyhow::Result<Vec<PluginRepositoryInstall>>,
) -> Option<Vec<PluginRepositoryInstall>> {
    match installs {
        Ok(installs) => Some(
            installs
                .into_iter()
                .filter(|install| install.repository_id == repository_id)
                .collect(),
        ),
        Err(error) if scope == WithdrawalScope::DeliveredOnly => {
            tracing::warn!(repository = %repository_id, %error, "could not read which plugin packages this repository delivered; its withdrawals wait for the next refresh");
            None
        }
        Err(error) => {
            tracing::warn!(repository = %repository_id, %error, "could not read which plugin packages this repository delivered; its withdrawals are applied without naming the plugins");
            Some(Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use rd_db::PluginRepositoryInstall;

    use super::{WithdrawalScope, delivered_by};

    fn install(repository_id: &str) -> PluginRepositoryInstall {
        PluginRepositoryInstall {
            plugin_id: "019d0000-0000-7000-8000-0000000002a1".to_owned(),
            version: "1.0.0".to_owned(),
            digest: "00".repeat(32),
            repository_id: repository_id.to_owned(),
            installed_at: "2026-10-08T00:00:00Z".to_owned(),
        }
    }

    /// An unreadable delivery record stops a third-party repository's withdrawals instead of
    /// turning every one of them into "not delivered here" (PL-03).
    #[test]
    fn an_unreadable_delivery_record_stops_a_third_party_repositorys_withdrawals() {
        let read = Err(anyhow::anyhow!("database is locked"));
        assert!(delivered_by("community", WithdrawalScope::DeliveredOnly, read).is_none());
    }

    /// The official repository withdraws anything; it goes on without the record.
    #[test]
    fn the_official_repository_withdraws_without_the_delivery_record() {
        let read = Err(anyhow::anyhow!("database is locked"));
        let delivered = delivered_by("official", WithdrawalScope::Everything, read);
        assert!(delivered.is_some_and(|delivered| delivered.is_empty()));
    }

    #[test]
    fn only_the_repositorys_own_deliveries_count() {
        let read = Ok(vec![install("community"), install("elsewhere")]);
        let delivered = delivered_by("community", WithdrawalScope::DeliveredOnly, read)
            .expect("a readable record");
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].repository_id, "community");
    }
}
