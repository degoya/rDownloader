//! Reference transfer backend demonstrating the plugin transfer contract.
//!
//! There is no native half here, unlike the bundled hoster resolvers: this plugin exists to
//! be run as a component by the contract tests, and a native build of it would be a second
//! implementation of the thing under test. The guest is gated so the crate still builds for
//! the host target, where `protocol` — the part that needs no socket — is unit-tested.

pub mod protocol;

#[cfg(target_arch = "wasm32")]
mod guest;
