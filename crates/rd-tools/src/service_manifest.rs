//! The manifest half of the managed tool service: the document in force, its refresh from
//! the configured URL and the bounded fetch behind it.

use chrono::Utc;

use super::{CACHED_MANIFEST, ManagedToolService, empty_manifest};
use crate::{
    compat,
    error::ToolError,
    manifest::{self, ToolManifest},
};

/// How long a manifest refresh may take.
const REFRESH_TIMEOUT_SECONDS: u64 = 30;

/// Largest manifest this will read, so a hostile endpoint cannot answer with a stream.
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

impl ManagedToolService {
    /// A snapshot of the manifest currently in force.
    #[must_use]
    pub fn manifest(&self) -> ToolManifest {
        self.0
            .manifest
            .read()
            .map(|manifest| manifest.clone())
            .unwrap_or_else(|_| empty_manifest())
    }

    /// Replaces the manifest in force **without verifying it**.
    ///
    /// Everything on the production path goes through [`Self::refresh_manifest`], which
    /// verifies the signature and the freshness rule before anything reaches here. This exists
    /// for the tests that have to drive the install path against a local server, and for
    /// tooling that already verified the document itself. Hidden so it does not read as part
    /// of the supported surface.
    #[cfg(any(test, feature = "test-support"))]
    pub fn adopt_unverified_manifest(&self, manifest: ToolManifest) {
        self.replace_manifest(manifest);
    }

    /// Fetches, verifies and adopts a newer manifest from the configured URL.
    ///
    /// Refuses a document that is not signed by the compiled-in tool-manifest root, and one
    /// whose sequence is not above what this installation has already accepted. The accepted
    /// sequence is persisted before the manifest is adopted, so a crash between the two
    /// leaves the replay floor raised rather than lowered.
    pub async fn refresh_manifest(&self) -> Result<ToolManifest, ToolError> {
        self.require_enabled()?;
        let url = self
            .0
            .settings
            .read()
            .ok()
            .and_then(|settings| settings.managed_tools_manifest_url.clone())
            // A setting left empty, not a fault of the service: it had answered `500
            // internal.error` (RD-1240-28).
            .ok_or(ToolError::ManifestUrlMissing)?;
        if !url.starts_with("https://") {
            return Err(ToolError::ManifestUntrusted(
                "the tool manifest URL is not https".to_owned(),
            ));
        }
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(REFRESH_TIMEOUT_SECONDS),
            fetch_manifest(&self.0.client, &url),
        )
        .await
        .map_err(|_| ToolError::DownloadFailed {
            name: "manifest".to_owned(),
            reason: format!("no answer within {REFRESH_TIMEOUT_SECONDS} seconds"),
        })??;
        let known = self.known_sequence().await;
        let manifest = manifest::verify(&bytes, known, Utc::now())?;
        self.0
            .database
            .accept_tool_manifest(
                i64::try_from(manifest.sequence).unwrap_or(i64::MAX),
                manifest.issued_at.to_rfc3339(),
            )
            .await?;
        let cached = self.0.store.root().join(CACHED_MANIFEST);
        if let Some(parent) = cached.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(&cached, &bytes).await;
        self.replace_manifest(manifest.clone());
        Ok(manifest)
    }

    pub(super) async fn known_sequence(&self) -> Option<u64> {
        self.0
            .database
            .tool_manifest_state()
            .await
            .ok()
            .flatten()
            .and_then(|state| u64::try_from(state.sequence).ok())
    }

    pub(super) fn replace_manifest(&self, manifest: ToolManifest) {
        // Rules travel with the builds they talk about, so adopting a manifest is also
        // adopting its policy — and failing to read that policy degrades to the compiled-in
        // base rather than to no policy at all.
        compat::adopt_manifest(&manifest);
        if let Ok(mut current) = self.0.manifest.write() {
            *current = manifest;
        }
    }
}

/// Reads a manifest document, bounded so a hostile endpoint cannot answer with a stream.
pub(super) async fn fetch_manifest(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<u8>, ToolError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| ToolError::DownloadFailed {
            name: "manifest".to_owned(),
            // With its causes: reqwest's top line is "error sending request" (RA-TR-04).
            reason: rd_core::error_with_causes(&error),
        })?;
    if !response.status().is_success() {
        return Err(ToolError::DownloadFailed {
            name: "manifest".to_owned(),
            reason: format!("the server answered {}", response.status()),
        });
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ToolError::DownloadFailed {
            name: "manifest".to_owned(),
            // With its causes: reqwest's top line is "error sending request" (RA-TR-04).
            reason: rd_core::error_with_causes(&error),
        })?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ToolError::DownloadFailed {
            name: "manifest".to_owned(),
            reason: format!("the manifest exceeds {MAX_MANIFEST_BYTES} bytes"),
        });
    }
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use rd_core::ManagedToolSettings;

    use crate::{ManagedToolService, ToolError};

    /// RD-1240-28: refreshing without a manifest URL is a coded refusal, not `internal.error`.
    #[tokio::test]
    async fn a_refresh_without_a_manifest_url_names_the_missing_setting() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = rd_db::Database::open(directory.path().join("tools.sqlite3"))
            .await
            .expect("database");
        let service = ManagedToolService::new(
            database,
            crate::store_root(directory.path()),
            ManagedToolSettings {
                managed_tools_enabled: true,
                managed_tools_manifest_url: None,
                tool_compatibility_overrides: Vec::new(),
            },
        );
        let refused = service.refresh_manifest().await.expect_err("refused");
        assert!(
            matches!(refused, ToolError::ManifestUrlMissing),
            "{refused:?}"
        );
        assert_eq!(refused.code(), "tools.manifest_url_missing");
    }
}
