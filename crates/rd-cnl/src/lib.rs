//! Click'n'Load's one rule, shared by the capture agent and the service (RD-1260-10).
//!
//! A hoster page hands links over in one of two shapes: `/flash/add` with the links in the
//! clear, or `/flash/addcrypted2` with an AES-128-CBC payload whose key the page sends as `jk`
//! -- the key itself, or a script that returns it. The capture agent receives that on its own
//! port; the browser extension, which cannot open a port, takes the same request in the browser
//! and hands its fields to the service. Both read the key, decrypt and bound the fields with this
//! crate, so neither accepts what the other refuses, and `jk` is never evaluated in a browser.

#![warn(unreachable_pub)]

mod decrypt;
pub mod jk;
pub mod pace;

pub use decrypt::{MAX_CRYPTED_BYTES, decrypt, decrypt_with};

/// The broad kind of a refused hand-over. Each caller answers it with a stable code of its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefusalKind {
    /// The payload could not be read: bad base64, bad padding, not UTF-8.
    InvalidPayload,
    /// No usable key: `jk` is missing, ambiguous, or the sandbox refused its script.
    InvalidKey,
    /// Nothing in the request can be handed to the LinkGrabber.
    NoLinks,
    /// The payload or a field exceeds its limit.
    TooLarge,
    /// More hand-overs than [`pace::MAX_HAND_OVERS`] within [`pace::WINDOW`].
    RateLimited,
}

/// A refused hand-over: its kind, and the account of it that belongs in the log only.
///
/// The account never goes back to the page: "invalid CNL padding" in an answer is a padding
/// oracle against the AES path.
#[derive(Debug)]
pub struct Refusal {
    pub kind: RefusalKind,
    pub source: anyhow::Error,
}

impl Refusal {
    pub fn new(kind: RefusalKind, source: impl Into<anyhow::Error>) -> Self {
        Self {
            kind,
            source: source.into(),
        }
    }
}

#[cfg(test)]
mod tests;
