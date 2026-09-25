//! The reference stream-transform plugin (RD-110-33, ADR 0011).
//!
//! It exists to be driven by `crates/rd-plugin-ext/tests/stream_transform_contract.rs`, which
//! is the only place the twelfth world is exercised end to end as a real component. Every
//! address it claims is on `transform.example.invalid`, which resolves nowhere: this plugin
//! describes a transform, it never fetches anything.
//!
//! What the contract tests need from it, and therefore what it offers, is one case per shape
//! the host has to handle -- a full description, a description without an integrity value,
//! and the three refusals. See [`plan::Case`].

pub mod plan;

#[cfg(target_arch = "wasm32")]
mod guest;
