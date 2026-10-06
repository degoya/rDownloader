//! The repository's own rules, read off its files: English-only Rust sources, the address forms of
//! the plugin catalogues, plugin paths read at run time, the `no-components` profile, plugin crate
//! versions.
//!
//! One test binary, each rule a module of it (RD-1120-08): every one was a binary of its own that
//! linked for a test reading a few files.

use std::path::{Path, PathBuf};

use serde_json::Value;

mod french_spanish_address;
mod german_address;
mod no_components_profile;
mod no_german;
mod plugin_crate_versions;
mod plugin_paths_at_run_time;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

/// Every string of a catalogue document, with its dotted key path.
fn strings(value: &Value, path: &str, out: &mut Vec<(String, String)>) {
    match value {
        Value::String(text) => out.push((path.to_owned(), text.clone())),
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                strings(child, &child_path, out);
            }
        }
        _ => {}
    }
}
