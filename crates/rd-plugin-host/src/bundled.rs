//! Installs `.rdplug` packages shipped next to the executable: newer versions of what is
//! installed, and on a fresh installation's first start the services that need no account.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::PluginId;

use crate::{
    BundledPackage, PluginInstaller, bundled_services::group_services, manifest::PluginType,
};

/// Which bundled packages a start installs that are not installed at all yet (RD-160-05).
///
/// A package whose plugin is installed is updated whatever the policy: an update run keeps what
/// the person chose current. What differs is a plugin that is not installed — the person did not
/// choose it, removed it, or it is new in this release — and that is offered as available rather
/// than installed behind their back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BundledPolicy {
    /// Install nothing new. Every start after the first, and the first start of an installation
    /// that already has plugins.
    InstalledOnly,
    /// The first start of a fresh installation: also install every service that needs no
    /// account or destination, so the service works before the setup wizard ran.
    FirstStart,
    /// Install every bundled package, as every start did before RD-160-05.
    All,
}

/// What `sync_bundled` did.
#[derive(Debug, Default)]
pub struct BundledSyncReport {
    pub installed: Vec<(PluginId, String)>,
    pub skipped: usize,
    /// Bundled packages whose plugin is not installed and that the policy left alone; the
    /// wizard and the plugin manager offer them.
    pub available: usize,
    pub rejected: Vec<(String, String)>,
    /// Installed versions removed because they sat under a bundled plugin's id while being a
    /// different plugin entirely — the leftovers of an id collision (RD-098-03).
    pub orphaned: Vec<(PluginId, String, String)>,
}

/// The `*.rdplug` files in `directory`, sorted; `None` when it is no folder. Blocking: async
/// code calls it through `spawn_blocking`.
fn bundled_packages(directory: &Path) -> std::io::Result<Option<Vec<PathBuf>>> {
    if !directory.is_dir() {
        return Ok(None);
    }
    let mut entries: Vec<_> = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rdplug"))
        })
        .collect();
    entries.sort();
    Ok(Some(entries))
}

/// Scans `directory` for `*.rdplug` files and installs every package whose version is newer
/// than every installed version of the same plugin id. A plugin with no installed version is
/// installed only as far as `policy` says. Older versions are never removed (the highest
/// installed version wins at load time).
///
/// Every package that verifies is remembered on `installer` as the bundle, so the plugin
/// manager can offer what was not installed.
pub async fn sync_bundled(
    installer: &PluginInstaller,
    directory: &Path,
    policy: BundledPolicy,
) -> Result<BundledSyncReport> {
    let mut report = BundledSyncReport::default();
    let listed = directory.to_path_buf();
    let Some(entries) = tokio::task::spawn_blocking(move || bundled_packages(&listed)).await??
    else {
        installer.set_bundled(Vec::new());
        return Ok(report);
    };
    let installed = installer.list_installed().await?;
    // What each bundled id *is*, so a leftover of a different type under the same id can be
    // recognised after the loop.
    let mut bundled_types: std::collections::HashMap<PluginId, PluginType> =
        std::collections::HashMap::new();
    let mut catalogue = Vec::new();
    // Packages of plugins that are not installed: whether they are wanted depends on the
    // service they belong to, which is known only once every manifest has been read.
    let mut absent: Vec<(String, PathBuf, PluginId)> = Vec::new();
    let _batch = crate::unsigned_notice::UnsignedBatch::open();
    for path in entries {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("?")
            .to_owned();
        let verifier = installer.verifier.clone();
        let candidate = path.clone();
        let package =
            match tokio::task::spawn_blocking(move || verifier.verify_file(&candidate)).await? {
                Ok(package) => package,
                Err(error) => {
                    report.rejected.push((name, error.to_string()));
                    continue;
                }
            };
        bundled_types.insert(package.manifest.id, package.manifest.plugin_type.clone());
        catalogue.push(BundledPackage::from_verified(path.clone(), &package));
        let bundled_version = semver::Version::parse(&package.manifest.version).ok();
        let newest_installed = installed
            .iter()
            .filter(|manifest| manifest.id == package.manifest.id)
            .filter_map(|manifest| semver::Version::parse(&manifest.version).ok())
            .max();
        let is_installed = installed
            .iter()
            .any(|manifest| manifest.id == package.manifest.id);
        if bundled_version.is_some() && !is_installed {
            absent.push((name, path, package.manifest.id));
            continue;
        }
        let is_newer = match (&bundled_version, &newest_installed) {
            (Some(bundled), Some(current)) => bundled > current,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if !is_newer {
            report.skipped += 1;
            continue;
        }
        match installer.install_verified(package).await {
            Ok(installed) => report
                .installed
                .push((installed.manifest.id, installed.manifest.version)),
            Err(error) => report.rejected.push((name, error.to_string())),
        }
    }
    let services = group_services(&catalogue);
    for (name, path, id) in absent {
        let wanted = match policy {
            BundledPolicy::InstalledOnly => false,
            BundledPolicy::All => true,
            BundledPolicy::FirstStart => services
                .iter()
                .find(|service| service.contains(&id))
                .is_some_and(|service| !service.needs_account),
        };
        if !wanted {
            report.available += 1;
            continue;
        }
        // Verified again rather than kept from the loop: holding every component of the bundle
        // in memory until the grouping is known costs more than a second check of the few
        // packages a first start installs.
        match installer.install(path).await {
            Ok(installed) => report
                .installed
                .push((installed.manifest.id, installed.manifest.version)),
            Err(error) => report.rejected.push((name, error.to_string())),
        }
    }
    installer.set_bundled(catalogue);
    remove_orphans(installer, &bundled_types, &mut report).await?;
    Ok(report)
}

/// Removes installed versions that sit under a bundled plugin's id but are a different plugin.
///
/// Until 0.9.8 the SponsorBlock enricher and the file-name tidier shipped with the same id, so
/// a machine that installed both ended up with two unrelated packages under one identity — and
/// because the loader keeps only the highest version per id, one of them was silently never
/// loaded. Moving SponsorBlock to its own id fixes new installations; this clears what the old
/// ones were left holding, which would otherwise linger forever and read like a plugin that had
/// been updated.
///
/// The rule is narrow on purpose: only ids a *bundled* package claims are considered, and only
/// a version whose `plugin_type` disagrees with what we ship under that id is removed. A
/// third-party plugin that collided with a bundled id would be removed too — but such a
/// collision has no good outcome, and the bundled package is the one that owns its own id.
async fn remove_orphans(
    installer: &PluginInstaller,
    bundled_types: &std::collections::HashMap<PluginId, PluginType>,
    report: &mut BundledSyncReport,
) -> Result<()> {
    for manifest in installer.list_installed().await? {
        let Some(expected) = bundled_types.get(&manifest.id) else {
            continue;
        };
        if *expected == manifest.plugin_type {
            continue;
        }
        if installer
            .remove_version(&manifest.id.to_string(), &manifest.version)
            .await?
        {
            report.orphaned.push((
                manifest.id,
                manifest.version,
                manifest.plugin_type.as_str().to_owned(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BundledPolicy, sync_bundled};
    use crate::{PluginInstaller, PluginVerifier, packager::package_plugin};

    const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";
    const DEV_PUBLIC_KEY: &str = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY=";

    fn manifest(version: &str) -> Vec<u8> {
        format!(
            r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.10.0"
id = "019d0000-0000-7000-8000-00000000bbbb"
name = "Bundled"
version = "{version}"
key_id = "dev"
public_key = "{DEV_PUBLIC_KEY}"
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "Bundled fixture"
author = "rDownloader"

[provider]
slug = "bundled"
kind = "hoster"
credentials = "api_key"
"#
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn installs_new_versions_and_skips_equal_or_older_ones() {
        let directory = tempfile::tempdir().expect("tempdir");
        let bundled = directory.path().join("bundled");
        std::fs::create_dir_all(&bundled).expect("bundled dir");
        std::fs::write(
            bundled.join("bundled-1.0.0.rdplug"),
            package_plugin(&manifest("1.0.0"), EMPTY_COMPONENT, &[], None).expect("package"),
        )
        .expect("write");
        let installer =
            PluginInstaller::new(directory.path().join("plugins"), PluginVerifier::new(true));
        let first = sync_bundled(&installer, &bundled, BundledPolicy::All)
            .await
            .expect("sync");
        assert_eq!(first.installed.len(), 1);
        let again = sync_bundled(&installer, &bundled, BundledPolicy::All)
            .await
            .expect("sync again");
        assert_eq!(again.installed.len(), 0);
        assert_eq!(again.skipped, 1);
        std::fs::write(
            bundled.join("bundled-1.1.0.rdplug"),
            package_plugin(&manifest("1.1.0"), EMPTY_COMPONENT, &[], None).expect("package"),
        )
        .expect("write");
        let upgraded = sync_bundled(&installer, &bundled, BundledPolicy::All)
            .await
            .expect("upgrade");
        assert_eq!(upgraded.installed.len(), 1);
        assert_eq!(upgraded.installed[0].1, "1.1.0");
        assert_eq!(installer.list_installed().await.expect("list").len(), 2);
    }

    /// The same id as [`manifest`], but a different kind of plugin — what an installation that
    /// received both halves of the RD-098-03 collision was left holding.
    fn foreign_manifest(version: &str) -> Vec<u8> {
        format!(
            r#"manifest_version = 3
plugin_type = "enricher"
api_version = "0.10.0"
id = "019d0000-0000-7000-8000-00000000bbbb"
name = "Squatter"
version = "{version}"
key_id = "dev"
public_key = "{DEV_PUBLIC_KEY}"

[metadata]
description = "Enricher fixture"
author = "rDownloader"

[extension]
slug = "squatter"
claims = ["squatter"]
"#
        )
        .into_bytes()
    }

    /// A version of another plugin type under a bundled plugin's id is the leftover of an id
    /// collision. Left in place it would linger forever and read like a plugin that had been
    /// updated, which is exactly how the collision stayed invisible for a release.
    #[tokio::test]
    async fn a_foreign_plugin_under_a_bundled_id_is_removed_and_reported() {
        let directory = tempfile::tempdir().expect("tempdir");
        let bundled = directory.path().join("bundled");
        std::fs::create_dir_all(&bundled).expect("bundled dir");
        let installer =
            PluginInstaller::new(directory.path().join("plugins"), PluginVerifier::new(true));

        // The state the collision left: the foreign package installed under the shared id.
        std::fs::write(
            bundled.join("squatter-0.9.0.rdplug"),
            package_plugin(&foreign_manifest("0.9.0"), EMPTY_COMPONENT, &[], None)
                .expect("package"),
        )
        .expect("write");
        sync_bundled(&installer, &bundled, BundledPolicy::All)
            .await
            .expect("seed");
        std::fs::remove_file(bundled.join("squatter-0.9.0.rdplug")).expect("remove");
        assert_eq!(installer.list_installed().await.expect("list").len(), 1);

        // Now the release ships only the plugin that owns the id.
        std::fs::write(
            bundled.join("bundled-1.0.0.rdplug"),
            package_plugin(&manifest("1.0.0"), EMPTY_COMPONENT, &[], None).expect("package"),
        )
        .expect("write");
        let report = sync_bundled(&installer, &bundled, BundledPolicy::All)
            .await
            .expect("sync");

        assert_eq!(report.installed.len(), 1);
        assert_eq!(
            report.orphaned.len(),
            1,
            "the leftover is reported, not silently kept"
        );
        assert_eq!(report.orphaned[0].1, "0.9.0");
        assert_eq!(report.orphaned[0].2, "enricher");
        let installed = installer.list_installed().await.expect("list");
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].plugin_type.as_str(), "resolver");
    }

    /// The bundled plugin's own older versions share its type and must survive: the cleanup is
    /// about foreign packages, not about pruning version history.
    #[tokio::test]
    async fn older_versions_of_the_same_plugin_are_left_alone() {
        let directory = tempfile::tempdir().expect("tempdir");
        let bundled = directory.path().join("bundled");
        std::fs::create_dir_all(&bundled).expect("bundled dir");
        let installer =
            PluginInstaller::new(directory.path().join("plugins"), PluginVerifier::new(true));
        for version in ["1.0.0", "1.1.0"] {
            std::fs::write(
                bundled.join(format!("bundled-{version}.rdplug")),
                package_plugin(&manifest(version), EMPTY_COMPONENT, &[], None).expect("package"),
            )
            .expect("write");
            sync_bundled(&installer, &bundled, BundledPolicy::All)
                .await
                .expect("sync");
        }
        let report = sync_bundled(&installer, &bundled, BundledPolicy::All)
            .await
            .expect("sync");
        assert!(report.orphaned.is_empty());
        assert_eq!(installer.list_installed().await.expect("list").len(), 2);
    }

    #[tokio::test]
    async fn unsigned_bundles_are_rejected_outside_development_mode() {
        let directory = tempfile::tempdir().expect("tempdir");
        let bundled = directory.path().join("bundled");
        std::fs::create_dir_all(&bundled).expect("bundled dir");
        std::fs::write(
            bundled.join("x.rdplug"),
            package_plugin(&manifest("1.0.0"), EMPTY_COMPONENT, &[], None).expect("package"),
        )
        .expect("write");
        let installer =
            PluginInstaller::new(directory.path().join("plugins"), PluginVerifier::new(false));
        let report = sync_bundled(&installer, &bundled, BundledPolicy::All)
            .await
            .expect("sync");
        assert!(report.installed.is_empty());
        assert_eq!(report.rejected.len(), 1);
    }

    /// A post-processing step: a service that needs no account, which a first start installs.
    fn account_free_manifest() -> Vec<u8> {
        format!(
            r#"manifest_version = 3
plugin_type = "postprocess"
api_version = "0.10.0"
id = "019d0000-0000-7000-8000-00000000cccc"
name = "Checksums"
version = "1.0.0"
key_id = "dev"
public_key = "{DEV_PUBLIC_KEY}"

[metadata]
description = "Post-processing fixture"
author = "rDownloader"

[extension]
slug = "checksums"
claims = []
"#
        )
        .into_bytes()
    }

    fn write_package(directory: &std::path::Path, name: &str, manifest: &[u8]) {
        std::fs::write(
            directory.join(name),
            package_plugin(manifest, EMPTY_COMPONENT, &[], None).expect("package"),
        )
        .expect("write");
    }

    /// RD-160-05: an update run keeps what is installed current and installs nothing new. A
    /// plugin that is not installed — never chosen, removed, or new in this release — is
    /// offered as available instead.
    #[tokio::test]
    async fn installed_only_updates_what_is_installed_and_offers_the_rest() {
        let directory = tempfile::tempdir().expect("tempdir");
        let bundled = directory.path().join("bundled");
        std::fs::create_dir_all(&bundled).expect("bundled dir");
        write_package(&bundled, "bundled-1.0.0.rdplug", &manifest("1.0.0"));
        write_package(&bundled, "checksums-1.0.0.rdplug", &account_free_manifest());
        let installer =
            PluginInstaller::new(directory.path().join("plugins"), PluginVerifier::new(true));

        let report = sync_bundled(&installer, &bundled, BundledPolicy::InstalledOnly)
            .await
            .expect("sync");
        assert!(report.installed.is_empty());
        assert_eq!(report.available, 2);
        assert!(installer.list_installed().await.expect("list").is_empty());
        let services = installer.bundled_services();
        let keys: Vec<&str> = services
            .iter()
            .map(|service| service.key.as_str())
            .collect();
        assert_eq!(keys, ["bundled", "checksums"], "both are offered");

        // The person installs one of them; the next release ships a newer version of it and a
        // newer version of nothing else.
        installer
            .install(bundled.join("bundled-1.0.0.rdplug"))
            .await
            .expect("install");
        write_package(&bundled, "bundled-1.1.0.rdplug", &manifest("1.1.0"));
        let update = sync_bundled(&installer, &bundled, BundledPolicy::InstalledOnly)
            .await
            .expect("update");
        assert_eq!(update.installed.len(), 1);
        assert_eq!(update.installed[0].1, "1.1.0");
        assert_eq!(update.available, 1, "the unchosen one stays available");
        let installed = installer.list_installed().await.expect("list");
        assert!(installed.iter().all(|manifest| manifest.name == "Bundled"));
    }

    /// RD-160-05: a fresh installation starts with the services that need no account, so it
    /// works before anybody opened the wizard; the ones that need an account wait to be chosen.
    #[tokio::test]
    async fn a_first_start_installs_only_what_needs_no_account() {
        let directory = tempfile::tempdir().expect("tempdir");
        let bundled = directory.path().join("bundled");
        std::fs::create_dir_all(&bundled).expect("bundled dir");
        write_package(&bundled, "bundled-1.0.0.rdplug", &manifest("1.0.0"));
        write_package(&bundled, "checksums-1.0.0.rdplug", &account_free_manifest());
        let installer =
            PluginInstaller::new(directory.path().join("plugins"), PluginVerifier::new(true));

        let report = sync_bundled(&installer, &bundled, BundledPolicy::FirstStart)
            .await
            .expect("sync");
        assert_eq!(report.installed.len(), 1);
        assert_eq!(report.available, 1);
        let installed = installer.list_installed().await.expect("list");
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].name, "Checksums");
    }
}
