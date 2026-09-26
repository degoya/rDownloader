//! Pins set on purpose and pins on a withdrawn version (RD-140-02), and the version choice
//! table they sit beside.

use rd_core::{DownloadId, DownloadState, PackageId, PluginId, ResolverPin};

use crate::{Database, NewDownload, NewPackage, NewPluginDigestRevocation, NewPluginVersionChoice};

async fn database(directory: &std::path::Path) -> Database {
    Database::open(directory.join("pins.sqlite"))
        .await
        .expect("database")
}

async fn download(database: &Database, state: DownloadState) -> DownloadId {
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "pins".to_owned(),
            destination: "downloads".to_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: "https://example.test/file".parse().expect("URL"),
            file_name: "file".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: state,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download")
        .id
}

fn pin(plugin_id: PluginId, version: &str) -> ResolverPin {
    ResolverPin {
        plugin_id,
        version: version.to_owned(),
    }
}

async fn withdraw(database: &Database, plugin_id: PluginId, version: &str) {
    database
        .revoke_plugin_digest(NewPluginDigestRevocation {
            digest: format!("{:0>64}", version.replace('.', "")),
            plugin_id: Some(plugin_id.to_string()),
            plugin_name: Some("Fixture".to_owned()),
            version: Some(version.to_owned()),
            reason: None,
        })
        .await
        .expect("withdraw");
}

async fn state_and_code(database: &Database, id: DownloadId) -> (DownloadState, Option<String>) {
    let download = database
        .get_download(id)
        .await
        .expect("read")
        .expect("download");
    (
        download.state,
        download.last_error.and_then(|failure| failure.code),
    )
}

/// The gap RD-140-02 found: a pin on a withdrawn version used to vanish at start without a
/// word. It is still released — the build can never run again — but the download is held and
/// says why, and only a person sends it on to the plugin's current version.
#[tokio::test]
async fn a_pin_on_a_withdrawn_version_is_released_visibly() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;
    let plugin = PluginId::new();
    let queued = download(&database, DownloadState::Queued).await;
    let paused = download(&database, DownloadState::Paused).await;
    let removed = download(&database, DownloadState::Queued).await;
    for id in [queued, paused] {
        database
            .claim_resolver_pin(id, pin(plugin, "1.0.0"))
            .await
            .expect("pin");
    }
    database
        .claim_resolver_pin(removed, pin(plugin, "0.9.0"))
        .await
        .expect("pin");
    withdraw(&database, plugin, "1.0.0").await;

    // 1.0.0 was withdrawn and 0.9.0 simply removed; neither loaded, 2.0.0 did.
    let freed = database
        .clear_unsatisfiable_resolver_pins(vec![(plugin.to_string(), "2.0.0".to_owned())])
        .await
        .expect("reconcile");

    assert_eq!(freed, 3);
    for id in [queued, paused, removed] {
        assert_eq!(database.resolver_pin(id).await.expect("pin"), None);
    }
    assert_eq!(
        state_and_code(&database, queued).await,
        (
            DownloadState::Blocked,
            Some(super::PINNED_VERSION_WITHDRAWN.to_owned())
        ),
        "a download that would have started on its own waits for a person"
    );
    assert_eq!(
        state_and_code(&database, paused).await,
        (
            DownloadState::Paused,
            Some(super::PINNED_VERSION_WITHDRAWN.to_owned())
        ),
        "one that already waits keeps its state and carries the reason"
    );
    assert_eq!(
        state_and_code(&database, removed).await,
        (DownloadState::Queued, None),
        "a version that is merely gone is released as before"
    );

    // And a held download resumes like any other blocked one.
    database
        .transition_download(queued, DownloadState::Queued)
        .await
        .expect("a blocked download can be sent back to the queue");
}

#[tokio::test]
async fn a_download_is_pinned_to_the_staged_version_only_while_it_is_not_running() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;
    let plugin = PluginId::new();
    let job = download(&database, DownloadState::Queued).await;
    database
        .claim_resolver_pin(job, pin(plugin, "1.0.0"))
        .await
        .expect("pin");

    // Unlike the claim, this replaces the pin a download already has.
    database
        .pin_download_resolver(job, pin(plugin, "2.0.0-rc.1"))
        .await
        .expect("repin");
    assert_eq!(
        database.resolver_pin(job).await.expect("pin"),
        Some(pin(plugin, "2.0.0-rc.1"))
    );

    database
        .transition_download(job, DownloadState::Resolving)
        .await
        .expect("start");
    let refused = database
        .pin_download_resolver(job, pin(plugin, "1.0.0"))
        .await
        .expect_err("a running download keeps its version");
    assert_eq!(
        crate::store_kind(&refused),
        Some(crate::StoreErrorKind::WrongState)
    );
    assert_eq!(
        database.resolver_pin(job).await.expect("pin"),
        Some(pin(plugin, "2.0.0-rc.1"))
    );
}

#[tokio::test]
async fn a_version_choice_is_stored_whole_and_refuses_the_same_version_twice() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(directory.path()).await;
    let choice = |active: Option<&str>, staged: Option<&str>| NewPluginVersionChoice {
        plugin_id: "plugin".to_owned(),
        active_version: active.map(str::to_owned),
        previous_version: Some("1.0.0".to_owned()),
        staged_version: staged.map(str::to_owned),
        update_policy: "automatic".to_owned(),
    };

    assert!(
        database
            .plugin_version_choice("plugin")
            .await
            .expect("read")
            .is_none()
    );
    database
        .save_plugin_version_choice(choice(Some("2.0.0"), Some("3.0.0")))
        .await
        .expect("save");
    database
        .save_plugin_version_choice(choice(Some("1.0.0"), None))
        .await
        .expect("replace");
    let stored = database
        .plugin_version_choice("plugin")
        .await
        .expect("read")
        .expect("row");
    assert_eq!(stored.active_version.as_deref(), Some("1.0.0"));
    assert_eq!(
        stored.staged_version, None,
        "a replace drops the old staging"
    );
    assert_eq!(stored.update_policy, "automatic");
    assert_eq!(
        database
            .list_plugin_version_choices()
            .await
            .expect("list")
            .len(),
        1
    );

    assert!(
        database
            .save_plugin_version_choice(choice(Some("2.0.0"), Some("2.0.0")))
            .await
            .is_err()
    );
    let mut unknown = choice(None, None);
    unknown.update_policy = "sometimes".to_owned();
    assert!(database.save_plugin_version_choice(unknown).await.is_err());
}
