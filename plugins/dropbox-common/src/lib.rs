//! What the three Dropbox plugins have to agree on.
//!
//! `dropbox`, `dropbox-crawler` and `dropbox-oauth` are siblings — a manifest carries exactly
//! one `plugin_type` — but three separate packages that disagreed about what a Dropbox address
//! is, or about what a metadata document looks like, would be worse than one package that could
//! not exist. So the parts they have to answer identically live here, in a plain library with
//! no manifest: `scripts/build-plugins.sh` packages directories that have one, so this is never
//! a plugin and never signed. The same arrangement `plugins/google-drive-common` uses.
//!
//! Deliberately *not* here: the failure codes. Each plugin owns its own `<slug>.` namespace and
//! its own translations, so a code cannot be emitted by one package and translated by another.

#![forbid(unsafe_code)]

pub mod address;
pub mod api_arg;
pub mod metadata;
pub mod reason;
