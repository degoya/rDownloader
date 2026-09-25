//! Streams whose bytes the host has to transform (RD-110-33, ADR 0011).
//!
//! The selection side of the twelfth world lives in `rd-plugin-host` beside the provider it
//! selects, because the scheduler needs it too and cannot depend on this crate (RD-103-02).
//! It is re-exported here so the adapters keep one import path with the other ten worlds.

pub use rd_plugin_host::extension::{StreamTransformInfo, StreamTransformProviders};
