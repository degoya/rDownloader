//! Premiumize cloud folders in the LinkGrabber (RD-104-03).
//!
//! The half of Premiumize the resolver could not reach. `resolve` answers with exactly one
//! file, so `transfer/directdl` returning several used to end in `premiumize.multi_file_source`
//! — a message that told people to split the source in the LinkGrabber, where nothing could.
//! This plugin is the step that message promised: `folder/list` and `item/details`, walked
//! under limits, handed back as the files that were behind the address all along.
//!
//! A sibling rather than a second world for the resolver, because a manifest carries exactly
//! one `plugin_type`. The tree already does this three times over — `premiumize-auth`,
//! `alldebrid-auth`, `debridlink-auth` — and the account, its API key and the provider row
//! stay where they are: with the resolver.
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
