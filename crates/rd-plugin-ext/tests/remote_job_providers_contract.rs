//! The providers the remote-job form offers, read from the manifests, are the providers the
//! built runners route by (RD-120-51).
//!
//! `GET /api/v1/remote-jobs/providers` used to answer from `RemoteJobRunners::providers`, and
//! building those runners compiles every installed component -- the first open of the page
//! after a start waited for all of it. It now answers from the signature-checked manifests
//! alone. This is the proof that the answer did not change on the way: every bundled
//! `remote-job` plugin is installed with its real, built component and its locales, beside a
//! resolver of one of the same providers, and the two answers are compared.
//!
//! Named `_contract` because it needs the built components, so the `no-components` nextest
//! profile leaves it out by name like the other tests that do. It also reads which of them name
//! a cache kind (RD-130-11), from the same built components.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Instant,
};

use rd_plugin_ext::{CacheKind, RemoteJobRunners};
use rd_plugin_host::{PluginInstaller, PluginVerifier};

/// The bundled plugin directories, read at run time rather than baked in with `env!`: a test
/// binary outlives the worktree that built it.
fn plugins_root() -> PathBuf {
    let crate_root =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR at run time");
    Path::new(&crate_root).join("../../plugins")
}

/// Every bundled plugin whose manifest says `remote-job`, by directory name.
fn remote_job_directories() -> Vec<String> {
    let mut directories: Vec<String> = std::fs::read_dir(plugins_root())
        .expect("the plugins directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            std::fs::read_to_string(entry.path().join("manifest.toml"))
                .is_ok_and(|manifest| manifest.contains("plugin_type = \"remote-job\""))
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    directories.sort();
    directories
}

/// Lays one bundled plugin out the way the installer does: manifest, built component and
/// locales, unsigned -- the verifier below runs in development mode, and what is compared
/// here is the reading of a manifest, not the signature, which both paths check alike.
fn install(root: &Path, directory: &str) {
    let source = plugins_root().join(directory);
    let manifest_text =
        std::fs::read_to_string(source.join("manifest.toml")).expect("the bundled manifest");
    let manifest: rd_plugin_host::PluginManifest =
        toml::from_str(&manifest_text).expect("a valid manifest");
    let target = root.join(manifest.id.to_string()).join(&manifest.version);
    std::fs::create_dir_all(target.join("locales")).expect("the version directory");
    std::fs::write(target.join("manifest.toml"), manifest_text).expect("the manifest");
    std::fs::write(
        target.join("component.wasm"),
        rd_plugin_host::artifact::component(&format!("rd-plugin-{directory}")),
    )
    .expect("the component");
    for entry in std::fs::read_dir(source.join("locales")).expect("the plugin's locales") {
        let entry = entry.expect("a locale file");
        std::fs::copy(entry.path(), target.join("locales").join(entry.file_name()))
            .expect("copy a locale file");
    }
}

#[tokio::test]
async fn the_manifests_name_the_providers_the_runners_route_by() {
    let directories = remote_job_directories();
    // Counted, so a remote-job plugin dropping out of the tree is a failure here rather than a
    // smaller set compared with itself.
    assert_eq!(
        directories.len(),
        6,
        "the bundled remote-job plugins: {directories:?}"
    );
    let installed = tempfile::tempdir().expect("tempdir");
    for directory in &directories {
        install(installed.path(), directory);
    }
    // A resolver of a provider one of them claims. Its type is not `remote-job`, so neither
    // answer may take anything from it -- and a load compiles it all the same, which is what
    // the timing below is about.
    install(installed.path(), "realdebrid");
    let installer = PluginInstaller::new(installed.path().to_owned(), PluginVerifier::new(true));

    let started = Instant::now();
    let runners = RemoteJobRunners::load(&installer, None)
        .await
        .expect("the runners");
    let compiled = started.elapsed();
    let started = Instant::now();
    let manifests = installer
        .verified_manifests()
        .await
        .expect("the verified manifests");
    let read = started.elapsed();

    let before = runners.providers();
    let after = RemoteJobRunners::claimed_providers(&manifests);
    assert_eq!(after, before, "the answer must stay the same");
    assert_eq!(
        after,
        BTreeSet::from(
            [
                "offcloud",
                "premiumize",
                "putio",
                "realdebrid",
                "seedr",
                "torbox"
            ]
            .map(str::to_owned)
        )
    );
    // Printed, not asserted: a timing threshold on a shared machine is a flaky test. What the
    // job file records is measured from this line.
    eprintln!(
        "{} packages: runners built in {compiled:?}, manifests read and verified in {read:?}",
        directories.len() + 1
    );
}

/// RD-130-11: of the bundled remote-job plugins, TorBox asks its cache about all three kinds
/// and Premiumize about magnets -- its resolver already asks about hoster links -- while the
/// other four name none and are never asked. Read from the real components, the way the link
/// check reads them.
#[tokio::test]
async fn only_torbox_and_premiumize_name_cache_kinds() {
    let installed = tempfile::tempdir().expect("tempdir");
    for directory in remote_job_directories() {
        install(installed.path(), &directory);
    }
    let installer = PluginInstaller::new(installed.path().to_owned(), PluginVerifier::new(true));
    let runners = RemoteJobRunners::load(&installer, None)
        .await
        .expect("the runners");
    assert_eq!(
        runners.cache_providers().await,
        vec![
            ("premiumize".to_owned(), vec![CacheKind::Torrent]),
            (
                "torbox".to_owned(),
                vec![CacheKind::Torrent, CacheKind::Usenet, CacheKind::Hoster]
            ),
        ]
    );
}
