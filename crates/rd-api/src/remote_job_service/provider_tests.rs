//! Which providers can take a job, answered without building a runner (RD-120-51).
//!
//! The remote-job page asks this the moment it opens, and it used to be answered by
//! `runners()`, which compiles every installed component on its first call. These tests install
//! packages whose component is not WebAssembly at all: a compile would refuse every one of them,
//! so an answer that still names their provider can only have come from the manifest. The
//! equality of the two answers over the real bundled components is proven in
//! `crates/rd-plugin-ext/tests/remote_job_providers_contract.rs`.

use std::{collections::BTreeSet, path::Path, sync::Arc};

use super::{RemoteJobService, tests::NoHost};

/// Bytes no compiler takes. Nothing here may try.
const NOT_A_COMPONENT: &[u8] = b"not a component";

/// The bundled manifest of `plugins/<directory>`, read at run time (AGENTS.md: a test binary
/// outlives the worktree that built it).
fn bundled_manifest(directory: &str) -> String {
    let crate_root =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR at run time");
    std::fs::read_to_string(
        Path::new(&crate_root)
            .join("../../plugins")
            .join(directory)
            .join("manifest.toml"),
    )
    .expect("the bundled manifest")
}

/// Lays one package out the way the installer does, unsigned and with a component that is not
/// one. Returns the manifest id.
fn install_unsigned(root: &Path, directory: &str) -> String {
    let manifest = bundled_manifest(directory);
    // Two top-level keys are all this needs, and `rd-api` has no TOML parser of its own.
    let field = |key: &str| {
        manifest
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key} = \"")))
            .and_then(|rest| rest.strip_suffix('"'))
            .expect("a top-level manifest field")
            .to_owned()
    };
    let (id, version) = (field("id"), field("version"));
    let version = root.join(&id).join(version);
    std::fs::create_dir_all(&version).expect("version directory");
    std::fs::write(version.join("manifest.toml"), manifest).expect("manifest");
    std::fs::write(version.join("component.wasm"), NOT_A_COMPONENT).expect("component");
    id
}

#[tokio::test]
async fn the_providers_come_from_the_manifests_and_no_runner_is_built() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("plugins");
    install_unsigned(&root, "realdebrid-torrents");
    install_unsigned(&root, "premiumize-transfers");
    // A resolver of the same provider: its type is not `remote-job`, so it offers nothing.
    install_unsigned(&root, "realdebrid");
    // A remote-job plugin the user switched off offers nothing either, as in a load.
    let switched_off = install_unsigned(&root, "torbox-jobs");

    let verifier = rd_plugin_host::PluginVerifier::new(true);
    let installer = rd_plugin_host::PluginInstaller::new(root, verifier);
    installer.set_disabled([switched_off]);
    let database = rd_db::Database::open(directory.path().join("providers.sqlite3"))
        .await
        .expect("database");
    let service = RemoteJobService::detached_over(
        database,
        installer,
        Arc::new(NoHost),
        // What building the runners would answer. The test fails before it gets here if
        // anything asks.
        vec![Err(
            "listing the providers must not build the runners".to_owned()
        )],
    );

    let providers = service.providers().await.expect("the providers");

    assert_eq!(
        providers,
        BTreeSet::from(["premiumize".to_owned(), "realdebrid".to_owned()])
    );
    assert!(
        !service.inner.runners.initialized(),
        "answering the page must not initialise the runners"
    );
}

/// No plugin directory at all -- a fresh installation before the bundled sync -- is an empty
/// answer, not an error the page would show as "could not be read".
#[tokio::test]
async fn no_installed_plugin_is_an_empty_answer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("providers.sqlite3"))
        .await
        .expect("database");
    let service = RemoteJobService::detached_over(
        database,
        rd_plugin_host::PluginInstaller::new(
            directory.path().join("absent"),
            rd_plugin_host::PluginVerifier::new(false),
        ),
        Arc::new(NoHost),
        Vec::new(),
    );

    assert!(service.providers().await.expect("the providers").is_empty());
    assert!(!service.inner.runners.initialized());
}
