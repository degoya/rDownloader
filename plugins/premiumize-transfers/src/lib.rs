//! Premiumize transfers: the half of Premiumize the resolver deliberately does not reach
//! (RD-120-23).
//!
//! `plugins/premiumize/` unlocks a foreign link through the account with `transfer/directdl`,
//! which the provider's own documentation describes as generating a direct address *"without
//! storing it in your cloud"*. That is the right thing for a link somebody pasted and the
//! wrong thing for a magnet: there is nothing to unlock yet, the work takes minutes or hours,
//! and what it leaves behind is a folder in an account. This plugin is the other half --
//! hand a source over, let it run, take the files when they are there.
//!
//! A sibling package rather than part of `plugins/premiumize/`, because a manifest carries
//! exactly one `plugin_type`. Four plugins now share one account and one provider row:
//!
//! - `plugins/premiumize-auth/` signs in and keeps the API key.
//! - `plugins/premiumize/` unlocks hoster links -- including the ones this plugin hands back.
//! - `plugins/premiumize-crawler/` lists what lies behind a cloud folder address.
//! - this one turns a magnet, a container or a plain address into a transfer at the provider.
//!
//! What the three of them read the same way lives in `plugins/premiumize-common/`: the
//! envelope, its error vocabulary and the `folder/list` shapes.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`source`] derives the content key, [`api`] holds the response shapes, the state mapping
//! and the two request bodies. `guest` is the thin wrapper around them and exists only on
//! `wasm32`.

pub mod api;
pub mod messages;
pub mod source;

#[cfg(target_arch = "wasm32")]
mod guest;
