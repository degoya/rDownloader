//! MediaFire folders in the LinkGrabber (RD-103-06).
//!
//! The half of MediaFire the resolver cannot reach. `resolve` answers with exactly one file,
//! so a folder address has no shape to come back in; `crawl` walks `folder/get_content`
//! under its own limits and hands back the files that were behind it all along, each with
//! the folder it sat in as its package suggestion. The same call answers for a list of file
//! keys (`/?key,key`), which the site itself renders as a folder.
//!
//! A sibling rather than a second world for the resolver, because a manifest carries exactly
//! one `plugin_type`. There is no account and no secret: the documented API answers for
//! public folders without a session token. What crosses between the two packages is nothing
//! but an address — the ones `mediafire_common::address` builds and the resolver claims.
//!
//! One thing the address cannot tell is whether a bare key (`/?<key>`) is a file or a
//! folder. The crawler claims it, asks `folder/get_info`, and when the API says the key is
//! no folder it disclaims the address with `unsupported` — "not mine after all" — so the
//! selection carries on to the resolver, which then treats it as the file it is.
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
