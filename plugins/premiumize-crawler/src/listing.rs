//! The listing shapes, which now live beside the other Premiumize plugins.
//!
//! `folder/list` and `item/details` are read by this crawler and by
//! `plugins/premiumize-transfers/`, which walks the very folder a finished transfer produced.
//! One reader, in `plugins/premiumize-common/`, rather than two that could drift apart over
//! the same odd answers — a size quoted as a string, an entry with no link, a folder with no
//! id. This module is the name the crawler already used for it.

pub use premiumize_common::listing::*;
