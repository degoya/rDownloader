//! Safe path, file-name, capacity and checksum handling.

mod archive_names;
mod capacity;
mod checksum;
mod child_process;
mod link;
mod long_path;
mod moves;
mod names;
mod part_file;
mod persistence;
mod private_dir;
mod protected;
mod storage;
mod template;
mod verified_move;

pub use archive_names::{ArchiveKind, ArchiveVolume, parse_archive_volume, strip_password_marker};
pub use capacity::{CapacityService, CapacityShortfall, CapacityVerdict, RootLimit, StorageTarget};
pub use checksum::{ComputedChecksum, checksum_range, compute_checksum, has_par2_magic};
pub use child_process::{
    CREATE_NO_WINDOW, NoConsoleWindow, RCLONE_VARIABLES, TOOL_VARIABLES, kept_variables, read_tail,
    restrict_environment,
};
pub use link::{
    LinkError, LinkSupport, LinkedDuplicate, link_duplicate, probe_link_support, same_file_system,
};
pub use long_path::long_path;
pub use moves::{move_directory, move_file};
pub use names::{
    collision_free_path, extraction_subfolder, package_directory, package_name_from_file_name,
    renamed_package_directory, sanitize_file_name, sanitize_file_name_within,
};
pub use part_file::{PartFile, existing_bytes, part_path};
pub use persistence::{PathPersistence, PersistenceProbe};
pub use private_dir::{
    create_private_dir_all, private_dir_exposure, protect_private_dir, restrict_to_owner,
    sid_from_whoami, unix_exposure, windows_exposure,
};
pub use protected::{ProtectedDirectory, protected_collision};
pub use storage::{StorageRoot, StorageRootProblem, ensure_usable};
pub use template::{
    MAX_TEMPLATE_DEPTH, MAX_TEMPLATE_LENGTH, TEMPLATE_FIELDS, TemplateError, TemplateValues,
    expand, validate,
};
pub use verified_move::{
    PlacedCopy, VerifiedMoveError, copy_verified, place_verified, release_source,
    verified_move_file,
};
