//! Native `Resolver` implementation.
//!
//! Nothing but metadata and conversions, written by `plugin_common::native_resolver!` (the
//! same struct and delegating methods for every plugin, RD-191-07): the protocol logic lives in
//! `rd-plugin-turbobit-common` and is the same code the WebAssembly component runs.

plugin_common::native_resolver!(
    /// Provider implementation that runs against a native or Component host adapter.
    HitfileResolver
);

#[cfg(test)]
#[path = "native/tests.rs"]
mod tests;
