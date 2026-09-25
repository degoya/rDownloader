//! Box folders and shared links in the LinkGrabber (RD-120-05).
//!
//! The half of Box the resolver cannot reach. `resolve` answers with exactly one file, so a
//! folder address has no shape to come back in; `crawl` walks `/2.0/folders/<id>/items` under
//! its own limits and hands back the files that were behind it all along, each with the folder
//! it sat in as its package suggestion. It also takes the bare `/s/<name>` shared link, which
//! Box spells the same way whether it points at a folder or at a file — it has to ask the API
//! either way, and when the answer is a single file it hands back that one file.
//!
//! A sibling rather than a second world for the resolver, because a manifest carries exactly
//! one `plugin_type`. The account, its token and the `box` provider row stay with
//! `plugins/box/`, which is the only one of the three that may declare a provider; what crosses
//! between them is nothing but an address — the canonical `app.box.com/file/<id>`, or
//! `app.box.com/s/<name>/file/<id>` for a file found through a shared link, that
//! `box_common::address` builds and the resolver claims.
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
