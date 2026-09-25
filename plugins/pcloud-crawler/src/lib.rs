//! pCloud folders and public links in the LinkGrabber (RD-120-06).
//!
//! The half of pCloud the resolver cannot reach. `resolve` answers with exactly one file, so a
//! folder address has no shape to come back in; `crawl` walks `listfolder` — or reads the tree
//! `showpublink` answers a public link with — under its own limits and hands back the files
//! that were behind it all along, each with the folder it sat in as its package suggestion.
//!
//! A sibling rather than a second world for the resolver, because a manifest carries exactly
//! one `plugin_type`. The account, its token and the `pcloud` provider row stay with
//! `plugins/pcloud/`, which is the only one of the three that may declare a provider; what
//! crosses between them is nothing but an address — the ones `pcloud_common::address` builds
//! and the resolver claims.
//!
//! **A bare public link is this plugin's, even when it holds a single file.** pCloud's link
//! code is opaque, so no address can say which it is, and `claims-url` and `match-url` have to
//! be decided from the address alone. So the line is `fileid`: without one an address is here,
//! with one it is the resolver's — and a one-file link comes back from here as that one file,
//! spelled with its `fileid`.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`target`] decides what is claimed, [`listing`] reads an entry, [`walk`] bounds the tree.
//! `guest` is the thin wrapper around the three and exists only on `wasm32`.

pub mod listing;
pub mod messages;
pub mod target;
pub mod walk;

#[cfg(target_arch = "wasm32")]
mod guest;
