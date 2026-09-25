//! Why a managed-tool operation was refused.
//!
//! Separate variants because the answers differ, and because the REST layer turns each one
//! into a stable code the web interface translates. "Something went wrong" would leave a user
//! unable to tell a hostile manifest from a slow mirror.

/// A managed-tool failure.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The manifest is not signed by a key this build trusts, or its signature does not hold.
    #[error("tool manifest is not signed by a trusted key: {0}")]
    ManifestUntrusted(String),
    /// The manifest is correctly signed but old — a replay, an expired document, or one from
    /// further in the future than clock skew explains.
    #[error("tool manifest is stale: {0}")]
    ManifestStale(#[from] rd_sign::StaleError),
    /// The downloaded bytes do not hash to what the manifest said they would.
    #[error("downloaded {name} {version} does not match the hash in the manifest")]
    HashMismatch { name: String, version: String },
    /// Asked to activate or roll back to a version that is not on disk.
    #[error("{name} {version} is not installed")]
    VersionNotInstalled { name: String, version: String },
    /// The bytes could not be fetched at all, or arrived malformed.
    #[error("could not download {name}: {reason}")]
    DownloadFailed { name: String, reason: String },
    /// A name outside the closed list of managed tools.
    #[error("{0} is not a tool this application manages")]
    NotManaged(String),
    /// The manifest carries no build of this tool for this platform and application version.
    #[error("the tool manifest offers no build of {name} for this platform")]
    NoRelease { name: String },
    /// Managed tools are switched off in the settings.
    #[error("managed external tools are switched off")]
    Disabled,
    /// A version a job is still executing cannot be removed.
    #[error("{name} {version} is in use by a running job")]
    InUse { name: String, version: String },
    /// There is no earlier version to fall back to.
    #[error("{name} has no earlier installed version to roll back to")]
    NothingToRollBackTo { name: String },
    /// Anything else, reported as an internal error rather than as a tool decision.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ToolError {
    /// The stable REST error code for this failure.
    ///
    /// Part of the API contract: the web client maps these to translated texts, so a value
    /// here is as much a published interface as a route is.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::ManifestUntrusted(_) => "tools.manifest_untrusted",
            Self::ManifestStale(_) => "tools.manifest_stale",
            Self::HashMismatch { .. } => "tools.hash_mismatch",
            Self::VersionNotInstalled { .. } => "tools.version_not_installed",
            Self::DownloadFailed { .. } => "tools.download_failed",
            Self::NotManaged(_) => "tools.not_managed",
            Self::NoRelease { .. } => "tools.no_release",
            Self::Disabled => "tools.disabled",
            Self::InUse { .. } => "tools.version_in_use",
            Self::NothingToRollBackTo { .. } => "tools.nothing_to_roll_back_to",
            Self::Other(_) => "internal.error",
        }
    }
}
