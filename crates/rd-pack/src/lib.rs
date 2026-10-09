//! The publishing half of the command line: plugin packages, their index, the application update
//! manifest and the tool manifest.
//!
//! `rdownloader plugin …`, `update …` and `tools …` call these commands, and so
//! does the `rd-pack` binary built from this crate (RD-150-20). `scripts/build-plugins.sh` used to build
//! the whole service in the release profile — `rd-api` on one core, then one thin-LTO link —
//! only to call `plugin package` on it; `rd-pack` depends on the plugin host and the trust
//! roots and nothing above them, so the scripts and the workflows get the same commands without
//! building the service. The commands live here once, so the two binaries cannot drift.

#![warn(unreachable_pub)]

pub mod plugin;
pub mod plugin_index;
pub mod tools_manifest;
pub mod update_manifest;
