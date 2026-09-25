//! Capacity policy shared by every runner (part of the `service.settings` blob, keys
//! prefixed `storage_`).

use serde::{Deserialize, Serialize};

use crate::ByteCount;

/// Default free space kept on every storage root, matching the reserve the HTTP worker
/// used before the threshold became configurable.
pub const DEFAULT_MINIMUM_FREE_BYTES: u64 = 256 * 1024 * 1024;

/// Default multiple of the threshold a job of unknown size needs before it may start.
pub const DEFAULT_UNKNOWN_SIZE_HEADROOM: u32 = 4;

/// Highest headroom factor the settings validation accepts.
pub const MAX_UNKNOWN_SIZE_HEADROOM: u32 = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct StorageSettings {
    /// Free space that must remain after a download finishes, for roots without an
    /// own threshold.
    pub storage_minimum_free_bytes: ByteCount,
    /// Resume a blocked storage root by itself once space is free again; off requires
    /// the explicit resume action.
    pub storage_auto_resume: bool,
    /// A job whose size is unknown may start while at least
    /// `threshold × factor` bytes are free — the documented policy for the case where
    /// no runner can say in advance how much a transfer will write.
    pub storage_unknown_size_headroom: u32,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            storage_minimum_free_bytes: ByteCount::new(DEFAULT_MINIMUM_FREE_BYTES)
                .expect("default storage threshold fits"),
            storage_auto_resume: true,
            storage_unknown_size_headroom: DEFAULT_UNKNOWN_SIZE_HEADROOM,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_MINIMUM_FREE_BYTES, StorageSettings};

    #[test]
    fn a_blob_without_the_keys_keeps_the_previous_reserve() {
        let legacy: StorageSettings = serde_json::from_str("{}").expect("empty blob");
        assert_eq!(
            legacy.storage_minimum_free_bytes.get(),
            DEFAULT_MINIMUM_FREE_BYTES
        );
        assert!(legacy.storage_auto_resume);
    }
}
