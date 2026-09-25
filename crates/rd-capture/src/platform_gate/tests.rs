//! What the gate refuses, tried rather than assumed.

use super::*;

#[test]
fn every_dependency_the_linux_build_reads_carries_a_written_reason() {
    let complaints = unaccounted_dependencies(&manifest_text());
    assert!(
        complaints.is_empty(),
        "the Linux build reads dependencies nobody has accounted for:\n{}",
        complaints.join("\n")
    );
}

#[test]
fn the_written_reasons_name_dependencies_that_are_really_there() {
    let manifest: toml::Value = toml::from_str(&manifest_text()).expect("the manifest is TOML");
    let mut declared: Vec<String> = Vec::new();
    for (kind, _) in KINDS {
        declared.extend(dependency_names(manifest.get(kind)));
    }
    let targets = manifest.get("target").and_then(toml::Value::as_table);
    for (_, entry) in targets.into_iter().flatten() {
        for (kind, _) in KINDS {
            declared.extend(dependency_names(entry.get(kind)));
        }
    }
    for (name, _) in LINUX_LINKED.iter().chain(LINUX_TOOLING) {
        assert!(
            declared.iter().any(|entry| entry == name),
            "{name} carries a reason in platform_gate/mod.rs but is no longer a dependency; a \
             list that names crates the manifest has dropped is a list nobody trusts"
        );
    }
}

#[test]
fn the_window_stack_stays_behind_a_gate_no_linux_build_passes() {
    let manifest: toml::Value = toml::from_str(&manifest_text()).expect("the manifest is TOML");
    let targets = manifest.get("target").and_then(toml::Value::as_table);
    let mut gated: Vec<String> = Vec::new();
    for (key, entry) in targets.into_iter().flatten() {
        if reaches_linux(key) {
            continue;
        }
        for (kind, _) in KINDS {
            gated.extend(dependency_names(entry.get(kind)));
        }
    }
    for crate_name in WINDOW_STACK {
        assert!(
            gated.iter().any(|name| name == crate_name),
            "{crate_name} is no longer declared behind a gate that excludes Linux"
        );
    }
}

#[test]
fn a_gated_dependency_written_as_an_inline_table_is_caught() {
    let manifest = "[dependencies]\ntao = { workspace = true }\n";
    assert_eq!(
        unaccounted_dependencies(manifest),
        vec![
            "tao in [dependencies] reaches a Linux build; it pulls a window stack behind it \
             and belongs behind the Windows/macOS gate"
                .to_string()
        ]
    );
}

#[test]
fn a_gated_dependency_written_with_a_version_is_caught() {
    let manifest = "[dependencies]\ntray-icon = \"0.21\"\n";
    assert_eq!(
        unaccounted_dependencies(manifest),
        vec![
            "tray-icon in [dependencies] reaches a Linux build; it pulls a window stack \
             behind it and belongs behind the Windows/macOS gate"
                .to_string()
        ]
    );
}

#[test]
fn a_second_target_table_is_read_rather_than_counted_as_shared_or_skipped() {
    // Three tables after the known gate: one that excludes Linux and is none of this
    // check's business, and two that do not.
    let manifest = "\
[target.'cfg(windows)'.dependencies]\n\
windows-only = \"1\"\n\
[target.'cfg(any(windows, target_os = \"macos\"))'.dependencies]\n\
tao = { workspace = true }\n\
[target.'cfg(unix)'.dependencies]\n\
open = { workspace = true }\n\
[target.'cfg(not(windows))'.dependencies]\n\
something-new = \"1\"\n";
    let complaints = unaccounted_dependencies(manifest);
    assert!(
        complaints.iter().any(|complaint| complaint
            == "open in [target.'cfg(unix)'.dependencies] reaches a Linux build; it pulls a \
                window stack behind it and belongs behind the Windows/macOS gate"),
        "{complaints:?}"
    );
    assert!(
        complaints.iter().any(|complaint| complaint.starts_with(
            "something-new in [target.'cfg(not(windows))'.dependencies] reaches a Linux build"
        )),
        "{complaints:?}"
    );
    assert!(
        !complaints
            .iter()
            .any(|complaint| complaint.starts_with("windows-only")),
        "a table that excludes Linux is none of this check's business: {complaints:?}"
    );
    assert!(
        !complaints.iter().any(|complaint| complaint
            .starts_with("tao in [target.'cfg(any(windows, target_os = \"macos\"))'")),
        "the real gate is not a complaint: {complaints:?}"
    );
}

#[test]
fn a_build_or_dev_dependency_is_read_as_well() {
    let manifest = "[build-dependencies]\ntao = { workspace = true }\n\
                    [dev-dependencies]\nsomething-new = \"1\"\n";
    let complaints = unaccounted_dependencies(manifest);
    assert!(
        complaints
            .iter()
            .any(|complaint| complaint.starts_with("tao in [build-dependencies]")),
        "{complaints:?}"
    );
    assert!(
        complaints
            .iter()
            .any(|complaint| complaint.starts_with("something-new in [dev-dependencies]")),
        "{complaints:?}"
    );
}

#[test]
fn a_cfg_is_decided_rather_than_matched_as_a_string() {
    assert!(!reaches_linux("cfg(any(windows, target_os = \"macos\"))"));
    assert!(!reaches_linux("cfg(windows)"));
    assert!(!reaches_linux("cfg(not(unix))"));
    assert!(!reaches_linux("cfg(target_family = \"windows\")"));
    assert!(!reaches_linux("cfg(all(unix, not(target_os = \"linux\")))"));
    assert!(!reaches_linux("x86_64-pc-windows-msvc"));
    assert!(reaches_linux("cfg(unix)"));
    assert!(reaches_linux("cfg(not(windows))"));
    assert!(reaches_linux("cfg(target_os = \"linux\")"));
    assert!(reaches_linux("cfg(any(windows, unix))"));
    assert!(reaches_linux("x86_64-unknown-linux-gnu"));
}

#[test]
fn a_cfg_this_cannot_decide_counts_as_reaching_linux() {
    // A feature is not a target: whoever writes one here has to say why the crate under it
    // is harmless, rather than getting a free pass from a check that shrugged.
    assert!(reaches_linux("cfg(feature = \"desktop\")"));
    assert!(reaches_linux("cfg(not(feature = \"headless\"))"));
    assert!(reaches_linux("cfg(some_future_key = \"value\")"));
    assert!(reaches_linux("cfg(all(windows"));
    assert!(reaches_linux("cfg(windows) and more"));
}

#[test]
fn everything_that_draws_is_compiled_only_where_it_can_be_shown() {
    let modules = declared_modules();
    for gated in GATED_MODULES {
        let declarations: Vec<_> = modules
            .iter()
            .filter(|(_, name, _)| name == gated)
            .collect();
        assert!(
            !declarations.is_empty(),
            "no module named {gated} is declared any more; the gate guards nothing"
        );
        for (path, _, governing) in declarations {
            let cfg = cfg_of(governing).unwrap_or_else(|| {
                panic!(
                    "{gated} in {path} is declared with no cfg that keeps it off Linux; what \
                     stands in front of it is {governing:?}"
                )
            });
            assert!(
                !reaches_linux(cfg),
                "{gated} in {path} is gated on {cfg}, which a Linux build satisfies"
            );
        }
    }
    for retired in RETIRED_MODULES {
        assert!(
            !modules.iter().any(|(_, name, _)| name == retired),
            "the {retired} module came back without this gate being reconsidered"
        );
    }
}

#[test]
fn a_gated_crate_under_another_name_is_caught_too() {
    // `package = "tao"` puts the window toolkit in under a key a list of forbidden names
    // never sees. The reason list does see it, because it reads keys and asks for a reason
    // for every one of them.
    let manifest = "[dependencies]\nhelper = { package = \"tao\", version = \"0.35\" }\n";
    let complaints = unaccounted_dependencies(manifest);
    assert!(
        complaints
            .iter()
            .any(|complaint| complaint.starts_with("helper in [dependencies]")),
        "{complaints:?}"
    );
}

#[test]
fn the_clipboard_client_keeps_the_features_that_keep_it_beside_the_toolkit() {
    // arboard is the one ungated crate that touches the display server, and a feature is
    // enough to move the line it stands on: `image-data` pulls the `image` crate in behind
    // it, under a name this check reads as one it already has a reason for. The features
    // are therefore part of the fact, and they are declared in the workspace manifest.
    let workspace =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml"))
            .expect("the workspace manifest is readable");
    let parsed: toml::Value = toml::from_str(&workspace).expect("the manifest is TOML");
    let arboard = parsed
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|dependencies| dependencies.get("arboard"))
        .expect("arboard is a workspace dependency");
    assert_eq!(
        arboard
            .get("default-features")
            .and_then(toml::Value::as_bool),
        Some(false),
        "arboard runs on its default features again; image-data is one of them and it \
         pulls the image crate into the headless Linux agent"
    );
    let features: Vec<&str> = arboard
        .get("features")
        .and_then(toml::Value::as_array)
        .map(|entries| entries.iter().filter_map(toml::Value::as_str).collect())
        .unwrap_or_default();
    assert_eq!(
        features,
        ["wayland-data-control"],
        "arboard carries features nobody weighed against the headless decision"
    );
}

#[test]
fn a_declaration_is_found_wherever_it_stands() {
    assert_eq!(declared_module("mod tray;"), Some(("tray", "")));
    assert_eq!(declared_module("    pub mod tray;"), Some(("tray", "")));
    assert_eq!(declared_module("pub(crate) mod tray;"), Some(("tray", "")));
    assert_eq!(
        declared_module("#[cfg(windows)] mod tray;"),
        Some(("tray", "#[cfg(windows)] "))
    );
    assert_eq!(declared_module("// mod tray;"), None);
    assert_eq!(declared_module("mod tray {"), None);
}

#[test]
fn the_attribute_in_front_of_a_declaration_is_the_one_that_governs_it() {
    let lines = ["#[cfg(windows)]", "", "// a comment in between"];
    assert_eq!(cfg_of(&preceding_attribute(&lines)), Some("cfg(windows)"));
    assert_eq!(cfg_of(&preceding_attribute(&["use std::fs;"])), None);
    assert_eq!(
        cfg_of("#[cfg(any(windows, target_os = \"macos\"))]"),
        Some("cfg(any(windows, target_os = \"macos\"))")
    );
}

#[test]
fn what_the_manifest_refuses_the_resolved_tree_refuses_too() {
    // The two halves of the guard share a subject and nothing else: this file judges what
    // `crates/rd-capture/Cargo.toml` declares, `scripts/check-capture-linux-tree.sh` judges
    // what `cargo tree` resolves. A crate dropping out of the second list would leave the
    // first one refusing it as a direct dependency while the same crate walked in behind
    // `rd-core`, which is the hole RD-109-37 was opened to close.
    let script = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-capture-linux-tree.sh"),
    )
    .expect("the Linux tree check is readable");
    let (_, rest) = script
        .split_once("\nFORBIDDEN=(\n")
        .expect("the Linux tree check names a FORBIDDEN list");
    let (body, _) = rest
        .split_once("\n)\n")
        .expect("the FORBIDDEN list closes on its own line");
    let listed: Vec<&str> = body
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    for crate_name in WINDOW_STACK {
        assert!(
            listed.contains(crate_name),
            "{crate_name} is refused as a direct dependency here but no longer named in \
             scripts/check-capture-linux-tree.sh, so it would reach a Linux build unremarked \
             the moment anything else pulled it in"
        );
    }
}
