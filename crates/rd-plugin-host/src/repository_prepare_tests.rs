//! The plugins an update fetches for the version it installs, and the start that installs them
//! (RD-1260-02): the RD-1251-01 case — every installed version refused by the new contract —
//! solved before the restart instead of after it.

use super::*;
use crate::{APP_VERSION, SUPPORTED_API_VERSIONS};

fn contracts() -> Vec<String> {
    SUPPORTED_API_VERSIONS
        .iter()
        .map(|contract| (*contract).to_owned())
        .collect()
}

/// The fixture plugin refused at 1.2.3, and the repository offering 1.2.4 for the contract the
/// target speaks.
async fn refused_with_an_offer() -> (Fixture, PluginRepositoryService) {
    let fixture = Fixture::new().await;
    install_refused(fixture.installer.root(), "1.2.3");
    let newer = package("1.2.4");
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &newer)],
        Revocations::default(),
    ));
    fixture.serve_package("1.2.4", newer);
    let service = fixture.service();
    (fixture, service)
}

fn prepared_root(fixture: &Fixture) -> PathBuf {
    fixture.root.join("prepared")
}

#[tokio::test]
async fn an_update_fetches_the_new_contracts_package_and_its_start_installs_it() {
    let (fixture, service) = refused_with_an_offer().await;
    let contracts = contracts();
    let outcome = service
        .prepare_update(UpdateTarget {
            version: APP_VERSION,
            contracts: &contracts,
        })
        .await
        .expect("prepare");
    assert_eq!(outcome.prepared.len(), 1);
    assert_eq!(outcome.prepared[0].version, "1.2.4");
    assert!(outcome.missing.is_empty(), "{:?}", outcome.missing);
    assert!(
        prepared_root(&fixture)
            .join(APP_VERSION)
            .join("sources.json")
            .is_file()
    );

    // The new version's start, before the bundle is read.
    let report = install_prepared(&fixture.installer, &fixture.database, &fixture.root).await;
    assert_eq!(report.installed.len(), 1, "{:?}", report.rejected);
    let installed = fixture.installer.list_installed().await.expect("installed");
    let versions: Vec<&str> = installed
        .iter()
        .map(|manifest| manifest.version.as_str())
        .collect();
    assert_eq!(versions, ["1.2.4"]);
    assert!(!prepared_root(&fixture).exists(), "the preparation stayed");
    let delivered = fixture
        .database
        .list_plugin_repository_installs()
        .await
        .expect("deliveries");
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].repository_id, OFFICIAL_REPOSITORY_ID);
    assert_eq!(delivered[0].version, "1.2.4");
}

/// Every update without a contract change: nothing is fetched, not even the index.
#[tokio::test]
async fn an_update_that_keeps_the_contract_fetches_nothing() {
    let fixture = Fixture::new().await;
    fixture
        .installer
        .install_bytes(package("1.2.3"))
        .await
        .expect("install");
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &package("1.2.4"))],
        Revocations::default(),
    ));
    let service = fixture.service();
    let contracts = contracts();
    let outcome = service
        .prepare_update(UpdateTarget {
            version: "99.0.0",
            contracts: &contracts,
        })
        .await
        .expect("prepare");
    assert!(outcome.prepared.is_empty() && outcome.missing.is_empty());
    assert_eq!(
        fixture.official().await.sequence,
        None,
        "the index was fetched"
    );
    assert!(!prepared_root(&fixture).exists());
}

/// Without a network the update goes ahead; the plugin is named, and the bundle stays its
/// fallback at the next start.
#[tokio::test]
async fn an_unreachable_repository_names_the_plugin_and_prepares_nothing() {
    let fixture = Fixture::new().await;
    install_refused(fixture.installer.root(), "1.2.3");
    let service = fixture.service();
    let contracts = contracts();
    let outcome = service
        .prepare_update(UpdateTarget {
            version: APP_VERSION,
            contracts: &contracts,
        })
        .await
        .expect("prepare");
    assert!(outcome.prepared.is_empty());
    assert_eq!(outcome.missing, ["Fixture"]);
    assert!(!prepared_root(&fixture).exists());
}

/// A preparation for another version — an update that was taken back — is removed by the start
/// and installs nothing.
#[tokio::test]
async fn a_start_removes_a_preparation_for_another_version() {
    let (fixture, service) = refused_with_an_offer().await;
    let contracts = contracts();
    service
        .prepare_update(UpdateTarget {
            version: "99.0.0",
            contracts: &contracts,
        })
        .await
        .expect("prepare");
    assert!(prepared_root(&fixture).join("99.0.0").is_dir());
    let report = install_prepared(&fixture.installer, &fixture.database, &fixture.root).await;
    assert!(report.installed.is_empty());
    assert!(!prepared_root(&fixture).exists());
}

/// `plugin.before_prepared_published` (recovery matrix): a preparation stopped after its packages
/// were written and before its folder got its version's name.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_preparation_crashed_before_its_rename_is_never_installed_and_the_start_removes_it() {
    let (fixture, service) = refused_with_an_offer().await;
    let contracts = contracts();
    let target = UpdateTarget {
        version: APP_VERSION,
        contracts: &contracts,
    };
    {
        let guard = rd_core::failpoint::FailpointGuard::once("plugin.before_prepared_published");
        assert!(service.prepare_update(target).await.is_err());
        assert!(guard.fired(), "the crash point was never reached");
    }
    let staging = prepared_root(&fixture).join(format!("{APP_VERSION}.partial"));
    assert!(staging.is_dir());
    assert!(!prepared_root(&fixture).join(APP_VERSION).exists());

    let report = install_prepared(&fixture.installer, &fixture.database, &fixture.root).await;
    assert!(report.installed.is_empty());
    assert!(!prepared_root(&fixture).exists(), "the staging stayed");
    assert!(
        fixture
            .installer
            .list_installed()
            .await
            .expect("installed")
            .is_empty()
    );
    assert_eq!(
        fixture
            .installer
            .list_refused()
            .await
            .expect("refused")
            .len(),
        1,
        "the refused version stays for the bundle to replace"
    );

    // The next preparation fetches it again, whole.
    let outcome = service.prepare_update(target).await.expect("prepare again");
    assert_eq!(outcome.prepared.len(), 1);
    assert!(prepared_root(&fixture).join(APP_VERSION).is_dir());
}
