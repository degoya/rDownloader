//! KrakenFiles WebAssembly Component guest.
//!
//! The whole adapter is shared: `plugin_guest` converts between the WIT vocabulary and the one
//! `crate::resolver` is written in, and exports the functions the world requires. What is
//! left here is naming the logic module.

plugin_guest::resolver_plugin!(crate::resolver);
