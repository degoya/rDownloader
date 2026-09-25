//! What the three OneDrive plugins have to agree on.
//!
//! `onedrive`, `onedrive-crawler` and `onedrive-oauth` are siblings — a manifest carries
//! exactly one `plugin_type` — but three separate packages that disagreed about what a OneDrive
//! address is, or about which sharing link is a folder and which a file, would be worse than
//! one package that could not exist. So the parts they have to answer identically live here, in
//! a plain library with no manifest: `scripts/build-plugins.sh` packages directories that have
//! one, so this is never a plugin and never signed. The same arrangement `plugins/xfs-common`
//! and `plugins/google-drive-common` already use.
//!
//! Deliberately *not* here: the failure codes. Each plugin owns its own `<slug>.` namespace and
//! its own translations, so a code cannot be emitted by one package and translated by another.

#![forbid(unsafe_code)]

pub mod address;
pub mod reason;
