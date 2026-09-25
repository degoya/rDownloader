use chrono::{Duration, Utc};
use rd_core::{
    AuthMethod, AuthOrigin, AuthProfileSelection, AuthScope, DownloadId, ImportMode, IngressSource,
    NzbSegmentState, PackageId, PostprocessKind, PostprocessState, ProxyKind,
};

use crate::{
    Database, FailedNzbImport, NewAccount, NewAuthProfile, NewCategory, NewCategoryRule,
    NewDownload, NewNzbFile, NewNzbImport, NewNzbSegment, NewPackage, NewProxyProfile,
    NewStorageRoot, NewUsenetServer, NzbImportChange, UpdateAccount, UpdateAuthProfile,
};

/// Default selection for tests that only care about proxy/account precedence.
const SELECTION: AuthProfileSelection = AuthProfileSelection::Auto;

fn probe_url() -> url::Url {
    "https://example.com/file.bin".parse().expect("url")
}

fn scope(input: &str, subdomains: bool) -> AuthScope {
    AuthScope::parse(input, subdomains).expect("scope")
}

fn new_profile(name: &str, host: &str, subdomains: bool) -> NewAuthProfile {
    NewAuthProfile {
        name: name.to_owned(),
        scope: scope(host, subdomains),
        method: AuthMethod::Bearer,
        origin: AuthOrigin::Manual,
        enabled: true,
        expires_at: None,
        username: None,
        secret_ref: Some(format!("vault://{name}")),
        certificate_ref: None,
    }
}

#[tokio::test]
async fn account_and_proxy_lists_never_serialize_secret_references() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("network.sqlite"))
        .await
        .expect("database");
    let proxy = database
        .create_proxy_profile(NewProxyProfile {
            name: "Local SOCKS".to_owned(),
            kind: ProxyKind::Socks5,
            endpoint: "socks5h://127.0.0.1:1080".parse().expect("URL"),
            username: Some("proxy-user".to_owned()),
            secret_ref: Some("secret://proxy/password".to_owned()),
        })
        .await
        .expect("proxy");
    database
        .create_account(NewAccount {
            provider: "premiumize".to_owned(),
            label: "Premiumize".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: Some("secret://premiumize/api-key".to_owned()),
            cookie_ref: None,
            proxy_profile_id: Some(proxy.id),
            enabled: true,
        })
        .await
        .expect("account");

    let proxy_json = serde_json::to_value(database.list_proxy_profiles().await.expect("proxies"))
        .expect("proxy JSON");
    let account_json = serde_json::to_value(database.list_accounts().await.expect("accounts"))
        .expect("account JSON");
    assert_eq!(proxy_json[0]["has_credentials"], true);
    assert!(proxy_json[0].get("secret_ref").is_none());
    assert_eq!(account_json[0]["has_secret"], true);
    assert_eq!(account_json[0]["has_cookies"], false);
    assert!(account_json[0].get("secret_ref").is_none());
}

#[tokio::test]
async fn account_updates_preserve_selected_secret_references() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("account-update.sqlite"))
        .await
        .expect("database");
    let account = database
        .create_account(NewAccount {
            provider: "ddownload".to_owned(),
            label: "Old label".to_owned(),
            username: Some("reader".to_owned()),
            credential_mode: None,
            secret_ref: Some("vault://password".to_owned()),
            cookie_ref: Some("vault://cookies".to_owned()),
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");

    let updated = database
        .update_account(
            account.id,
            UpdateAccount {
                provider: "ddownload".to_owned(),
                label: "New label".to_owned(),
                username: Some("reader".to_owned()),
                credential_mode: None,
                secret_ref: Some("vault://password".to_owned()),
                cookie_ref: None,
                proxy_profile_id: None,
                enabled: false,
            },
        )
        .await
        .expect("update account");

    assert_eq!(updated.label, "New label");
    assert!(updated.has_secret);
    assert!(!updated.has_cookies);
    assert!(!updated.enabled);
    assert_eq!(
        database
            .account_secret_refs(account.id)
            .await
            .expect("secret refs"),
        Some((Some("vault://password".to_owned()), None))
    );
}

#[tokio::test]
async fn proxy_precedence_is_job_then_account_then_global() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("precedence.sqlite"))
        .await
        .expect("database");
    let mut proxies = Vec::new();
    for name in ["global", "account", "job"] {
        proxies.push(
            database
                .create_proxy_profile(NewProxyProfile {
                    name: name.to_owned(),
                    kind: ProxyKind::Http,
                    endpoint: format!("http://{name}.example.test:8080")
                        .parse()
                        .expect("URL"),
                    username: None,
                    secret_ref: None,
                })
                .await
                .expect("proxy"),
        );
    }
    let account = database
        .create_account(NewAccount {
            provider: "premiumize".to_owned(),
            label: "account".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: Some(proxies[1].id),
            enabled: true,
        })
        .await
        .expect("account");

    let global = database
        .network_client_config(None, None, Some(proxies[0].id), SELECTION, &probe_url())
        .await
        .expect("global config");
    let account_config = database
        .network_client_config(
            Some(account.id),
            None,
            Some(proxies[0].id),
            SELECTION,
            &probe_url(),
        )
        .await
        .expect("account config");
    let job = database
        .network_client_config(
            Some(account.id),
            Some(proxies[2].id),
            Some(proxies[0].id),
            SELECTION,
            &probe_url(),
        )
        .await
        .expect("job config");

    assert_eq!(global.proxy.expect("global proxy").id, proxies[0].id);
    assert_eq!(
        account_config.proxy.expect("account proxy").id,
        proxies[1].id
    );
    assert_eq!(job.proxy.expect("job proxy").id, proxies[2].id);
}

#[tokio::test]
async fn resolver_refresh_can_only_be_claimed_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("refresh.sqlite"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "refresh".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: "https://example.test/file".parse().expect("URL"),
            file_name: "file".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    assert!(
        database
            .claim_resolver_refresh(download.id)
            .await
            .expect("first claim")
    );
    assert!(
        !database
            .claim_resolver_refresh(download.id)
            .await
            .expect("second claim")
    );
}

/// The pre-resume refresh is transient on purpose (RD-108-17).
///
/// A second resume of an expired capture claims a second slot from the windowed budget and
/// starts again from the address the person consented to — there is no write-back that could
/// hand it a renewed URL of unknown lifetime. This test fails the moment one is added.
#[tokio::test]
async fn a_second_resume_re_refreshes_instead_of_reusing_a_stored_url() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("replay-refresh.sqlite"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "replay".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let captured = rd_core::CapturedRequest {
        effective_url: Some(
            "https://cdn.example.net/f.bin?Expires=1000000000"
                .parse()
                .expect("URL"),
        ),
        method: "GET".to_owned(),
        expires_at: Some(Utc::now() - Duration::hours(2)),
        approved_origins: vec!["https://cdn.example.net".to_owned()],
        ..rd_core::CapturedRequest::default()
    };
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: "https://hoster.example/dl/1".parse().expect("URL"),
            file_name: "f.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: Some(Box::new(crate::NewReplayTemplate {
                request: captured.clone(),
                consent: rd_core::ReplayConsent {
                    granted_at: Utc::now(),
                    template_hash: "hash".to_owned(),
                    approved_origins: vec!["https://cdn.example.net".to_owned()],
                },
                body_ref: None,
                candidate_id: None,
            })),
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    // First resume: the expired capture is refreshed, which costs one slot.
    assert!(
        database
            .claim_replay_refresh(download.id)
            .await
            .expect("first claim")
    );
    let after_first = database
        .request_template(download.id)
        .await
        .expect("template")
        .expect("a template");
    assert_eq!(
        after_first.request, captured,
        "a refresh must not rewrite the consented capture"
    );

    // Second resume: nothing stored a renewed address, so the same decision is taken again
    // against the same consented capture, and a second slot is spent knowingly.
    assert!(
        database
            .claim_replay_refresh(download.id)
            .await
            .expect("second claim")
    );
    let after_second = database
        .request_template(download.id)
        .await
        .expect("template")
        .expect("a template");
    assert_eq!(after_second.request, captured);
    assert_eq!(
        after_second.request.effective_url, captured.effective_url,
        "the stored address stays the captured one across refreshes"
    );
    assert_eq!(after_second.consent.template_hash, "hash");
}

#[tokio::test]
async fn download_can_be_created_paused_without_entering_the_runnable_queue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("paused.sqlite"))
        .await
        .expect("database");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "paused".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");

    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id: package.id,
            source: "https://example.test/file".parse().expect("URL"),
            file_name: "file".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Paused,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    assert_eq!(download.state, rd_core::DownloadState::Paused);
    assert_eq!(
        database
            .get_download(download.id)
            .await
            .expect("read download")
            .expect("stored download")
            .state,
        rd_core::DownloadState::Paused
    );
}

#[tokio::test]
async fn first_resolver_version_pin_wins_and_survives_reads() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("pin.sqlite"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "pin".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: "https://example.test/file".parse().expect("URL"),
            file_name: "file".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    let first = rd_core::ResolverPin {
        plugin_id: rd_core::PluginId::new(),
        version: "1.0.0".to_owned(),
    };
    let second = rd_core::ResolverPin {
        plugin_id: rd_core::PluginId::new(),
        version: "2.0.0".to_owned(),
    };

    assert_eq!(
        database
            .claim_resolver_pin(download.id, first.clone())
            .await
            .expect("first claim"),
        first
    );
    assert_eq!(
        database
            .claim_resolver_pin(download.id, second)
            .await
            .expect("second claim"),
        first
    );
    assert_eq!(
        database.resolver_pin(download.id).await.expect("read pin"),
        Some(first.clone())
    );

    // A version this build still has keeps its pin: the whole point of pinning is that a
    // plugin upgrade cannot move a job that is already running.
    let freed = database
        .clear_unsatisfiable_resolver_pins(vec![(
            first.plugin_id.to_string(),
            first.version.clone(),
        )])
        .await
        .expect("reconcile pins");
    assert_eq!(freed, 0);
    assert_eq!(
        database.resolver_pin(download.id).await.expect("read pin"),
        Some(first.clone())
    );

    // A version that is gone would make the job unresolvable for ever, so the pin goes and
    // the job resolves through the current build of the same plugin instead.
    let freed = database
        .clear_unsatisfiable_resolver_pins(vec![(first.plugin_id.to_string(), "2.0.0".to_owned())])
        .await
        .expect("reconcile pins");
    assert_eq!(freed, 1);
    assert_eq!(
        database.resolver_pin(download.id).await.expect("read pin"),
        None
    );
}

/// Every kind writes and reads back as itself.
///
/// The read path used to be a hand-written match, so a variant added to the enum was stored
/// correctly and loaded as `Http`. Walking the whole enum means a new kind cannot be added
/// without this test seeing it.
#[test]
fn every_download_kind_survives_a_round_trip_through_the_column() {
    for kind in [
        rd_core::DownloadKind::Http,
        rd_core::DownloadKind::Usenet,
        rd_core::DownloadKind::Media,
        rd_core::DownloadKind::Gallery,
        rd_core::DownloadKind::Record,
        rd_core::DownloadKind::Torrent,
        rd_core::DownloadKind::Ftp,
        rd_core::DownloadKind::Sftp,
        rd_core::DownloadKind::Plugin,
    ] {
        let stored = serde_json::to_string(&kind).expect("serialize");
        let column = stored.trim_matches('"');
        assert_eq!(crate::models::parse_kind(column), kind, "{column}");
    }
    assert_eq!(
        crate::models::parse_kind("something-a-later-version-invented"),
        rd_core::DownloadKind::Http
    );
}

#[tokio::test]
async fn usenet_server_password_is_redacted_and_priority_is_stable() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("usenet.sqlite"))
        .await
        .expect("database");
    let created = database
        .create_usenet_server(NewUsenetServer {
            name: "Primary".to_owned(),
            host: "news.example.test".to_owned(),
            port: 563,
            tls: true,
            username: Some("reader".to_owned()),
            password_ref: Some("vault://password".to_owned()),
            proxy_profile_id: None,
            priority: 10,
            max_connections: 8,
            enabled: true,
        })
        .await
        .expect("server");
    let servers = database.list_usenet_servers().await.expect("servers");
    assert_eq!(servers[0].priority, 10);
    assert!(servers[0].has_password);
    let json = serde_json::to_value(&servers[0]).expect("JSON");
    assert!(json.get("password_ref").is_none());
    let runtime = database
        .usenet_connection_config(created.id)
        .await
        .expect("runtime config")
        .expect("server exists");
    assert_eq!(runtime.password_ref.as_deref(), Some("vault://password"));

    let updated = database
        .update_usenet_server(
            created.id,
            NewUsenetServer {
                name: "Primary updated".to_owned(),
                host: "news2.example.test".to_owned(),
                port: 119,
                tls: false,
                username: Some("reader".to_owned()),
                password_ref: runtime.password_ref,
                proxy_profile_id: None,
                priority: 20,
                max_connections: 4,
                enabled: false,
            },
        )
        .await
        .expect("update server");
    assert_eq!(updated.name, "Primary updated");
    assert_eq!(updated.priority, 20);
    assert!(updated.has_password);
    assert!(!updated.enabled);
}

#[tokio::test]
async fn category_rules_are_applied_when_links_enter_the_collector() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("routing.sqlite"))
        .await
        .expect("database");
    let root = database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            NewStorageRoot {
                name: "Downloads".to_owned(),
                path: directory.path().to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root");
    let category = database
        .create_category(NewCategory {
            name: "Hoster".to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root.id,
            relative_path: "hoster".to_owned(),
            is_default: false,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category");
    database
        .create_category_rule(NewCategoryRule {
            name: "DDownload".to_owned(),
            priority: 10,
            source: None,
            domain: Some("ddownload.com".to_owned()),
            protocol: Some("https".to_owned()),
            extension: None,
            mime_type: None,
            name_regex: None,
            category_id: category.id,
            enabled: true,
        })
        .await
        .expect("rule");
    let (_, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Clipboard,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            urls: vec!["https://ddownload.com/abc123xyz".parse().expect("URL")],
            providers: vec![None],
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(candidates[0].category_id, Some(category.id));
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].category_id, Some(category.id));
}

#[tokio::test]
async fn captured_request_metadata_survives_a_reopen_of_the_database() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("capture.sqlite");
    let request = rd_core::CapturedRequest {
        effective_url: Some("https://cdn.example.com/a/report.pdf".parse().expect("URL")),
        method: "GET".to_owned(),
        referrer: Some("https://example.com/downloads".to_owned()),
        user_agent: Some("Mozilla/5.0".to_owned()),
        content_disposition: Some("attachment; filename=\"report.pdf\"".to_owned()),
        headers: vec![rd_core::CapturedHeader {
            name: "accept".to_owned(),
            value: "*/*".to_owned(),
        }],
        ..rd_core::CapturedRequest::default()
    };
    let candidate_id = {
        let database = Database::open(path.clone()).await.expect("database");
        let (_, _, candidates) = database
            .add_collector_batch(crate::NewCollectorBatch {
                package_hints: Vec::new(),
                mirror_hints: Vec::new(),
                source: IngressSource::BrowserDownload,
                source_label: Some("Chrome".to_owned()),
                package_name: None,
                password: None,
                passwords: Vec::new(),
                category_id: None,
                priority: None,
                urls: vec!["https://files.example.com/report.pdf".parse().expect("URL")],
                providers: vec![None],
                file_names: vec![Some("report.pdf".to_owned())],
                sizes: Vec::new(),
                requests: vec![Some(request.clone())],
                body_refs: vec![None],
                auto_check: false,
                source_attributes: Vec::new(),
            })
            .await
            .expect("batch");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].request.as_ref(), Some(&request));
        candidates[0].id
    };
    // Reopening proves the metadata lives in the database, not in memory.
    let database = Database::open(path).await.expect("reopen");
    let candidates = database.list_candidates().await.expect("candidates");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, candidate_id);
    assert_eq!(candidates[0].request.as_ref(), Some(&request));
    assert_eq!(candidates[0].file_name.as_deref(), Some("report.pdf"));
}

#[tokio::test]
async fn collector_intake_groups_multipart_links_and_locks_packages_for_enqueue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = [
        "https://ddownload.com/aaa111bbb/Game.part1.rar",
        "https://ddownload.com/aaa222bbb/Game.part2.rar",
        "https://1fichier.com/?xyz",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: Some("test".to_owned()),
            package_name: None,
            password: Some("pw".to_owned()),
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 2, "archive set + loose link");
    assert_eq!(packages[0].name, "Game");
    assert!(packages[0].has_password);
    assert!(
        candidates
            .iter()
            .all(|c| c.state == rd_core::LinkCandidateState::Checking)
    );
    assert_eq!(candidates[0].position, 1);
    assert_eq!(candidates[1].position, 2);

    // A checking package cannot be enqueued; recording results unlocks it.
    assert!(
        database
            .claim_package_for_enqueue(packages[0].id, None)
            .await
            .is_err()
    );
    for candidate in &candidates {
        database
            .record_candidate_check(
                candidate.id,
                Some(rd_core::LinkCheckResult {
                    url: candidate.url.clone(),
                    status: rd_core::LinkStatus::Online,
                    file_name: Some("Renamed.part1.rar".to_owned()),
                    size: rd_core::ByteCount::new(10).ok(),
                    media: None,
                }),
                None,
                false,
                None,
            )
            .await
            .expect("record");
    }
    let listed = database.list_candidates().await.expect("candidates");
    assert!(
        listed
            .iter()
            .all(|c| c.state == rd_core::LinkCandidateState::Online)
    );
    assert_eq!(listed[0].size.map(|s| s.get()), Some(10));

    // Reorder packages: loose link first.
    database
        .reorder_collector_packages(vec![packages[1].id, packages[0].id])
        .await
        .expect("reorder");
    let ordered = database.list_collector_packages().await.expect("packages");
    assert_eq!(ordered[0].id, packages[1].id);

    // Claim locks all links atomically; failure restores the previous states.
    let claimed = database
        .claim_package_for_enqueue(packages[0].id, None)
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 2);
    assert!(
        database
            .claim_package_for_enqueue(packages[0].id, None)
            .await
            .is_err()
    );
    let restore: Vec<_> = claimed
        .iter()
        .map(|(c, previous)| (c.id, *previous))
        .collect();
    database
        .finish_package_enqueue(packages[0].id, false, restore)
        .await
        .expect("restore");
    let claimed = database
        .claim_package_for_enqueue(packages[0].id, None)
        .await
        .expect("claim again");
    database
        .finish_package_enqueue(packages[0].id, true, Vec::new())
        .await
        .expect("finish");
    assert_eq!(claimed.len(), 2);
    let remaining = database.list_collector_packages().await.expect("packages");
    assert_eq!(remaining.len(), 1, "enqueued package disappears");
    assert_eq!(remaining[0].batch_id, batch.id);
}

#[tokio::test]
async fn collector_groups_keep_the_password_of_their_own_declared_link() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector-passwords.sqlite"))
        .await
        .expect("database");
    let urls = vec![
        "https://indexer.test/get/one.nzb".parse().expect("url"),
        "https://indexer.test/get/two.nzb".parse().expect("url"),
        "https://indexer.test/get/three.nzb".parse().expect("url"),
    ];
    let (_, packages, _) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Subscription,
            source_label: Some("Indexer".to_owned()),
            package_name: None,
            password: None,
            passwords: vec![
                Some("first-secret".to_owned()),
                Some("second-secret".to_owned()),
                None,
            ],
            category_id: None,
            priority: None,
            providers: vec![Some(rd_core::NZB_PROVIDER.to_owned()); 3],
            urls,
            file_names: vec![
                Some("First release".to_owned()),
                Some("Second release".to_owned()),
                Some("No password release".to_owned()),
            ],
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");

    assert_eq!(packages.len(), 3);
    let password = |name: &str| {
        packages
            .iter()
            .find(|package| package.name == name)
            .and_then(|package| package.password.as_deref())
    };
    assert_eq!(password("First release"), Some("first-secret"));
    assert_eq!(password("Second release"), Some("second-secret"));
    assert_eq!(password("No password release"), None);
}

/// One statement per changed column, one re-read, and the order the caller asked for.
///
/// The per-id loop this replaced issued up to eight statements plus a read for every package,
/// which is what made a bulk edit of a few hundred packages block the single writer.
#[tokio::test]
async fn updating_many_collector_packages_returns_them_in_the_requested_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector-bulk.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let category = routing_category(&database, root, "Movies", true).await;
    let urls: Vec<url::Url> = [
        "https://ddownload.com/aaa111bbb/Game.part1.rar",
        "https://ddownload.com/aaa222bbb/Game.part2.rar",
        "https://1fichier.com/?xyz",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (_, packages, _) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 2, "archive set + loose link");

    // Reversed, plus an id nobody has: the reply must follow the request and skip the stranger.
    let updated = database
        .update_collector_packages(
            vec![
                packages[1].id,
                packages[0].id,
                rd_core::CollectorPackageId::new(),
            ],
            crate::CollectorPackageChange {
                category_id: Some(Some(category.id)),
                priority: Some(rd_core::DownloadPriority::High),
                ..crate::CollectorPackageChange::default()
            },
        )
        .await
        .expect("update");

    assert_eq!(
        updated.iter().map(|package| package.id).collect::<Vec<_>>(),
        vec![packages[1].id, packages[0].id]
    );
    assert!(
        updated
            .iter()
            .all(|package| package.category_id == Some(category.id)
                && package.priority == rd_core::DownloadPriority::High)
    );
    // The candidates carry the same routing, which is what the second statement per column does.
    let candidates = database.list_candidates().await.expect("candidates");
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.category_id == Some(category.id)
                && candidate.priority == rd_core::DownloadPriority::High)
    );
}

/// Regrouping reads and groups outside the write transaction and still regroups.
///
/// The names the online check revealed split one auto-named package into two; the write phase
/// has to create the missing package, move the candidates and drop what is left empty.
#[tokio::test]
async fn regrouping_splits_a_batch_once_the_check_revealed_the_real_names() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector-regroup.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = [
        "https://ddownload.com/aaa111bbb/one",
        "https://ddownload.com/aaa222bbb/two",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(
        packages.len(),
        1,
        "two nameless links share one auto-named package"
    );
    for (candidate, name) in candidates
        .iter()
        .zip(["Movie.part1.rar", "Series.S01E01.mkv"])
    {
        database
            .record_candidate_check(
                candidate.id,
                Some(rd_core::LinkCheckResult {
                    url: candidate.url.clone(),
                    status: rd_core::LinkStatus::Online,
                    file_name: Some(name.to_owned()),
                    size: None,
                    media: None,
                }),
                None,
                false,
                None,
            )
            .await
            .expect("record");
    }

    database
        .regroup_collector_batches(vec![batch.id])
        .await
        .expect("regroup");

    let regrouped = database.list_collector_packages().await.expect("packages");
    assert_eq!(regrouped.len(), 2, "one release per revealed name");
    assert!(
        regrouped.iter().any(|package| package.name == "Movie"),
        "the archive set is named after its base name, got {:?}",
        regrouped
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>()
    );
    let candidates = database.list_candidates().await.expect("candidates");
    let package_ids: std::collections::BTreeSet<_> = candidates
        .iter()
        .filter_map(|candidate| candidate.package_id)
        .collect();
    assert_eq!(package_ids.len(), 2, "the two links no longer share one");
}

/// The downmagaz case of RD-120-17, in the order it actually happens.
///
/// A site rule reads the release title off the page and hands it to every link it found as
/// `package_hint`, so intake names the package after it. The online check then runs, one of
/// the links sits on a service with no resolver and therefore never gets a file name, and the
/// check ends -- always -- in a regroup of the batch.
///
/// That regroup was where the name went. It re-derived every auto-named package from the file
/// names then known, with no hint to go on, so the stated title was replaced: by the common
/// stem when enough names had arrived, and otherwise by the host of the first link without
/// one -- which is why it looked as though the unsupported hoster had renamed the package.
/// It had not; it had only supplied the fallback. A package the source named is no longer
/// auto-named, so the regroup does not reach it.
///
/// The two addresses are the ones RD-120-19 measured on the reported page
/// (`docs/adr/0019-the-address-a-board-shows-is-not-the-hoster.md`): the page carries exactly
/// these two and no others. Neither is a hoster -- both are affiliate link-cloakers -- so the
/// reporter's suspicion that an unsupported hoster renamed the package is wrong twice over.
/// `nfile.cc` hides `novafile.org`, which nothing claims, so the check answers `unsupported`.
/// `dwp.la` hides `downup.me`, which `plugins/xfs-generic` already claims; its file name is
/// what the check returns for it once the redirect is followed, which is RD-120-19's subject.
/// Until it is, that link has no name either, and the regroup reaches the same fallback one
/// step earlier -- a single loose name yields no common stem, so both orders end at
/// `nfile.cc`, the host of the first loose link, which is the name the owner saw.
#[tokio::test]
async fn a_package_name_a_site_rule_stated_survives_the_regroup_after_the_check() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector-hint.sqlite"))
        .await
        .expect("database");
    const TITLE: &str = "The Economist USA 09.19.2026";
    let urls: Vec<url::Url> = [
        // The link the reporter suspected. Nothing claims what it hides, so the check ends in
        // `unsupported` and this candidate never gets a file name -- and, being first, it is
        // the host the fallback would have used.
        "https://nfile.cc/qK7XDAwq",
        "https://dwp.la/d/dro",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            // What `rd_plugin_ext::FolderCrawlers` puts on every link a rule produced.
            package_hints: vec![Some(TITLE.to_owned()); urls.len()],
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            // No explicit name: the rule's title is the only one there is, which is exactly
            // the case that used to be treated as a guess.
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 1, "one page, one package");
    assert_eq!(packages[0].name, TITLE, "intake takes the rule's title");
    assert!(
        !packages[0].auto_named,
        "a name the source stated is not a guess"
    );

    for (candidate, name) in candidates.iter().zip([
        None,
        Some("The_Economist_USA_-_19_September_2026_downmagaz.net.pdf"),
    ]) {
        match name {
            Some(name) => database
                .record_candidate_check(
                    candidate.id,
                    Some(rd_core::LinkCheckResult {
                        url: candidate.url.clone(),
                        status: rd_core::LinkStatus::Online,
                        file_name: Some(name.to_owned()),
                        size: None,
                        media: None,
                    }),
                    None,
                    false,
                    None,
                )
                .await
                .expect("record"),
            None => database
                .mark_candidate_unsupported(
                    candidate.id,
                    rd_core::CandidateMessage::coded(
                        "collector.check_unknown",
                        "no service can check this address",
                    ),
                    None,
                )
                .await
                .expect("unsupported"),
        }
    }

    database
        .regroup_collector_batches(vec![batch.id])
        .await
        .expect("regroup");

    let after = database.list_collector_packages().await.expect("packages");
    assert_eq!(
        after.len(),
        1,
        "the batch is still one package, got {after:?}"
    );
    assert_eq!(
        after[0].name, TITLE,
        "the regroup must not rename what the rule stated"
    );
    let candidates = database.list_candidates().await.expect("candidates");
    assert_eq!(candidates.len(), 2);
    for candidate in &candidates {
        assert_eq!(
            candidate.package_id,
            Some(after[0].id),
            "every link stays in the package the rule named"
        );
    }
}

/// The getcomics case of RD-120-17: the same loss, with not one file name to fall back on.
///
/// The title and the addresses are the ones the shipped rule reads out of the recorded page
/// `crates/rd-siterules/tests/fixtures/getcomics-release.html`; that the rule produces exactly
/// these is what `rd-siterules`' `a_getcomics_post_becomes_a_package_of_hoster_links` asserts,
/// and this case takes them from there and carries them the rest of the way.
///
/// The rule yields six addresses out of that page; the three used here are its file hosts,
/// in the order the fixture carries them. The check named none of them -- which is the case
/// the owner saw -- so the regroup had no common stem either and fell straight through to the
/// host of the first link: the comic's title became `1024terabox.com`. From the outside that
/// reads as "the name did not come through at all", which is how the owner reported it, but
/// it is the same regroup as the downmagaz case. Had the check read a name out of the
/// `datanodes.to` path instead, the package would have been renamed to that file's stem --
/// still not the comic's title, and still the same line.
#[tokio::test]
async fn a_getcomics_package_keeps_the_title_when_no_link_reveals_a_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("collector-getcomics.sqlite"))
        .await
        .expect("database");
    const TITLE: &str = "Absolute Green Arrow #5 (2026)";
    let urls: Vec<url::Url> = [
        "https://1024terabox.com/s/13Fmfzo7LL0FMZ8j1l4ANGw",
        "https://vikingfile.com/f/7hJu75ACQL",
        "https://datanodes.to/6ass8jkpo3j9/Absolute_Green_Arrow_005_(2026)_(digital)_(Pyrate-DCP).cbz",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: vec![Some(TITLE.to_owned()); urls.len()],
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name, TITLE);

    // The check answers for all three and names none of them, which is what these hosters do.
    for candidate in &candidates {
        database
            .record_candidate_check(
                candidate.id,
                Some(rd_core::LinkCheckResult {
                    url: candidate.url.clone(),
                    status: rd_core::LinkStatus::Online,
                    file_name: None,
                    size: None,
                    media: None,
                }),
                None,
                false,
                None,
            )
            .await
            .expect("record");
    }
    database
        .regroup_collector_batches(vec![batch.id])
        .await
        .expect("regroup");

    let after = database.list_collector_packages().await.expect("packages");
    assert_eq!(after.len(), 1, "got {after:?}");
    assert_eq!(
        after[0].name, TITLE,
        "the comic's title, not the first address's host name"
    );
}

#[tokio::test]
async fn nzb_hash_dedup_preserves_routing_metadata() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb.sqlite"))
        .await
        .expect("database");
    let first = database
        .add_nzb_import(NewNzbImport {
            name: "test.nzb".to_owned(),
            sha256: "ab".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: Some("/watch/test.nzb".to_owned()),
            password: None,
            announce_arrival: true,
            files: Vec::new(),
        })
        .await
        .expect("first import");
    let duplicate = database
        .add_nzb_import(NewNzbImport {
            name: "copy.nzb".to_owned(),
            sha256: "ab".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Review,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: Vec::new(),
        })
        .await
        .expect("duplicate import");
    assert_eq!(first.state, rd_core::NzbImportState::Imported);
    assert_eq!(duplicate.import_mode, ImportMode::Enqueue);
    assert!(duplicate.duplicate);
}

/// Storage root for the category-routing tests; the path itself is never written to.
async fn routing_root(database: &Database, directory: &std::path::Path) -> rd_core::StorageRootId {
    database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            NewStorageRoot {
                name: "Downloads".to_owned(),
                path: directory.to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("storage root")
        .id
}

async fn routing_category(
    database: &Database,
    root_id: rd_core::StorageRootId,
    name: &str,
    is_default: bool,
) -> rd_core::Category {
    database
        .create_category(NewCategory {
            name: name.to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root_id,
            relative_path: name.to_lowercase(),
            is_default,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category")
}

/// A rule that only an NZB arriving through the given door can match.
async fn nzb_rule(database: &Database, source: IngressSource, category_id: rd_core::CategoryId) {
    database
        .create_category_rule(NewCategoryRule {
            name: "NZB files".to_owned(),
            priority: 10,
            source: Some(source),
            domain: None,
            protocol: None,
            extension: Some("nzb".to_owned()),
            mime_type: None,
            name_regex: None,
            category_id,
            enabled: true,
        })
        .await
        .expect("rule");
}

fn dropped_nzb(
    name: &str,
    sha256: &str,
    category_id: Option<rd_core::CategoryId>,
    source: IngressSource,
    source_path: Option<&str>,
) -> NewNzbImport {
    NewNzbImport {
        name: name.to_owned(),
        sha256: sha256.to_owned(),
        category_id,
        source,
        priority: None,
        import_mode: ImportMode::Review,
        source_path: source_path.map(str::to_owned),
        password: None,
        announce_arrival: true,
        files: vec![NewNzbFile {
            subject: "payload.bin".to_owned(),
            poster: "poster".to_owned(),
            groups: vec!["alt.binaries.test".to_owned()],
            segments: vec![NewNzbSegment {
                number: 1,
                bytes: 128,
                message_id: "payload-1@example.test".to_owned(),
            }],
        }],
    }
}

/// A folder that names a category has decided; neither a matching rule nor the default
/// category may talk it out of that.
#[tokio::test]
async fn a_hotfolder_nzb_keeps_the_category_the_folder_chose() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-explicit.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let chosen = routing_category(&database, root, "Chosen", false).await;
    let by_rule = routing_category(&database, root, "ByRule", false).await;
    routing_category(&database, root, "Fallback", true).await;
    nzb_rule(&database, IngressSource::HotFolder, by_rule.id).await;

    let import = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &"a1".repeat(32),
            Some(chosen.id),
            IngressSource::HotFolder,
            Some("/watch/release.nzb"),
        ))
        .await
        .expect("import");

    assert_eq!(import.category_id, Some(chosen.id));
}

/// The reported defect: a folder without a category left the NZB with none at all, so a rule
/// for `source = hotfolder`, `extension = nzb` could never fire.
#[tokio::test]
async fn an_uncategorised_hotfolder_nzb_takes_the_category_of_a_matching_rule() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-rule.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let by_rule = routing_category(&database, root, "ByRule", false).await;
    routing_category(&database, root, "Fallback", true).await;
    nzb_rule(&database, IngressSource::HotFolder, by_rule.id).await;

    let import = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &"a2".repeat(32),
            None,
            IngressSource::HotFolder,
            Some("/watch/release.nzb"),
        ))
        .await
        .expect("import");
    assert_eq!(import.category_id, Some(by_rule.id));

    // The badge the package shows is the point of the whole exercise.
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("byrule"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    assert_eq!(package.category_id, Some(by_rule.id));
}

fn broken_nzb(name: &str, sha256: &str, error: &str) -> FailedNzbImport {
    FailedNzbImport {
        name: name.to_owned(),
        sha256: sha256.to_owned(),
        source_path: Some(format!("/watch/failed/{name}")),
        error: error.to_owned(),
    }
}

/// RD-108-20: the drop that used to vanish. An NZB nobody threw in by hand could not be
/// parsed, and produced no row at all - so the LinkGrabber showed nothing and the only trace
/// was a log line and a file under `failed/`.
#[tokio::test]
async fn an_unreadable_hotfolder_nzb_is_listed_with_its_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-failed.sqlite"))
        .await
        .expect("database");

    let recorded = database
        .record_nzb_import_failure(broken_nzb(
            "broken.nzb",
            &"f1".repeat(32),
            "NZB could not be parsed",
        ))
        .await
        .expect("failure recorded");

    assert_eq!(recorded.state, rd_core::NzbImportState::Failed);
    assert_eq!(recorded.error.as_deref(), Some("NZB could not be parsed"));

    let listed = database.list_nzb_imports().await.expect("list");
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].id, recorded.id);
    assert_eq!(listed[0].name, "broken.nzb");
    assert_eq!(listed[0].state, rd_core::NzbImportState::Failed);
    assert_eq!(listed[0].error.as_deref(), Some("NZB could not be parsed"));
    assert_eq!(
        listed[0].source_path.as_deref(),
        Some("/watch/failed/broken.nzb")
    );
    assert_eq!(listed[0].file_count, 0);

    // Nothing to queue: the row says what went wrong, it is not a candidate.
    let refused = database
        .enqueue_nzb_import(
            recorded.id,
            directory.path().join("downloads"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect_err("enqueue refused");
    assert_eq!(
        crate::store_kind(&refused),
        Some(crate::StoreErrorKind::WrongState),
        "{refused:#}"
    );

    // And it can be cleared, which is what makes room for the file to be imported again.
    database
        .delete_nzb_import(recorded.id)
        .await
        .expect("delete");
    assert!(database.list_nzb_imports().await.expect("list").is_empty());
}

/// The same bytes are the same drop: the reconciliation pass that sees the file again must
/// update the reason, not stack up a second row - `sha256` is unique, so a blind insert would
/// fail outright.
#[tokio::test]
async fn the_same_broken_nzb_arriving_again_updates_the_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-failed-twice.sqlite"))
        .await
        .expect("database");
    let sha256 = "f2".repeat(32);

    let first = database
        .record_nzb_import_failure(broken_nzb("broken.nzb", &sha256, "NZB could not be parsed"))
        .await
        .expect("first failure");
    let second = database
        .record_nzb_import_failure(broken_nzb("broken.nzb", &sha256, "storage root is full"))
        .await
        .expect("second failure");

    assert_eq!(first.id, second.id);
    let listed = database.list_nzb_imports().await.expect("list");
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].error.as_deref(), Some("storage root is full"));
}

/// An import that already became a package worked; a later refusal of the same file - a second
/// drop of a copy, say - must not retract that and mark the package's origin failed.
#[tokio::test]
async fn a_queued_import_is_not_marked_failed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-failed-queued.sqlite"))
        .await
        .expect("database");
    let sha256 = "f3".repeat(32);
    let import = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &sha256,
            None,
            IngressSource::HotFolder,
            Some("/watch/release.nzb"),
        ))
        .await
        .expect("import");
    database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("downloads"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let unchanged = database
        .record_nzb_import_failure(broken_nzb("release.nzb", &sha256, "late refusal"))
        .await
        .expect("recorded");

    assert_eq!(unchanged.state, rd_core::NzbImportState::Enqueued);
    assert_eq!(unchanged.error, None);
}

/// No folder category and no rule that fits: the category marked as default, exactly as a
/// pasted link gets it.
#[tokio::test]
async fn an_uncategorised_hotfolder_nzb_falls_back_to_the_default_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-default.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let fallback = routing_category(&database, root, "Fallback", true).await;
    let uploads = routing_category(&database, root, "Uploads", false).await;
    // A rule that cannot match this drop: it is about uploads, not about watched folders.
    nzb_rule(&database, IngressSource::Manual, uploads.id).await;

    let import = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &"a3".repeat(32),
            None,
            IngressSource::HotFolder,
            Some("/watch/release.nzb"),
        ))
        .await
        .expect("import");

    assert_eq!(import.category_id, Some(fallback.id));
}

/// The upload has no path on disk and no folder behind it, and still goes the same way.
#[tokio::test]
async fn an_uploaded_nzb_is_routed_like_a_drop() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-upload.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let by_rule = routing_category(&database, root, "Uploads", false).await;
    routing_category(&database, root, "Fallback", true).await;
    nzb_rule(&database, IngressSource::Manual, by_rule.id).await;

    let import = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &"a4".repeat(32),
            None,
            IngressSource::Manual,
            None,
        ))
        .await
        .expect("import");

    assert_eq!(import.category_id, Some(by_rule.id));
}

/// The same file dropped into a different folder is routed again instead of keeping what the
/// first drop decided.
#[tokio::test]
async fn an_nzb_dropped_again_adopts_the_category_of_the_second_drop() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-redrop.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let first_folder = routing_category(&database, root, "Movies", false).await;
    let second_folder = routing_category(&database, root, "Series", false).await;
    let sha256 = "a5".repeat(32);

    let first = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &sha256,
            Some(first_folder.id),
            IngressSource::HotFolder,
            Some("/watch/movies/release.nzb"),
        ))
        .await
        .expect("first import");
    assert_eq!(first.category_id, Some(first_folder.id));

    let again = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &sha256,
            Some(second_folder.id),
            IngressSource::HotFolder,
            Some("/watch/series/release.nzb"),
        ))
        .await
        .expect("second import");

    assert!(again.duplicate);
    assert_eq!(again.id, first.id);
    assert_eq!(again.category_id, Some(second_folder.id));
    assert_eq!(
        database
            .list_nzb_imports()
            .await
            .expect("imports")
            .into_iter()
            .find(|import| import.id == first.id)
            .and_then(|import| import.category_id),
        Some(second_folder.id),
        "the stored row, not just the returned copy, carries the new category"
    );
}

#[tokio::test]
async fn nzb_routing_metadata_can_change_until_enqueue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-routing.sqlite"))
        .await
        .expect("database");
    let root = database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            NewStorageRoot {
                name: "Downloads".to_owned(),
                path: directory.path().to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root");
    let category = database
        .create_category(NewCategory {
            name: "Changed".to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root.id,
            relative_path: "changed".to_owned(),
            is_default: false,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "routing.nzb".to_owned(),
            sha256: "ac".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: Some(rd_core::DownloadPriority::Low),
            import_mode: ImportMode::Review,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "routing.bin".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: 128,
                    message_id: "routing-1@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");

    let updated = database
        .update_nzb_import(
            import.id,
            NzbImportChange {
                category_id: Some(Some(category.id)),
                priority: Some(rd_core::DownloadPriority::High),
            },
        )
        .await
        .expect("update import");
    assert_eq!(updated.category_id, Some(category.id));
    assert_eq!(updated.priority, Some(rd_core::DownloadPriority::High));

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("changed"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    assert_eq!(package.category_id, Some(category.id));
    assert_eq!(package.priority, rd_core::DownloadPriority::High);
    assert!(
        database
            .update_nzb_import(
                import.id,
                NzbImportChange {
                    category_id: Some(None),
                    priority: None,
                },
            )
            .await
            .is_err(),
        "an enqueued import is edited through its download package instead"
    );
}

/// RD-107-09: "add paused" has to reach the rows an NZB import creates.
///
/// Before this, `enqueue_import` wrote the literal `'queued'` for every download row, so the
/// scheduler picked a "paused" NZB up straight away. The package row itself stays `queued` —
/// that is what the collector path does too, and the dispatcher keys off the download state.
#[tokio::test]
async fn enqueuing_an_nzb_import_paused_creates_paused_download_rows() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("paused.sqlite"))
        .await
        .expect("database");

    let import = |name: &str, digest: &str, message: &str| NewNzbImport {
        name: name.to_owned(),
        sha256: digest.repeat(32),
        category_id: None,
        priority: None,
        import_mode: ImportMode::Enqueue,
        source: IngressSource::Manual,
        source_path: None,
        password: None,
        announce_arrival: false,
        files: vec![NewNzbFile {
            subject: format!("{name}.bin"),
            poster: "poster".to_owned(),
            groups: vec!["alt.binaries.test".to_owned()],
            segments: vec![NewNzbSegment {
                number: 1,
                bytes: 128,
                message_id: format!("{message}@example.test"),
            }],
        }],
    };

    let paused_import = database
        .add_nzb_import(import("paused.nzb", "a1", "paused-1"))
        .await
        .expect("import");
    let paused_package = database
        .enqueue_nzb_import(
            paused_import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            true,
        )
        .await
        .expect("enqueue paused");

    let started_import = database
        .add_nzb_import(import("started.nzb", "b2", "started-1"))
        .await
        .expect("import");
    let started_package = database
        .enqueue_nzb_import(
            started_import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue started");

    let downloads = database.list_downloads().await.expect("downloads");
    let states = |package: PackageId| {
        downloads
            .iter()
            .filter(|file| file.package_id == package)
            .map(|file| file.state)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        states(paused_package.id),
        vec![rd_core::DownloadState::Paused],
        "an NZB enqueued paused must not be dispatchable"
    );
    assert_eq!(
        states(started_package.id),
        vec![rd_core::DownloadState::Queued],
        "the ordinary path still starts immediately"
    );
    assert_eq!(
        paused_package.state,
        rd_core::PackageState::Queued,
        "the package row stays queued, exactly as the collector path leaves it"
    );
}

#[tokio::test]
async fn forgetting_import_history_removes_the_import_but_keeps_the_package() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("forget.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "history.nzb".to_owned(),
            sha256: "ef".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "history.bin".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: 128,
                    message_id: "history-1@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    database
        .forget_nzb_import_history(package.id)
        .await
        .expect("forget history");

    assert!(
        database
            .list_nzb_imports()
            .await
            .expect("imports")
            .is_empty()
    );
    assert!(
        database
            .list_packages()
            .await
            .expect("packages")
            .iter()
            .any(|candidate| candidate.id == package.id)
    );
    // Without an import link the call is a no-op instead of an error.
    database
        .forget_nzb_import_history(package.id)
        .await
        .expect("idempotent");
}

#[tokio::test]
async fn nzb_segment_attempt_and_crc_are_persistent() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("segments.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "segments.nzb".to_owned(),
            sha256: "cd".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "file.bin".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: 128,
                    message_id: "part-1@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");
    let files = database.list_nzb_files(import.id).await.expect("files");
    let segment_id = files[0].segments[0].id;
    database
        .set_nzb_segment_state(segment_id, NzbSegmentState::Downloading, None)
        .await
        .expect("attempt");
    database
        .set_nzb_segment_state(segment_id, NzbSegmentState::Completed, Some(0x1234_abcd))
        .await
        .expect("completion");

    let files = database.list_nzb_files(import.id).await.expect("files");
    let segment = &files[0].segments[0];
    assert_eq!(segment.state, NzbSegmentState::Completed);
    assert_eq!(segment.server_attempts, 1);
    assert_eq!(segment.crc32.as_deref(), Some("1234abcd"));
}

#[tokio::test]
async fn usenet_file_and_postprocess_checkpoints_survive_recovery() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("checkpoints.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "checkpoint.nzb".to_owned(),
            sha256: "fe".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "archive.zip".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: 42,
                    message_id: "checkpoint@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");
    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    database
        .checkpoint_nzb_file_output(file.id, "/downloads/archive.zip".to_owned())
        .await
        .expect("file checkpoint");
    database
        .checkpoint_postprocess(
            import.id.to_string(),
            PostprocessKind::ExtractZip,
            "/downloads/archive.zip".to_owned(),
            PostprocessState::Running,
            Some("/downloads/archive".to_owned()),
            None,
        )
        .await
        .expect("postprocess checkpoint");
    database.recover_interrupted().await.expect("recovery");

    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    assert_eq!(file.output_path.as_deref(), Some("/downloads/archive.zip"));
    let steps = database
        .list_postprocess_steps(&import.id.to_string())
        .await
        .expect("steps");
    assert_eq!(steps[0].state, PostprocessState::Queued);
    assert_eq!(steps[0].output_path.as_deref(), Some("/downloads/archive"));
}

#[tokio::test]
async fn packages_and_downloads_follow_priority_then_manual_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("queue-order.sqlite"))
        .await
        .expect("database");
    let mut ids = Vec::new();
    for (name, priority) in [
        ("first-normal", rd_core::DownloadPriority::Normal),
        ("high", rd_core::DownloadPriority::High),
        ("second-normal", rd_core::DownloadPriority::Normal),
        ("low", rd_core::DownloadPriority::Low),
    ] {
        let package = database
            .create_package(NewPackage {
                id: PackageId::new(),
                name: name.to_owned(),
                destination: directory.path().to_string_lossy().into_owned(),
                category_id: None,
                priority,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        database
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id: package.id,
                source: format!("https://example.test/{name}").parse().expect("URL"),
                file_name: name.to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: AuthProfileSelection::Auto,
                initial_state: rd_core::DownloadState::Queued,
                kind: rd_core::DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                replay: None,
                mirror_group: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download");
        ids.push(package.id);
    }
    let names = |packages: Vec<rd_core::DownloadPackage>| {
        packages
            .into_iter()
            .map(|package| package.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(database.list_packages().await.expect("packages")),
        ["high", "first-normal", "second-normal", "low"]
    );
    let files = database.list_downloads().await.expect("downloads");
    assert_eq!(
        files
            .iter()
            .map(|file| file.file_name.as_str())
            .collect::<Vec<_>>(),
        ["high", "first-normal", "second-normal", "low"]
    );

    // Manual order inside the normal tier: second-normal before first-normal.
    database
        .reorder_packages(vec![ids[1], ids[2], ids[0], ids[3]])
        .await
        .expect("reorder");
    assert_eq!(
        names(database.list_packages().await.expect("packages")),
        ["high", "second-normal", "first-normal", "low"]
    );

    // Priority beats manual position; a category change writes the destination the caller
    // resolved for that one package and remembers where its data used to be.
    let previous = database.list_packages().await.expect("packages");
    let previous = previous
        .iter()
        .find(|package| package.id == ids[3])
        .expect("package")
        .destination
        .clone();
    let updated = database
        .update_packages(
            vec![ids[3]],
            crate::PackageChange {
                category: Some(crate::CategoryAssignment {
                    category_id: None,
                    destinations: std::collections::HashMap::from([(
                        ids[3],
                        "/tmp/elsewhere/low".to_owned(),
                    )]),
                }),
                priority: Some(rd_core::DownloadPriority::High),
                name: None,
                password: None,
                postprocess_level: None,
                script: None,
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].priority, rd_core::DownloadPriority::High);
    assert_eq!(updated[0].destination, "/tmp/elsewhere/low");
    assert_eq!(
        database
            .package_previous_destination(ids[3])
            .await
            .expect("previous destination"),
        Some(previous),
        "the former directory is kept until it has been swept"
    );
    database
        .clear_package_previous_destination(ids[3])
        .await
        .expect("clear");
    assert_eq!(
        database
            .package_previous_destination(ids[3])
            .await
            .expect("previous destination"),
        None,
        "and is forgotten once the sweep has run"
    );
    assert_eq!(
        names(database.list_packages().await.expect("packages")),
        ["high", "low", "second-normal", "first-normal"]
    );
}

/// External runners report progress in throttled samples and may finish without a final one,
/// which used to leave a completed download stuck at a partial percentage in the UI.
#[tokio::test]
async fn completing_a_download_reports_it_as_fully_transferred() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("complete.sqlite"))
        .await
        .expect("database");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "media".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id: package.id,
            source: "https://example.test/clip".parse().expect("URL"),
            file_name: "clip.mp4".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Media,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    // The last sample yt-dlp produced before it finished.
    database
        .set_download_progress(download.id, 28_839_279, Some(40_391_148))
        .await
        .expect("progress");
    for next in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
    ] {
        database
            .transition_download(download.id, next)
            .await
            .expect("transition");
    }
    let completed = database
        .complete_download(download.id, "clip.mp4".to_owned(), None)
        .await
        .expect("complete");

    assert_eq!(completed.state, rd_core::DownloadState::Completed);
    assert_eq!(
        completed.committed_bytes.get(),
        completed.total_bytes.expect("total").get(),
        "a completed download must report 100 %"
    );
    assert_eq!(completed.committed_bytes.get(), 40_391_148);
}

/// A stalled sample can overshoot the announced total; the completed row must not claim more
/// bytes than it reports as the size.
#[tokio::test]
async fn completing_a_download_absorbs_an_overshooting_sample() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("overshoot.sqlite"))
        .await
        .expect("database");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "http".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id: package.id,
            source: "https://example.test/file.bin".parse().expect("URL"),
            file_name: "file.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    database
        .set_download_progress(download.id, 90_331_545, Some(87_450_419))
        .await
        .expect("progress");
    for next in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
    ] {
        database
            .transition_download(download.id, next)
            .await
            .expect("transition");
    }

    let completed = database
        .complete_download(download.id, "file.bin".to_owned(), None)
        .await
        .expect("complete");
    assert_eq!(
        completed.committed_bytes.get(),
        completed.total_bytes.expect("total").get()
    );
    assert_eq!(completed.committed_bytes.get(), 90_331_545);
}

#[tokio::test]
async fn token_scopes_separate_capture_from_api_access() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("tokens.sqlite"))
        .await
        .expect("database");
    let capture_sha = "aa".repeat(32);
    let api_sha = "bb".repeat(32);
    database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "Browser".to_owned(),
            capture_sha.clone(),
            vec![rd_core::CAPTURE_SCOPE.to_owned()],
        )
        .await
        .expect("capture token");
    let api_token = database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "Assistant".to_owned(),
            api_sha.clone(),
            vec![rd_core::API_SCOPE.to_owned()],
        )
        .await
        .expect("api token");

    let valid = |sha: String, scope: &'static str| {
        let database = database.clone();
        async move {
            database
                .capture_token_valid(&sha, scope)
                .await
                .expect("check")
        }
    };
    assert!(valid(capture_sha.clone(), rd_core::CAPTURE_SCOPE).await);
    assert!(!valid(capture_sha.clone(), rd_core::API_SCOPE).await);
    assert!(valid(api_sha.clone(), rd_core::API_SCOPE).await);
    assert!(!valid(api_sha.clone(), rd_core::CAPTURE_SCOPE).await);
    // Full API access covers the read-only surface, so one token is enough for a client
    // that both acts and reads; the reverse never holds.
    assert!(valid(api_sha.clone(), rd_core::API_READ_SCOPE).await);
    assert!(!valid(capture_sha.clone(), rd_core::API_READ_SCOPE).await);

    let capture_list = database
        .list_capture_tokens(&[rd_core::CAPTURE_SCOPE])
        .await
        .expect("capture list");
    let api_list = database
        .list_capture_tokens(&[rd_core::API_SCOPE])
        .await
        .expect("api list");
    assert_eq!(capture_list.len(), 1);
    assert_eq!(capture_list[0].label, "Browser");
    assert_eq!(api_list.len(), 1);
    assert_eq!(api_list[0].label, "Assistant");

    database
        .revoke_capture_token(api_token.id)
        .await
        .expect("revoke");
    assert!(!valid(api_sha, rd_core::API_SCOPE).await);
}

#[tokio::test]
async fn auth_profiles_survive_a_restart_and_keep_matching() {
    // The acceptance criterion is that import and use work unchanged after a server
    // restart, so the database is closed and reopened from the same directory.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("auth.sqlite");
    let created = {
        let database = Database::open(path.clone()).await.expect("database");
        database
            .create_auth_profile(new_profile("intranet", "https://files.example.com/", false))
            .await
            .expect("profile")
    };

    let database = Database::open(path).await.expect("reopen");
    let matched = database
        .match_auth_profile(&"https://files.example.com/report.pdf".parse().expect("url"))
        .await
        .expect("match")
        .expect("profile still matches after restart");
    assert_eq!(matched.id, created.id);
    assert_eq!(matched.secret_ref.as_deref(), Some("vault://intranet"));
}

#[tokio::test]
async fn auto_match_prefers_the_most_specific_scope() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    database
        .create_auth_profile(new_profile("wide", "example.com", true))
        .await
        .expect("wide");
    let narrow = database
        .create_auth_profile(new_profile("narrow", "https://cdn.example.com/", false))
        .await
        .expect("narrow");

    let matched = database
        .match_auth_profile(&"https://cdn.example.com/f.bin".parse().expect("url"))
        .await
        .expect("match")
        .expect("profile");
    assert_eq!(matched.id, narrow.id, "more host labels must win");
}

#[tokio::test]
async fn auto_match_skips_disabled_and_expired_profiles() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let mut expiring = new_profile("expired", "expired.example", false);
    expiring.expires_at = Some(Utc::now() - Duration::hours(1));
    database
        .create_auth_profile(expiring)
        .await
        .expect("create");
    let disabled = database
        .create_auth_profile(new_profile("disabled", "disabled.example", false))
        .await
        .expect("create");
    database
        .set_auth_profile_enabled(disabled.id, false)
        .await
        .expect("disable");

    for host in ["https://expired.example/f", "https://disabled.example/f"] {
        assert!(
            database
                .match_auth_profile(&host.parse().expect("url"))
                .await
                .expect("match")
                .is_none(),
            "{host}"
        );
    }
}

#[tokio::test]
async fn a_pinned_profile_fails_the_job_instead_of_downloading_unauthenticated() {
    // Silently continuing without the credential would write a login page over the file.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let profile = database
        .create_auth_profile(new_profile("pinned", "example.com", false))
        .await
        .expect("create");
    let url = probe_url();

    let resolved = database
        .network_client_config(
            None,
            None,
            None,
            AuthProfileSelection::Pinned(profile.id),
            &url,
        )
        .await
        .expect("pinned config");
    assert_eq!(resolved.auth.expect("profile").id, profile.id);

    database
        .set_auth_profile_enabled(profile.id, false)
        .await
        .expect("disable");
    assert!(
        database
            .network_client_config(
                None,
                None,
                None,
                AuthProfileSelection::Pinned(profile.id),
                &url,
            )
            .await
            .is_err(),
        "a disabled pinned profile must fail the job"
    );

    // Explicitly asking for no profile stays silent even where one would match.
    let none = database
        .network_client_config(None, None, None, AuthProfileSelection::None, &url)
        .await
        .expect("none config");
    assert!(none.auth.is_none());
}

#[tokio::test]
async fn a_pinned_profile_outside_its_scope_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let profile = database
        .create_auth_profile(new_profile("scoped", "example.com", false))
        .await
        .expect("create");
    assert!(
        database
            .network_client_config(
                None,
                None,
                None,
                AuthProfileSelection::Pinned(profile.id),
                &"https://other.tld/f".parse().expect("url"),
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn captured_profiles_are_never_created_enabled() {
    // A capture token lives in a browser; it must not be able to mint a usable credential.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let mut input = new_profile("captured", "example.com", false);
    input.origin = AuthOrigin::BrowserCapture;
    input.enabled = true;
    let profile = database.create_auth_profile(input).await.expect("create");
    assert!(!profile.enabled, "capture intake must land disabled");
    assert!(
        database
            .match_auth_profile(&probe_url())
            .await
            .expect("match")
            .is_none()
    );

    let approved = database
        .set_auth_profile_enabled(profile.id, true)
        .await
        .expect("approve");
    assert!(approved.enabled);
}

#[tokio::test]
async fn updating_and_deleting_a_profile_reports_orphaned_secret_references() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let mut input = new_profile("rotate", "example.com", false);
    input.certificate_ref = Some("vault://cert".to_owned());
    let profile = database.create_auth_profile(input).await.expect("create");

    let (updated, orphaned) = database
        .update_auth_profile(
            profile.id,
            UpdateAuthProfile {
                name: "rotate".to_owned(),
                scope: scope("example.com", false),
                method: AuthMethod::Bearer,
                enabled: true,
                expires_at: None,
                username: None,
                secret_ref: Some("vault://rotated".to_owned()),
                certificate_ref: Some("vault://cert".to_owned()),
            },
        )
        .await
        .expect("update");
    // Only the replaced reference is orphaned; the untouched certificate stays in use.
    assert_eq!(orphaned, vec!["vault://rotate".to_owned()]);
    assert!(updated.has_secret && updated.has_client_certificate);

    let remaining = database
        .delete_auth_profile(profile.id)
        .await
        .expect("delete");
    assert_eq!(
        remaining,
        vec!["vault://rotated".to_owned(), "vault://cert".to_owned()]
    );
    assert!(
        database
            .list_auth_profiles()
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn deleting_a_profile_releases_the_jobs_that_pinned_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("auth.sqlite"))
        .await
        .expect("database");
    let profile = database
        .create_auth_profile(new_profile("pinned", "example.com", false))
        .await
        .expect("create");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "package".to_owned(),
            destination: directory.path().display().to_string(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id: package.id,
            source: probe_url(),
            file_name: "file.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Pinned(profile.id),
            initial_state: rd_core::DownloadState::Paused,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    assert_eq!(
        download.auth_profile,
        AuthProfileSelection::Pinned(profile.id)
    );

    database
        .delete_auth_profile(profile.id)
        .await
        .expect("delete");
    let reloaded = database
        .get_download(download.id)
        .await
        .expect("load")
        .expect("download");
    // Falling back to auto-matching beats leaving a dangling pin that fails every retry.
    assert_eq!(reloaded.auth_profile, AuthProfileSelection::Auto);
}

/// Migration 0034 adds the media format inventory column. Rows written before it — a
/// download whose selection is a bare preset, and a candidate with no inventory at all —
/// must survive untouched, because the alternative is rewriting rows that may be
/// mid-download.
#[tokio::test]
async fn media_rows_written_before_the_format_selector_still_load() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("media-legacy.sqlite"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "media".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");

    // Shaped exactly like a 0.6 row: no contract version, no criteria.
    let legacy = rd_core::MediaSelection {
        page_url: "https://www.youtube.com/watch?v=abc".parse().expect("url"),
        variant_id: "1080p".to_owned(),
        format: "bv*[height<=1080]+ba/b[height<=1080]".to_owned(),
        kind: rd_core::MediaKind::Video,
        ext: "mp4".to_owned(),
        title: "clip".to_owned(),
        contract_version: 0,
        criteria: None,
        resolved: None,
    };
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: "https://www.youtube.com/watch?v=abc".parse().expect("URL"),
            file_name: "clip.mp4".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: SELECTION,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Media,
            media: Some(legacy.clone()),
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");

    let reloaded = database
        .get_download(download.id)
        .await
        .expect("load")
        .expect("download")
        .media
        .expect("media selection");
    assert_eq!(reloaded, legacy, "the stored blob round-trips unchanged");
    assert!(reloaded.is_legacy());
    assert_eq!(
        reloaded
            .effective_criteria()
            .expect("a preset resolves")
            .max_height,
        Some(1080),
        "the preset bridge is what keeps old rows downloadable"
    );

    // A candidate that never had an inventory reads back as "nothing stored", not as an
    // error, so the endpoint can fall back to the bounded variant list.
    let (_, _, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            urls: vec!["https://example.test/file.bin".parse().expect("url")],
            providers: vec![None],
            file_names: vec![None],
            sizes: vec![None],
            requests: vec![None],
            body_refs: vec![None],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    let candidate = candidates.first().expect("one candidate");
    assert_eq!(
        database
            .candidate_media_state(candidate.id)
            .await
            .expect("state loads"),
        None
    );
}

// ---- Subscriptions (RD-080-07) ----

fn new_subscription(name: &str) -> crate::NewSubscription {
    crate::NewSubscription {
        source_categories: Vec::new(),
        name: name.to_owned(),
        url: "https://example.test/c/channel".parse().expect("url"),
        kind: rd_core::SubscriptionKind::Media,
        enabled: true,
        mode: rd_core::SubscriptionMode::Review,
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        interval_seconds: 3_600,
        filters: rd_core::SubscriptionFilters::default(),
        backlog: rd_core::BacklogPolicy::FromNow,
        category_map: Vec::new(),
        every_release: false,
        view: rd_core::SubscriptionView::List,
        autoplay: false,
        card_ratio: rd_core::SubscriptionCardRatio::TwoOne,
        schedule: None,
        secret_ref: None,
    }
}

fn new_item(key: &str) -> crate::NewSubscriptionItem {
    crate::NewSubscriptionItem {
        item_key: key.to_owned(),
        title: format!("Item {key}"),
        url: format!("https://example.test/watch?v={key}")
            .parse()
            .expect("url"),
        published_at: Some(Utc::now()),
        duration_seconds: Some(600),
        state: rd_core::SubscriptionItemState::Pending,
        reason: None,
        source_category: None,
        media_type: None,
        attributes: std::collections::BTreeMap::new(),
        password: None,
    }
}

#[tokio::test]
async fn an_item_is_archived_once_however_often_a_poll_repeats_it() {
    // The whole once-only guarantee. A feed that re-lists the same entry, an overlapping
    // poll and a poll interrupted before it finished must all produce one row and one
    // download, which is why the UNIQUE index does the work instead of a read-then-write.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Channel"))
        .await
        .expect("subscription");

    let first = database
        .record_subscription_items(subscription.id, vec![new_item("a"), new_item("b")])
        .await
        .expect("first poll");
    assert_eq!(first.len(), 2);

    // The same two, plus one that is genuinely new.
    let second = database
        .record_subscription_items(
            subscription.id,
            vec![new_item("b"), new_item("a"), new_item("c")],
        )
        .await
        .expect("second poll");
    assert_eq!(
        second.len(),
        1,
        "only the new item should be returned, got {second:?}"
    );
    assert_eq!(second[0].item_key, "c");

    let archived = database
        .subscription_items(subscription.id, 100)
        .await
        .expect("items");
    assert_eq!(archived.len(), 3);
}

/// What an indexer said about a hit survives the round trip (RD-101-17).
#[tokio::test]
async fn an_items_attributes_and_password_are_stored_and_read_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");

    let mut item = new_item("a");
    item.attributes = [
        (
            "coverurl".to_owned(),
            "https://indexer.test/c.jpg".to_owned(),
        ),
        ("imdbscore".to_owned(), "7.8".to_owned()),
        ("size".to_owned(), "4509715660".to_owned()),
    ]
    .into_iter()
    .collect();
    item.password = Some("hunter2".to_owned());

    let created = database
        .record_subscription_items(subscription.id, vec![item])
        .await
        .expect("poll");
    assert_eq!(created.len(), 1);

    let archived = database
        .subscription_items(subscription.id, 100)
        .await
        .expect("items");
    let stored = archived.first().expect("one item");
    assert_eq!(
        stored.attributes.get("coverurl").map(String::as_str),
        Some("https://indexer.test/c.jpg")
    );
    assert_eq!(
        stored.attributes.get("imdbscore").map(String::as_str),
        Some("7.8")
    );
    assert_eq!(stored.password.as_deref(), Some("hunter2"));
}

#[tokio::test]
async fn subscription_item_pages_reach_past_two_hundred_with_stable_totals() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscription-pages.sqlite"))
        .await
        .expect("database");
    let mut input = new_subscription("Indexer");
    input.kind = rd_core::SubscriptionKind::Indexer;
    let subscription = database
        .create_subscription(input)
        .await
        .expect("subscription");
    let items = (0..205)
        .map(|number| new_item(&format!("item-{number:03}")))
        .collect();
    database
        .record_subscription_items(subscription.id, items)
        .await
        .expect("items");

    let first = database
        .subscription_item_page(
            subscription.id,
            Some(rd_core::SubscriptionItemState::Pending),
            50,
            0,
        )
        .await
        .expect("first page");
    let last = database
        .subscription_item_page(
            subscription.id,
            Some(rd_core::SubscriptionItemState::Pending),
            50,
            200,
        )
        .await
        .expect("last page");
    let repeated = database
        .subscription_item_page(
            subscription.id,
            Some(rd_core::SubscriptionItemState::Pending),
            50,
            200,
        )
        .await
        .expect("same last page");

    assert_eq!(first.items.len(), 50);
    assert_eq!(last.items.len(), 5);
    assert_eq!(last.total, 205);
    assert_eq!(last.counts.pending, 205);
    assert_eq!(last.run_total, 0);
    assert_eq!(
        last.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        repeated
            .items
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>()
    );
    assert!(
        first
            .items
            .iter()
            .all(|first| last.items.iter().all(|last| first.id != last.id))
    );

    let summary = database
        .subscription_review_summary()
        .await
        .expect("summary");
    assert_eq!(summary.pending_total, 205);
    assert_eq!(summary.subscriptions.len(), 1);
    assert_eq!(summary.subscriptions[0].pending, 205);
}

#[tokio::test]
async fn a_pending_bulk_snapshot_does_not_capture_items_that_arrive_later() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscription-bulk.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");
    database
        .record_subscription_items(subscription.id, vec![new_item("a"), new_item("b")])
        .await
        .expect("first items");
    let snapshot = database
        .pending_subscription_item_ids(subscription.id)
        .await
        .expect("snapshot");
    database
        .record_subscription_items(subscription.id, vec![new_item("later")])
        .await
        .expect("later item");
    assert_eq!(
        database
            .set_pending_subscription_items_state(
                snapshot,
                rd_core::SubscriptionItemState::Dismissed,
            )
            .await
            .expect("bulk state"),
        2
    );

    let page = database
        .subscription_item_page(subscription.id, None, 50, 0)
        .await
        .expect("page");
    assert_eq!(page.counts.dismissed, 2);
    assert_eq!(page.counts.pending, 1);
    assert_eq!(
        page.items
            .iter()
            .find(|item| item.item_key == "later")
            .expect("later item")
            .state,
        rd_core::SubscriptionItemState::Pending
    );
}

#[tokio::test]
async fn clearing_subscription_history_preserves_pending_items_only() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscription-history.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");
    let created = database
        .record_subscription_items(
            subscription.id,
            vec![
                new_item("pending"),
                new_item("queued"),
                new_item("dismissed"),
                new_item("skipped"),
            ],
        )
        .await
        .expect("items");
    for (key, state) in [
        ("queued", rd_core::SubscriptionItemState::Queued),
        ("dismissed", rd_core::SubscriptionItemState::Dismissed),
        ("skipped", rd_core::SubscriptionItemState::Skipped),
    ] {
        let id = created
            .iter()
            .find(|item| item.item_key == key)
            .expect("item")
            .id;
        database
            .set_subscription_item_state(id, state)
            .await
            .expect("state");
    }
    for _ in 0..2 {
        database
            .finish_subscription_run(
                subscription.id,
                Utc::now(),
                crate::PollResult {
                    found: 4,
                    accepted: 1,
                    skipped: 3,
                    error: None,
                    next_run_at: Utc::now() + Duration::hours(1),
                    consecutive_failures: 0,
                    etag: None,
                    last_modified: None,
                },
            )
            .await
            .expect("run");
    }

    let removed = database
        .clear_subscription_history(subscription.id)
        .await
        .expect("clear");
    assert_eq!(removed.deleted_items, 3);
    assert_eq!(removed.deleted_runs, 2);
    let remaining = database
        .subscription_item_page(subscription.id, None, 50, 0)
        .await
        .expect("remaining");
    assert_eq!(remaining.total, 1);
    assert_eq!(remaining.items[0].item_key, "pending");
    assert_eq!(remaining.counts.pending, 1);
    assert_eq!(remaining.run_total, 0);
}

/// A later poll that answers without the extended block must not erase what is known.
///
/// The upsert refreshes a still-pending row, so a plain `excluded.attributes_json` would
/// replace a full attribute set with nothing the first time an indexer omitted it.
#[tokio::test]
async fn a_repoll_without_attributes_keeps_the_ones_already_stored() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");

    let mut rich = new_item("a");
    rich.attributes = [("imdbscore".to_owned(), "7.8".to_owned())]
        .into_iter()
        .collect();
    rich.password = Some("hunter2".to_owned());
    database
        .record_subscription_items(subscription.id, vec![rich])
        .await
        .expect("first poll");

    // The same item, this time with nothing said about it.
    let bare = new_item("a");
    assert!(bare.attributes.is_empty());
    database
        .record_subscription_items(subscription.id, vec![bare])
        .await
        .expect("second poll");

    let archived = database
        .subscription_items(subscription.id, 100)
        .await
        .expect("items");
    let stored = archived.first().expect("one item");
    assert_eq!(
        stored.attributes.get("imdbscore").map(String::as_str),
        Some("7.8"),
        "a silent poll must not blank the details"
    );
    assert_eq!(stored.password.as_deref(), Some("hunter2"));
}

#[tokio::test]
async fn two_subscriptions_do_not_share_an_archive() {
    // The key is only unique *within* a subscription: the same video legitimately appears
    // in a channel and in a playlist, and each has to decide for itself.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let first = database
        .create_subscription(new_subscription("Channel"))
        .await
        .expect("first");
    let second = database
        .create_subscription(new_subscription("Playlist"))
        .await
        .expect("second");

    assert_eq!(
        database
            .record_subscription_items(first.id, vec![new_item("a")])
            .await
            .expect("first")
            .len(),
        1
    );
    assert_eq!(
        database
            .record_subscription_items(second.id, vec![new_item("a")])
            .await
            .expect("second")
            .len(),
        1
    );
}

#[tokio::test]
async fn the_archive_survives_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("subscriptions.sqlite");
    let id = {
        let database = Database::open(&path).await.expect("database");
        let subscription = database
            .create_subscription(new_subscription("Channel"))
            .await
            .expect("subscription");
        database
            .record_subscription_items(subscription.id, vec![new_item("a")])
            .await
            .expect("poll");
        subscription.id
    };

    let database = Database::open(&path).await.expect("reopen");
    let repeated = database
        .record_subscription_items(id, vec![new_item("a")])
        .await
        .expect("poll after restart");
    assert!(
        repeated.is_empty(),
        "a restart must not make an archived item new again"
    );
}

#[tokio::test]
async fn a_new_subscription_is_due_at_once_and_a_finished_poll_pushes_it_out() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Channel"))
        .await
        .expect("subscription");
    // No next_run_at: the backlog decision is made and shown immediately rather than an
    // interval from now.
    let due = database.due_subscriptions(Utc::now()).await.expect("due");
    assert_eq!(due.len(), 1);
    assert!(!due[0].primed);

    let next = Utc::now() + Duration::hours(1);
    database
        .finish_subscription_run(
            subscription.id,
            Utc::now(),
            crate::PollResult {
                found: 3,
                accepted: 1,
                skipped: 2,
                error: None,
                next_run_at: next,
                consecutive_failures: 0,
                etag: None,
                last_modified: None,
            },
        )
        .await
        .expect("finish");

    assert!(
        database
            .due_subscriptions(Utc::now())
            .await
            .expect("due")
            .is_empty()
    );
    let stored = database
        .subscription(subscription.id)
        .await
        .expect("get")
        .expect("exists");
    // Primed, and never unprimed: re-applying the backlog cutoff later would discard
    // everything published since the subscription was switched on.
    assert!(stored.primed);
    let runs = database
        .subscription_runs(subscription.id, 10)
        .await
        .expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (runs[0].found, runs[0].accepted, runs[0].skipped),
        (3, 1, 2)
    );
}

#[tokio::test]
async fn a_scheduled_script_subscription_is_armed_once_and_a_new_schedule_clears_its_time() {
    // RD-130-19. The script travels as a `script:` address, the expression as a column, and
    // arming only ever fills an empty next run.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let mut input = new_subscription("Daily links");
    input.kind = rd_core::SubscriptionKind::Script;
    input.url = "script:daily-links.sh".parse().expect("url");
    input.schedule = Some("0 6 * * *".to_owned());
    let created = database
        .create_subscription(input.clone())
        .await
        .expect("subscription");
    assert_eq!(created.kind, rd_core::SubscriptionKind::Script);
    assert_eq!(created.script_name(), Some("daily-links.sh"));
    assert_eq!(created.schedule.as_deref(), Some("0 6 * * *"));
    assert!(created.next_run_at.is_none());

    // Whole seconds, so the comparison below is about the row and not about precision.
    let six =
        chrono::DateTime::from_timestamp(Utc::now().timestamp() + 3 * 3_600, 0).expect("time");
    assert!(
        database
            .arm_subscription(created.id, six)
            .await
            .expect("arm")
    );
    // Armed already: a second arm, say from a tick racing a finished run, changes nothing.
    assert!(
        !database
            .arm_subscription(created.id, six + Duration::hours(1))
            .await
            .expect("arm again")
    );
    let stored = database
        .subscription(created.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(stored.next_run_at, Some(six));
    assert_eq!(stored.kind, rd_core::SubscriptionKind::Script);
    assert!(
        database
            .due_subscriptions(Utc::now())
            .await
            .expect("due")
            .is_empty()
    );

    // The same expression again keeps the time; another one clears it for the poller to arm.
    let (kept, _) = database
        .update_subscription(created.id, input.clone())
        .await
        .expect("update");
    assert_eq!(kept.next_run_at, Some(six));
    input.schedule = Some("30 7 * * 1-5".to_owned());
    let (changed, _) = database
        .update_subscription(created.id, input)
        .await
        .expect("update");
    assert_eq!(changed.schedule.as_deref(), Some("30 7 * * 1-5"));
    assert!(changed.next_run_at.is_none());
}

#[tokio::test]
async fn deleting_a_subscription_takes_its_archive_with_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Channel"))
        .await
        .expect("subscription");
    database
        .record_subscription_items(subscription.id, vec![new_item("a")])
        .await
        .expect("poll");

    database
        .delete_subscription(subscription.id)
        .await
        .expect("delete");
    assert!(
        database
            .subscription(subscription.id)
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        database
            .subscription_items(subscription.id, 100)
            .await
            .expect("items")
            .is_empty()
    );
}

/// Builds a definition with one action and no condition.
fn new_automation(name: &str, enabled: bool) -> crate::NewAutomation {
    crate::NewAutomation {
        name: name.to_owned(),
        enabled,
        trigger: rd_automation::Trigger::PackageCompleted,
        condition: rd_automation::ConditionNode::Always,
        actions: vec![rd_automation::Action::PausePackage],
    }
}

#[tokio::test]
async fn saving_an_automation_writes_a_new_version_every_time() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("automations.sqlite"))
        .await
        .expect("database");

    let created = database
        .upsert_automation(None, new_automation("Pause big files", false))
        .await
        .expect("create");
    assert_eq!(created.version, 1);
    assert!(!created.enabled);

    let updated = database
        .upsert_automation(Some(created.id), new_automation("Pause big files", true))
        .await
        .expect("update");
    assert_eq!(updated.version, 2, "an edit must not rewrite version 1");
    assert!(updated.enabled);

    // Both versions are still readable, because a run points at the one it started under.
    let versions = database
        .automation_versions(created.id)
        .await
        .expect("versions");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].version, 2, "newest first");
    assert_eq!(versions[1].version, 1);
}

#[tokio::test]
async fn only_enabled_automations_are_active() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("active.sqlite"))
        .await
        .expect("database");

    let off = database
        .upsert_automation(None, new_automation("Disabled", false))
        .await
        .expect("create");
    let on = database
        .upsert_automation(None, new_automation("Enabled", true))
        .await
        .expect("create");

    let active = database.active_automation_versions().await.expect("active");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].automation_id, on.id);

    // Enabling one brings exactly its current version into force.
    database
        .set_automation_enabled(off.id, true)
        .await
        .expect("enable");
    let active = database.active_automation_versions().await.expect("active");
    assert_eq!(active.len(), 2);
}

#[tokio::test]
async fn the_same_event_queues_a_run_only_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("idempotent.sqlite"))
        .await
        .expect("database");

    let automation = database
        .upsert_automation(None, new_automation("Once", true))
        .await
        .expect("create");
    let version = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    let event = rd_core::EventId::new();
    let key = rd_automation::idempotency_key(version.id, &event);
    let run = crate::NewRun {
        automation_id: automation.id,
        automation_version_id: version.id,
        event_id: event.to_string(),
        package_id: None,
        idempotency_key: key,
    };

    assert!(
        database
            .queue_automation_run(run.clone())
            .await
            .expect("queue")
    );
    // Replaying the same event after a crash must not run the automation a second time.
    assert!(
        !database
            .queue_automation_run(run)
            .await
            .expect("queue again")
    );
    assert_eq!(
        database
            .automation_runs(Some(automation.id), 10)
            .await
            .expect("runs")
            .len(),
        1
    );
}

#[tokio::test]
async fn an_edited_automation_may_see_an_event_the_previous_version_handled() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("reedit.sqlite"))
        .await
        .expect("database");

    let automation = database
        .upsert_automation(None, new_automation("Twice", true))
        .await
        .expect("create");
    let first = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    let event = rd_core::EventId::new();
    assert!(
        database
            .queue_automation_run(crate::NewRun {
                automation_id: automation.id,
                automation_version_id: first.id,
                event_id: event.to_string(),
                package_id: None,
                idempotency_key: rd_automation::idempotency_key(first.id, &event),
            })
            .await
            .expect("queue")
    );

    database
        .upsert_automation(Some(automation.id), new_automation("Twice", true))
        .await
        .expect("edit");
    let second = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    assert_ne!(first.id, second.id);

    // The key is per version, not per automation: an edited definition is entitled to act
    // on an event the previous one already saw.
    assert!(
        database
            .queue_automation_run(crate::NewRun {
                automation_id: automation.id,
                automation_version_id: second.id,
                event_id: event.to_string(),
                package_id: None,
                idempotency_key: rd_automation::idempotency_key(second.id, &event),
            })
            .await
            .expect("queue")
    );
}

#[tokio::test]
async fn a_run_interrupted_mid_flight_is_queued_again_after_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("recover.sqlite");
    let database = Database::open(&path).await.expect("database");

    let automation = database
        .upsert_automation(None, new_automation("Recoverable", true))
        .await
        .expect("create");
    let version = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    let event = rd_core::EventId::new();
    database
        .queue_automation_run(crate::NewRun {
            automation_id: automation.id,
            automation_version_id: version.id,
            event_id: event.to_string(),
            package_id: None,
            idempotency_key: rd_automation::idempotency_key(version.id, &event),
        })
        .await
        .expect("queue");
    let run = database
        .automation_runs(Some(automation.id), 1)
        .await
        .expect("runs")
        .remove(0);

    // The process dies here: the run is claimed but its outcome was never recorded.
    database
        .record_automation_attempt(run.id, rd_automation::RunState::Running, 0, 1, None, None)
        .await
        .expect("claim");
    drop(database);

    let database = Database::open(&path).await.expect("reopen");
    assert_eq!(
        database.recover_automation_runs().await.expect("recover"),
        1
    );
    let due = database.due_automation_runs(Utc::now()).await.expect("due");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].state, rd_automation::RunState::Queued);
}

#[tokio::test]
async fn a_retry_only_becomes_due_once_its_backoff_has_passed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("due.sqlite"))
        .await
        .expect("database");

    let automation = database
        .upsert_automation(None, new_automation("Retrying", true))
        .await
        .expect("create");
    let version = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    let event = rd_core::EventId::new();
    database
        .queue_automation_run(crate::NewRun {
            automation_id: automation.id,
            automation_version_id: version.id,
            event_id: event.to_string(),
            package_id: None,
            idempotency_key: rd_automation::idempotency_key(version.id, &event),
        })
        .await
        .expect("queue");
    let run = database
        .automation_runs(Some(automation.id), 1)
        .await
        .expect("runs")
        .remove(0);

    let later = Utc::now() + Duration::minutes(5);
    database
        .record_automation_attempt(
            run.id,
            rd_automation::RunState::Retrying,
            0,
            1,
            Some(later),
            Some("target refused".to_owned()),
        )
        .await
        .expect("record");

    assert!(
        database
            .due_automation_runs(Utc::now())
            .await
            .expect("due")
            .is_empty(),
        "a retry must not be picked up before its backoff has passed"
    );
    assert_eq!(
        database
            .due_automation_runs(later + Duration::seconds(1))
            .await
            .expect("due")
            .len(),
        1
    );
}

#[tokio::test]
async fn deleting_an_automation_takes_its_versions_and_runs_with_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("delete.sqlite"))
        .await
        .expect("database");

    let automation = database
        .upsert_automation(None, new_automation("Temporary", true))
        .await
        .expect("create");
    let version = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    let event = rd_core::EventId::new();
    database
        .queue_automation_run(crate::NewRun {
            automation_id: automation.id,
            automation_version_id: version.id,
            event_id: event.to_string(),
            package_id: None,
            idempotency_key: rd_automation::idempotency_key(version.id, &event),
        })
        .await
        .expect("queue");

    database
        .delete_automation(automation.id)
        .await
        .expect("delete");
    assert!(database.list_automations().await.expect("list").is_empty());
    assert!(
        database
            .automation_runs(None, 10)
            .await
            .expect("runs")
            .is_empty(),
        "runs outlived the automation they belong to"
    );
}

/// Every column a category write names must have a value bound to it.
///
/// This is not a hypothetical: adding `delete_par2` left the update statement with one more
/// placeholder than value, so *every* category edit failed at runtime while the code compiled
/// and every existing test still passed. Exercising both write paths and reading the result
/// back is what makes the next such column a test failure instead of a support ticket.
#[tokio::test]
async fn a_category_survives_both_write_paths_with_every_field_intact() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("categories.sqlite"))
        .await
        .expect("database");
    let root = database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            NewStorageRoot {
                name: "Downloads".to_owned(),
                path: directory.path().to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root");
    let created = database
        .create_category(NewCategory {
            name: "Movies".to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root.id,
            relative_path: "movies".to_owned(),
            is_default: false,
            postprocess_level: Some(rd_core::PostprocessLevel::Delete),
            script: Some("done.sh".to_owned()),
            cleanup_extensions: Some(vec!["nfo".to_owned()]),
            recursive_unpack: Some(true),
            sfv_verify: Some(false),
            safe_postproc: Some(false),
            delete_par2: Some(true),
            upload_enabled: Some(true),
            upload_remote: Some("archive:movies".to_owned()),
        })
        .await
        .expect("category");
    assert_eq!(created.delete_par2, Some(true));
    assert_eq!(created.safe_postproc, Some(false));

    // The general update path, which is the one that was broken.
    let updated = database
        .update_category(
            created.id,
            NewCategory {
                name: "Films".to_owned(),
                color: "#F87171".to_owned(),
                storage_root_id: root.id,
                relative_path: "films".to_owned(),
                is_default: false,
                postprocess_level: Some(rd_core::PostprocessLevel::Unpack),
                script: None,
                cleanup_extensions: None,
                recursive_unpack: Some(false),
                sfv_verify: Some(true),
                safe_postproc: Some(true),
                delete_par2: Some(false),
                upload_enabled: Some(false),
                upload_remote: None,
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.name, "Films");
    assert_eq!(updated.delete_par2, Some(false));
    assert_eq!(updated.safe_postproc, Some(true));

    // And the post-processing path, which carries the plugin steps.
    database
        .update_category_postprocess(
            created.id,
            crate::CategoryPostprocess {
                level: Some(rd_core::PostprocessLevel::Delete),
                script: Some("after.sh".to_owned()),
                cleanup_extensions: Some(vec!["sfv".to_owned()]),
                recursive_unpack: Some(true),
                sfv_verify: Some(false),
                safe_postproc: Some(false),
                delete_par2: Some(true),
                plugin_steps: Some(vec!["019d0000-0000-7000-8000-000000000106".to_owned()]),
                upload_enabled: Some(true),
                upload_remote: Some("archive:films".to_owned()),
            },
        )
        .await
        .expect("postprocess update");

    let stored = database
        .list_categories()
        .await
        .expect("categories")
        .into_iter()
        .find(|category| category.id == created.id)
        .expect("category is still there");
    assert_eq!(stored.delete_par2, Some(true));
    assert_eq!(stored.safe_postproc, Some(false));
    assert_eq!(stored.script.as_deref(), Some("after.sh"));
    assert_eq!(
        stored.plugin_steps.as_deref(),
        Some(["019d0000-0000-7000-8000-000000000106".to_owned()].as_slice())
    );
    assert_eq!(stored.upload_remote.as_deref(), Some("archive:films"));
}

/// A repeat poll corrects an item nobody has decided on, and leaves decided ones alone.
///
/// The address and the declared media type belong to the feed. An item archived before either
/// was read correctly is otherwise stuck with what was stored then — which is exactly the
/// situation an upgrade leaves behind, and it is not something a person can repair by hand.
#[tokio::test]
async fn a_repeat_poll_refreshes_undecided_items_only() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::Database::open(&directory.path().join("test.sqlite3"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");

    let stale = crate::NewSubscriptionItem {
        url: "https://indexer.test/api?t=get&amp;id=a"
            .parse()
            .expect("url"),
        media_type: None,
        ..new_item("a")
    };
    let decided = crate::NewSubscriptionItem {
        url: "https://indexer.test/api?t=get&amp;id=b"
            .parse()
            .expect("url"),
        media_type: None,
        ..new_item("b")
    };
    let created = database
        .record_subscription_items(subscription.id, vec![stale, decided])
        .await
        .expect("first poll");
    assert_eq!(created.len(), 2);
    let decided_id = created
        .iter()
        .find(|item| item.item_key == "b")
        .expect("b")
        .id;
    database
        .set_subscription_item_state(decided_id, rd_core::SubscriptionItemState::Queued)
        .await
        .expect("decide");

    // The same entries, read correctly this time.
    let corrected = |key: &str| crate::NewSubscriptionItem {
        url: format!("https://indexer.test/api?t=get&id={key}")
            .parse()
            .expect("url"),
        media_type: Some("application/x-nzb".to_owned()),
        ..new_item(key)
    };
    let second = database
        .record_subscription_items(subscription.id, vec![corrected("a"), corrected("b")])
        .await
        .expect("second poll");

    assert!(
        second.is_empty(),
        "a refreshed item is not a discovery: {second:?}"
    );
    let archived = database
        .subscription_items(subscription.id, 100)
        .await
        .expect("items");
    let item = |key: &str| {
        archived
            .iter()
            .find(|item| item.item_key == key)
            .unwrap_or_else(|| panic!("{key} missing"))
    };
    assert_eq!(
        item("a").url.as_str(),
        "https://indexer.test/api?t=get&id=a",
        "the undecided one is corrected"
    );
    assert_eq!(item("a").media_type.as_deref(), Some("application/x-nzb"));
    assert_eq!(
        item("b").url.as_str(),
        "https://indexer.test/api?t=get&amp;id=b",
        "a decision is never undone by a re-listing"
    );
    assert_eq!(item("b").media_type, None);
    assert_eq!(item("b").state, rd_core::SubscriptionItemState::Queued);
}

/// A link whose check failed outright stays queueable.
///
/// Reported from use: a DDownload account whose sign-in did not work made the batched check
/// fail, every link of the batch landed in `error`, and the package could then not be added to
/// the downloader at all. `error` was the only state `claim_package_for_enqueue` left out —
/// stricter than `offline`, which means the file is known to be gone.
#[tokio::test]
async fn a_package_whose_check_failed_can_still_be_enqueued() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("failed-check.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = ["https://ddownload.com/aaa111bbb/Movie.mkv"]
        .iter()
        .map(|value| value.parse().expect("URL"))
        .collect();
    let (_batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: Some("test".to_owned()),
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");

    // No result at all — what the account failure produces.
    database
        .record_candidate_check(
            candidates[0].id,
            None,
            Some(rd_core::CandidateMessage::plain(
                "Provider account check failed",
            )),
            false,
            None,
        )
        .await
        .expect("record");
    let listed = database.list_candidates().await.expect("candidates");
    assert_eq!(listed[0].state, rd_core::LinkCandidateState::Error);
    assert!(
        listed[0]
            .error
            .as_deref()
            .is_some_and(|message| message.contains("account check failed")),
        "the reason has to survive, or nobody can tell why nothing was confirmed"
    );

    let claimed = database
        .claim_package_for_enqueue(packages[0].id, None)
        .await
        .expect("a link that could not be checked is still the user's call");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].1, rd_core::LinkCandidateState::Error);
}

/// A claim narrowed to some links of a package leaves the others where they were.
///
/// Reported from use: with the LinkGrabber filtered to one hoster, "add to the queue" sent the
/// links of every other hoster along. The links the claim leaves out keep their state and their
/// package, so the package outlives the enqueue with exactly them in it.
#[tokio::test]
async fn a_narrowed_claim_leaves_the_other_links_in_their_package() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("narrowed-claim.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = [
        "https://one.example/a.bin",
        "https://two.example/b.bin",
        "https://one.example/c.bin",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (_batch, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: Some("Release".to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(packages.len(), 1);
    let hidden = candidates[1].id;

    let claimed = database
        .claim_package_for_enqueue(
            packages[0].id,
            Some(vec![candidates[0].id, candidates[2].id]),
        )
        .await
        .expect("claim");
    let claimed_ids: Vec<_> = claimed.iter().map(|(candidate, _)| candidate.id).collect();
    assert_eq!(claimed_ids, [candidates[0].id, candidates[2].id]);
    database
        .finish_package_enqueue(packages[0].id, true, Vec::new())
        .await
        .expect("finish");

    let listed = database.list_candidates().await.expect("candidates");
    let left = listed
        .iter()
        .find(|candidate| candidate.id == hidden)
        .expect("the hidden link is still in the LinkGrabber");
    assert_eq!(left.package_id, Some(packages[0].id));
    assert_eq!(left.state, candidates[1].state);
    let remaining = database.list_collector_packages().await.expect("packages");
    assert_eq!(
        remaining.len(),
        1,
        "the package stays for the link it still holds"
    );
    assert_eq!(remaining[0].id, packages[0].id);

    // A narrowing that names nothing enqueueable in the package is refused like an empty one.
    let error = database
        .claim_package_for_enqueue(packages[0].id, Some(vec![candidates[0].id]))
        .await
        .expect_err("nothing of this package is named");
    assert_eq!(
        crate::store_kind(&error),
        Some(crate::StoreErrorKind::NoEnqueueableLinks)
    );
}

/// The manual file order inside a package is written, read back, and still there after the
/// service is restarted — the whole point of storing it instead of keeping it in the view.
#[tokio::test]
async fn download_order_inside_a_package_survives_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("queue.sqlite");
    let package_id = PackageId::new();
    let ordered: Vec<DownloadId>;

    {
        let database = Database::open(&path).await.expect("database");
        database
            .create_package(NewPackage {
                id: package_id,
                name: "Release".to_owned(),
                destination: directory.path().to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let mut created = Vec::new();
        for name in ["one.bin", "two.bin", "three.bin"] {
            let file = database
                .create_download(NewDownload {
                    id: DownloadId::new(),
                    package_id,
                    source: format!("https://example.test/{name}").parse().expect("URL"),
                    file_name: name.to_owned(),
                    total_bytes: None,
                    expected_checksum: None,
                    account_id: None,
                    proxy_profile_id: None,
                    auth_profile: AuthProfileSelection::Auto,
                    initial_state: rd_core::DownloadState::Queued,
                    kind: rd_core::DownloadKind::Http,
                    media: None,
                    remote_credential_id: None,
                    replay: None,
                    mirror_group: None,
                    enrichment: Vec::new(),
                    secret_fragment: None,
                })
                .await
                .expect("download");
            created.push(file.id);
        }
        let names = |files: Vec<rd_core::DownloadFile>| {
            files
                .into_iter()
                .map(|file| file.file_name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(database.list_downloads().await.expect("downloads")),
            ["one.bin", "two.bin", "three.bin"]
        );

        ordered = vec![created[2], created[0], created[1]];
        database
            .reorder_downloads(package_id, ordered.clone())
            .await
            .expect("reorder");
        assert_eq!(
            names(database.list_downloads().await.expect("downloads")),
            ["three.bin", "one.bin", "two.bin"]
        );
    }

    // A second `Database::open` on the same file is the restart: nothing of the first instance
    // survives except what it wrote.
    let restarted = Database::open(&path).await.expect("reopened database");
    let files = restarted.list_downloads().await.expect("downloads");
    assert_eq!(
        files
            .iter()
            .map(|file| file.file_name.as_str())
            .collect::<Vec<_>>(),
        ["three.bin", "one.bin", "two.bin"]
    );
    assert_eq!(
        files.iter().map(|file| file.id).collect::<Vec<_>>(),
        ordered
    );
    assert_eq!(
        files.iter().map(|file| file.position).collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

/// Re-scoping a live token, and the trail it has to leave.
///
/// The trail is the point. Fixed scopes were their own audit story — a token could only ever
/// do what it was born to do — and changeable scopes replace that story with a written one.
/// So this asserts both halves: the new areas take effect for the digest that was already
/// there, and the `events` table can afterwards say what the token was issued with and what
/// it was changed to.
#[tokio::test]
async fn token_scopes_can_be_rewritten_and_both_steps_are_recorded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("rescope.sqlite"))
        .await
        .expect("database");
    let digest = "cc".repeat(32);
    let token = database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "Assistant".to_owned(),
            digest.clone(),
            vec![rd_core::API_READ_SCOPE.to_owned()],
        )
        .await
        .expect("api token");
    assert!(
        !database
            .capture_token_valid(&digest, rd_core::API_CONFIG_SCOPE)
            .await
            .expect("check")
    );

    let widened = database
        .update_capture_token_scopes(
            token.id,
            vec![
                rd_core::API_READ_SCOPE.to_owned(),
                rd_core::API_CONFIG_SCOPE.to_owned(),
            ],
        )
        .await
        .expect("widen");
    assert_eq!(widened.id, token.id);
    assert_eq!(widened.label, "Assistant");
    assert_eq!(widened.created_at, token.created_at);
    // The digest is untouched, which is what "the bearer keeps working" means at this layer.
    assert!(
        database
            .capture_token_valid(&digest, rd_core::API_CONFIG_SCOPE)
            .await
            .expect("check")
    );

    let narrowed = database
        .update_capture_token_scopes(token.id, vec![rd_core::API_READ_SCOPE.to_owned()])
        .await
        .expect("narrow");
    assert_eq!(narrowed.scopes, vec![rd_core::API_READ_SCOPE.to_owned()]);
    assert!(
        !database
            .capture_token_valid(&digest, rd_core::API_CONFIG_SCOPE)
            .await
            .expect("check")
    );

    let payloads = sqlx::query_scalar::<_, String>(
        "SELECT payload_json FROM events WHERE kind = 'capture_changed' ORDER BY occurred_at, id",
    )
    .fetch_all(&database.readers)
    .await
    .expect("events");
    let payloads: Vec<serde_json::Value> = payloads
        .iter()
        .map(|payload| serde_json::from_str(payload).expect("payload"))
        .filter(|payload: &serde_json::Value| {
            payload["capture_token_id"] == serde_json::json!(token.id)
        })
        .collect();
    assert_eq!(payloads.len(), 3, "{payloads:?}");
    assert_eq!(payloads[0]["issued"], serde_json::json!(true));
    assert_eq!(
        payloads[0]["scopes"],
        serde_json::json!([rd_core::API_READ_SCOPE])
    );
    assert_eq!(payloads[1]["scopes_changed"], serde_json::json!(true));
    assert_eq!(
        payloads[1]["previous_scopes"],
        serde_json::json!([rd_core::API_READ_SCOPE])
    );
    assert_eq!(
        payloads[1]["scopes"],
        serde_json::json!([rd_core::API_READ_SCOPE, rd_core::API_CONFIG_SCOPE])
    );
    assert_eq!(
        payloads[2]["previous_scopes"],
        serde_json::json!([rd_core::API_READ_SCOPE, rd_core::API_CONFIG_SCOPE])
    );
}

/// A revoked token is gone, not merely inert: re-scoping must not resurrect one.
#[tokio::test]
async fn a_revoked_or_unknown_token_cannot_be_rescoped() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("rescope-revoked.sqlite"))
        .await
        .expect("database");
    let token = database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            "Assistant".to_owned(),
            "dd".repeat(32),
            vec![rd_core::API_READ_SCOPE.to_owned()],
        )
        .await
        .expect("api token");
    database
        .revoke_capture_token(token.id)
        .await
        .expect("revoke");

    for id in [token.id, rd_core::CaptureTokenId::new()] {
        let error = database
            .update_capture_token_scopes(id, vec![rd_core::API_SCOPE.to_owned()])
            .await
            .expect_err("no live token");
        assert!(error.to_string().contains("not found"), "{error}");
    }
}

/// The prefix rewrite a folder rename does, and the one thing it must not do.
///
/// `postprocess_steps` is keyed by `(owner_id, kind, source_path)`, so a stored path is an
/// identity and not a note. It has to follow the folder. A *sibling* folder whose name merely
/// starts with the same characters must not, which is why the match is on whole path
/// components rather than on a plain string prefix.
#[tokio::test]
async fn renaming_a_package_folder_carries_its_stored_paths_and_leaves_its_siblings_alone() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("rename.sqlite3"))
        .await
        .expect("database");
    let base = directory.path().join("library");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "Show S01".to_owned(),
            destination: base.join("Show S01").to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let owner = package.id.to_string();
    let old = base.join("Show S01");
    // Three shapes that have to survive the rewrite: a file below the folder, the folder
    // itself as an output, and a path in a folder that only looks like a prefix match.
    for (kind, source, output) in [
        (
            rd_core::PostprocessKind::ExtractZip,
            old.join("archive.zip").to_string_lossy().into_owned(),
            Some(old.to_string_lossy().into_owned()),
        ),
        (
            rd_core::PostprocessKind::Cleanup,
            base.join("Show S01 Extras")
                .join("archive.zip")
                .to_string_lossy()
                .into_owned(),
            None,
        ),
    ] {
        database
            .checkpoint_postprocess(
                owner.clone(),
                kind,
                source,
                rd_core::PostprocessState::Completed,
                output,
                None,
            )
            .await
            .expect("checkpoint");
    }

    let renamed = base.join("Show S01 Complete");
    let updated = database
        .rename_package_directory(
            package.id,
            "Show S01 Complete".to_owned(),
            renamed.to_string_lossy().into_owned(),
        )
        .await
        .expect("rename")
        .expect("package");

    assert_eq!(updated.name, "Show S01 Complete");
    assert_eq!(updated.destination, renamed.to_string_lossy());
    assert_eq!(
        database
            .package_previous_destination(package.id)
            .await
            .expect("previous destination")
            .as_deref(),
        Some(old.to_string_lossy().as_ref()),
        "the disk move has to stay outstanding until it has actually run"
    );

    let steps = database
        .list_postprocess_steps(&owner)
        .await
        .expect("steps");
    let moved = steps
        .iter()
        .find(|step| step.kind == rd_core::PostprocessKind::ExtractZip)
        .expect("the extract step");
    assert_eq!(
        moved.source_path,
        renamed.join("archive.zip").to_string_lossy(),
        "the step's identity did not follow its folder"
    );
    assert_eq!(
        moved.output_path.as_deref(),
        Some(renamed.to_string_lossy().as_ref()),
        "the folder itself, stored as an output, did not follow"
    );
    let untouched = steps
        .iter()
        .find(|step| step.kind == rd_core::PostprocessKind::Cleanup)
        .expect("the cleanup step");
    assert_eq!(
        untouched.source_path,
        base.join("Show S01 Extras")
            .join("archive.zip")
            .to_string_lossy(),
        "a sibling folder that starts with the same characters was dragged along"
    );
}

/// A rename of a package that does not exist is a `None`, not an error and not a silent write.
#[tokio::test]
async fn renaming_a_package_that_is_gone_reports_nothing_rather_than_writing() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("rename.sqlite3"))
        .await
        .expect("database");
    assert!(
        database
            .rename_package_directory(
                PackageId::new(),
                "New Name".to_owned(),
                "/tmp/new".to_owned()
            )
            .await
            .expect("rename")
            .is_none()
    );
}

/// A Usenet package stores where each assembled file landed, on the import's rows rather than
/// on its own; the rename has to reach them through the import or they keep naming a folder
/// that no longer exists.
#[tokio::test]
async fn renaming_a_usenet_package_folder_carries_the_assembled_file_paths_too() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("rename-usenet.sqlite3"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "Show S01.nzb".to_owned(),
            sha256: "ab".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "archive.zip".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: 42,
                    message_id: "usenet-rename@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");
    let base = directory.path().join("library");
    let package = database
        .enqueue_nzb_import(
            import.id,
            base.clone(),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    let old = std::path::PathBuf::from(&package.destination);
    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    database
        .checkpoint_nzb_file_output(
            file.id,
            old.join("archive.zip").to_string_lossy().into_owned(),
        )
        .await
        .expect("file checkpoint");

    let renamed = base.join("Show S01 Complete");
    database
        .rename_package_directory(
            package.id,
            "Show S01 Complete".to_owned(),
            renamed.to_string_lossy().into_owned(),
        )
        .await
        .expect("rename")
        .expect("package");

    let file = &database.list_nzb_files(import.id).await.expect("files")[0];
    assert_eq!(
        file.output_path.as_deref(),
        Some(renamed.join("archive.zip").to_string_lossy().as_ref()),
        "the assembled file still names the folder the package no longer has"
    );
}

/// One NZB file with a single segment, for the PAR2 postponement fixtures.
fn nzb_file(subject: &str) -> NewNzbFile {
    NewNzbFile {
        subject: subject.to_owned(),
        poster: "poster".to_owned(),
        groups: vec!["alt.binaries.test".to_owned()],
        segments: vec![NewNzbSegment {
            number: 1,
            bytes: 128,
            message_id: format!("{subject}@example.test"),
        }],
    }
}

/// A release with a payload, a main PAR2 index and three recovery volumes.
fn par2_release(digest: &str) -> NewNzbImport {
    NewNzbImport {
        name: "release.nzb".to_owned(),
        sha256: digest.repeat(32),
        category_id: None,
        priority: None,
        import_mode: ImportMode::Enqueue,
        source: IngressSource::Manual,
        source_path: None,
        password: None,
        announce_arrival: false,
        files: vec![
            nzb_file("payload.zip"),
            nzb_file("release.par2"),
            nzb_file("release.vol000+01.par2"),
            nzb_file("release.vol001+02.par2"),
            nzb_file("release.vol003+16.par2"),
        ],
    }
}

/// RD-107-04: the recovery volumes wait, the main index does not.
///
/// SABnzbd's `postpone_pars`. The index is what answers whether anything is damaged at all,
/// so it comes down with the payload; the volumes are the repair material and a package that
/// arrives intact should never pay for them.
#[tokio::test]
async fn queuing_an_nzb_postpones_its_recovery_volumes_but_not_its_index() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("par2.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(par2_release("c1"))
        .await
        .expect("import");

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let states: Vec<(String, rd_core::DownloadState)> = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .map(|file| (file.file_name, file.state))
        .collect();
    let state_of = |name: &str| {
        states
            .iter()
            .find(|(file_name, _)| file_name == name)
            .map(|(_, state)| *state)
            .unwrap_or_else(|| panic!("no row for {name}"))
    };
    assert_eq!(state_of("payload.zip"), rd_core::DownloadState::Queued);
    assert_eq!(state_of("release.par2"), rd_core::DownloadState::Queued);
    for volume in [
        "release.vol000+01.par2",
        "release.vol001+02.par2",
        "release.vol003+16.par2",
    ] {
        assert_eq!(
            state_of(volume),
            rd_core::DownloadState::Skipped,
            "{volume} should have been postponed"
        );
    }
}

/// RD-120-16: a postponed volume is not a mirror, and the queue has to be able to say so.
///
/// `Skipped` carries both meanings, and the one thing that separates them is the group key:
/// a mirror is only ever written as skipped together with the group it stands down for
/// (`rd_scheduler::control::stand_down_siblings_of` tests exactly that), while a postponed
/// recovery volume has no second source at all. The interface derives its wording from this,
/// so a group key appearing here would make it call a volume a mirror again.
#[tokio::test]
async fn a_postponed_recovery_volume_carries_no_mirror_group() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("par2-no-mirror.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(par2_release("c9"))
        .await
        .expect("import");

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let postponed: Vec<rd_core::DownloadFile> = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .filter(|file| file.state == rd_core::DownloadState::Skipped)
        .collect();
    assert_eq!(postponed.len(), 3, "the three volumes of the set");
    for file in postponed {
        assert!(
            file.mirror_group.is_none(),
            "{} waits for the repair, not for another link",
            file.file_name
        );
        assert!(file.recovery, "{} is repair data", file.file_name);
    }
}

/// RD-108-23: the subject form of the live finding - the release name quoted first, the file
/// name second. Every row used to be named after its whole subject line, so no row was PAR2,
/// nothing was postponed, and a lost volume counted as a package error.
#[tokio::test]
async fn queuing_an_nzb_whose_subjects_quote_the_release_first_names_and_postpones_its_rows() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("release-first.sqlite"))
        .await
        .expect("database");
    let subject = |index: usize, name: &str| {
        format!(
            "\"Starfight.1984.German.AC3.DL.1080p.BluRay.x265-FuN\" - [{index:02}/50] - \"{name}\" yEnc (1/13)"
        )
    };
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "Starfight.1984.German.AC3.DL.1080p.BluRay.x265-FuN.nzb".to_owned(),
            sha256: "c4".repeat(32),
            category_id: None,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source: IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![
                nzb_file(&subject(
                    1,
                    "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.part01.rar",
                )),
                nzb_file(&subject(43, "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.par2")),
                nzb_file(&subject(
                    44,
                    "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.vol03+04.par2",
                )),
                nzb_file(&subject(
                    45,
                    "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.vol07+08.par2",
                )),
            ],
        })
        .await
        .expect("import");

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let mut rows: Vec<(String, bool, rd_core::DownloadState)> = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .map(|file| (file.file_name, file.recovery, file.state))
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        rows,
        [
            (
                "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.par2".to_owned(),
                true,
                rd_core::DownloadState::Queued
            ),
            (
                "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.part01.rar".to_owned(),
                false,
                rd_core::DownloadState::Queued
            ),
            (
                "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.vol03+04.par2".to_owned(),
                true,
                rd_core::DownloadState::Skipped
            ),
            (
                "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.vol07+08.par2".to_owned(),
                true,
                rd_core::DownloadState::Skipped
            ),
        ]
    );
}

/// RD-108-23: the PAR2 decision is taken again when the real name arrives.
///
/// The index's subject announces no usable name, so at enqueue time nothing is postponed
/// (there is no index to verify with) and the row is named after its subject. Once the
/// assembled file settles the name, the row is marked, the set's waiting volumes are
/// postponed - and the volume that is already downloading is not touched, nor the payload.
#[tokio::test]
async fn settling_the_index_name_marks_the_row_and_postpones_the_waiting_volumes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("settle.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "late-index.nzb".to_owned(),
            sha256: "c5".repeat(32),
            category_id: None,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source: IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![
                nzb_file("payload.zip"),
                nzb_file("Release index without a name (1/3)"),
                nzb_file("release.vol000+01.par2"),
                nzb_file("release.vol001+02.par2"),
                nzb_file("release.vol003+16.par2"),
                nzb_file("other.vol000+01.par2"),
            ],
        })
        .await
        .expect("import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    let rows = || async {
        database
            .list_downloads()
            .await
            .expect("downloads")
            .into_iter()
            .filter(|file| file.package_id == package.id)
            .collect::<Vec<_>>()
    };
    let row_named = |rows: &[rd_core::DownloadFile], name: &str| {
        rows.iter()
            .find(|file| file.file_name == name)
            .cloned()
            .unwrap_or_else(|| panic!("no row for {name}"))
    };
    let queued = rows().await;
    assert!(
        queued
            .iter()
            .all(|file| file.state == rd_core::DownloadState::Queued),
        "without a recognisable index nothing is postponed at enqueue time"
    );
    let index = row_named(&queued, "Release index without a name (1_3)");
    assert!(!index.recovery, "the subject line says nothing about PAR2");
    let running = row_named(&queued, "release.vol001+02.par2");
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
    ] {
        database
            .transition_download(running.id, state)
            .await
            .expect("volume on its way");
    }

    let postponed = database
        .settle_nzb_recovery(index.id, "release.par2".to_owned(), false)
        .await
        .expect("settle");

    assert_eq!(postponed, 2, "the two waiting volumes of the set");
    let settled = rows().await;
    let index = row_named(&settled, "release.par2");
    assert!(index.recovery, "the settled name says PAR2");
    let state_of = |name: &str| row_named(&settled, name).state;
    assert_eq!(
        state_of("release.vol000+01.par2"),
        rd_core::DownloadState::Skipped
    );
    assert_eq!(
        state_of("release.vol003+16.par2"),
        rd_core::DownloadState::Skipped
    );
    assert_eq!(
        state_of("release.vol001+02.par2"),
        rd_core::DownloadState::Downloading,
        "a volume already on its way is never postponed retroactively"
    );
    assert_eq!(state_of("payload.zip"), rd_core::DownloadState::Queued);
    assert_eq!(
        state_of("other.vol000+01.par2"),
        rd_core::DownloadState::Queued,
        "a volume of another set is not this index's business"
    );
}

/// RD-108-23: content outranks the name, and the marking follows a rename.
#[tokio::test]
async fn settling_marks_par2_content_under_any_name_and_a_rename_decides_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("settle-content.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "obfuscated.nzb".to_owned(),
            sha256: "c6".repeat(32),
            category_id: None,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source: IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![nzb_file("a1b2c3.bin"), nzb_file("d4e5f6.bin")],
        })
        .await
        .expect("import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    let rows: Vec<rd_core::DownloadFile> = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .collect();
    let by_content = rows
        .iter()
        .find(|file| file.file_name == "a1b2c3.bin")
        .expect("first row");
    let by_rename = rows
        .iter()
        .find(|file| file.file_name == "d4e5f6.bin")
        .expect("second row");

    database
        .settle_nzb_recovery(by_content.id, "a1b2c3.bin".to_owned(), true)
        .await
        .expect("settle");
    let settled = database
        .get_download(by_content.id)
        .await
        .expect("row")
        .expect("row exists");
    assert!(
        settled.recovery,
        "a PAR2 header marks the row whatever its name"
    );

    let renamed = database
        .rename_download(by_rename.id, "d4e5f6.par2".to_owned())
        .await
        .expect("rename");
    assert!(renamed.recovery, "the marking follows the new name");
    let renamed = database
        .rename_download(by_rename.id, "d4e5f6.rar".to_owned())
        .await
        .expect("rename back");
    assert!(!renamed.recovery, "and follows it back");
}

/// RD-107-04: `enable_all_par` restores the behaviour of fetching every volume.
#[tokio::test]
async fn enable_all_par_queues_every_recovery_volume_with_the_payload() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("all-par.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "enable_all_par": true }),
        )
        .await
        .expect("settings");
    let import = database
        .add_nzb_import(par2_release("c2"))
        .await
        .expect("import");

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let postponed = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .filter(|file| file.state == rd_core::DownloadState::Skipped)
        .count();
    assert_eq!(postponed, 0, "nothing may be held back with the switch on");
}

/// A set whose recovery data is volumes only has nothing to verify with if they all wait.
#[tokio::test]
async fn an_nzb_without_a_main_index_postpones_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("no-index.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "volumes-only.nzb".to_owned(),
            sha256: "c3".repeat(32),
            category_id: None,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source: IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![nzb_file("payload.zip"), nzb_file("release.vol000+01.par2")],
        })
        .await
        .expect("import");

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let postponed = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .filter(|file| file.state == rd_core::DownloadState::Skipped)
        .count();
    assert_eq!(
        postponed, 0,
        "with no index to verify against, holding the volumes back leaves nothing to repair from"
    );
}

/// RD-107-04: a step's stable code and its parameters survive the round trip.
#[tokio::test]
async fn a_postprocess_step_keeps_its_code_and_parameters() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("coded.sqlite"))
        .await
        .expect("database");
    let owner = PackageId::new().to_string();

    database
        .checkpoint_postprocess_coded(
            owner.clone(),
            PostprocessKind::Par2,
            "/tmp/release.par2".to_owned(),
            PostprocessState::Failed,
            None,
            Some("PAR2 repair needs 9 blocks but only 3 are available".to_owned()),
            Some("postprocess.par2_not_enough_blocks".to_owned()),
            [
                ("needed".to_owned(), "9".to_owned()),
                ("available".to_owned(), "3".to_owned()),
            ]
            .into_iter()
            .collect(),
        )
        .await
        .expect("checkpoint");

    let steps = database
        .list_postprocess_steps(&owner)
        .await
        .expect("steps");
    let step = steps.first().expect("one step");
    assert_eq!(
        step.code.as_deref(),
        Some("postprocess.par2_not_enough_blocks")
    );
    assert_eq!(step.params.get("needed").map(String::as_str), Some("9"));
    assert_eq!(step.params.get("available").map(String::as_str), Some("3"));

    // A later outcome without a code must clear the old one rather than leave it standing.
    database
        .checkpoint_postprocess(
            owner.clone(),
            PostprocessKind::Par2,
            "/tmp/release.par2".to_owned(),
            PostprocessState::Completed,
            None,
            Some("repaired=true".to_owned()),
        )
        .await
        .expect("checkpoint");
    let steps = database
        .list_postprocess_steps(&owner)
        .await
        .expect("steps");
    let step = steps.first().expect("one step");
    assert_eq!(step.code, None);
    assert!(step.params.is_empty());
}

/// RD-107-02. What the indexer declared reaches the candidate and survives a restart.
///
/// The restart is the point: a poll writes the attributes, the service stops, and the online
/// check that asks an enricher runs afterwards. If they lived anywhere but in the row, the
/// plugin would be asked with nothing.
#[tokio::test]
async fn declared_indexer_attributes_survive_a_reopen_of_the_database() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("declared.sqlite");
    let declared: std::collections::BTreeMap<String, String> = [
        ("imdb".to_owned(), "tt0111161".to_owned()),
        ("imdbscore".to_owned(), "9.3".to_owned()),
    ]
    .into_iter()
    .collect();
    let candidate_id = {
        let database = Database::open(path.clone()).await.expect("database");
        let (_, _, candidates) = database
            .add_collector_batch(crate::NewCollectorBatch {
                package_hints: Vec::new(),
                mirror_hints: Vec::new(),
                source: IngressSource::Subscription,
                source_label: Some("Indexer".to_owned()),
                package_name: None,
                password: None,
                passwords: Vec::new(),
                category_id: None,
                priority: None,
                urls: vec![
                    "https://indexer.example/api?t=get&id=1"
                        .parse()
                        .expect("URL"),
                    "https://indexer.example/api?t=get&id=2"
                        .parse()
                        .expect("URL"),
                ],
                providers: vec![None, None],
                file_names: vec![
                    Some("Release.One".to_owned()),
                    Some("Release.Two".to_owned()),
                ],
                sizes: Vec::new(),
                requests: Vec::new(),
                body_refs: Vec::new(),
                auto_check: false,
                // Only the first link was declared; the second stands for every link no
                // subscription produced.
                source_attributes: vec![declared.clone(), std::collections::BTreeMap::new()],
            })
            .await
            .expect("batch");
        assert_eq!(candidates.len(), 2);
        let bare = database
            .candidate_source_attributes(candidates[1].id)
            .await
            .expect("read");
        assert!(bare.is_empty(), "an undeclared link must be asked bare");
        candidates[0].id
    };
    let database = Database::open(path).await.expect("reopen");
    let stored = database
        .candidate_source_attributes(candidate_id)
        .await
        .expect("read");
    assert_eq!(stored, declared);
}

/// RD-107-02. The enricher fields reach the package and its queue rows, and stay there.
#[tokio::test]
async fn enrichment_is_carried_onto_the_package_and_its_downloads() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("carry.sqlite");
    let field = |name: &str| rd_core::EnrichmentField {
        name: name.to_owned(),
        value: "9.3".to_owned(),
        plugin_id: "imdb-enricher".to_owned(),
        fetched_at: Utc::now(),
    };
    let (package_id, download_id) = {
        let database = Database::open(path.clone()).await.expect("database");
        let package_id = PackageId::new();
        database
            .create_package(NewPackage {
                id: package_id,
                name: "carry".to_owned(),
                destination: directory.path().to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let download = database
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id,
                source: "https://example.test/file".parse().expect("URL"),
                file_name: "file".to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: AuthProfileSelection::Auto,
                initial_state: rd_core::DownloadState::Queued,
                kind: rd_core::DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                replay: None,
                mirror_group: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download");
        // Nothing before the carry: the column is what this test is about.
        assert!(download.enrichment.is_empty());
        database
            .carry_enrichment(
                package_id,
                vec![field("imdb.score")],
                vec![(download.id, vec![field("imdb.score")])],
            )
            .await
            .expect("carry");
        // Twice, because an enqueue can be retried and a replace must say the same thing.
        database
            .carry_enrichment(
                package_id,
                vec![field("imdb.score")],
                vec![(download.id, vec![field("imdb.score")])],
            )
            .await
            .expect("carry again");
        (package_id, download.id)
    };
    let database = Database::open(path).await.expect("reopen");
    let package = database
        .list_packages()
        .await
        .expect("packages")
        .into_iter()
        .find(|package| package.id == package_id)
        .expect("package");
    assert_eq!(package.enrichment.len(), 1);
    assert_eq!(package.enrichment[0].name, "imdb.score");
    assert_eq!(package.enrichment[0].plugin_id, "imdb-enricher");
    let download = database
        .get_download(download_id)
        .await
        .expect("download read")
        .expect("download");
    assert_eq!(download.enrichment.len(), 1);
    assert_eq!(download.enrichment[0].value, "9.3");
}

/// RD-108-15: the enricher's write and the enqueue are two writer commands with no order
/// between them, so both orders have to end in the same place.
///
/// `when_written` decides which of the two wins the race, and the assertions do not change with
/// it: the fields belong on the package and on the queue row either way. Before this was fixed,
/// everything but [`EnrichedAt::BeforeTheClaim`] was lost — the enqueue copied the snapshot the
/// claim handed it, and the candidate row the fields landed on is detached moments later.
#[derive(Clone, Copy, Debug)]
enum EnrichedAt {
    /// The order that always worked: the fields are there when the promotion claims the links.
    BeforeTheClaim,
    /// The enricher answers while the enqueue is running, before the queue rows exist.
    WhileTheRowsAreWritten,
    /// The enricher answers after the whole promotion is over.
    AfterTheEnqueue,
}

async fn enrichment_survives_the_enqueue(when_written: EnrichedAt) {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("race.sqlite"))
        .await
        .expect("database");
    let url: url::Url = "https://example.test/release.bin".parse().expect("URL");
    let field = rd_core::EnrichmentField {
        name: "imdb.score".to_owned(),
        value: "9.3".to_owned(),
        plugin_id: "imdb-enricher".to_owned(),
        fetched_at: Utc::now(),
    };
    let (_, packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Subscription,
            source_label: Some("Indexer".to_owned()),
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None],
            urls: vec![url.clone()],
            file_names: vec![None],
            sizes: vec![None],
            requests: vec![None],
            body_refs: vec![None],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    let enrich = || database.set_candidate_enrichment(candidates[0].id, vec![field.clone()]);
    if matches!(when_written, EnrichedAt::BeforeTheClaim) {
        enrich().await.expect("enrichment");
    }

    // The promotion, step by step: claim the links, write the queue rows, release the claim.
    let claimed = database
        .claim_package_for_enqueue(packages[0].id, None)
        .await
        .expect("claim");
    if matches!(when_written, EnrichedAt::WhileTheRowsAreWritten) {
        enrich().await.expect("enrichment");
    }
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "release".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            // What the claim saw, which is what the enqueue has to work from.
            enrichment: claimed[0].0.enrichment.clone(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: url.clone(),
            file_name: "release.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: claimed[0].0.enrichment.clone(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    database
        .finish_package_enqueue(packages[0].id, true, Vec::new())
        .await
        .expect("finish");
    if matches!(when_written, EnrichedAt::AfterTheEnqueue) {
        enrich().await.expect("enrichment");
    }

    let package = database
        .list_packages()
        .await
        .expect("packages")
        .into_iter()
        .find(|package| package.id == package_id)
        .expect("package");
    assert_eq!(
        package.enrichment.len(),
        1,
        "the package must carry the field, {when_written:?}"
    );
    assert_eq!(package.enrichment[0].name, "imdb.score");
    assert_eq!(package.enrichment[0].plugin_id, "imdb-enricher");
    let stored = database
        .get_download(download.id)
        .await
        .expect("download read")
        .expect("download");
    assert_eq!(
        stored.enrichment.len(),
        1,
        "the queue row must carry it too, {when_written:?}"
    );
    assert_eq!(stored.enrichment[0].value, "9.3");
}

#[tokio::test]
async fn enrichment_written_before_the_claim_reaches_the_package_and_the_download() {
    enrichment_survives_the_enqueue(EnrichedAt::BeforeTheClaim).await;
}

#[tokio::test]
async fn enrichment_written_while_the_enqueue_runs_reaches_the_package_and_the_download() {
    enrichment_survives_the_enqueue(EnrichedAt::WhileTheRowsAreWritten).await;
}

#[tokio::test]
async fn enrichment_written_after_the_enqueue_reaches_the_package_and_the_download() {
    enrichment_survives_the_enqueue(EnrichedAt::AfterTheEnqueue).await;
}

/// RD-107-06: the duplicate guard of a job that runs at a provider, at the level it is made.
///
/// `torrents/addMagnet` is not idempotent — it answers with a new id every time — so the only
/// place a second submit can be prevented is *before* the first one: a row carrying the content
/// key, written first, with a unique index behind it. A person pasting the same magnet twice
/// therefore finds the job they already started instead of starting a second one, and so does a
/// restart that replays the same intake.
#[tokio::test]
async fn a_second_remote_job_for_the_same_content_is_refused_rather_than_created() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("remote-jobs.sqlite"))
        .await
        .expect("database");
    let account = database
        .create_account(NewAccount {
            provider: "realdebrid".to_owned(),
            label: "Real-Debrid".to_owned(),
            username: Some("client-id".to_owned()),
            credential_mode: None,
            secret_ref: Some("secret://realdebrid/client-secret".to_owned()),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");

    let claim = |id: rd_core::RemoteJobId| crate::ClaimRemoteJob {
        id,
        account_id: account.id,
        plugin_id: "019d0000-0000-7000-8000-00000000011d".to_owned(),
        content_key: "c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2".to_owned(),
        source_kind: rd_core::RemoteJobSourceKind::Magnet,
        source: b"magnet:?xt=urn:btih:c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2".to_vec(),
        package_id: None,
    };

    let first = database
        .claim_remote_job(claim(rd_core::RemoteJobId::new()))
        .await
        .expect("first claim");
    assert_eq!(first.state, rd_core::RemoteJobState::Submitting);
    assert!(first.remote_id.is_none(), "nothing has been submitted yet");

    // The second claim is refused, not merged and not overwritten: a row that silently became
    // a different job would be the duplicate arriving by another door.
    database
        .claim_remote_job(claim(rd_core::RemoteJobId::new()))
        .await
        .expect_err("a second job for the same content on the same account");

    // And the caller can find what it collided with, which is what makes the refusal usable.
    let existing = database
        .remote_job_by_content(account.id, &first.content_key)
        .await
        .expect("lookup")
        .expect("the job that was already there");
    assert_eq!(existing.id, first.id);
}

/// A restart in the middle of polling must not start a second job at the provider.
///
/// Three things together make that true, and this exercises all three: the identifier is
/// written the moment the provider names it, a written identifier ends the submit path for
/// ever, and a row that ended cannot be walked back into a state that would submit again.
#[tokio::test]
async fn a_remote_job_that_was_named_by_the_provider_never_submits_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("remote-restart.sqlite"))
        .await
        .expect("database");
    let account = database
        .create_account(NewAccount {
            provider: "realdebrid".to_owned(),
            label: "Real-Debrid".to_owned(),
            username: Some("client-id".to_owned()),
            credential_mode: None,
            secret_ref: Some("secret://realdebrid/client-secret".to_owned()),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    let id = rd_core::RemoteJobId::new();
    database
        .claim_remote_job(crate::ClaimRemoteJob {
            id,
            account_id: account.id,
            plugin_id: "019d0000-0000-7000-8000-00000000011d".to_owned(),
            content_key: "deadbeef".to_owned(),
            source_kind: rd_core::RemoteJobSourceKind::Magnet,
            source: b"magnet:?xt=urn:btih:deadbeef".to_vec(),
            package_id: None,
        })
        .await
        .expect("claim");

    // The submit went out. Before the answer comes back, the attempt is on the row: that is
    // what tells a restart to ask the provider what it holds rather than to send again.
    let attempted = database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                count_submit_attempt: true,
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("attempt counted");
    assert_eq!(attempted.submit_attempts, 1);
    assert_eq!(attempted.submit_step(), rd_core::SubmitStep::Adopt);

    // The identifier arrives and is written as the very next thing that happens.
    let named = database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                remote_id: Some("RDTORRENT1".to_owned()),
                state: Some(rd_core::RemoteJobState::Preparing),
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("named");
    assert_eq!(named.remote_id.as_deref(), Some("RDTORRENT1"));
    assert_eq!(named.submit_step(), rd_core::SubmitStep::Poll);

    // A restart re-reads the row and reaches the same conclusion, which is the whole point of
    // the row being where the state lives.
    let reopened = database.remote_job(id).await.expect("read").expect("job");
    assert_eq!(reopened.submit_step(), rd_core::SubmitStep::Poll);

    // A second identifier is a second job at the provider; overwriting would lose the first.
    database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                remote_id: Some("RDTORRENT2".to_owned()),
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect_err("a second remote identifier");

    // And nothing walks a job back into the state that would submit.
    database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                state: Some(rd_core::RemoteJobState::Submitting),
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect_err("back to submitting");
}

/// A late poll answer must not resurrect a job somebody deleted at the provider.
#[tokio::test]
async fn a_discarded_remote_job_is_not_brought_back_by_a_late_answer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("remote-discard.sqlite"))
        .await
        .expect("database");
    let account = database
        .create_account(NewAccount {
            provider: "realdebrid".to_owned(),
            label: "Real-Debrid".to_owned(),
            username: Some("client-id".to_owned()),
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    let id = rd_core::RemoteJobId::new();
    database
        .claim_remote_job(crate::ClaimRemoteJob {
            id,
            account_id: account.id,
            plugin_id: "019d0000-0000-7000-8000-00000000011d".to_owned(),
            content_key: "feedface".to_owned(),
            source_kind: rd_core::RemoteJobSourceKind::Container,
            source: b"d8:announce".to_vec(),
            package_id: None,
        })
        .await
        .expect("claim");
    database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                remote_id: Some("RDTORRENT3".to_owned()),
                state: Some(rd_core::RemoteJobState::Discarded),
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect("discarded");
    database
        .advance_remote_job(
            id,
            crate::AdvanceRemoteJob {
                state: Some(rd_core::RemoteJobState::Working),
                ..crate::AdvanceRemoteJob::default()
            },
        )
        .await
        .expect_err("a late poll answer");
    // The row stays, so the person can still see what happened to it.
    let job = database.remote_job(id).await.expect("read").expect("job");
    assert_eq!(job.state, rd_core::RemoteJobState::Discarded);
}

/// Which records hold a plugin version, and which ones only remember it (RD-108-10).
///
/// Installing a plugin never removes the older version, because a job already under way keeps
/// the version that started it. Clearing the leftovers by hand therefore needs one honest
/// answer to "is anything still bound to this version", and the two records that name a
/// version are the resolver pin and the transfer checkpoint.
#[tokio::test]
async fn only_unfinished_work_holds_a_plugin_version() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("plugin-versions.sqlite"))
        .await
        .expect("database");
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "versions".to_owned(),
            destination: directory.path().to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let new_download = |name: &str| NewDownload {
        id: DownloadId::new(),
        package_id,
        source: probe_url(),
        file_name: name.to_owned(),
        total_bytes: None,
        expected_checksum: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: SELECTION,
        initial_state: rd_core::DownloadState::Queued,
        kind: rd_core::DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        enrichment: Vec::new(),
        secret_fragment: None,
    };
    let pinned = database
        .create_download(new_download("pinned.bin"))
        .await
        .expect("pinned download");
    let checkpointed = database
        .create_download(new_download("checkpointed.bin"))
        .await
        .expect("checkpointed download");

    let plugin = rd_core::PluginId::new();
    let id = plugin.to_string();
    database
        .claim_resolver_pin(
            pinned.id,
            rd_core::ResolverPin {
                plugin_id: plugin,
                version: "1.0.0".to_owned(),
            },
        )
        .await
        .expect("pin");
    database
        .save_plugin_transfer(
            checkpointed.id,
            id.clone(),
            "1.0.0".to_owned(),
            Some(b"resume".to_vec()),
        )
        .await
        .expect("checkpoint");

    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage"),
        2,
        "the pin and the checkpoint each hold the version they name"
    );
    // The version an upgrade installed beside it holds nothing on its own.
    assert_eq!(
        database
            .plugin_version_usage(&id, "2.0.0")
            .await
            .expect("usage of the newer version"),
        0
    );

    // The blockers are named, oldest first, so a refusal can point at the job in the way.
    assert_eq!(
        database
            .plugin_version_blockers(&id, "1.0.0", 2)
            .await
            .expect("blockers"),
        vec!["pinned.bin".to_owned(), "checkpointed.bin".to_owned()]
    );

    // The one exception the query makes, and the one the documentation states as the rule: a
    // completed download never runs again, so it releases the version the moment it finishes.
    let finished = database
        .create_download(new_download("finished.bin"))
        .await
        .expect("finished download");
    database
        .claim_resolver_pin(
            finished.id,
            rd_core::ResolverPin {
                plugin_id: plugin,
                version: "1.0.0".to_owned(),
            },
        )
        .await
        .expect("pin the third download");
    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage while all three are unfinished"),
        3
    );
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
        rd_core::DownloadState::Completed,
    ] {
        database
            .transition_download(finished.id, state)
            .await
            .expect("transition towards completion");
    }
    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage after one of them completed"),
        2,
        "a completed download releases the version it pinned"
    );

    // Cancelling releases nothing. `ProgressControl::cancel` keeps the partial data and
    // `resume` puts the job back in the queue, so the version it would resume with is still
    // spoken for.
    database
        .transition_download(pinned.id, rd_core::DownloadState::Cancelled)
        .await
        .expect("cancel");
    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage after cancelling"),
        2,
        "a cancelled download can be resumed, so it keeps its pin"
    );

    // Deleting it does, and takes the pin with it.
    database
        .delete_download(pinned.id)
        .await
        .expect("delete the cancelled download");
    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage after deleting"),
        1
    );

    database
        .clear_plugin_transfer(checkpointed.id)
        .await
        .expect("clear checkpoint");
    assert_eq!(
        database
            .plugin_version_usage(&id, "1.0.0")
            .await
            .expect("usage after the transfer finished"),
        0
    );
    assert!(
        database
            .plugin_version_blockers(&id, "1.0.0", 2)
            .await
            .expect("blockers")
            .is_empty()
    );
}

/// The per-package read is what keeps a completion check off the whole `downloads` table, so
/// it has to return exactly that package's files, in queue order, and nothing else.
#[tokio::test]
async fn downloads_for_package_returns_only_that_package_in_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("per-package.sqlite"))
        .await
        .expect("database");
    let mut packages = Vec::new();
    for (name, files) in [
        ("first", ["a.bin", "b.bin"]),
        ("second", ["c.bin", "d.bin"]),
    ] {
        let package_id = PackageId::new();
        database
            .create_package(NewPackage {
                id: package_id,
                name: name.to_owned(),
                destination: directory.path().to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        for file_name in files {
            database
                .create_download(NewDownload {
                    id: DownloadId::new(),
                    package_id,
                    source: format!("https://example.test/{file_name}")
                        .parse()
                        .expect("URL"),
                    file_name: file_name.to_owned(),
                    total_bytes: None,
                    expected_checksum: None,
                    account_id: None,
                    proxy_profile_id: None,
                    auth_profile: SELECTION,
                    initial_state: rd_core::DownloadState::Queued,
                    kind: rd_core::DownloadKind::Http,
                    media: None,
                    remote_credential_id: None,
                    replay: None,
                    mirror_group: None,
                    enrichment: Vec::new(),
                    secret_fragment: None,
                })
                .await
                .expect("download");
        }
        packages.push(package_id);
    }

    let names = |files: Vec<rd_core::DownloadFile>| {
        files
            .into_iter()
            .map(|file| file.file_name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(
            database
                .downloads_for_package(packages[0])
                .await
                .expect("first package")
        ),
        ["a.bin", "b.bin"]
    );
    assert_eq!(
        names(
            database
                .downloads_for_package(packages[1])
                .await
                .expect("second package")
        ),
        ["c.bin", "d.bin"]
    );
    assert!(
        database
            .downloads_for_package(PackageId::new())
            .await
            .expect("unknown package")
            .is_empty(),
        "a package that does not exist has no files, not everybody else's"
    );
}

/// The delivery history is trimmed in the write that queues a delivery, and a delivery the
/// worker still owes an attempt is never the row that gets dropped.
#[tokio::test]
async fn notification_deliveries_are_trimmed_per_rule_but_never_while_pending() {
    use rd_notify::{DeliveryState, NotificationEvent, Severity, TargetKind};

    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("notifications.sqlite"))
        .await
        .expect("database");
    let target = database
        .upsert_notification_target(
            None,
            crate::NewNotificationTarget {
                name: "Webhook".to_owned(),
                kind: TargetKind::Webhook,
                enabled: true,
                endpoint: "https://example.test/hook".to_owned(),
                config: serde_json::json!({}),
                secret_ref: None,
                clear_secret: false,
            },
        )
        .await
        .expect("target");
    let rule = database
        .upsert_notification_rule(
            None,
            crate::NewNotificationRule {
                name: "Everything".to_owned(),
                enabled: true,
                target_id: target.id,
                events: vec![NotificationEvent::PackageCompleted],
                category_id: None,
                min_severity: Severity::Info,
            },
        )
        .await
        .expect("rule");

    async fn queue(database: &Database, rule: &rd_notify::NotificationRule, index: i64) {
        assert!(
            database
                .queue_notification_delivery(crate::NewDelivery {
                    rule_id: rule.id,
                    target_id: rule.target_id,
                    idempotency_key: format!("event-{index}"),
                    event: NotificationEvent::PackageCompleted,
                    title: format!("Package {index}"),
                    body: "done".to_owned(),
                })
                .await
                .expect("queue delivery"),
            "each key is fresh, so each insert must be a new row"
        );
    }
    async fn deliveries(database: &Database) -> Vec<rd_notify::Delivery> {
        database
            .list_notification_deliveries(u32::MAX)
            .await
            .expect("deliveries")
    }

    let cap = crate::notify_store::MAX_DELIVERIES_PER_RULE;
    for index in 0..=cap {
        queue(&database, &rule, index).await;
    }
    assert_eq!(
        deliveries(&database).await.len() as i64,
        cap + 1,
        "nothing was attempted yet, so the trim had nothing it was allowed to drop"
    );

    // Settle everything but the very first one; that one stays queued and must survive.
    for delivery in deliveries(&database).await {
        if delivery.title != "Package 0" {
            database
                .record_notification_attempt(
                    delivery.id,
                    DeliveryState::Delivered,
                    1,
                    None,
                    Some(200),
                    None,
                )
                .await
                .expect("record attempt");
        }
    }
    queue(&database, &rule, cap + 1).await;

    let stored = deliveries(&database).await;
    assert_eq!(
        stored.len() as i64,
        cap + 1,
        "the trim keeps the newest {cap} plus the pending row it refused to drop"
    );
    assert!(
        stored.iter().any(
            |delivery| delivery.title == "Package 0" && delivery.state == DeliveryState::Queued
        ),
        "trimming a queued delivery would discard the notification, not just its record"
    );
}

/// One collector package holding one link, as a LinkGrabber entry of the "collector" kind.
async fn grabber_package(database: &Database, url: &str) -> rd_core::CollectorPackageId {
    let (_, packages, _) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            urls: vec![url.parse().expect("URL")],
            providers: vec![None],
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    packages[0].id
}

fn collector_entry(id: rd_core::CollectorPackageId) -> rd_core::GrabberEntryRef {
    rd_core::GrabberEntryRef {
        kind: rd_core::GrabberEntryKind::Collector,
        id: id.into_uuid(),
    }
}

fn nzb_entry(id: rd_core::NzbImportId) -> rd_core::GrabberEntryRef {
    rd_core::GrabberEntryRef {
        kind: rd_core::GrabberEntryKind::Nzb,
        id: id.into_uuid(),
    }
}

/// Both kinds draw from one position sequence, so a mixed order survives a round trip.
///
/// The interleaving is the whole point: two independent sequences can only ever put one kind
/// before the other, which is why an NZB import could not be dragged between two packages.
#[tokio::test]
async fn a_mixed_linkgrabber_order_round_trips_through_one_position_sequence() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("grabber.sqlite"))
        .await
        .expect("database");
    let first_package = grabber_package(&database, "https://ddownload.com/first").await;
    let first_import = database
        .add_nzb_import(dropped_nzb(
            "first.nzb",
            &"a1".repeat(32),
            None,
            IngressSource::Manual,
            None,
        ))
        .await
        .expect("first import")
        .id;
    let second_package = grabber_package(&database, "https://ddownload.com/second").await;
    let second_import = database
        .add_nzb_import(dropped_nzb(
            "second.nzb",
            &"b2".repeat(32),
            None,
            IngressSource::Manual,
            None,
        ))
        .await
        .expect("second import")
        .id;

    database
        .reorder_grabber_entries(
            vec![
                nzb_entry(second_import),
                collector_entry(first_package),
                nzb_entry(first_import),
                collector_entry(second_package),
            ],
            None,
        )
        .await
        .expect("reorder");

    let packages = database
        .list_collector_packages()
        .await
        .expect("packages")
        .into_iter()
        .map(|package| (package.id, package.position))
        .collect::<Vec<_>>();
    let imports = database
        .list_nzb_imports()
        .await
        .expect("imports")
        .into_iter()
        .map(|import| (import.id, import.position))
        .collect::<Vec<_>>();
    assert_eq!(packages, vec![(first_package, 2), (second_package, 4)]);
    assert_eq!(imports, vec![(second_import, 1), (first_import, 3)]);
}

/// An entry that names no row is refused, and the order that was there stays untouched.
///
/// The refusal has to happen before anything is written: the unknown id updates no row and
/// reports success, so the remaining entries would silently take the positions of the list the
/// caller thought it was sending.
#[tokio::test]
async fn a_reorder_naming_an_unknown_entry_is_refused_and_writes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("unknown.sqlite"))
        .await
        .expect("database");
    let package = grabber_package(&database, "https://ddownload.com/only").await;
    let import = database
        .add_nzb_import(dropped_nzb(
            "only.nzb",
            &"c3".repeat(32),
            None,
            IngressSource::Manual,
            None,
        ))
        .await
        .expect("import")
        .id;
    let before = (
        database.list_collector_packages().await.expect("packages")[0].position,
        database.list_nzb_imports().await.expect("imports")[0].position,
    );

    let error = database
        .reorder_grabber_entries(
            vec![
                nzb_entry(import),
                collector_entry(package),
                collector_entry(rd_core::CollectorPackageId::new()),
            ],
            None,
        )
        .await
        .expect_err("unknown entry");
    assert_eq!(
        crate::store_kind(&error),
        Some(crate::StoreErrorKind::NotFound)
    );

    // An id of the wrong kind is the same mistake by another route: both are UUIDs, so only the
    // declared kind says which table to look in.
    let wrong_kind = database
        .reorder_grabber_entries(
            vec![nzb_entry(rd_core::NzbImportId::from_uuid(
                package.into_uuid(),
            ))],
            None,
        )
        .await
        .expect_err("wrong kind");
    assert_eq!(
        crate::store_kind(&wrong_kind),
        Some(crate::StoreErrorKind::NotFound)
    );

    let after = (
        database.list_collector_packages().await.expect("packages")[0].position,
        database.list_nzb_imports().await.expect("imports")[0].position,
    );
    assert_eq!(before, after);
}

/// Upgrading to the shared sequence does not move anything the list was already showing.
///
/// Migration 0074 is the only place where existing rows of both tables are numbered against each
/// other, and getting it wrong reshuffles every LinkGrabber in the field exactly once. The two
/// cases are the two orders that existed before it: nobody had dragged, and somebody had.
#[tokio::test]
async fn the_shared_order_backfill_preserves_the_order_the_list_showed_before() {
    for dragged in [false, true] {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("backfill.sqlite");
        let mut connection = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .expect("connect");

        // The schema as an installation in the field has it: everything up to, but excluding,
        // the migration under test.
        let before = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                sqlx::migrate!()
                    .iter()
                    .filter(|migration| migration.version < 74)
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            ignore_missing: false,
            locking: true,
            no_tx: false,
        };
        before.run(&mut connection).await.expect("migrate to 0073");

        sqlx::query("INSERT INTO collector_batches (id, source, created_at) VALUES ('batch', 'manual', '2026-01-01T00:00:00Z')")
            .execute(&mut connection)
            .await
            .expect("batch");
        // Four entries, alternating by creation time: package, import, package, import.
        for (id, created_at, position) in [
            ("pkg-a", "2026-01-01T00:00:01Z", i64::from(dragged) * 2),
            ("pkg-b", "2026-01-01T00:00:03Z", i64::from(dragged)),
        ] {
            sqlx::query(
                "INSERT INTO collector_packages (id, batch_id, name, auto_named, priority, position, created_at, updated_at) \
                 VALUES (?, 'batch', ?, 0, 0, ?, ?, ?)",
            )
            .bind(id)
            .bind(id)
            .bind(position)
            .bind(created_at)
            .bind(created_at)
            .execute(&mut connection)
            .await
            .expect("package");
        }
        for (id, created_at) in [
            ("nzb-a", "2026-01-01T00:00:02Z"),
            ("nzb-b", "2026-01-01T00:00:04Z"),
        ] {
            sqlx::query(
                "INSERT INTO nzb_imports (id, name, sha256, state, file_count, segment_count, total_bytes, import_mode, created_at, updated_at) \
                 VALUES (?, ?, ?, 'imported', 0, 0, 0, 'review', ?, ?)",
            )
            .bind(id)
            .bind(id)
            .bind(id)
            .bind(created_at)
            .bind(created_at)
            .execute(&mut connection)
            .await
            .expect("import");
        }

        sqlx::migrate!()
            .run(&mut connection)
            .await
            .expect("migrate to 0074");

        let order: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM (SELECT id AS id, position AS position, created_at AS created_at FROM collector_packages \
             UNION ALL SELECT id, position, created_at FROM nzb_imports) ORDER BY position, created_at, id",
        )
        .fetch_all(&mut connection)
        .await
        .expect("order");
        let positions: Vec<i64> = sqlx::query_scalar(
            "SELECT position FROM (SELECT id AS id, position AS position, created_at AS created_at FROM collector_packages \
             UNION ALL SELECT id, position, created_at FROM nzb_imports) ORDER BY position, created_at, id",
        )
        .fetch_all(&mut connection)
        .await
        .expect("positions");

        assert_eq!(positions, vec![1, 2, 3, 4], "one gapless shared sequence");
        if dragged {
            // Spine after the drag: pkg-b, pkg-a. The client emitted nzb-a first (pkg-b is newer
            // than it) and nzb-b last (nothing in the spine is newer).
            assert_eq!(order, vec!["nzb-a", "pkg-b", "pkg-a", "nzb-b"]);
        } else {
            assert_eq!(order, vec!["pkg-a", "nzb-a", "pkg-b", "nzb-b"]);
        }
    }
}

/// A drag deep in the list names the moved row and the row it landed behind, nothing else.
///
/// Without the anchor a partial list could only describe a prefix, so moving the last of four
/// entries one step up meant sending three of them - and in a real LinkGrabber, hundreds. The
/// anchor is what keeps the request the same size wherever the row sits.
#[tokio::test]
async fn an_anchored_reorder_moves_one_entry_without_naming_the_rows_above_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("anchor.sqlite"))
        .await
        .expect("database");
    let first = grabber_package(&database, "https://ddownload.com/one").await;
    let second = grabber_package(&database, "https://ddownload.com/two").await;
    let third = grabber_package(&database, "https://ddownload.com/three").await;
    let import = database
        .add_nzb_import(dropped_nzb(
            "last.nzb",
            &"d4".repeat(32),
            None,
            IngressSource::Manual,
            None,
        ))
        .await
        .expect("import")
        .id;

    // The import is last; drop it behind the first package without mentioning the other two.
    database
        .reorder_grabber_entries(vec![nzb_entry(import)], Some(collector_entry(first)))
        .await
        .expect("anchored reorder");

    let packages = database
        .list_collector_packages()
        .await
        .expect("packages")
        .into_iter()
        .map(|package| (package.id, package.position))
        .collect::<Vec<_>>();
    let imports = database
        .list_nzb_imports()
        .await
        .expect("imports")
        .into_iter()
        .map(|item| (item.id, item.position))
        .collect::<Vec<_>>();
    assert_eq!(
        packages,
        vec![(first, 1), (second, 3), (third, 4)],
        "the untouched packages keep their relative order and close up behind the import"
    );
    assert_eq!(imports, vec![(import, 2)]);

    // An anchor that names no row is refused, exactly like an unknown entry: splicing behind
    // nothing would silently fall back to the head of the list.
    let error = database
        .reorder_grabber_entries(
            vec![nzb_entry(import)],
            Some(collector_entry(rd_core::CollectorPackageId::new())),
        )
        .await
        .expect_err("unknown anchor");
    assert_eq!(
        crate::store_kind(&error),
        Some(crate::StoreErrorKind::NotFound)
    );
}

/// Two obfuscated rows of one package, ready for a verdict to be held open on the first.
///
/// Neither subject says anything about PAR2 — the case RD-108-23 named as its remaining
/// limit — so nothing but the content of the assembled files can answer the question.
async fn obfuscated_pair(
    directory: &std::path::Path,
    digest: &str,
    names: &[&str],
) -> (Database, rd_core::PackageId, Vec<rd_core::DownloadFile>) {
    let database = Database::open(directory.join("verdict.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "obfuscated.nzb".to_owned(),
            sha256: digest.repeat(32),
            category_id: None,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source: IngressSource::Manual,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: names.iter().map(|name| nzb_file(name)).collect(),
        })
        .await
        .expect("import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    let rows = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package.id)
        .collect();
    (database, package.id, rows)
}

/// Drives a row to `Verifying` and holds its verdict open, the way the Usenet runner does
/// when a file is assembled with holes (RD-108-24).
async fn defer_in_verifying(database: &Database, id: rd_core::DownloadId, missing: usize) {
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
    ] {
        database
            .transition_download(id, state)
            .await
            .expect("on its way");
    }
    database
        .defer_par2_verdict(id, missing)
        .await
        .expect("defer the verdict");
    database
        .transition_download(id, rd_core::DownloadState::Verifying)
        .await
        .expect("held for the verdict");
}

fn row_of(rows: &[rd_core::DownloadFile], name: &str) -> rd_core::DownloadFile {
    rows.iter()
        .find(|file| file.file_name == name)
        .cloned()
        .unwrap_or_else(|| panic!("no row for {name}"))
}

async fn state_of(database: &Database, id: rd_core::DownloadId) -> rd_core::DownloadFile {
    database
        .get_download(id)
        .await
        .expect("row")
        .expect("row exists")
}

/// RD-108-24: the verdict waits for the set and then sends the file to repair.
///
/// The payload finishes first with a hole and nothing in the package has declared itself as
/// PAR2 yet — the exact moment at which the old code said `usenet.segments_missing_no_par2`
/// about a set that carries PAR2. The row waits instead, and when the second file turns out
/// to be recovery data the verdict is `Completed`, which is what hands it to the repair.
#[tokio::test]
async fn a_held_verdict_completes_once_the_settled_set_turns_out_to_carry_par2() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, _package, rows) =
        obfuscated_pair(directory.path(), "d1", &["a1b2c3.bin", "d4e5f6.bin"]).await;
    let payload = row_of(&rows, "a1b2c3.bin");
    let other = row_of(&rows, "d4e5f6.bin");

    defer_in_verifying(&database, payload.id, 2).await;

    let held = state_of(&database, payload.id).await;
    assert_eq!(
        held.state,
        rd_core::DownloadState::Verifying,
        "a sibling is still queued, so nothing is decided yet"
    );
    assert_eq!(
        held.last_error
            .as_ref()
            .and_then(|error| error.code.clone()),
        Some("usenet.segments_missing_awaiting_par2".to_owned()),
        "the row says what it is waiting for"
    );

    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
    ] {
        database
            .transition_download(other.id, state)
            .await
            .expect("sibling on its way");
    }
    database
        .settle_nzb_recovery(other.id, "d4e5f6.bin".to_owned(), true)
        .await
        .expect("settle the sibling as PAR2 by content");
    database
        .complete_download(other.id, "d4e5f6.bin".to_owned(), None)
        .await
        .expect("sibling complete");

    let decided = state_of(&database, payload.id).await;
    assert_eq!(
        decided.state,
        rd_core::DownloadState::Completed,
        "the settled set carries PAR2, so the hole is the repair's business: {:?}",
        decided.last_error
    );
    assert!(
        decided.last_error.is_none(),
        "the note about the open verdict is cleared with the verdict"
    );
}

/// RD-108-24: the limit this job must not move — a set without PAR2 still fails, with the
/// same code and the same `missing` count, as soon as the set has settled.
#[tokio::test]
async fn a_held_verdict_fails_with_the_old_message_when_the_settled_set_has_no_par2() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, _package, rows) =
        obfuscated_pair(directory.path(), "d2", &["a1b2c3.bin", "d4e5f6.bin"]).await;
    let payload = row_of(&rows, "a1b2c3.bin");
    let other = row_of(&rows, "d4e5f6.bin");

    defer_in_verifying(&database, payload.id, 3).await;
    assert_eq!(
        state_of(&database, payload.id).await.state,
        rd_core::DownloadState::Verifying
    );

    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
    ] {
        database
            .transition_download(other.id, state)
            .await
            .expect("sibling on its way");
    }
    database
        .complete_download(other.id, "d4e5f6.bin".to_owned(), None)
        .await
        .expect("sibling complete");

    let decided = state_of(&database, payload.id).await;
    assert_eq!(decided.state, rd_core::DownloadState::Failed);
    let failure = decided.last_error.expect("a failure is recorded");
    assert_eq!(
        failure.code.as_deref(),
        Some("usenet.segments_missing_no_par2"),
        "the message a set without PAR2 has always given"
    );
    assert_eq!(
        failure.params.get("missing").map(String::as_str),
        Some("3"),
        "the count survives the wait"
    );
}

/// RD-108-24: a restart in the middle of the open verdict keeps the file and takes the
/// verdict, rather than fetching the whole file again to ask the same question.
///
/// The row is left exactly as a crash between the two writes leaves it: `Verifying`, marked,
/// and with nothing that would transition it again.
#[tokio::test]
async fn a_restart_decides_a_held_verdict_whose_set_has_nothing_left_running() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, _package, rows) = obfuscated_pair(directory.path(), "d3", &["a1b2c3.bin"]).await;
    let payload = row_of(&rows, "a1b2c3.bin");
    for state in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
    ] {
        database
            .transition_download(payload.id, state)
            .await
            .expect("on its way");
    }
    database
        .defer_par2_verdict(payload.id, 1)
        .await
        .expect("defer the verdict");

    database.recover_interrupted().await.expect("recovery");

    let decided = state_of(&database, payload.id).await;
    assert_eq!(
        decided.state,
        rd_core::DownloadState::Failed,
        "nothing is left that could bring PAR2, so the verdict is final"
    );
    assert_eq!(
        decided
            .last_error
            .and_then(|failure| failure.code)
            .as_deref(),
        Some("usenet.segments_missing_no_par2")
    );
}

/// RD-108-24: a restart does not requeue a row whose verdict is open, and does not decide it
/// while the package still has work queued.
#[tokio::test]
async fn a_restart_leaves_a_held_verdict_open_while_a_sibling_is_still_queued() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, _package, rows) =
        obfuscated_pair(directory.path(), "d4", &["a1b2c3.bin", "d4e5f6.bin"]).await;
    let payload = row_of(&rows, "a1b2c3.bin");
    let other = row_of(&rows, "d4e5f6.bin");
    defer_in_verifying(&database, payload.id, 2).await;

    database.recover_interrupted().await.expect("recovery");

    let held = state_of(&database, payload.id).await;
    assert_eq!(
        held.state,
        rd_core::DownloadState::Verifying,
        "the assembled file is kept; requeueing it would fetch it all again"
    );
    assert_eq!(
        held.last_error.and_then(|failure| failure.code).as_deref(),
        Some("usenet.segments_missing_awaiting_par2")
    );
    assert_eq!(
        state_of(&database, other.id).await.state,
        rd_core::DownloadState::Queued,
        "the sibling is what the verdict is still waiting for"
    );
}

/// A password somebody appended to an address never reaches a candidate row (RD-109-32).
///
/// RD-108-07 closed this for a link a crawler claims: the fragment becomes an encrypted auth
/// profile and the address is stored bare. A link no crawler claims took the other path and
/// kept its fragment — one typo in the host name was enough to write a share password into
/// `link_candidates.url` in clear text and show it in every LinkGrabber row. Every intake path
/// ends in this writer, so the rule lives here rather than in one of the six handlers.
#[tokio::test]
async fn a_password_in_a_fragment_never_reaches_a_candidate_row() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("fragment.sqlite");
    let database = Database::open(path.clone()).await.expect("database");
    let (_, _, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            urls: vec![
                "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4#s3cret"
                    .parse()
                    .expect("url"),
            ],
            providers: vec![None],
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].url.as_str(),
        "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4",
        "the stored address is the pasted one without its fragment"
    );

    // The column itself, not the parsed value: this is the row a backup keeps and the
    // interface prints.
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("pool");
    let stored: String = sqlx::query_scalar("SELECT url FROM link_candidates")
        .fetch_one(&pool)
        .await
        .expect("stored url");
    pool.close().await;
    assert!(
        !stored.contains("s3cret") && !stored.contains('#'),
        "the candidate row still carries the password: {stored}"
    );
}

/// Two different situations must not reach the reader as one sentence.
///
/// Reported from use (RD-109-43): two links of two different hosters both stood in the
/// LinkGrabber with „Check result missing", `0 B` and the mark `duplicate`. The mark is
/// correct — both addresses had been added before — but the sentence is not: a `duplicate`
/// row can only be reached through the branch that *has* a result, so the plugin had
/// answered, and what it answered was `Unknown`. „Check result missing" is what
/// `link_check_service` passed unconditionally, for the URL a plugin skipped and for the URL
/// a plugin could not judge alike.
#[tokio::test]
async fn an_unknown_answer_and_a_missing_answer_say_different_things() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("check-message.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = [
        "https://1fichier.com/?8x6wertoi51r8vptrojn",
        "https://rapidgator.net/file/4b8480192d90789c5040bb6e43ff4976/Movie.rar.html",
    ]
    .iter()
    .map(|value| value.parse().expect("URL"))
    .collect();
    let (_batch, _packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: Some("test".to_owned()),
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");

    // The plugin answered about this URL and said it cannot tell.
    database
        .record_candidate_check(
            candidates[0].id,
            Some(rd_core::LinkCheckResult {
                url: candidates[0].url.clone(),
                status: rd_core::LinkStatus::Unknown,
                file_name: None,
                size: None,
                media: None,
            }),
            Some(rd_core::CandidateMessage::coded(
                "collector.check_unknown_no_account",
                "The hoster could not say whether this link is still available",
            )),
            true,
            None,
        )
        .await
        .expect("record");
    // The plugin answered for the batch but said nothing about this URL.
    database
        .record_candidate_check(
            candidates[1].id,
            None,
            Some(rd_core::CandidateMessage::coded(
                "collector.check_no_result",
                "The check returned no answer for this link",
            )),
            false,
            None,
        )
        .await
        .expect("record");

    let listed = database.list_candidates().await.expect("candidates");
    let unknown = listed
        .iter()
        .find(|c| c.id == candidates[0].id)
        .expect("unknown row");
    let missing = listed
        .iter()
        .find(|c| c.id == candidates[1].id)
        .expect("missing row");
    assert_eq!(
        unknown.state,
        rd_core::LinkCandidateState::Duplicate,
        "the reported row: the duplicate mark survives an Unknown answer"
    );
    assert_ne!(
        unknown.error, missing.error,
        "an answer of `Unknown` and no answer at all are not the same thing"
    );
    // The code is what the interface translates; the text is only the English fallback.
    assert_eq!(
        unknown.error_code.as_deref(),
        Some("collector.check_unknown_no_account")
    );
    assert_eq!(
        missing.error_code.as_deref(),
        Some("collector.check_no_result")
    );
    assert_eq!(missing.state, rd_core::LinkCandidateState::Error);
}

/// A conclusive answer clears the message a previous check left behind.
///
/// Without this the sentence and the state beside it contradict each other: a link that was
/// `Unknown` an hour ago and is `online` now would still carry "the hoster could not say".
#[tokio::test]
async fn a_conclusive_answer_clears_the_previous_message() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("check-cleared.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = ["https://1fichier.com/?8x6wertoi51r8vptrojn"]
        .iter()
        .map(|value| value.parse().expect("URL"))
        .collect();
    let (_batch, _packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: Some("test".to_owned()),
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    let id = candidates[0].id;
    let url = candidates[0].url.clone();
    database
        .record_candidate_check(
            id,
            Some(rd_core::LinkCheckResult {
                url: url.clone(),
                status: rd_core::LinkStatus::Unknown,
                file_name: None,
                size: None,
                media: None,
            }),
            Some(rd_core::CandidateMessage::coded(
                "collector.check_unknown",
                "The hoster could not say whether this link is still available",
            )),
            false,
            None,
        )
        .await
        .expect("record");
    database
        .claim_candidates_for_check(vec![id])
        .await
        .expect("claim");
    database
        .record_candidate_check(
            id,
            Some(rd_core::LinkCheckResult {
                url,
                status: rd_core::LinkStatus::Online,
                file_name: Some("Movie.rar".to_owned()),
                size: rd_core::ByteCount::new(425_123_456).ok(),
                media: None,
            }),
            None,
            false,
            None,
        )
        .await
        .expect("record");
    let listed = database.list_candidates().await.expect("candidates");
    let row = listed.iter().find(|c| c.id == id).expect("row");
    assert_eq!(row.state, rd_core::LinkCandidateState::Online);
    assert_eq!(row.error, None);
    assert_eq!(row.error_code, None);
}

/// A LinkGrabber batch with one mirror hint per link, everything else left at nothing.
fn mirror_batch(
    urls: &[&str],
    file_names: Vec<Option<String>>,
    hints: Vec<Option<rd_core::MirrorHint>>,
) -> crate::NewCollectorBatch {
    crate::NewCollectorBatch {
        package_hints: Vec::new(),
        mirror_hints: hints,
        source: IngressSource::Manual,
        source_label: None,
        package_name: Some("Release".to_owned()),
        password: None,
        passwords: Vec::new(),
        category_id: None,
        priority: None,
        urls: urls.iter().map(|url| url.parse().expect("URL")).collect(),
        providers: vec![None; urls.len()],
        file_names,
        sizes: Vec::new(),
        requests: Vec::new(),
        body_refs: Vec::new(),
        auto_check: false,
        source_attributes: Vec::new(),
    }
}

/// Source 1, and the criterion that the group outlives the process (RD-110-18).
///
/// A release page states that its five links are the same file. Nothing about the links
/// themselves says so — five hosters, five names — so this is the only source that can group
/// them, and it has to reach the database rather than a value someone computed once.
#[tokio::test]
async fn a_declared_mirror_group_survives_a_reopen_of_the_database() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("mirrors.sqlite");
    let hint = rd_core::MirrorHint {
        group: "release-page|https://board.example.org/a".to_owned(),
        quality: Some("1080p".to_owned()),
        language: Some("German".to_owned()),
    };
    let urls = [
        "https://one.example/a",
        "https://two.example/b",
        "https://three.example/c",
        "https://four.example/d",
        "https://five.example/e",
    ];
    {
        let database = Database::open(path.clone()).await.expect("database");
        let (_, _, candidates) = database
            .add_collector_batch(mirror_batch(
                &urls,
                vec![
                    Some("one.rar".to_owned()),
                    Some("two.rar".to_owned()),
                    Some("three.rar".to_owned()),
                    Some("four.rar".to_owned()),
                    Some("five.rar".to_owned()),
                ],
                vec![Some(hint.clone()); urls.len()],
            ))
            .await
            .expect("batch");
        assert_eq!(candidates.len(), 5);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.mirror.is_some()),
            "all five links the page declared are one group"
        );
    }
    // Reopening proves the group lives in the database, not in the value the intake returned.
    let database = Database::open(path).await.expect("reopen");
    let candidates = database.list_candidates().await.expect("candidates");
    assert_eq!(candidates.len(), 5);
    let mirrors: Vec<&rd_core::CandidateMirror> = candidates
        .iter()
        .filter_map(|c| c.mirror.as_ref())
        .collect();
    assert_eq!(mirrors.len(), 5);
    assert!(
        mirrors
            .iter()
            .all(|mirror| mirror.group == mirrors[0].group),
        "one group, not five"
    );
    assert!(
        mirrors
            .iter()
            .all(|mirror| mirror.source == rd_core::MirrorSource::Declared)
    );
    assert_eq!(
        mirrors.iter().filter(|mirror| mirror.selected).count(),
        1,
        "exactly one mirror is the chosen one"
    );
    assert_eq!(mirrors[0].quality.as_deref(), Some("1080p"));
    assert_eq!(mirrors[0].language.as_deref(), Some("German"));
}

/// A mirror is not a duplicate, and the duplicate state must not swallow one (RD-110-18).
///
/// Five different addresses for one file are five mirrors; the same address a second time is
/// a duplicate and is no mirror of the first, because starting it would fetch the very bytes
/// the first one already failed to get.
#[tokio::test]
async fn a_mirror_is_not_marked_as_a_duplicate() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-duplicate.sqlite"))
        .await
        .expect("database");
    let hint = rd_core::MirrorHint {
        group: "release".to_owned(),
        quality: None,
        language: None,
    };
    let urls = [
        "https://one.example/a",
        "https://two.example/b",
        "https://three.example/c",
        "https://four.example/d",
        "https://five.example/e",
    ];
    let (_, _, candidates) = database
        .add_collector_batch(mirror_batch(
            &urls,
            vec![Some("release.rar".to_owned()); urls.len()],
            vec![Some(hint.clone()); urls.len()],
        ))
        .await
        .expect("batch");
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.state != rd_core::LinkCandidateState::Duplicate),
        "a second hoster is not a second copy of the link"
    );
    assert_eq!(
        candidates
            .iter()
            .filter(|candidate| candidate.mirror.is_some())
            .count(),
        5
    );
    // The same address again: that one *is* a duplicate, and it joins no group.
    let (_, _, again) = database
        .add_collector_batch(mirror_batch(
            &urls[..1],
            vec![Some("release.rar".to_owned())],
            vec![Some(hint)],
        ))
        .await
        .expect("second batch");
    assert_eq!(again[0].state, rd_core::LinkCandidateState::Duplicate);
    assert!(again[0].mirror.is_none(), "a duplicate mirrors nothing");
    // And the five it was a copy of kept their group.
    let listed = database.list_candidates().await.expect("candidates");
    assert_eq!(
        listed
            .iter()
            .filter(|candidate| candidate.mirror.is_some())
            .count(),
        5
    );
}

/// Sources 2 and 3: what the online check leaves behind becomes the group (RD-110-18).
///
/// At intake these two links have no name and no size, so nothing can be said about them.
/// The check fills both in, and the regroup that follows it is where the answer changes from
/// "no idea" to "the same file" — which is why the grouping is recomputed there and not only
/// once at intake.
#[tokio::test]
async fn the_online_check_turns_names_and_sizes_into_a_mirror_group() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-check.sqlite"))
        .await
        .expect("database");
    let urls = ["https://one.example/dl", "https://two.example/dl"];
    let (batch, _, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            // No package name: only an automatically named package is regrouped, which is
            // exactly the one the online check is allowed to rearrange.
            package_name: None,
            ..mirror_batch(&urls, Vec::new(), Vec::new())
        })
        .await
        .expect("batch");
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.mirror.is_none()),
        "nothing is known about these links yet"
    );
    let ids: Vec<rd_core::CandidateId> = candidates.iter().map(|candidate| candidate.id).collect();
    database
        .claim_candidates_for_check(ids.clone())
        .await
        .expect("claim");
    for (index, id) in ids.iter().enumerate() {
        database
            .record_candidate_check(
                *id,
                Some(rd_core::LinkCheckResult {
                    url: urls[index].parse().expect("URL"),
                    status: rd_core::LinkStatus::Online,
                    file_name: Some("Show.S01E01.German.1080p.mkv".to_owned()),
                    size: rd_core::ByteCount::new(1_000_000).ok(),
                    media: None,
                }),
                None,
                false,
                None,
            )
            .await
            .expect("check");
    }
    database
        .regroup_collector_batches(vec![batch.id])
        .await
        .expect("regroup");
    let listed = database.list_candidates().await.expect("candidates");
    let mirrors: Vec<&rd_core::CandidateMirror> =
        listed.iter().filter_map(|c| c.mirror.as_ref()).collect();
    assert_eq!(mirrors.len(), 2, "the check made these two one file");
    assert_eq!(mirrors[0].group, mirrors[1].group);
    assert_eq!(mirrors[0].source, rd_core::MirrorSource::NameAndSize);
    assert_eq!(mirrors.iter().filter(|mirror| mirror.selected).count(), 1);
    // The facets the release name spells out, for the selection that comes next (RD-110-19).
    assert_eq!(mirrors[0].quality.as_deref(), Some("1080p"));
    assert_eq!(mirrors[0].language.as_deref(), Some("German"));
}

/// A name alone is a proposal, and it has to read as one: the two links below never
/// reported a size, so nothing corroborated the name they share.
#[tokio::test]
async fn a_shared_name_without_a_size_is_only_a_proposed_mirror() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-proposal.sqlite"))
        .await
        .expect("database");
    let urls = ["https://one.example/a", "https://two.example/b"];
    let (_, _, candidates) = database
        .add_collector_batch(mirror_batch(
            &urls,
            vec![Some("Show.S01E01.mkv".to_owned()); 2],
            Vec::new(),
        ))
        .await
        .expect("batch");
    let mirrors: Vec<&rd_core::CandidateMirror> = candidates
        .iter()
        .filter_map(|candidate| candidate.mirror.as_ref())
        .collect();
    assert_eq!(mirrors.len(), 2);
    assert_eq!(mirrors[0].source, rd_core::MirrorSource::Name);
}

/// One release page's links, as a site rule delivers them: a declared group, three qualities.
///
/// The names differ, so only the declaration can hold them together — which is the case the
/// preference is interesting in, because it then has a genuine choice to make.
async fn release_page(
    database: &Database,
    prefix: &str,
    names: &[&str],
) -> Vec<rd_core::LinkCandidate> {
    let urls: Vec<url::Url> = names
        .iter()
        .enumerate()
        .map(|(index, _)| {
            format!("https://h{index}.example/{prefix}/{index}")
                .parse()
                .expect("URL")
        })
        .collect();
    let hint = rd_core::MirrorHint {
        group: prefix.to_owned(),
        quality: None,
        language: None,
    };
    let (_, _, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: vec![Some(prefix.to_owned()); urls.len()],
            mirror_hints: vec![Some(hint); urls.len()],
            source: IngressSource::Manual,
            source_label: None,
            package_name: Some(prefix.to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: names.iter().map(|name| Some((*name).to_owned())).collect(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    candidates
}

/// The chosen mirror's file name, read back from the store rather than from the return value.
async fn chosen_mirror(database: &Database, group: &str) -> String {
    let candidates = database.list_candidates().await.expect("candidates");
    candidates
        .into_iter()
        .find(|candidate| {
            candidate
                .mirror
                .as_ref()
                .is_some_and(|mirror| mirror.group == group && mirror.selected)
        })
        .and_then(|candidate| candidate.file_name)
        .unwrap_or_default()
}

/// The standing preference survives the package it was set in: it decides the mirror of a
/// package that arrives afterwards, without being set again (RD-110-19).
#[tokio::test]
async fn a_mirror_preference_decides_the_next_package_too() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-preference.sqlite"))
        .await
        .expect("database");
    release_page(
        &database,
        "first",
        &["Show.E01.German.720p.mkv", "Show.E01.German.1080p.mkv"],
    )
    .await;
    assert_eq!(
        chosen_mirror(&database, "first").await,
        "Show.E01.German.720p.mkv",
        "with nothing preferred the first member stays chosen"
    );

    database
        .set_mirror_preference(rd_core::MirrorPreference {
            quality: Some("1080p".to_owned()),
            language: None,
            hoster: None,
            ..rd_core::MirrorPreference::default()
        })
        .await
        .expect("preference");
    assert_eq!(
        chosen_mirror(&database, "first").await,
        "Show.E01.German.1080p.mkv",
        "the preference re-chooses the groups that already exist"
    );

    // The package that arrives afterwards never saw the preference being set.
    release_page(
        &database,
        "second",
        &["Other.E02.German.720p.mkv", "Other.E02.German.1080p.mkv"],
    )
    .await;
    assert_eq!(
        chosen_mirror(&database, "second").await,
        "Other.E02.German.1080p.mkv",
        "a preference that stops at the package it was set in is no preference"
    );
    assert_eq!(
        database.mirror_preference().await.expect("read").quality,
        Some("1080p".to_owned())
    );
}

/// Hidden hosters are stored with the preference and outlive a restart (RD-130-21), and a group
/// whose first member sits at a hidden hoster chooses a shown one instead — before and after the
/// restart, and for a package that arrives afterwards.
#[tokio::test]
async fn hidden_hosters_survive_a_restart_and_are_never_the_chosen_mirror() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("hidden-hosters.sqlite");
    {
        let database = Database::open(path.clone()).await.expect("database");
        release_page(
            &database,
            "first",
            &["Show.E01.720p.mkv", "Show.E01.1080p.mkv"],
        )
        .await;
        assert_eq!(chosen_mirror(&database, "first").await, "Show.E01.720p.mkv");
        database
            .set_mirror_preference(rd_core::MirrorPreference {
                hidden_hosters: vec!["h0.example".to_owned(), "gone.example".to_owned()],
                ..rd_core::MirrorPreference::default()
            })
            .await
            .expect("preference");
        assert_eq!(
            chosen_mirror(&database, "first").await,
            "Show.E01.1080p.mkv",
            "the member at the hidden hoster stops being the chosen one"
        );
    }
    let database = Database::open(path).await.expect("reopen");
    assert_eq!(
        database
            .mirror_preference()
            .await
            .expect("read")
            .hidden_hosters,
        ["h0.example", "gone.example"],
        "a hoster hidden before the restart is still hidden after it"
    );
    assert_eq!(
        chosen_mirror(&database, "first").await,
        "Show.E01.1080p.mkv"
    );
    release_page(
        &database,
        "second",
        &["Other.E02.720p.mkv", "Other.E02.1080p.mkv"],
    )
    .await;
    assert_eq!(
        chosen_mirror(&database, "second").await,
        "Other.E02.1080p.mkv",
        "the package that arrives afterwards is chosen under the same hidden hosters"
    );
}

/// A mirror somebody pinned is the package's way out of the standing preference, and it stays
/// that way: neither a later preference nor a regroup takes the decision back.
#[tokio::test]
async fn a_pinned_mirror_overrides_the_preference_and_survives_a_regroup() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-pin.sqlite"))
        .await
        .expect("database");
    database
        .set_mirror_preference(rd_core::MirrorPreference {
            quality: Some("1080p".to_owned()),
            language: None,
            hoster: None,
            ..rd_core::MirrorPreference::default()
        })
        .await
        .expect("preference");
    let candidates = release_page(
        &database,
        "release",
        &["Show.E01.720p.mkv", "Show.E01.1080p.mkv"],
    )
    .await;
    assert_eq!(
        chosen_mirror(&database, "release").await,
        "Show.E01.1080p.mkv"
    );

    let pinned = database
        .set_mirror_pin(candidates[0].id, true)
        .await
        .expect("pin");
    assert!(pinned, "the link is in a group, so there was a choice");
    assert_eq!(
        chosen_mirror(&database, "release").await,
        "Show.E01.720p.mkv",
        "the pin outranks the preference"
    );

    // A second preference, and a regroup on top of it: both rewrite `mirror_selected` in full.
    database
        .set_mirror_preference(rd_core::MirrorPreference {
            quality: Some("1080p".to_owned()),
            language: None,
            hoster: None,
            ..rd_core::MirrorPreference::default()
        })
        .await
        .expect("preference again");
    let batch = candidates[0].batch_id;
    database
        .regroup_collector_batches(vec![batch])
        .await
        .expect("regroup");
    assert_eq!(
        chosen_mirror(&database, "release").await,
        "Show.E01.720p.mkv",
        "a regroup must not quietly revise a decision somebody made"
    );

    // Releasing it hands the group back to the preference.
    assert!(
        database
            .set_mirror_pin(candidates[0].id, false)
            .await
            .expect("release")
    );
    assert_eq!(
        chosen_mirror(&database, "release").await,
        "Show.E01.1080p.mkv"
    );
}

/// A proposed group, as the third source leaves it: one shared name, no size behind it.
async fn proposed_pair(database: &Database, name: &str) -> Vec<rd_core::LinkCandidate> {
    let urls = [
        format!("https://one.example/{name}"),
        format!("https://two.example/{name}"),
    ];
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    let (_, _, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            // No package name, so the package is auto-named and the regroup after the online
            // check is allowed to touch it -- which is the path this pair exists to test.
            package_name: None,
            ..mirror_batch(&refs, vec![Some(format!("{name}.mkv")); 2], Vec::new())
        })
        .await
        .expect("batch");
    candidates
}

/// How many of these candidates are in a mirror group, read back from the store.
async fn grouped_count(database: &Database, ids: &[rd_core::CandidateId]) -> usize {
    database
        .list_candidates()
        .await
        .expect("candidates")
        .into_iter()
        .filter(|candidate| ids.contains(&candidate.id) && candidate.mirror.is_some())
        .count()
}

/// RD-110-34, and the criterion the whole job lives on: a dissolved proposal stays dissolved
/// through **all three** places that recompute the groups.
///
/// The order below is the order the three would undo it. The online check is the dangerous
/// one — it hands the same two links a name *and* a matching size, which is the evidence that
/// would promote the refused proposal to a name-and-size group — so the refusal is stored
/// about the pair rather than about the source that happened to produce it. Intake runs next,
/// and has to leave the decision alone while still grouping links it says nothing about. The
/// move is last, and carries the decision into another package, where the two meet two more
/// links of the same name: the set they would join is the one that was refused, so none of
/// the four is grouped.
#[tokio::test]
async fn a_dissolved_proposal_survives_the_check_an_intake_and_a_move() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-dissolve.sqlite"))
        .await
        .expect("database");
    let first = proposed_pair(&database, "Show.S01E01").await;
    let ids: Vec<rd_core::CandidateId> = first.iter().map(|candidate| candidate.id).collect();
    assert_eq!(
        first[0].mirror.as_ref().expect("a group").source,
        rd_core::MirrorSource::Name,
        "a shared name and nothing else is a proposal"
    );

    assert_eq!(
        database
            .dissolve_mirror_group(ids[0])
            .await
            .expect("dissolve"),
        crate::MirrorDissolve::Dissolved
    );
    assert_eq!(
        grouped_count(&database, &ids).await,
        0,
        "the proposal is gone and its links stand on their own"
    );

    // Path one: the online check, which fills in the very evidence that would have promoted
    // the group to `name_and_size`.
    database
        .claim_candidates_for_check(ids.clone())
        .await
        .expect("claim");
    for (index, id) in ids.iter().enumerate() {
        database
            .record_candidate_check(
                *id,
                Some(rd_core::LinkCheckResult {
                    url: first[index].url.clone(),
                    status: rd_core::LinkStatus::Online,
                    file_name: Some("Show.S01E01.mkv".to_owned()),
                    size: rd_core::ByteCount::new(1_000_000).ok(),
                    media: None,
                }),
                None,
                false,
                None,
            )
            .await
            .expect("check");
    }
    database
        .regroup_collector_batches(vec![first[0].batch_id])
        .await
        .expect("regroup");
    assert_eq!(
        grouped_count(&database, &ids).await,
        0,
        "a size arriving afterwards does not overrule the person who looked at both files"
    );

    // Path two: intake. It recomputes the packages of the batch it wrote, and it must group
    // the links the refusal never named while leaving the refused pair alone.
    let second = proposed_pair(&database, "Other.S01E01").await;
    let other: Vec<rd_core::CandidateId> = second.iter().map(|candidate| candidate.id).collect();
    assert_eq!(
        grouped_count(&database, &other).await,
        2,
        "links a refusal never named are grouped as before"
    );
    assert_eq!(
        grouped_count(&database, &ids).await,
        0,
        "and the refused pair is untouched by an intake elsewhere"
    );

    // Path three: the move. Both refused links go into the package of two further links that
    // share their file name, which is the set the refusal poisoned.
    let third = proposed_pair(&database, "Show.S01E01b").await;
    let target = third[0].package_id.expect("a package");
    for id in &ids {
        database
            .set_candidate_file_name(*id, "Show.S01E01b.mkv".to_owned())
            .await
            .expect("rename");
    }
    database
        .move_candidates(ids.clone(), crate::MoveTarget::Existing(target))
        .await
        .expect("move");
    assert_eq!(
        grouped_count(&database, &ids).await,
        0,
        "the decision travels with the links into another package"
    );
    let joined: Vec<rd_core::CandidateId> = third.iter().map(|candidate| candidate.id).collect();
    assert_eq!(
        grouped_count(&database, &joined).await,
        0,
        "and the set the two would have joined is the one that was refused"
    );
}

/// A declaration and a name-and-size agreement are refused rather than asked about
/// (RD-110-34).
///
/// A contradiction against either is a finding about the *source* — a site rule that declares
/// wrongly, two files that genuinely agree on name and size — and taking one apart would fix
/// a single package while leaving the rule to do the same thing on the next page.
#[tokio::test]
async fn only_a_proposed_mirror_group_can_be_dissolved() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-dissolve-refused.sqlite"))
        .await
        .expect("database");
    let declared = release_page(&database, "release", &["Show.720p.mkv", "Show.1080p.mkv"]).await;
    assert_eq!(
        declared[0].mirror.as_ref().expect("a group").source,
        rd_core::MirrorSource::Declared
    );
    assert_eq!(
        database
            .dissolve_mirror_group(declared[0].id)
            .await
            .expect("dissolve"),
        crate::MirrorDissolve::NotProposed
    );
    let ids: Vec<rd_core::CandidateId> = declared.iter().map(|candidate| candidate.id).collect();
    assert_eq!(
        grouped_count(&database, &ids).await,
        2,
        "the declared group is still there"
    );

    // And a link that is a mirror of nothing is a request about something that does not exist.
    let lonely = database
        .add_collector_batch(mirror_batch(
            &["https://alone.example/x"],
            vec![Some("Alone.mkv".to_owned())],
            Vec::new(),
        ))
        .await
        .expect("batch")
        .2;
    assert_eq!(
        database
            .dissolve_mirror_group(lonely[0].id)
            .await
            .expect("dissolve"),
        crate::MirrorDissolve::NotGrouped
    );
}

/// A pin and a dissolve never contradict each other (RD-110-34).
///
/// Pinning states which member of a group is fetched; dissolving states there is no group.
/// The second answers a question the first assumed, so the pin goes with the group rather
/// than surviving it as a decision about nothing — and pinning afterwards is refused for the
/// same reason it is refused on any ungrouped link.
#[tokio::test]
async fn dissolving_a_group_takes_its_pin_with_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-dissolve-pin.sqlite"))
        .await
        .expect("database");
    let pair = proposed_pair(&database, "Show.S01E02").await;
    assert!(
        database
            .set_mirror_pin(pair[1].id, true)
            .await
            .expect("pin")
    );
    assert_eq!(
        database
            .dissolve_mirror_group(pair[1].id)
            .await
            .expect("dissolve"),
        crate::MirrorDissolve::Dissolved
    );
    let ids: Vec<rd_core::CandidateId> = pair.iter().map(|candidate| candidate.id).collect();
    assert_eq!(grouped_count(&database, &ids).await, 0);
    assert!(
        !database
            .set_mirror_pin(pair[1].id, true)
            .await
            .expect("pin again"),
        "a pin on a link that is a mirror of nothing is refused, dissolve or not"
    );
}

/// Pinning a link that is a mirror of nothing is a request about something that does not
/// exist, and it is refused rather than silently succeeding.
#[tokio::test]
async fn pinning_a_link_without_a_group_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("mirror-pin-none.sqlite"))
        .await
        .expect("database");
    let candidates = release_page(&database, "lonely", &["Only.One.mkv"]).await;
    assert!(
        candidates[0].mirror.is_none(),
        "a single link is a mirror of nothing"
    );
    assert!(
        !database
            .set_mirror_pin(candidates[0].id, true)
            .await
            .expect("pin")
    );
}

/// RD-120-36: a cache answer is stored as the time of the check, on an `Online` link, and
/// the next check that does not say "cached" clears it. A cache expires without notice, so
/// an old answer must not outlive the check that replaced it.
///
/// RD-130-11: the provider that answered travels with the time, is cleared with it, and is
/// never written without it -- a name next to no time would describe no check.
#[tokio::test]
async fn a_cache_answer_is_a_time_and_the_next_check_clears_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("cached-at.sqlite"))
        .await
        .expect("database");
    let urls: Vec<url::Url> = vec!["https://www.example.com/file/abc.rar".parse().expect("URL")];
    let (_batch, _packages, candidates) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: Some("test".to_owned()),
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None; urls.len()],
            urls,
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: true,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    let id = candidates[0].id;
    let answer = |status| rd_core::LinkCheckResult {
        url: candidates[0].url.clone(),
        status,
        file_name: None,
        size: None,
        media: None,
    };

    let before = chrono::Utc::now();
    database
        .record_candidate_check(
            id,
            Some(answer(rd_core::LinkStatus::Cached)),
            None,
            false,
            Some("torbox".to_owned()),
        )
        .await
        .expect("record");
    let cached = database
        .get_candidate(id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(cached.state, rd_core::LinkCandidateState::Online);
    let cached_at = cached.cached_at.expect("a cache answer carries its time");
    assert!(cached_at >= before);
    assert_eq!(
        Some(cached_at),
        cached.checked_at,
        "the time is the check's own"
    );
    assert_eq!(cached.cached_by.as_deref(), Some("torbox"));

    database
        .claim_candidates_for_check(vec![id])
        .await
        .expect("claim");
    database
        .record_candidate_check(
            id,
            Some(answer(rd_core::LinkStatus::Online)),
            None,
            false,
            // A provider handed in without a cached result is not written.
            Some("torbox".to_owned()),
        )
        .await
        .expect("record");
    let rechecked = database
        .get_candidate(id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(rechecked.state, rd_core::LinkCandidateState::Online);
    assert_eq!(
        rechecked.cached_at, None,
        "an older cache answer does not survive"
    );
    assert_eq!(
        rechecked.cached_by, None,
        "no provider without the time it answered"
    );

    // A link nothing here can check keeps its state and its message, and still carries the
    // cache answer of the provider that holds it.
    database
        .claim_candidates_for_check(vec![id])
        .await
        .expect("claim");
    database
        .mark_candidate_unsupported(
            id,
            rd_core::CandidateMessage::coded(
                "collector.check_no_source",
                "no service can check this address",
            ),
            Some("torbox".to_owned()),
        )
        .await
        .expect("unsupported");
    let unsupported = database
        .get_candidate(id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(unsupported.state, rd_core::LinkCandidateState::Unsupported);
    assert_eq!(
        unsupported.error_code.as_deref(),
        Some("collector.check_no_source")
    );
    assert!(unsupported.cached_at.is_some());
    assert_eq!(unsupported.cached_by.as_deref(), Some("torbox"));

    database
        .claim_candidates_for_check(vec![id])
        .await
        .expect("claim");
    database
        .mark_candidate_unsupported(
            id,
            rd_core::CandidateMessage::coded(
                "collector.check_no_source",
                "no service can check this address",
            ),
            None,
        )
        .await
        .expect("unsupported");
    let cleared = database
        .get_candidate(id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(cleared.cached_at, None);
    assert_eq!(cleared.cached_by, None);
}
