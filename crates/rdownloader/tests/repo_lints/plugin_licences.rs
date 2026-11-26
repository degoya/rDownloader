//! A bundled plugin declares the licence its code is under (RD-1480-13).
//!
//! Until 1.48 `plugins/metadata.toml` said MIT for all of them, while the sources and the crates
//! they link were GPL-3.0-or-later, and the interface showed what the package declared. The
//! templates under `sdk/` and the contract are MIT on purpose; `scripts/check-sdk-templates.sh`
//! holds those.

use crate::workspace_root;

/// The `license` lines of a TOML file, their values without quotes.
fn licences(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("license = "))
        .map(|value| value.trim().trim_matches('"'))
        .collect()
}

#[test]
fn every_bundled_plugin_declares_the_workspace_licence() {
    let root = workspace_root();
    let read = |relative: &str| {
        std::fs::read_to_string(root.join(relative))
            .unwrap_or_else(|error| panic!("{relative}: {error}"))
    };
    let workspace = read("Cargo.toml");
    let [licence] = licences(&workspace)[..] else {
        panic!("Cargo.toml names one licence, [workspace.package]'s");
    };
    assert_eq!(
        licences(&read("plugins/metadata.toml")),
        [licence],
        "plugins/metadata.toml"
    );

    let mut manifests = 0;
    for entry in std::fs::read_dir(root.join("plugins")).expect("plugins directory") {
        let directory = entry.expect("a directory entry").path();
        let Ok(cargo) = std::fs::read_to_string(directory.join("Cargo.toml")) else {
            continue;
        };
        assert!(
            cargo
                .lines()
                .any(|line| line.trim() == "license.workspace = true"),
            "{} does not inherit the workspace licence",
            directory.display()
        );
        if let Ok(manifest) = std::fs::read_to_string(directory.join("manifest.toml")) {
            manifests += 1;
            for own in licences(&manifest) {
                assert_eq!(own, licence, "{}", directory.display());
            }
        }
    }
    assert!(manifests > 0, "plugins/ holds no manifest");
}
