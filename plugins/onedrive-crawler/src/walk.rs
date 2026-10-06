//! Walking a OneDrive folder tree under the shared crawler limits.
//!
//! The limits, the breadth-first order and the cycle guard are `plugin_common::walk`'s, shared
//! by every folder crawler (RD-191-07): what a person gets back from pasting a folder should not
//! depend on which cloud it was in. What is OneDrive's own is here: a folder is named by its
//! item id, and a file keeps its id, name and size. The walk starts at the shared root's own
//! id, so a folder that links back to it is not read twice; the caller reaches that root (depth
//! `0`) through the share alone. Graph answers `/children` a page at a time and hands back an
//! `@odata.nextLink`; the page cap is applied by the caller, which is the only one that sees
//! the pages.

pub use plugin_common::walk::{Found, Limit, MAX_PAGES, join};

/// A folder still to be read, by its item id.
pub type Pending = plugin_common::walk::Pending<String>;

/// The state of one crawl.
pub type Walk = plugin_common::walk::Walk<String, Found>;
