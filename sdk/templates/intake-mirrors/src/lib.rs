//! A scaffold intake parser that states every source of a file. It compiles, packages and
//! passes conformance as it is.
//!
//! A plain intake parser proposes one link per file, which is right for the LinkGrabber and
//! loses what a mirror list is for: the same bytes at several addresses, ranked, with the hash
//! they must match. This one proposes the best address through `parse`, as every parser does,
//! and states the whole set through `sets`; the transfer then fetches chunks from several
//! sources at once.
//!
//! Three things the host guarantees, which shape how this is written:
//!
//! - **You propose, the application decides.** What `parse` returns goes through the same
//!   review, blocklist and routing a pasted link does. A set is kept beside its candidate, and
//!   only when the candidate survives.
//! - **A set is matched by its primary address.** `sets` names each file by the address `parse`
//!   proposed for it; a set whose `primary-url` no candidate carries is dropped.
//! - **Every field is checked.** A known scheme, no password in an address, at most 32
//!   sources, hex of the right width. A field that fails is refused, so state only what the
//!   document actually said.
//!
//! The format this scaffold reads is made up for it — one file per line, sources in order of
//! preference; [`list`] describes it. Replace that module with your format's reader. A fresh
//! scaffold's `cargo test` runs it on the host target; `guest` exists only on `wasm32`.

pub mod list;

#[cfg(target_arch = "wasm32")]
mod guest;
