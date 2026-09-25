//! Shared, target-independent logic for hoster plugins built on an XFileSharing-clone (XFS)
//! script (the family of hosting engines DDownload, KatFile and many others run — documented API
//! at `<host>/api/{account,file}/...`, cookie-backed premium web flow via an `op=download2` HTML
//! form). Extracted from `plugins/ddownload` (Task 11): every function here keeps ddownload's
//! existing behavior as its default parameterization, generalized so a second XFS clone (KatFile)
//! can reuse the exact same request shapes, error classification and HTML parsing instead of
//! duplicating them.
//!
//! `rlib` only, no `rd-core`/`rd-plugin-api`/host dependency: both the native (`async`, `rd-core`
//! `Failure`) and WebAssembly guest (`wit_bindgen`-generated `Failure`) adapters of a consuming
//! plugin call into this crate and convert its cfg-free outcomes into their own `Failure`
//! representation — mirroring how `plugins/keep2share`'s `api.rs` already separates target-neutral
//! logic from the two target-specific adapters.

pub mod api;
pub mod free;
pub mod login;
pub mod page;
