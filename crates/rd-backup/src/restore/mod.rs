//! Restoring a full backup (RD-160-03): preview, test restore, path remap and the cutover.
//!
//! Three steps, each asking for the passphrase again (owner's decision, 2026-09-28) — the key
//! the service keeps for its scheduled runs is never used to open an archive:
//!
//! * **Preview** ([`inspect::read_archive`]) reads the whole archive and checks every member
//!   against the manifest, keeps the small parts in memory and writes nothing.
//! * **Test restore** unpacks into a throwaway folder below `restore-work/`, migrates the
//!   database copy with this build's migrations (a copy from a newer build is refused), applies
//!   the path plan ([`plan`]) to the copy, reports what it found, and removes the folder.
//! * **Restore** does the same into a folder it then stages ([`cutover::stage`]); the switch
//!   happens at the next start ([`cutover::apply_pending`]), and the previous installation stays
//!   startable until the restored one has started once.
//!
//! Moving storage roots between systems — Windows drive letters and shares, POSIX paths — is
//! [`paths`]; a remapped path can never leave its root.
//!
//! The settings bundle inside the archive, with its sealed credentials, is the service's to
//! read: this crate hands it over as bytes.

pub mod cutover;
pub mod inspect;
pub mod paths;
pub mod plan;

/// Why a restore step failed: a stable code for the interface, and the detail.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{code}: {detail}")]
pub struct RestoreError {
    pub code: &'static str,
    pub detail: String,
}

impl RestoreError {
    #[must_use]
    pub fn new(code: &'static str, detail: impl std::fmt::Display) -> Self {
        Self {
            code,
            detail: detail.to_string(),
        }
    }
}
