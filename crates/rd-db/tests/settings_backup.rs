use rd_core::{
    AccountId, AuthMethod, AuthOrigin, AuthProfileId, AuthScope, Category, CategoryId,
    CategoryRule, CategoryRuleId, EventKind, HotFolderConfig, HotFolderExecutor, HotFolderId,
    ImportMode, ProxyKind, ProxyProfileId, StorageRootConfig, StorageRootId, StreamChannelId,
    UsenetServerId,
};
use rd_db::{
    ConfigReplacement, Database, ReplacementAccount, ReplacementAuthProfile,
    ReplacementProxyProfile, ReplacementStreamChannel, ReplacementUsenetServer,
};

fn replacement(label: &str) -> ConfigReplacement {
    let root_id = StorageRootId::new();
    let category_id = CategoryId::new();
    let proxy_id = ProxyProfileId::new();
    ConfigReplacement {
        storage_roots: vec![StorageRootConfig {
            id: root_id,
            name: format!("{label} root"),
            path: format!("/{label}/downloads"),
            is_default: true,
            minimum_free_bytes: None,
        }],
        categories: vec![Category {
            id: category_id,
            name: format!("{label} category"),
            color: "#336699".to_owned(),
            storage_root_id: root_id,
            relative_path: "movies".to_owned(),
            is_default: true,
            postprocess_level: Some(rd_core::PostprocessLevel::Unpack),
            script: Some("finish.sh".to_owned()),
            cleanup_extensions: Some(vec!["nfo".to_owned()]),
            recursive_unpack: Some(true),
            // Non-default (the global setting is on) so the round trip proves it survives.
            sfv_verify: Some(false),
            safe_postproc: Some(false),
            delete_par2: None,
            upload_enabled: Some(true),
            upload_remote: Some("archive:movies".to_owned()),
            // Round-tripped so a category seeding override survives export and import.
            seeding: Some(rd_core::SeedingPolicyOverride {
                enabled: Some(false),
                ratio_milli: Some(2_500),
                time: Some(rd_core::SeedTimeLimit::Unlimited),
            }),
            // Round-tripped for the same reason: a category's plugin steps are configuration
            // somebody chose, and an export that dropped them would restore a quieter setup.
            plugin_steps: Some(vec!["019d0000-0000-7000-8000-000000000106".to_owned()]),
        }],
        category_rules: vec![CategoryRule {
            id: CategoryRuleId::new(),
            name: format!("{label} rule"),
            priority: 10,
            source: Some(rd_core::IngressSource::Manual),
            domain: Some("example.test".to_owned()),
            protocol: None,
            extension: Some("mkv".to_owned()),
            mime_type: None,
            name_regex: None,
            category_id,
            enabled: true,
        }],
        hotfolders: vec![HotFolderConfig {
            id: HotFolderId::new(),
            name: format!("{label} hotfolder"),
            executor: HotFolderExecutor::Daemon,
            path: format!("/{label}/watch"),
            recursive: true,
            category_id: Some(category_id),
            import_mode: ImportMode::Review,
            processed_path: "processed".to_owned(),
            failed_path: "failed".to_owned(),
            enabled: true,
        }],
        proxy_profiles: vec![ReplacementProxyProfile {
            id: proxy_id,
            name: format!("{label} proxy"),
            kind: ProxyKind::Socks5,
            endpoint: "socks5h://127.0.0.1:1080".parse().expect("proxy URL"),
            username: Some("proxy-user".to_owned()),
            secret_ref: Some(format!("vault://{label}-proxy")),
        }],
        accounts: vec![ReplacementAccount {
            id: AccountId::new(),
            provider: "premiumize".to_owned(),
            label: format!("{label} account"),
            username: None,
            credential_mode: None,
            secret_ref: Some(format!("vault://{label}-secret")),
            cookie_ref: Some(format!("vault://{label}-cookies")),
            proxy_profile_id: Some(proxy_id),
            enabled: true,
        }],
        usenet_servers: vec![ReplacementUsenetServer {
            id: UsenetServerId::new(),
            name: format!("{label} usenet"),
            host: "news.example.test".to_owned(),
            port: 563,
            tls: true,
            username: Some("reader".to_owned()),
            password_ref: Some(format!("vault://{label}-nntp")),
            proxy_profile_id: Some(proxy_id),
            priority: 5,
            max_connections: 8,
            enabled: true,
        }],
        stream_channels: vec![ReplacementStreamChannel {
            id: StreamChannelId::new(),
            url: "https://example.test/live".to_owned(),
            name: format!("{label} stream"),
            quality: Some("720p".to_owned()),
            category_id: Some(category_id),
            enabled: true,
            recording: rd_core::RecordingPolicy::default(),
        }],
        subscriptions: vec![rd_db::ReplacementSubscription {
            id: rd_core::SubscriptionId::new(),
            name: format!("{label} channel"),
            url: "https://example.test/c/channel".to_owned(),
            kind: rd_core::SubscriptionKind::Media,
            enabled: true,
            mode: rd_core::SubscriptionMode::Review,
            category_id: Some(category_id),
            priority: rd_core::DownloadPriority::default(),
            interval_seconds: 3_600,
            filters: rd_core::SubscriptionFilters::default(),
            backlog: rd_core::BacklogPolicy::FromNow,
            category_map: Vec::new(),
            source_categories: Vec::new(),
            every_release: false,
            view: rd_core::SubscriptionView::List,
            autoplay: false,
            card_ratio: rd_core::SubscriptionCardRatio::OneOne,
            schedule: None,
            secret_ref: None,
        }],
        auth_profiles: vec![ReplacementAuthProfile {
            id: AuthProfileId::new(),
            name: format!("{label} profile"),
            scope: AuthScope::parse("files.example.test", true).expect("scope"),
            method: AuthMethod::Bearer,
            origin: AuthOrigin::Manual,
            enabled: true,
            expires_at: None,
            username: None,
            secret_ref: Some(format!("vault://{label}-token")),
            certificate_ref: None,
        }],
    }
}

#[tokio::test]
async fn replacement_swaps_all_config_atomically_and_emits_refresh_events() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("backup.sqlite"))
        .await
        .expect("database");
    database
        .replace_config(replacement("old"))
        .await
        .expect("initial config");

    let expected = replacement("new");
    let ids = (
        expected.storage_roots[0].id,
        expected.categories[0].id,
        expected.category_rules[0].id,
        expected.hotfolders[0].id,
        expected.proxy_profiles[0].id,
        expected.accounts[0].id,
        expected.usenet_servers[0].id,
        expected.stream_channels[0].id,
        expected.auth_profiles[0].id,
    );
    let mut events = database.subscribe();
    database
        .replace_config(expected.clone())
        .await
        .expect("replacement");

    assert_eq!(
        database.list_storage_roots().await.expect("roots")[0].id,
        ids.0
    );
    // A card ratio somebody chose (RD-120-42) is restored, not reset to the `2:1` default.
    assert_eq!(
        database.list_subscriptions().await.expect("subscriptions")[0].card_ratio,
        rd_core::SubscriptionCardRatio::OneOne
    );
    let restored = database.list_categories().await.expect("categories");
    assert_eq!(restored[0].id, ids.1);
    // A category seeding override must survive the round trip, not silently reset to
    // inheriting the global policy.
    let seeding = restored[0].seeding.expect("seeding override restored");
    assert_eq!(seeding.enabled, Some(false));
    assert_eq!(seeding.ratio(), Some(2.5));
    assert_eq!(seeding.time, Some(rd_core::SeedTimeLimit::Unlimited));
    assert_eq!(
        database.list_category_rules().await.expect("rules")[0].id,
        ids.2
    );
    assert_eq!(
        database.list_hotfolders().await.expect("hotfolders")[0].id,
        ids.3
    );
    assert_eq!(
        database.list_proxy_profiles().await.expect("proxies")[0].id,
        ids.4
    );
    assert_eq!(
        database.list_accounts().await.expect("accounts")[0].id,
        ids.5
    );
    assert_eq!(
        database.list_usenet_servers().await.expect("servers")[0].id,
        ids.6
    );
    assert_eq!(
        database.list_stream_channels().await.expect("streams")[0].id,
        ids.7
    );
    let profiles = database.list_auth_profiles().await.expect("profiles");
    assert_eq!(profiles[0].id, ids.8);
    // The re-minted reference must survive the swap, otherwise the restored profile would
    // be present but unusable.
    assert_eq!(profiles[0].secret_ref.as_deref(), Some("vault://new-token"));
    assert!(profiles[0].scope.include_subdomains);
    assert_eq!(profiles[0].scope.host, "files.example.test");

    assert_eq!(
        database
            .account_secret_refs(ids.5)
            .await
            .expect("account refs"),
        Some((
            Some("vault://new-secret".to_owned()),
            Some("vault://new-cookies".to_owned())
        ))
    );
    assert_eq!(
        database
            .usenet_connection_config(ids.6)
            .await
            .expect("server config")
            .expect("server")
            .password_ref
            .as_deref(),
        Some("vault://new-nntp")
    );

    let emitted = (0..7)
        .map(|_| events.try_recv().expect("refresh event").kind)
        .collect::<Vec<_>>();
    assert_eq!(
        emitted,
        vec![
            EventKind::CategoryChanged,
            EventKind::HotFolderChanged,
            EventKind::AuthProfileChanged,
            EventKind::AccountChanged,
            EventKind::ProxyChanged,
            EventKind::UsenetChanged,
            EventKind::StreamChanged,
        ]
    );

    let mut invalid = replacement("invalid");
    let mut duplicate = invalid.storage_roots[0].clone();
    duplicate.id = StorageRootId::new();
    invalid.storage_roots.push(duplicate);
    assert!(database.replace_config(invalid).await.is_err());
    assert_eq!(
        database.list_categories().await.expect("categories")[0].id,
        ids.1
    );
    assert_eq!(
        database.list_accounts().await.expect("accounts")[0].id,
        ids.5
    );
}

/// A bundle exported before the single-default invariant, or hand-edited afterwards, must not
/// be able to leave the install without a default — restore repairs instead of refusing.
#[tokio::test]
async fn restoring_a_bundle_without_a_default_promotes_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("defaults.sqlite"))
        .await
        .expect("database");

    let mut bundle = replacement("broken");
    let extra_root = StorageRootId::new();
    bundle.storage_roots[0].is_default = false;
    bundle.storage_roots[0].name = "Zulu".to_owned();
    bundle.storage_roots.push(StorageRootConfig {
        id: extra_root,
        name: "Alpha".to_owned(),
        path: "/alpha".to_owned(),
        is_default: false,
        minimum_free_bytes: None,
    });

    database.replace_config(bundle).await.expect("restore");

    let roots = database.list_storage_roots().await.expect("roots");
    let defaults: Vec<&str> = roots
        .iter()
        .filter(|root| root.is_default)
        .map(|root| root.name.as_str())
        .collect();
    assert_eq!(defaults, vec!["Alpha"]);
}

/// The mirror case: a bundle claiming two defaults restores as exactly one.
#[tokio::test]
async fn restoring_a_bundle_with_two_defaults_keeps_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("two.sqlite"))
        .await
        .expect("database");

    let mut bundle = replacement("greedy");
    bundle.storage_roots[0].name = "Zulu".to_owned();
    bundle.storage_roots.push(StorageRootConfig {
        id: StorageRootId::new(),
        name: "Alpha".to_owned(),
        path: "/alpha".to_owned(),
        is_default: true,
        minimum_free_bytes: None,
    });

    database.replace_config(bundle).await.expect("restore");

    let defaults: Vec<String> = database
        .list_storage_roots()
        .await
        .expect("roots")
        .into_iter()
        .filter(|root| root.is_default)
        .map(|root| root.name)
        .collect();
    assert_eq!(defaults, vec!["Alpha".to_owned()]);
}
/// The same repair for categories: a bundle with no default leaves routing without the
/// fallback it uses for every link no rule matched.
#[tokio::test]
async fn restoring_a_bundle_without_a_default_category_promotes_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("category-defaults.sqlite"))
        .await
        .expect("database");

    let mut bundle = replacement("broken");
    bundle.categories[0].is_default = false;
    bundle.categories[0].name = "Zulu".to_owned();
    let mut extra = bundle.categories[0].clone();
    extra.id = CategoryId::new();
    extra.name = "Alpha".to_owned();
    extra.relative_path = "alpha".to_owned();
    bundle.categories.push(extra);

    database.replace_config(bundle).await.expect("restore");

    let defaults: Vec<String> = database
        .list_categories()
        .await
        .expect("categories")
        .into_iter()
        .filter(|category| category.is_default)
        .map(|category| category.name)
        .collect();
    assert_eq!(defaults, vec!["Alpha".to_owned()]);
}

/// And the mirror case: two default categories are refused outright by the partial unique
/// index, so the restore has to reduce them to one instead of failing.
#[tokio::test]
async fn restoring_a_bundle_with_two_default_categories_keeps_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("two-categories.sqlite"))
        .await
        .expect("database");

    let mut bundle = replacement("greedy");
    bundle.categories[0].name = "Zulu".to_owned();
    let mut extra = bundle.categories[0].clone();
    extra.id = CategoryId::new();
    extra.name = "Alpha".to_owned();
    extra.relative_path = "alpha".to_owned();
    bundle.categories.push(extra);

    database.replace_config(bundle).await.expect("restore");

    let defaults: Vec<String> = database
        .list_categories()
        .await
        .expect("categories")
        .into_iter()
        .filter(|category| category.is_default)
        .map(|category| category.name)
        .collect();
    assert_eq!(defaults, vec!["Alpha".to_owned()]);
}
