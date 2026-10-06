//! Shared, target-independent logic for hoster plugins built on an XFileSharing-clone (XFS)
//! script (the family of hosting engines DDownload, KatFile and many others run — documented API
//! at `<host>/api/{account,file}/...`, cookie-backed premium web flow via an `op=download2` HTML
//! form). Extracted from `plugins/ddownload` (Task 11): every function here keeps ddownload's
//! existing behavior as its default parameterization, generalized so a second XFS clone (KatFile)
//! can reuse the exact same request shapes, error classification and HTML parsing instead of
//! duplicating them.
//!
//! `rlib` only, no `rd-core`/`rd-plugin-api`/host dependency. It builds on `plugin-common`, the
//! neutral vocabulary every plugin's protocol logic is written in, so the glue the XFS plugins
//! used to copy — status classification, envelope conversion, the range probe — lives in
//! [`glue`] once (RD-191-07); `plugin-common` adds no import to a guest.

pub mod api;
pub mod free;
pub mod glue;
pub mod login;
pub mod page;
pub mod session_trace;
pub mod site;
pub mod standard;
#[cfg(feature = "test-support")]
pub mod test_support;
