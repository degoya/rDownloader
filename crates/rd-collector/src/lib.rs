//! LinkGrabber, NZB and DLC intake, categories, routing rules and LinkFilter rules.

#![warn(unreachable_pub)]

mod categories;
mod container;
mod crawljob;
mod dlc;
mod grouping;
mod link_filters;
mod links;
mod mirror_separations;
mod mirrors;
mod nzb;
mod rdlinks;
mod rsdf;
mod textlist;

pub use categories::{CategoryContext, CategoryRules, select_category};
pub use container::ContainerFormat;
pub use crawljob::{Crawljob, MAX_CRAWLJOB_BYTES, read_crawljob, write_crawljob};
pub use dlc::{
    DLCRYPT_DEST_TYPE, DlcContainer, DlcDocument, DlcFile, DlcPackage, MAX_DLC_BYTES, decrypt_dlc,
    split_dlc_container,
};
pub use grouping::{Group, GroupInput, common_stem, container_name, group_links};
pub use link_filters::{LinkFilterContext, LinkFilters, compile_name_pattern};
pub use links::{canonical_url, extract_urls};
pub use mirror_separations::MirrorSeparations;
pub use mirrors::{MirrorInput, group_mirrors, language_of, quality_of};
pub use nzb::{
    MAX_NZB_BYTES, NzbDocument, NzbFile, NzbSegment, looks_like_file_name, nzb_refusal_code,
    parse_nzb, render_nzb, subject_file_name,
};
pub use rdlinks::{
    LinksDocument, LinksEntry, LinksFile, LinksKdf, LinksNzb, LinksPackage, MAX_RDLINKS_BYTES,
    MAX_RDLINKS_LINKS, RDLINKS_FORMAT, SealedLinks, carries_scheme, link_count, nzb_count,
    read_links_file, read_sealed_plaintext, sealed_plaintext, write_links_file, write_sealed_file,
};
pub use rsdf::{MAX_RSDF_BYTES, decode_rsdf};
pub use textlist::{MAX_TEXT_LIST_BYTES, parse_link_list};
