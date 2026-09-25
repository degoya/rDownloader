//! Open directory listings in the LinkGrabber (RD-107-05).
//!
//! The long tail, covered the way JDownloader covers it: one plugin rather than one plugin
//! per server. Apache's `mod_autoindex`, nginx's `autoindex`, lighttpd's `mod_dirlisting`
//! and Caddy's `file_server browse` produce four different pages that all say the same thing
//! — here are the names under this path — and every one of them marks the parent the same
//! way, which is what [`listing::is_index`] recognises.
//!
//! This is the first *generic* crawler, and it is the reason the selection had to learn two
//! things first (RD-107-05, host gap 3). It claims an address by its shape — a path that
//! ends in a slash — so it is asked last, after every crawler that names a service, and when
//! the page turns out not to be a listing at all it says `unsupported` and the address goes
//! on to whoever is next instead of ending there.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the
//! component: [`target`] decides what is claimed, [`listing`] reads a page, [`walk`] bounds
//! the tree. `guest` is the thin wrapper around the three and exists only on `wasm32`.

pub mod listing;
pub mod messages;
pub mod target;
pub mod walk;

#[cfg(target_arch = "wasm32")]
mod guest;
