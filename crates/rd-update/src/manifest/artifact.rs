//! One downloadable file of a release, as the signed manifest lists it, and the rules every such
//! entry obeys — in the application's list and in the capture agent's own (RD-1210-03) alike.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// One downloadable file of a release.
///
/// `platform`, `arch` and `kind` are open strings rather than enums on purpose: a later release
/// that adds a platform or an installer kind must not make every older installation refuse the
/// whole manifest. What this build does not know it simply never selects; an unknown field it
/// ignores.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Artifact {
    /// `linux`, `windows` or `macos`.
    pub platform: String,
    /// `x86_64` or `aarch64`.
    pub arch: String,
    /// `archive` (the portable `.tar.gz`/`.zip`), `msi`, `deb` or `rpm`.
    pub kind: String,
    /// Absolute `https://` URL.
    pub url: String,
    /// 64 lowercase hex characters.
    pub sha256: String,
    /// Exact byte size.
    pub size: u64,
}

/// The artifact kinds this build knows.
pub mod kind {
    pub const ARCHIVE: &str = "archive";
    pub const MSI: &str = "msi";
    pub const DEB: &str = "deb";
    pub const RPM: &str = "rpm";
}

impl Artifact {
    fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("platform", &self.platform),
            ("arch", &self.arch),
            ("kind", &self.kind),
        ] {
            if value.is_empty()
                || value.len() > 32
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err(format!("{name} {value:?} is not a lowercase identifier"));
            }
        }
        if self.url.len() > 1024 {
            return Err("the URL is longer than 1024 bytes".to_owned());
        }
        let url = url::Url::parse(&self.url).map_err(|error| format!("URL: {error}"))?;
        if url.scheme() != "https"
            || url.host_str().is_none_or(str::is_empty)
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(format!("{} is not a plain https:// URL", self.url));
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("sha256 is not 64 lowercase hex characters".to_owned());
        }
        if self.size == 0 || self.size > super::MAX_ARTIFACT_BYTES {
            return Err(format!(
                "size {} is outside 1..={}",
                self.size,
                super::MAX_ARTIFACT_BYTES
            ));
        }
        Ok(())
    }
}

/// Refuses a list with an entry that breaks the rules above, or with one platform, architecture
/// and kind listed twice. `label` names the list in the reason (`agent ` for the agent's).
pub(super) fn validate_list(artifacts: &[Artifact], label: &str) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for artifact in artifacts {
        let entry = format!(
            "{label}{}/{}/{}",
            artifact.platform, artifact.arch, artifact.kind
        );
        artifact
            .validate()
            .map_err(|reason| format!("{entry}: {reason}"))?;
        if !seen.insert((
            artifact.platform.as_str(),
            artifact.arch.as_str(),
            artifact.kind.as_str(),
        )) {
            return Err(format!("{entry} is listed twice"));
        }
    }
    Ok(())
}
