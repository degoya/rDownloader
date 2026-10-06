//! Walking a Box folder tree under the shared crawler limits.
//!
//! The limits, the breadth-first order and the cycle guard are `plugin_common::walk`'s, shared
//! by every folder crawler (RD-191-07): what a person gets back from pasting a folder should not
//! depend on which cloud it was in. What is Box's own is here: a folder is named by its item id
//! — the account root included, which is `0` — and a file keeps its id, name and size. Box
//! answers `/items` a page at a time by offset; the page cap is applied by the caller, which is
//! the only one that sees the pages.

pub use plugin_common::walk::{Found, Limit, MAX_PAGES, join};

/// A folder still to be read, by the item id Box names it with.
pub type Pending = plugin_common::walk::Pending<String>;

/// The state of one crawl.
pub type Walk = plugin_common::walk::Walk<String, Found>;
