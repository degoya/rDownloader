//! MediaFire WebAssembly Component guest.
//!
//! The whole adapter is shared: `plugin_guest` converts between the WIT vocabulary and the one
//! `crate::resolver` is written in, and exports the functions the world requires.

plugin_guest::resolver_plugin!(crate::resolver);
