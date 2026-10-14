//! The start's sweep over fixture version directories laid out as installs leave them.

use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};

use crate::{
    MANIFEST_VERSION, PluginInstaller, PluginVerifier, SUPPORTED_API_VERSIONS, VersionChoice,
    package_digest, public_key_base64, tests::fixture_manifest,
};

const ID: &str = "019d0000-0000-7000-8000-00000000abcd";
const COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";

fn signing() -> SigningKey {
    SigningKey::from_bytes(&[21_u8; 32])
}

fn installer(root: &Path) -> PluginInstaller {
    let verifier = PluginVerifier::new(false);
    verifier
        .trust_key("fixture-v1".to_owned(), signing().verifying_key())
        .expect("trust key");
    PluginInstaller::new(root.to_owned(), verifier)
}

fn current_api() -> String {
    format!("api_version = \"{}\"", SUPPORTED_API_VERSIONS[0])
}

/// A manifest as this build writes it.
fn current(manifest: String) -> String {
    manifest
}

/// A manifest built against a contract this build no longer links.
fn old_contract(manifest: String) -> String {
    manifest.replace(&current_api(), r#"api_version = "0.9.0""#)
}

/// Writes `<root>/<id>/<version>` the way an install leaves it, signed over the edited manifest.
fn install(root: &Path, version: &str, edit: fn(String) -> String) -> PathBuf {
    let signing = signing();
    let manifest = edit(
        fixture_manifest(&public_key_base64(&signing))
            .replace("version = \"1.2.3\"", &format!("version = \"{version}\"")),
    )
    .into_bytes();
    let signature = STANDARD.encode(
        signing
            .sign(&package_digest(&manifest, COMPONENT, &[]))
            .to_bytes(),
    );
    let directory = root.join(ID).join(version);
    std::fs::create_dir_all(&directory).expect("version dir");
    std::fs::write(directory.join("manifest.toml"), manifest).expect("manifest");
    std::fs::write(directory.join("component.wasm"), COMPONENT).expect("component");
    std::fs::write(directory.join("signature.ed25519"), signature).expect("signature");
    directory
}

#[tokio::test]
async fn old_versions_beside_a_loadable_one_are_removed_without_a_warning() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let old = install(root, "1.0.0", old_contract);
    let older_revision = install(root, "0.5.0", |manifest| {
        manifest.replace(
            &format!("manifest_version = {MANIFEST_VERSION}"),
            &format!("manifest_version = {}", MANIFEST_VERSION - 1),
        )
    });
    let running = install(root, "2.0.0", current);
    let installer = installer(root);

    let sweep = installer.sweep_unloadable_versions().await;
    let mut removed = sweep.removed.clone();
    removed.sort();
    assert_eq!(
        removed,
        [
            (ID.to_owned(), "0.5.0".to_owned()),
            (ID.to_owned(), "1.0.0".to_owned())
        ]
    );
    assert!(sweep.warned.is_empty(), "{sweep:?}");
    assert!(!old.exists() && !older_revision.exists());
    assert!(running.is_dir());
    assert!(
        installer
            .list_incompatible()
            .await
            .expect("incompatible")
            .is_empty()
    );
    assert_eq!(installer.load_verified().await.expect("load").len(), 1);
    // The next start finds nothing left to do.
    let again = installer.sweep_unloadable_versions().await;
    assert!(
        again.removed.is_empty() && again.warned.is_empty(),
        "{again:?}"
    );
}

#[tokio::test]
async fn a_version_refused_for_another_reason_stays_with_one_warning_for_its_plugin() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let tampered = install(root, "1.0.0", current);
    std::fs::write(tampered.join("component.wasm"), b"\0asm\x0d\0\x01\0\0").expect("tamper");
    // A newer contract waits for a newer build; it is no version this one outgrew.
    let newer = install(root, "3.0.0", |manifest| {
        manifest.replace(&current_api(), r#"api_version = "99.0.0""#)
    });
    let running = install(root, "2.0.0", current);
    let installer = installer(root);

    let sweep = installer.sweep_unloadable_versions().await;
    assert!(sweep.removed.is_empty(), "{sweep:?}");
    assert_eq!(sweep.warned.len(), 1, "{sweep:?}");
    assert!(sweep.warned[0].loadable);
    assert_eq!(sweep.warned[0].name, "Fixture");
    assert_eq!(sweep.warned[0].versions, ["1.0.0", "3.0.0"]);
    assert!(tampered.is_dir() && newer.is_dir() && running.is_dir());
    // Reported now, so every later pass over them logs at debug level.
    assert!(!installer.reported.first(&tampered));
    assert!(!installer.reported.first(&newer));
    assert!(installer.reported.first(&running));
}

#[tokio::test]
async fn a_plugin_without_a_loadable_version_keeps_its_versions_and_one_warning() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let first = install(root, "1.0.0", old_contract);
    let second = install(root, "1.1.0", old_contract);
    let installer = installer(root);

    let sweep = installer.sweep_unloadable_versions().await;
    assert!(sweep.removed.is_empty(), "{sweep:?}");
    assert_eq!(sweep.warned.len(), 1, "{sweep:?}");
    assert!(!sweep.warned[0].loadable);
    assert_eq!(sweep.warned[0].versions, ["1.0.0", "1.1.0"]);
    assert!(first.is_dir() && second.is_dir());
}

#[tokio::test]
async fn the_chosen_active_and_staged_versions_are_never_removed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let active = install(root, "1.0.0", old_contract);
    let staged = install(root, "1.5.0", old_contract);
    let neither = install(root, "2.0.0", old_contract);
    let running = install(root, "3.0.0", current);
    let installer = installer(root);
    installer.set_version_choices(
        [(
            ID.to_owned(),
            VersionChoice {
                active: Some("1.0.0".to_owned()),
                staged: Some("1.5.0".to_owned()),
            },
        )]
        .into_iter()
        .collect(),
    );

    let sweep = installer.sweep_unloadable_versions().await;
    assert_eq!(sweep.removed, [(ID.to_owned(), "2.0.0".to_owned())]);
    assert!(!neither.exists());
    assert!(active.is_dir() && staged.is_dir() && running.is_dir());
    assert_eq!(sweep.warned.len(), 1, "{sweep:?}");
    assert!(sweep.warned[0].loadable);
    assert_eq!(sweep.warned[0].versions, ["1.0.0", "1.5.0"]);
}
