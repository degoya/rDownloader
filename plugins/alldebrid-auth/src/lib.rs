//! AllDebrid sign-in through the provider's PIN flow (RD-090-13).
//!
//! The same contract as a device flow, in a different shape: the person is given a short PIN
//! and an address to type it at, and the application asks the provider whether it has been
//! confirmed. Having both bundled is deliberate — a contract that fits only one of them would
//! be a contract for OAuth rather than for signing in.
//!
//! Reading the provider's answers lives in [`flow`], which knows nothing about the plugin
//! contract, so it can be unit-tested without a WebAssembly target.

pub mod flow;

#[cfg(target_arch = "wasm32")]
mod guest;
