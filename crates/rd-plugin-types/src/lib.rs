//! The vocabulary the plugins share with the service: failures, the three identifiers a plugin
//! names, byte counts, checksum algorithms and what a link check reports.
//!
//! Split out of `rd-core` (RD-1190-08, CORE-06). The plugins and `rd-plugin-api` depend on this
//! crate and nothing else of the workspace, so a change to the service's own types in `rd-core`
//! rebuilds and re-tests none of them. `rd-core` re-exports every item here, so the service keeps
//! naming them `rd_core::…`. A type belongs here only when a plugin names it; a new service type
//! goes into `rd-core`.

#![warn(unreachable_pub)]

mod download;
mod error;
mod ids;
mod link;

pub use download::{ByteCount, ChecksumAlgorithm};
pub use error::{Failure, FailureKind, MessageParams};
pub use ids::{AccountId, PluginId, ProxyProfileId};
pub use link::{LinkStatus, PluginLinkCheck};

/// Maximum representable byte count in persistent storage.
pub const MAX_PERSISTED_BYTES: u64 = i64::MAX as u64;

/// The longest wait a server, an indexer or a plugin may ask for. A day: long enough for any
/// quota that resets daily, short enough that a mistaken or hostile value cannot park a
/// download for years (or overflow the clock arithmetic that turns it into a due time).
pub const MAX_RETRY_AFTER_SECONDS: u64 = 24 * 60 * 60;
