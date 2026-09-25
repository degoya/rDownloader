//! LinkGrabber, NZB and DLC intake, categories and routing rules.

mod categories;
mod container;
mod dlc;
mod grouping;
mod links;
mod mirror_separations;
mod mirrors;
mod nzb;
mod rsdf;
mod textlist;

pub use categories::{CategoryContext, select_category};
pub use container::ContainerFormat;
pub use dlc::{
    DLCRYPT_DEST_TYPE, DlcContainer, DlcDocument, DlcFile, DlcPackage, MAX_DLC_BYTES, decrypt_dlc,
    split_dlc_container,
};
pub use grouping::{Group, GroupInput, common_stem, group_links};
pub use links::{canonical_url, extract_urls};
pub use mirror_separations::MirrorSeparations;
pub use mirrors::{MirrorInput, group_mirrors, language_of, quality_of};
pub use nzb::{
    MAX_NZB_BYTES, NzbDocument, NzbFile, NzbSegment, looks_like_file_name, parse_nzb,
    subject_file_name,
};
pub use rsdf::{MAX_RSDF_BYTES, decode_rsdf};
pub use textlist::{MAX_TEXT_LIST_BYTES, parse_link_list};
