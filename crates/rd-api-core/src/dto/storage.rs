//! Storage roots and their free space.

use super::*;

/// Whether writes below a storage root outlive the container.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StoragePersistence {
    /// On a mount that survives, or not in a container at all.
    Persistent,
    /// In the container's writable layer or on a memory-backed filesystem: everything below
    /// it is deleted when the container is removed or recreated.
    Ephemeral,
    /// The mount table could not be read, or the path could not be matched against it.
    Unknown,
}

impl From<rd_files::PathPersistence> for StoragePersistence {
    fn from(value: rd_files::PathPersistence) -> Self {
        match value {
            rd_files::PathPersistence::Persistent => Self::Persistent,
            rd_files::PathPersistence::Ephemeral => Self::Ephemeral,
            rd_files::PathPersistence::Unknown => Self::Unknown,
        }
    }
}

/// A storage root plus the runtime verdict on its path.
///
/// Deliberately not a field on `rd_core::StorageRootConfig`: that type is also the backup
/// type, and a machine-local verdict written into an exported bundle would mean nothing on
/// the machine that restores it.
#[derive(Serialize, ToSchema)]
pub struct StorageRootResponse {
    #[serde(flatten)]
    pub root: rd_core::StorageRootConfig,
    pub persistence: StoragePersistence,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateStorageRootRequest {
    pub name: String,
    pub path: String,
    pub is_default: bool,
    /// Free space kept on this root; empty inherits `storage_minimum_free_bytes`.
    #[serde(default)]
    pub minimum_free_bytes: Option<rd_core::ByteCount>,
}

/// Free and total capacity of one storage root.
#[derive(Serialize, ToSchema)]
pub struct StorageSpace {
    pub id: rd_core::StorageRootId,
    pub name: String,
    pub path: String,
    pub is_default: bool,
    /// `None` when the path is currently unreachable.
    pub free_bytes: Option<rd_core::ByteCount>,
    pub total_bytes: Option<rd_core::ByteCount>,
}
