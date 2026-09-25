//! Pixeldrain list addresses in the LinkGrabber (RD-120-07).
//!
//! Pixeldrain publishes two public link shapes and a resolver can only serve one of them: `/u/`
//! names one file and `/l/` names a collection. `plugins/pixeldrain/` takes the first; this
//! takes the second, reads `GET /api/list/{id}` -- the endpoint the feasibility measurement of
//! 2026-09-22 found beside the file endpoints -- and hands back one candidate per entry. It is
//! a second package rather than a second code path because a manifest carries exactly one
//! `plugin_type`.
//!
//! One request and nothing else. The file links it produces are `/u/{id}` addresses the sibling
//! resolver then handles one at a time, so this never follows a link and never fetches a byte
//! of content.
//!
//! Everything that can be tested without a WebAssembly toolchain lives outside the component:
//! [`list`] decides what is claimed and reads an answer, [`messages`] holds the codes, and
//! `guest` is the thin wrapper around the two that exists only on `wasm32`.

pub mod list;
pub mod messages;

#[cfg(target_arch = "wasm32")]
mod guest;
