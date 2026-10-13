//! RD-1280-18: the shared fields a bundled manifest leaves to `plugins/metadata.toml`, and the
//! third-party manifest that needs no such file.

use std::path::PathBuf;

use super::{COMPLETED_MARKER, ManifestDefaults, complete_manifest};
use crate::{PluginLimits, PluginManifest, manifest::validate_manifest};

const KEY: &str = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY=";

/// A key other than the release key: a real Ed25519 point, so a manifest naming it validates.
fn other_key() -> String {
    crate::public_key_base64(&ed25519_dalek::SigningKey::from_bytes(&[7; 32]))
}

const DEFAULTS: &str = r#"# The shared fields.
manifest_version = 3
api_version = "0.11.0"
key_id = "rdownloader-release-v1"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="

[metadata]
author = "DEGOYA.digital"
homepage = "https://rdownloader.net"
license = "MIT"
min_app_version = "1.9.0"
"#;

/// A bundled resolver's source as the conversion leaves it: nothing the defaults supply.
const SOURCE: &str = r#"plugin_type = "resolver"
id = "019d0000-0000-7000-8000-00000000abcd"
name = "Fixture"
version = "1.0.0"
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["fixture.example"]

[metadata]
description = "A fixture resolver."

[provider]
slug = "fixture"
kind = "hoster"
credentials = "api_key"
"#;

fn defaults() -> ManifestDefaults {
    ManifestDefaults::parse(DEFAULTS).expect("the defaults parse")
}

fn manifest(text: &str) -> PluginManifest {
    toml::from_str(text).unwrap_or_else(|error| panic!("{error}\n{text}"))
}

#[test]
fn every_missing_field_is_written_where_its_table_is() {
    let completed = complete_manifest(SOURCE, Some(&defaults()), None).expect("complete");
    let expected = format!(
        "{COMPLETED_MARKER}\nmanifest_version = 3\napi_version = \"0.11.0\"\n\
         key_id = \"rdownloader-release-v1\"\npublic_key = \"{KEY}\"\n"
    );
    assert!(completed.starts_with(&expected), "{completed}");
    assert!(
        completed.contains(
            "[metadata]\nauthor = \"DEGOYA.digital\"\nhomepage = \"https://rdownloader.net\"\n\
             license = \"MIT\"\nmin_app_version = \"1.9.0\"\ndescription = \"A fixture resolver.\"\n"
        ),
        "{completed}"
    );
    validate_manifest(&manifest(&completed)).expect("the completed manifest is valid");
}

/// Reading a source for its domains needs no completion; packaging it without one is refused,
/// so no package ever lacks a shared field.
#[test]
fn an_uncompleted_source_parses_and_is_refused_as_a_package() {
    let source = manifest(SOURCE);
    assert_eq!(source.domains(), ["fixture.example".to_owned()]);
    assert_eq!(source.manifest_version, 0);
    assert!(source.key_id.is_empty() && source.metadata.author.is_empty());
    let error = validate_manifest(&source).expect_err("an incomplete package");
    assert!(error.to_string().contains("manifest version"), "{error:#}");
}

#[test]
fn a_field_the_manifest_states_is_kept() {
    let own = SOURCE.replace(
        "description = \"A fixture resolver.\"\n",
        "description = \"A fixture resolver.\"\nmin_app_version = \"1.27.0\"\n",
    );
    let completed = complete_manifest(&own, Some(&defaults()), None).expect("complete");
    let parsed = manifest(&completed);
    assert_eq!(parsed.metadata.min_app_version.as_deref(), Some("1.27.0"));
    assert_eq!(completed.matches("min_app_version").count(), 1);
}

#[test]
fn a_complete_manifest_keeps_every_byte() {
    let complete = complete_manifest(SOURCE, Some(&defaults()), None).expect("complete");
    let again = complete_manifest(&complete, Some(&defaults()), Some(KEY)).expect("again");
    assert_eq!(again, complete);
    assert_eq!(
        complete_manifest(SOURCE, None, None).expect("no defaults"),
        SOURCE
    );
}

#[test]
fn the_signing_key_supplies_the_public_half_and_never_the_name() {
    let same = complete_manifest(SOURCE, Some(&defaults()), Some(KEY)).expect("same key");
    assert_eq!(
        same,
        complete_manifest(SOURCE, Some(&defaults()), None).expect("defaults alone")
    );
    let other_key = other_key();
    let other = complete_manifest(SOURCE, Some(&defaults()), Some(&other_key)).expect("other");
    assert!(other.contains(&format!("public_key = \"{other_key}\"\n")));
    assert!(!other.contains(KEY));
    let keyed_only = complete_manifest(SOURCE, None, Some(&other_key)).expect("key alone");
    assert!(keyed_only.starts_with(&format!(
        "{COMPLETED_MARKER}\npublic_key = \"{other_key}\"\n"
    )));
    assert_eq!(manifest(&keyed_only).key_id, "");
}

#[test]
fn a_manifest_without_metadata_gets_the_table() {
    let bare = SOURCE.replace("[metadata]\ndescription = \"A fixture resolver.\"\n\n", "");
    let completed = complete_manifest(&bare, Some(&defaults()), None).expect("complete");
    assert!(completed.ends_with("\n[metadata]\nauthor = \"DEGOYA.digital\"\nhomepage = \"https://rdownloader.net\"\nlicense = \"MIT\"\nmin_app_version = \"1.9.0\"\n"));
    let table: toml::Table = toml::from_str(&completed).expect("TOML");
    assert_eq!(table["metadata"]["author"].as_str(), Some("DEGOYA.digital"));
}

#[test]
fn the_defaults_hold_shared_fields_only() {
    for (text, refusal) in [
        (
            "version = \"1.0.0\"\n",
            "`version` is not a field plugins share",
        ),
        (
            "[metadata]\ndescription = \"x\"\n",
            "`description` is not a field plugins share in [metadata]",
        ),
        (
            "[limits]\nfuel = 1\n",
            "top-level fields and [metadata] only",
        ),
        ("key_id = \n", "not TOML"),
    ] {
        let error = ManifestDefaults::parse(text).expect_err(text);
        assert!(format!("{error:#}").contains(refusal), "{text}: {error:#}");
    }
}

fn workspace_root() -> PathBuf {
    let crate_dir = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR at run time");
    PathBuf::from(crate_dir).join("../..")
}

/// Every bundled manifest becomes a valid package manifest with `plugins/metadata.toml`.
#[test]
fn every_bundled_manifest_completes_to_a_valid_one() {
    let plugins = workspace_root().join("plugins");
    let defaults = ManifestDefaults::read(&plugins.join("metadata.toml")).expect("defaults");
    let mut checked = 0;
    for entry in std::fs::read_dir(&plugins).expect("plugins") {
        let path = entry.expect("entry").path().join("manifest.toml");
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let completed = complete_manifest(&source, Some(&defaults), None)
            .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        validate_manifest(&manifest(&completed))
            .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        checked += 1;
    }
    assert!(
        checked > 50,
        "only {checked} manifests under {}",
        plugins.display()
    );
}

/// A third party has no `plugins/metadata.toml`: a scaffolded manifest is complete and valid
/// on its own, and packaging it changes none of its bytes (RD-1280-18).
#[test]
fn a_third_party_manifest_is_complete_without_the_shared_file() {
    let templates = workspace_root().join("sdk/templates");
    let other_key = other_key();
    let mut checked = 0;
    for entry in std::fs::read_dir(&templates).expect("templates") {
        let path = entry.expect("entry").path().join("manifest.toml");
        let Ok(template) = std::fs::read_to_string(&path) else {
            continue;
        };
        let scaffolded = template
            .replace("{{PLUGIN_NAME}}", "Demo")
            .replace("{{PLUGIN_SLUG}}", "demo")
            .replace("{{PLUGIN_ID}}", "019d0000-0000-7000-8000-00000000abcd")
            .replace("REPLACE_WITH_YOUR_BASE64_ED25519_PUBLIC_KEY", &other_key);
        let packaged = complete_manifest(&scaffolded, None, Some(&other_key))
            .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        assert_eq!(packaged, scaffolded, "{}", path.display());
        validate_manifest(&manifest(&packaged))
            .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        checked += 1;
    }
    assert!(checked >= 13, "only {checked} templates");
}

/// RD-1280-18: `[limits]` names what it changes; a field it leaves out, or the whole table, is
/// the host's default -- the values the 75 manifests used to repeat.
#[test]
fn a_limit_the_manifest_leaves_out_is_the_default() {
    assert_eq!(
        PluginLimits::default(),
        PluginLimits {
            memory_bytes: 64 * 1024 * 1024,
            fuel: 2_000_000_000,
            timeout_milliseconds: 15_000,
            max_response_bytes: 8 * 1024 * 1024,
            wait_budget_milliseconds: 600_000,
        }
    );
    assert_eq!(manifest(SOURCE).limits, PluginLimits::default());
    let one = manifest(&format!("{SOURCE}\n[limits]\nfuel = 500000000\n"));
    assert_eq!(
        one.limits,
        PluginLimits {
            fuel: 500_000_000,
            ..PluginLimits::default()
        }
    );
    let empty = manifest(&format!("{SOURCE}\n[limits]\n"));
    assert_eq!(empty.limits, PluginLimits::default());
    assert_eq!(crate::MAX_FUEL, 20 * crate::DEFAULT_FUEL);
}
