use clap::ValueEnum;

use super::{NewPluginArgs, NewPluginType};

/// Every `--type` value has a template, and the template declares that very type.
///
/// The guard that makes the enum cheap to extend: adding a world is one variant, and if
/// its template is missing or names another `plugin_type`, this fails here instead of the
/// first time somebody runs `plugin new` — which is what happened to `oauth`, offered by
/// the core since RD-103-00 and scaffoldable by nobody.
#[test]
fn every_scaffoldable_type_has_a_template_that_declares_it() {
    let templates = super::sdk_templates().expect("sdk templates");
    for variant in NewPluginType::value_variants() {
        let name = variant.name();
        let directory = templates.join(&name);
        assert!(
            directory.is_dir(),
            "`plugin new --type {name}` has no template at {}",
            directory.display()
        );
        let manifest =
            std::fs::read_to_string(directory.join("manifest.toml")).expect("template manifest");
        let plugin_type = variant.manifest_type();
        assert!(
            manifest.contains(&format!("plugin_type = \"{plugin_type}\"")),
            "{name} template declares another plugin_type"
        );
        let guest =
            std::fs::read_to_string(directory.join("src/guest.rs")).expect("template src/guest.rs");
        assert!(
            guest.contains(&format!("world: \"{name}-plugin\",")),
            "{name} template builds another world"
        );
        assert!(
            directory.join("wit/rdownloader.wit").is_file(),
            "{name} template ships no copy of the contract"
        );
    }
}

/// The contract copy every template carries is the one this core speaks.
///
/// CI checks the same thing with `diff -u`; having it here too means a template that
/// drifted is caught by a plain `cargo nextest run -p rd-pack`.
#[test]
fn every_template_carries_the_current_contract() {
    let contract = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../rd-plugin-api/wit/rdownloader.wit"),
    )
    .expect("the contract");
    let templates = super::sdk_templates().expect("sdk templates");
    for entry in std::fs::read_dir(&templates).expect("read templates") {
        let path = entry.expect("entry").path().join("wit/rdownloader.wit");
        if !path.is_file() {
            continue;
        }
        let copy = std::fs::read_to_string(&path).expect("template contract");
        assert_eq!(copy, contract, "{} is out of date", path.display());
    }
}

/// A fresh scaffold ignores its private key, its build output and its packages before the
/// key exists (audit K1): `git add .` in a new plugin repository committed the key.
#[test]
fn a_scaffold_ignores_its_signing_key() {
    let root = std::env::temp_dir().join(format!("rd-pack-scaffold-{}", uuid::Uuid::now_v7()));
    let out = root.join("probe");
    super::scaffold(&NewPluginArgs {
        plugin_type: NewPluginType::Resolver,
        out: out.clone(),
        name: None,
    })
    .expect("scaffold");
    let ignore = std::fs::read_to_string(out.join(".gitignore")).expect("a .gitignore");
    let lines: Vec<&str> = ignore.lines().collect();
    assert!(
        out.join("plugin-signing.key").is_file(),
        "the scaffold's key"
    );
    for entry in ["plugin-signing.key", "target/", "*.rdplug"] {
        assert!(lines.contains(&entry), "{entry} is not ignored: {ignore:?}");
    }
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// A template's own `.gitignore` is kept, and an entry it already has is not repeated.
#[test]
fn the_scaffold_gitignore_keeps_what_the_template_wrote() {
    let root = std::env::temp_dir().join(format!("rd-pack-gitignore-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&root).expect("directory");
    std::fs::write(root.join(".gitignore"), "/notes\ntarget/").expect("template ignore");
    super::write_gitignore(&root).expect("write");
    let ignore = std::fs::read_to_string(root.join(".gitignore")).expect("read");
    assert_eq!(ignore, "/notes\ntarget/\nplugin-signing.key\n*.rdplug\n");
    std::fs::remove_dir_all(&root).expect("clean up");
}
