//! The bundled sync over versions this build refuses (RD-1251-01): an installation updated
//! across a contract change holds only packages of the old contract, and the bundle of the new
//! release is what replaces them.

use std::path::Path;

use super::{
    BundledPolicy, sync_bundled,
    tests::{EMPTY_COMPONENT, manifest},
};
use crate::{PluginInstaller, PluginVerifier, packager::package_plugin};

const ID: &str = "019d0000-0000-7000-8000-00000000bbbb";

/// Leaves `version` installed the way an earlier release did, built against a contract this
/// build no longer speaks.
fn install_refused(root: &Path, version: &str) {
    let directory = root.join(ID).join(version);
    std::fs::create_dir_all(&directory).expect("version dir");
    let manifest = String::from_utf8(manifest(version))
        .expect("utf-8")
        .replace(r#"api_version = "0.11.0""#, r#"api_version = "0.10.0""#);
    std::fs::write(directory.join("manifest.toml"), manifest).expect("manifest");
    std::fs::write(directory.join(crate::COMPONENT_MEMBER), EMPTY_COMPONENT).expect("component");
}

fn bundle(directory: &Path, version: &str) {
    std::fs::create_dir_all(directory).expect("bundled dir");
    std::fs::write(
        directory.join(format!("bundled-{version}.rdplug")),
        package_plugin(&manifest(version), EMPTY_COMPONENT, &[], None).expect("package"),
    )
    .expect("write");
}

#[tokio::test]
async fn a_refused_plugin_is_replaced_by_its_bundled_version_on_an_ordinary_start() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plugins = directory.path().join("plugins");
    let bundled = directory.path().join("bundled");
    install_refused(&plugins, "1.0.0");
    bundle(&bundled, "1.1.0");
    let installer = PluginInstaller::new(plugins, PluginVerifier::new(true));
    assert!(installer.list_installed().await.expect("list").is_empty());
    assert_eq!(installer.list_refused().await.expect("refused").len(), 1);

    // Every start after the first installs nothing new -- but this plugin is installed.
    let report = sync_bundled(&installer, &bundled, BundledPolicy::InstalledOnly)
        .await
        .expect("sync");
    assert_eq!(report.installed.len(), 1, "{report:?}");
    assert_eq!(report.installed[0].1, "1.1.0");
    let installed = installer.list_installed().await.expect("list");
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].version, "1.1.0");
    // The refused version is gone with its replacement in place, not left listed as refused.
    assert!(
        installer
            .list_incompatible()
            .await
            .expect("incompatible")
            .is_empty()
    );
}

#[tokio::test]
async fn a_refused_version_newer_than_the_bundle_stays() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plugins = directory.path().join("plugins");
    let bundled = directory.path().join("bundled");
    install_refused(&plugins, "2.0.0");
    bundle(&bundled, "1.1.0");
    let installer = PluginInstaller::new(plugins, PluginVerifier::new(true));
    let report = sync_bundled(&installer, &bundled, BundledPolicy::InstalledOnly)
        .await
        .expect("sync");
    assert!(report.installed.is_empty(), "{report:?}");
    assert_eq!(report.skipped, 1);
    assert_eq!(
        installer
            .list_incompatible()
            .await
            .expect("incompatible")
            .len(),
        1
    );
}

#[tokio::test]
async fn an_install_removes_only_the_older_refused_versions_of_its_own_plugin() {
    let directory = tempfile::tempdir().expect("tempdir");
    let plugins = directory.path().join("plugins");
    install_refused(&plugins, "1.0.0");
    install_refused(&plugins, "3.0.0");
    let installer = PluginInstaller::new(plugins, PluginVerifier::new(true));
    installer
        .install_bytes(
            package_plugin(&manifest("2.0.0"), EMPTY_COMPONENT, &[], None).expect("package"),
        )
        .await
        .expect("install");
    let refused: Vec<String> = installer
        .list_refused()
        .await
        .expect("refused")
        .into_iter()
        .map(|refused| refused.version)
        .collect();
    assert_eq!(refused, ["3.0.0"]);
    assert_eq!(installer.list_installed().await.expect("list").len(), 1);
}
