//! Isolated PAR2 verification and safe, password-aware archive extraction.

mod archive;
mod direct;
mod error;
mod extract_set;
mod multipart;
mod par2;
mod passwords;
mod progress;
mod rar;
mod rar_args;
mod rar_exit;
mod sevenz_format;
mod sfv;
mod tool_env;
mod zip_format;

#[cfg(test)]
mod boundary_tests;
#[cfg(test)]
mod direct_tests;
#[cfg(test)]
mod rar_args_tests;
#[cfg(test)]
mod seven_zip_args_tests;
#[cfg(test)]
mod seven_zip_password_tests;
#[cfg(test)]
mod tests;

pub use archive::{ArchiveLimits, ExtractionReport, STAGING_PREFIX};
pub use direct::{
    DIRECT_STAGING_PREFIX, DirectRequest, DirectStaging, VolumeAsk, adopt_direct,
    extract_rar_direct,
};
pub use error::ExtractionError;
pub use extract_set::{ExtractRequest, extract_with_passwords};
pub use multipart::{ArchiveSet, MultiVolumeReader, group_archive_sets};
pub use par2::{
    Par2Error, Par2Report, has_par2_magic, is_main_par2, is_par2_index, is_par2_volume, par2_set,
    par2_volume_belongs_to, par2_volume_blocks, verify_and_repair, verify_set, volume_blocks,
};
pub use passwords::{load_password_file, password_candidates};
pub use progress::{ExtractProgress, ProgressSender, parse_tool_percent, percent_of};
pub use rar::{ExternalRarTool, RarToolKind, test_rar};
pub use rd_files::ArchiveKind;
pub use rd_files::{RCLONE_VARIABLES, restrict_environment};
pub use sfv::{SfvEntry, SfvReport, is_sfv, parse_sfv, verify_sfv};
