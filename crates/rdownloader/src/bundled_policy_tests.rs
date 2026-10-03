//! A bundled service removed after the first start stays removed (RD-180-14).
//!
//! The setup wizard removes what the person unticks. The first start of a fresh installation
//! installs the services that need no account and records `plugins.bundled_first_start`; every
//! later start only updates what is installed, so an installation whose person removed every
//! one of them — nothing installed, the state of a fresh one — still gets nothing back.

use super::{BUNDLED_FIRST_START_SETTING, sync_bundled_plugins};

const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";
const DEV_PUBLIC_KEY: &str = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY=";

/// A post-processing step: a service that needs no account, which a first start installs.
fn account_free_manifest() -> String {
    format!(
        r#"manifest_version = 3
plugin_type = "postprocess"
api_version = "0.10.0"
id = "019d0000-0000-7000-8000-0000000180e1"
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
}

#[tokio::test]
async fn a_service_removed_after_the_first_start_is_not_installed_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let bundle = directory.path().join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle dir");
    let package = rd_plugin_host::package_plugin(
        account_free_manifest().as_bytes(),
        EMPTY_COMPONENT,
        &[],
        None,
    )
    .expect("package");
    std::fs::write(bundle.join("checksums.rdplug"), package).expect("write");
    let database = rd_db::Database::open(directory.path().join("policy.sqlite3"))
        .await
        .expect("database");
    let plugins = rd_plugin_host::PluginInstaller::new(
        directory.path().join("plugins"),
        rd_plugin_host::PluginVerifier::new(true),
    );

    sync_bundled_plugins(&database, &plugins, Some(bundle.clone()), false).await;
    let installed = plugins.list_installed().await.expect("list");
    assert_eq!(installed.len(), 1, "the first start installs it");
    assert!(
        database
            .get_setting(BUNDLED_FIRST_START_SETTING)
            .await
            .expect("marker")
            .is_some(),
        "and records that it ran"
    );

    // What the wizard's removal leaves: nothing installed, as on a fresh installation.
    assert!(
        plugins
            .remove_version(&installed[0].id.to_string(), &installed[0].version)
            .await
            .expect("remove")
    );
    sync_bundled_plugins(&database, &plugins, Some(bundle), false).await;
    assert!(
        plugins.list_installed().await.expect("list").is_empty(),
        "a later start leaves the removed service out"
    );
    assert_eq!(plugins.bundled_services().len(), 1, "it stays available");
}
