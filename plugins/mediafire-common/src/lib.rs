//! What the MediaFire resolver and the MediaFire folder crawler share (RD-103-06).
//!
//! Two plugins, one service: the resolver answers for a file, the crawler for a folder, and
//! both read the same addresses and the same API envelope. What each one *does* with an
//! answer stays in its own crate, and so do the failure codes — `mediafire.*` for the one,
//! `mediafire_crawler.*` for the other — because a catalogue belongs to exactly one package.
//! This crate carries no manifest and is not packaged.
//!
//! Nothing here reaches the network: [`address`] reads and builds addresses, [`api`] reads
//! the documents the API answers with. Both are plain functions over strings and bytes, which
//! is what lets them be tested on the host without a WebAssembly toolchain.

pub mod address;
pub mod api;
