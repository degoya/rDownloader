//! What the three pCloud plugins have to agree on.
//!
//! `pcloud`, `pcloud-crawler` and `pcloud-oauth` are siblings — a manifest carries exactly one
//! `plugin_type` — but three separate packages that disagreed about what a pCloud address is,
//! or about which of pCloud's two data centres an address belongs to, would be worse than one
//! package that could not exist. So the parts they have to answer identically live here, in a
//! plain library with no manifest: `scripts/build-plugins.sh` packages directories that have
//! one, so this is never a plugin and never signed. The same arrangement
//! `plugins/dropbox-common` and `plugins/google-drive-common` use.
//!
//! Deliberately *not* here: the failure codes. Each plugin owns its own `<slug>.` namespace and
//! its own translations, so a code cannot be emitted by one package and translated by another.

#![forbid(unsafe_code)]

pub mod address;
pub mod api;
pub mod metadata;
