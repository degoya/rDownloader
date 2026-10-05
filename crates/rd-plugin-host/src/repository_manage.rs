//! Adding and removing a third-party repository: the probe that checks a pasted key signs the
//! index, the approval that records it, and the removal that forgets its cache and offers.
//!
//! Split out of `repository.rs` (PLUG-21).

use chrono::Utc;
use rd_db::PluginRepository;
use rd_sign::TrustStore;
use serde::Deserialize;

use super::{PluginRepositoryService, RepositoryError, RepositoryProbe, parse_https};
use crate::index::{self, IndexError};

impl PluginRepositoryService {
    /// Fetches a third-party index and checks that `public_key` signs it.
    pub async fn probe(
        &self,
        url: &str,
        public_key: &str,
    ) -> Result<RepositoryProbe, RepositoryError> {
        let url = parse_https(url)?;
        let key = rd_sign::decode_public_key(public_key.trim())
            .map_err(|_| RepositoryError::InvalidKey)?;
        let bytes = self.fetch(&url, index::MAX_INDEX_BYTES as u64).await?;
        // The envelope names its key only by id, never the key itself; each id it names is
        // tried with the pasted key, so the approval binds the id the index actually uses.
        for key_id in signature_key_ids(&bytes) {
            let trust = TrustStore::new();
            trust.trust(key_id.clone(), key)?;
            match index::verify_with(&bytes, &trust, None, Utc::now()) {
                Ok(index) => {
                    return Ok(RepositoryProbe {
                        url,
                        key_id,
                        public_key: base64_key(&key),
                        fingerprint: rd_sign::key_fingerprint(&key),
                        bytes,
                        index,
                    });
                }
                Err(IndexError::BadSignature(_) | IndexError::Untrusted(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(RepositoryError::KeyDoesNotSign)
    }

    /// Records a probed repository whose key the person approved, and adopts its index.
    pub async fn add(
        &self,
        name: String,
        probe: RepositoryProbe,
    ) -> Result<PluginRepository, RepositoryError> {
        let existing = self.0.database.list_plugin_repositories().await?;
        if existing
            .iter()
            .any(|repository| repository.url.as_deref() == Some(probe.url.as_str()))
        {
            return Err(RepositoryError::AlreadyAdded);
        }
        let repository = self
            .0
            .database
            .add_plugin_repository(rd_db::NewPluginRepository {
                id: uuid::Uuid::now_v7().to_string(),
                name,
                url: probe.url.to_string(),
                key_id: probe.key_id,
                public_key: probe.public_key,
                fingerprint: probe.fingerprint,
            })
            .await?;
        self.accept(&repository, &probe.url, probe.bytes, probe.index)
            .await?;
        Ok(repository)
    }

    /// Removes a third-party repository, its cache and its offers. What was installed from it
    /// stays installed.
    pub async fn remove(&self, id: &str) -> Result<bool, RepositoryError> {
        let removed = self
            .0
            .database
            .delete_plugin_repository(id.to_owned())
            .await?;
        if removed {
            let _ = tokio::fs::remove_file(self.cache_path(id)).await;
            if let Ok(mut indexes) = self.0.indexes.write() {
                indexes.remove(id);
            }
        }
        Ok(removed)
    }
}

/// The key ids an index's envelope names, read without trusting anything else in it.
fn signature_key_ids(bytes: &[u8]) -> Vec<String> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        signatures: Vec<Signature>,
    }
    #[derive(Deserialize)]
    struct Signature {
        key_id: String,
    }
    serde_json::from_slice::<Envelope>(bytes)
        .map(|envelope| {
            let mut ids: Vec<String> = envelope
                .signatures
                .into_iter()
                .map(|signature| signature.key_id)
                .filter(|id| !id.is_empty() && id.len() <= 128)
                .collect();
            ids.dedup();
            ids.truncate(8);
            ids
        })
        .unwrap_or_default()
}

fn base64_key(key: &rd_sign::VerifyingKey) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(key.as_bytes())
}
