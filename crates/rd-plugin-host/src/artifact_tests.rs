//! Tests of `artifact`: the content rule, the source set, and parity with the script.

use super::*;

/// A component path that need not exist: `staleness` only prints it.
fn artefact() -> PathBuf {
    artifact_path("rd-plugin-example-oauth")
}

/// A workspace of its own, shaped like the real one where it matters: a plugin that
/// depends on a shared plugin library, which depends on a second one, and a core crate
/// and the WIT contract beside them.
fn workspace() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temporary workspace");
    let write = |relative: &str, text: &str| {
        let path = root.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        std::fs::write(path, text).expect("file");
    };
    write(
        "plugins/sample/Cargo.toml",
        "[dependencies]\n\
         common = { path = \"../common\" }\n\
         rd-core = { path = \"../../crates/rd-core\" }\n\
         [package.metadata.component.target]\n\
         path = \"../../crates/rd-plugin-api/wit\"\n",
    );
    write("plugins/sample/manifest.toml", "version = \"1.0.0\"\n");
    write("plugins/sample/src/lib.rs", "pub fn resolve() {}\n");
    write("plugins/sample/locales/en.json", "{}\n");
    write(
        "plugins/common/Cargo.toml",
        "[dependencies]\nguest = { path = \"../guest\" }\n",
    );
    write("plugins/common/src/lib.rs", "pub fn shared() {}\n");
    write("plugins/guest/Cargo.toml", "[package]\n");
    write("plugins/guest/src/lib.rs", "pub fn guest() {}\n");
    write("crates/rd-core/Cargo.toml", "[package]\n");
    write("crates/rd-core/src/lib.rs", "pub fn core() {}\n");
    write(
        "crates/rd-plugin-api/wit/rdownloader.wit",
        "package rdownloader:plugin;\n",
    );
    root
}

fn hash(root: &tempfile::TempDir) -> String {
    source_hash(root.path(), "rd-plugin-sample")
}

fn change(root: &tempfile::TempDir, relative: &str) {
    let path = root.path().join(relative);
    let mut text = std::fs::read_to_string(&path).expect("readable");
    text.push_str("// changed\n");
    std::fs::write(path, text).expect("writable");
}

/// A stamp that vouches for `bytes` built from sources hashing to `sources`.
fn stamp_for(sources: &str, bytes: &[u8]) -> String {
    format!("{sources} {}\n", hex_sha256(bytes))
}

#[test]
fn a_package_name_maps_to_its_directory_its_artefact_and_its_stamp() {
    assert!(
        plugin_directory(Path::new("/w"), "rd-plugin-google-drive-oauth")
            .ends_with("plugins/google-drive-oauth")
    );
    let artefact = artifact_path("rd-plugin-google-drive-oauth");
    assert!(artefact.ends_with("wasm32-unknown-unknown/release/rd_plugin_google_drive_oauth.wasm"));
    assert!(
        stamp_path(&artefact).ends_with("release/rd_plugin_google_drive_oauth.wasm.src-sha256")
    );
}

#[test]
fn the_sources_are_the_crate_its_plugin_libraries_and_the_contract_but_no_core_crate() {
    let root = workspace();
    let base = root.path().canonicalize().expect("root");
    let mut found: Vec<String> = sources(&base, "rd-plugin-sample")
        .iter()
        .map(|path| {
            path.strip_prefix(&base)
                .expect("inside")
                .display()
                .to_string()
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            "crates/rd-plugin-api/wit/rdownloader.wit",
            "plugins/common/Cargo.toml",
            "plugins/common/src/lib.rs",
            "plugins/guest/Cargo.toml",
            "plugins/guest/src/lib.rs",
            "plugins/sample/Cargo.toml",
            "plugins/sample/manifest.toml",
            "plugins/sample/src/lib.rs",
        ]
    );
}

#[test]
fn the_real_tree_follows_a_shared_plugin_library_and_stops_at_crates() {
    // `plugins/ddownload` depends on `plugins/common` and on `crates/rd-core`. The first
    // is followed, the second is where the trail deliberately stops.
    let root = workspace_root();
    let sources = sources(&root, "rd-plugin-ddownload");
    assert!(
        sources
            .iter()
            .any(|path| path.ends_with("plugins/common/src/lib.rs")),
        "the shared plugin library is missing from {sources:?}"
    );
    assert!(
        sources
            .iter()
            .any(|path| path.ends_with("rd-plugin-api/wit/rdownloader.wit")),
        "the WIT contract is missing from {sources:?}"
    );
    let core = root.join("crates/rd-core").canonicalize();
    if let Ok(core) = core {
        assert!(
            !sources.iter().any(|path| path.starts_with(&core)),
            "the trail should not have left plugins/"
        );
    }
}

#[test]
fn the_source_hash_ignores_file_times_and_the_checkout_location() {
    let first = workspace();
    let second = workspace();
    let before = hash(&first);
    // What a checkout does to every file: new times, same content.
    let lib = first.path().join("plugins/sample/src/lib.rs");
    let text = std::fs::read(&lib).expect("readable");
    std::fs::write(&lib, text).expect("rewritten");

    assert_eq!(hash(&first), before);
    assert_eq!(hash(&second), before, "another location, the same sources");
}

#[test]
fn a_change_to_the_plugin_itself_changes_the_source_hash() {
    let root = workspace();
    let before = hash(&root);
    change(&root, "plugins/sample/src/lib.rs");
    assert_ne!(hash(&root), before);
}

#[test]
fn a_change_to_a_shared_plugin_library_changes_the_source_hash() {
    let root = workspace();
    let before = hash(&root);
    change(&root, "plugins/guest/src/lib.rs");
    assert_ne!(hash(&root), before, "a library two levels down counts too");
}

#[test]
fn a_change_to_the_contract_changes_the_source_hash() {
    let root = workspace();
    let before = hash(&root);
    change(&root, "crates/rd-plugin-api/wit/rdownloader.wit");
    assert_ne!(hash(&root), before);
}

#[test]
fn the_bindings_cargo_component_generates_are_not_a_source() {
    // A checkout that built the plugin has `src/bindings.rs`, a fresh one does not; both must
    // agree with the same stamp. A module that merely has the same name deeper down counts.
    let root = workspace();
    let before = hash(&root);
    let generated = root.path().join("plugins/sample/src/bindings.rs");
    std::fs::write(&generated, "// generated\n").expect("bindings");
    assert_eq!(hash(&root), before);

    let nested = root.path().join("plugins/sample/src/guest/bindings.rs");
    std::fs::create_dir_all(nested.parent().expect("parent")).expect("directory");
    std::fs::write(&nested, "pub fn real() {}\n").expect("module");
    assert_ne!(hash(&root), before);
}

#[test]
fn a_change_outside_the_source_set_does_not() {
    let root = workspace();
    let before = hash(&root);
    change(&root, "crates/rd-core/src/lib.rs");
    change(&root, "plugins/sample/locales/en.json");
    assert_eq!(hash(&root), before);
}

#[test]
fn a_stamp_for_these_bytes_and_these_sources_is_current() {
    let bytes = b"\0asm component";
    let stamp = stamp_for("abc", bytes);
    assert_eq!(
        staleness(
            "rd-plugin-example-oauth",
            &artefact(),
            bytes,
            Some(&stamp),
            "abc"
        ),
        None
    );
}

#[test]
fn a_component_without_a_stamp_is_stale() {
    let complaint =
        staleness("rd-plugin-example-oauth", &artefact(), b"x", None, "abc").expect("stale");
    assert!(complaint.contains("carries no source stamp"));
    assert!(complaint.contains("scripts/build-plugins.sh --components-only example-oauth"));
}

#[test]
fn a_component_rebuilt_without_a_stamp_is_stale() {
    // The stamp was written for another build of the same file name: a bare
    // `cargo component build` since, here or in another checkout.
    let stamp = stamp_for("abc", b"the bytes the stamp saw");
    let complaint = staleness(
        "rd-plugin-example-oauth",
        &artefact(),
        b"other bytes",
        Some(&stamp),
        "abc",
    )
    .expect("stale");
    assert!(complaint.contains("describes other bytes"));
}

#[test]
fn a_component_built_from_other_sources_is_stale() {
    let bytes = b"component";
    let stamp = stamp_for("0123456789abcdef", bytes);
    let complaint = staleness(
        "rd-plugin-example-oauth",
        &artefact(),
        bytes,
        Some(&stamp),
        "fedcba9876543210",
    )
    .expect("stale");
    assert!(complaint.contains("sources changed since it was built"));
    assert!(complaint.contains("0123456789ab"));
    assert!(complaint.contains("fedcba987654"));
}

#[test]
fn a_garbled_stamp_is_stale_and_does_not_panic() {
    for stamp in [
        "",
        "only-one-field",
        "\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9} x",
    ] {
        assert!(
            staleness(
                "rd-plugin-example-oauth",
                &artefact(),
                b"x",
                Some(stamp),
                "abc"
            )
            .is_some(),
            "{stamp:?}"
        );
    }
}

#[test]
fn a_component_that_was_never_built_names_the_build_command() {
    // The case that used to be a silent `return`: the run has to say this, because the
    // alternative is a green for guest code nobody loaded.
    let complaint = missing("rd-plugin-example-oauth", &artefact());

    assert!(complaint.contains("has not been built in this checkout"));
    assert!(complaint.contains("scripts/build-plugins.sh --components-only example-oauth"));
    assert!(complaint.contains("rd_plugin_example_oauth.wasm"));
    assert!(complaint.contains("-P no-components"));
}

/// The script and this module must agree on the definition, or `check.sh` and the tests
/// would disagree about the same component. Linux only: it runs the script, which needs
/// bash and coreutils.
#[cfg(target_os = "linux")]
#[test]
fn the_script_computes_the_same_source_hash() {
    let root = workspace_root();
    for package in ["rd-plugin-example-oauth", "rd-plugin-ddownload"] {
        let output = std::process::Command::new("bash")
            .arg(root.join("scripts/build-plugins.sh"))
            .args(["--source-hash", plugin_name(package)])
            .current_dir(&root)
            .output()
            .expect("the script runs");
        assert!(output.status.success(), "{output:?}");
        let script = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        assert_eq!(script, source_hash(&root, package), "{package}");
    }
}
