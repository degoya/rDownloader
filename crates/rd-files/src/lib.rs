//! Safe path, file-name, capacity and checksum handling.

mod archive_names;
mod capacity;
mod checksum;
mod link;
mod long_path;
mod moves;
mod names;
mod part_file;
mod persistence;
mod storage;
mod template;
mod verified_move;

pub use archive_names::{ArchiveKind, ArchiveVolume, parse_archive_volume, strip_password_marker};
pub use capacity::{CapacityService, CapacityShortfall, CapacityVerdict, RootLimit, StorageTarget};
pub use checksum::{ComputedChecksum, checksum_range, compute_checksum, has_par2_magic};
pub use link::{
    LinkError, LinkSupport, LinkedDuplicate, link_duplicate, probe_link_support, same_file_system,
};
pub use long_path::long_path;
pub use moves::{move_directory, move_file};
pub use names::{
    collision_free_path, package_directory, package_name_from_file_name, renamed_package_directory,
    sanitize_file_name, sanitize_file_name_within,
};
pub use part_file::{PartFile, existing_bytes, part_path};
pub use persistence::{PathPersistence, PersistenceProbe};
pub use storage::{StorageRoot, StorageRootProblem, ensure_usable};
pub use template::{
    MAX_TEMPLATE_DEPTH, MAX_TEMPLATE_LENGTH, TEMPLATE_FIELDS, TemplateError, TemplateValues,
    expand, validate,
};
pub use verified_move::{
    PlacedCopy, VerifiedMoveError, place_verified, release_source, verified_move_file,
};
