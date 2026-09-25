//! OneDrive and SharePoint folders in the LinkGrabber (RD-106-05).
//!
//! The half of OneDrive the resolver cannot reach. `resolve` answers with exactly one file, so
//! a folder link has no shape to come back in; `crawl` walks `/children` under its own limits
//! and hands back the files that were behind it all along, each with the folder it sat in as
//! its package suggestion. It also takes the long `onedrive.live.com` address that does not
//! say whether it names a folder or a file — it has to ask Graph either way, and when the
//! answer is a single file it hands back that one file.
//!
//! A sibling rather than a second world for the resolver, because a manifest carries exactly
//! one `plugin_type`. The account, its token and the `onedrive` provider row stay with
//! `plugins/onedrive/`, which is the only one of the three that may declare a provider; what
//! crosses between them is nothing but an address — the canonical
//! `graph.microsoft.com/v1.0/shares/<share>/items/<item>` that
//! `onedrive_common::address::item_address` builds and the resolver claims.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`target`] decides what is claimed, [`listing`] reads an answer, [`walk`] bounds the tree.
//! `guest` is the thin wrapper around the three and exists only on `wasm32`.

pub mod listing;
pub mod messages;
pub mod target;
pub mod walk;

#[cfg(target_arch = "wasm32")]
mod guest;
