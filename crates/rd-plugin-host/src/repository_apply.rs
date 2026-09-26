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
        let delivered: Vec<PluginRepositoryInstall> = self
            .database()
            .list_plugin_repository_installs()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|install| install.repository_id == repository.id)
            .collect();
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
            if verifier.is_package_revoked(&digest).unwrap_or(false) {
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
                    let _ = verifier.revoke_package_digest(digest);
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
            if verifier.is_key_withdrawn(&key.fingerprint).unwrap_or(false) {
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
            let _ = verifier.withdraw_key(&key.fingerprint);
            // A trusted key of that id *and* fingerprint stops being trusted in memory too; one
            // with the same id and another key is somebody else's and stays.
            if verifier
                .trusted_keys
                .key(&key.key_id)
                .ok()
                .flatten()
                .is_some_and(|trusted| key_fingerprint(&trusted) == key.fingerprint)
            {
                let _ = verifier.revoke_key(&key.key_id);
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
