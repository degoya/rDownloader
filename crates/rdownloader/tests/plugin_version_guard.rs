//! Guards RD-120-47, "same version, same content": `scripts/build-plugins.sh --list-unbumped`
//! names a plugin whose built component, manifest or locales differ from a signed package that
//! already carries the plugin's current version.
//!
//! An installation only takes a bundled package whose version is newer than the installed one.
//! On 2026-09-23 six plugins had changed and kept their version, so the owner's instance kept
//! running their old code, and a fixed bug came back word for word. Every test runs against
//! freshly built code, so only a comparison with what was signed can see it. The packaging guard
//! in the same script and the stage in `scripts/check.sh` both ask this question through the same
//! function, `package_drift`.
//!
//! The cases run the script against a throwaway tree: `CARGO_TARGET_DIR` points it at a fake
//! component, `RD_PLUGIN_PACKAGES` at a fake signed set. The plugin itself -- manifest and
//! locales -- is a real one from `plugins/`, read and never written. The packages are built with
//! the `zip` tool; the packager stores its members verbatim, which is all the comparison reads.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    Path::new(&manifest)
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

/// The first plugin, by name, that has a manifest and an English catalogue, with its version.
fn real_plugin() -> (String, String) {
    let mut names: Vec<String> = std::fs::read_dir(workspace_root().join("plugins"))
        .expect("plugins directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        let directory = workspace_root().join("plugins").join(&name);
        let Ok(manifest) = std::fs::read_to_string(directory.join("manifest.toml")) else {
            continue;
        };
        if !directory.join("locales/en.json").is_file() {
            continue;
        }
        let version = manifest
            .lines()
            .find_map(|line| line.strip_prefix("version = \""))
            .and_then(|rest| rest.strip_suffix('"'))
            .expect("the manifest declares a version")
            .to_owned();
        return (name, version);
    }
    panic!("no plugin with a manifest and locales/en.json");
}

struct Tree {
    root: tempfile::TempDir,
    name: String,
    version: String,
}

impl Tree {
    /// A built component whose bytes are `component`, and no signed package yet.
    fn new(component: &[u8]) -> Self {
        let (name, version) = real_plugin();
        let root = tempfile::tempdir().expect("temporary directory");
        let release = root.path().join("target/wasm32-unknown-unknown/release");
        std::fs::create_dir_all(&release).expect("release directory");
        std::fs::write(
            release.join(format!("rd_plugin_{}.wasm", name.replace('-', "_"))),
            component,
        )
        .expect("component");
        Self {
            root,
            name,
            version,
        }
    }

    fn plugin(&self) -> PathBuf {
        workspace_root().join("plugins").join(&self.name)
    }

    fn packages(&self) -> PathBuf {
        self.root.path().join("dist/plugins")
    }

    /// Signs nothing: writes `<name>-<version>.rdplug` with the plugin's real manifest and
    /// locales and the given component, then lets `edit` change the staged members first.
    fn package(&self, version: &str, component: &[u8], edit: impl FnOnce(&Path)) {
        let staging = self.root.path().join(format!("staging-{version}"));
        std::fs::create_dir_all(staging.join("locales")).expect("staging directory");
        std::fs::copy(
            self.plugin().join("manifest.toml"),
            staging.join("manifest.toml"),
        )
        .expect("manifest");
        std::fs::write(staging.join("component.wasm"), component).expect("component");
        std::fs::write(staging.join("signature.ed25519"), b"not a signature").expect("signature");
        for entry in std::fs::read_dir(self.plugin().join("locales")).expect("locales") {
            let entry = entry.expect("locale entry");
            std::fs::copy(
                entry.path(),
                staging.join("locales").join(entry.file_name()),
            )
            .expect("locale");
        }
        edit(&staging);

        std::fs::create_dir_all(self.packages()).expect("packages directory");
        let package = self
            .packages()
            .join(format!("{}-{version}.rdplug", self.name));
        let status = Command::new("zip")
            .current_dir(&staging)
            .arg("-q")
            .arg("-r")
            .arg(&package)
            .args([
                "manifest.toml",
                "component.wasm",
                "signature.ed25519",
                "locales",
            ])
            .status()
            .expect("run zip");
        assert!(status.success(), "zip failed: {status:?}");
    }

    /// Runs the query for this plugin and returns its standard output.
    fn unbumped(&self) -> String {
        let output = Command::new("bash")
            .arg(workspace_root().join("scripts/build-plugins.sh"))
            .args(["--list-unbumped", &self.name])
            .env("CARGO_TARGET_DIR", self.root.path().join("target"))
            .env("RD_PLUGIN_PACKAGES", self.packages())
            .output()
            .expect("run build-plugins.sh");
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).expect("UTF-8 output")
    }

    fn named(&self, member: &str) -> String {
        format!("{} {} {member} ", self.name, self.version)
    }
}

const BUILT: &[u8] = b"\0asm component as built";

#[test]
fn the_same_content_at_the_same_version_is_a_plain_rebuild() {
    let tree = Tree::new(BUILT);
    tree.package(&tree.version, BUILT, |_| {});
    assert_eq!(tree.unbumped(), "");
}

#[test]
fn a_changed_component_under_a_signed_version_is_named() {
    let tree = Tree::new(BUILT);
    tree.package(&tree.version, b"\0asm component as signed", |_| {});
    let output = tree.unbumped();
    assert!(
        output.starts_with(&tree.named("component.wasm")),
        "expected {} in {output:?}",
        tree.named("component.wasm")
    );
    assert!(
        output.trim_end().ends_with(".rdplug"),
        "the line names the package: {output:?}"
    );
}

#[test]
fn a_changed_manifest_under_a_signed_version_is_named() {
    let tree = Tree::new(BUILT);
    tree.package(&tree.version, BUILT, |staging| {
        let path = staging.join("manifest.toml");
        let mut manifest = std::fs::read_to_string(&path).expect("staged manifest");
        manifest.push_str("# signed before the last edit\n");
        std::fs::write(path, manifest).expect("edited manifest");
    });
    let output = tree.unbumped();
    assert!(
        output.starts_with(&tree.named("manifest.toml")),
        "{output:?}"
    );
}

#[test]
fn a_changed_or_missing_locale_under_a_signed_version_is_named() {
    let tree = Tree::new(BUILT);
    tree.package(&tree.version, BUILT, |staging| {
        std::fs::write(staging.join("locales/en.json"), b"{}\n").expect("edited locale");
    });
    assert!(
        tree.unbumped().starts_with(&tree.named("locales")),
        "an edited catalogue"
    );

    let tree = Tree::new(BUILT);
    tree.package(&tree.version, BUILT, |staging| {
        std::fs::remove_file(staging.join("locales/en.json")).expect("dropped locale");
    });
    assert!(
        tree.unbumped().starts_with(&tree.named("locales")),
        "a catalogue the signed package does not carry"
    );
}

#[test]
fn a_raised_version_has_nothing_to_compare_against() {
    let tree = Tree::new(BUILT);
    tree.package("0.0.0", b"\0asm an older release", |_| {});
    assert_eq!(tree.unbumped(), "");
}

#[test]
fn an_absent_signed_set_is_not_an_error() {
    let tree = Tree::new(BUILT);
    assert!(!tree.packages().exists());
    assert_eq!(tree.unbumped(), "");
}

#[test]
fn a_plugin_without_a_built_component_is_left_to_list_missing() {
    let tree = Tree::new(BUILT);
    tree.package(&tree.version, b"\0asm component as signed", |_| {});
    std::fs::remove_dir_all(tree.root.path().join("target")).expect("drop the target");
    assert_eq!(tree.unbumped(), "");
}
