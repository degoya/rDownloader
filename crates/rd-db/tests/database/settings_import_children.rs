//! A settings import replaces the configuration, not what hangs off it (DB-01).
//!
//! Accounts and stream channels own state that is not configuration through `ON DELETE
//! CASCADE`: an account its sign-in (`auth_flows`, `auth_flow_parts`) and its remote jobs, a
//! channel its recording schedules and their runs. Deleting every row and inserting the bundle
//! again took all of that with it even when the bundle named the very same ids, and left the
//! sign-in's tokens in the vault with nothing pointing at them. These cases seed every child
//! table, import, and check that the children of a named parent survive with their vault
//! entries, and that the children of a dropped parent go with theirs.

use std::path::Path;

use chrono::{Duration, Utc};
use rd_core::{
    AccountId, AuthFlowState, RemoteJobSourceKind, ScheduleKind, StreamChannelId, StreamScheduleId,
};
use rd_db::{
    ClaimRemoteJob, ConfigReplacement, Database, NewAccount, NewStreamChannel, NewStreamSchedule,
    NewSubscription, NewSubscriptionItem, PlannedOccurrence, PollResult, ReplacementAccount,
    ReplacementStreamChannel, ReplacementSubscription, UpsertAuthFlow,
};

async fn open(directory: &Path) -> Database {
    let database = Database::open(directory.join("import.sqlite"))
        .await
        .expect("database");
    database
        .install_file_vault(directory.join("secrets"))
        .await
        .expect("vault");
    database
}

/// Encrypted entries in the vault, the master key not counted.
fn vault_entries(directory: &Path) -> usize {
    std::fs::read_dir(directory.join("secrets")).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".secret"))
            .count()
    })
}

/// What one seeded account holds besides its own row.
struct SignedIn {
    id: AccountId,
    provider: String,
    references: Vec<String>,
}

async fn signed_in_account(database: &Database, provider: &str) -> SignedIn {
    let vault = database.secret_vault().expect("vault").clone();
    let id = database
        .create_account(NewAccount {
            provider: provider.to_owned(),
            label: format!("{provider} account"),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account")
        .id;
    let refresh = vault.put_string("refresh".to_owned()).await.expect("put");
    let access = vault.put_string("access".to_owned()).await.expect("put");
    let key = vault.put_string("key".to_owned()).await.expect("put");
    let part = vault.put_string("part".to_owned()).await.expect("put");
    database
        .upsert_auth_flow(UpsertAuthFlow {
            account_id: id,
            plugin_id: "demo-plugin".to_owned(),
            state: AuthFlowState::Authorized,
            verification_url: None,
            user_code: None,
            expires_at: None,
            next_poll_at: None,
            message: None,
            token_expires_at: Some(Utc::now() + Duration::hours(1)),
            refresh_ref: Some(refresh.clone()),
            access_ref: Some(access.clone()),
            key_ref: Some(key.clone()),
            callback_state: None,
            flow_state: None,
        })
        .await
        .expect("flow");
    database
        .set_auth_flow_part(id, "client_secret".to_owned(), part.clone())
        .await
        .expect("part");
    database
        .claim_remote_job(ClaimRemoteJob {
            id: rd_core::RemoteJobId::new(),
            account_id: id,
            plugin_id: "demo-plugin".to_owned(),
            content_key: format!("torrent:{provider}"),
            source_kind: RemoteJobSourceKind::Magnet,
            source: b"magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709".to_vec(),
            source_name: None,
            package_id: None,
        })
        .await
        .expect("remote job");
    SignedIn {
        id,
        provider: provider.to_owned(),
        references: vec![refresh, access, key, part],
    }
}

async fn scheduled_channel(database: &Database, name: &str) -> (StreamChannelId, StreamScheduleId) {
    let channel = database
        .create_stream_channel(NewStreamChannel {
            url: format!("https://example.test/{name}"),
            name: name.to_owned(),
            quality: None,
            category_id: None,
            enabled: true,
            recording: rd_core::RecordingPolicy::default(),
        })
        .await
        .expect("channel");
    let start = Utc::now() + Duration::days(1);
    let schedule = database
        .create_stream_schedule(NewStreamSchedule {
            channel_id: channel.id,
            name: format!("{name} schedule"),
            enabled: true,
            kind: ScheduleKind::Once { start },
            timezone: "UTC".to_owned(),
            window_minutes: 60,
            lead_minutes: 0,
            trail_minutes: 0,
            replay_from_start: false,
        })
        .await
        .expect("schedule");
    database
        .plan_stream_runs(
            schedule.id,
            channel.id,
            vec![PlannedOccurrence {
                starts_at: start,
                ends_at: start + Duration::hours(1),
            }],
        )
        .await
        .expect("run");
    (channel.id, schedule.id)
}

fn named_account(account: &SignedIn, label: &str) -> ReplacementAccount {
    ReplacementAccount {
        id: account.id,
        provider: account.provider.clone(),
        label: label.to_owned(),
        username: None,
        credential_mode: None,
        secret_ref: None,
        cookie_ref: None,
        proxy_profile_id: None,
        enabled: true,
    }
}

fn named_channel(id: StreamChannelId, name: &str) -> ReplacementStreamChannel {
    ReplacementStreamChannel {
        id,
        url: format!("https://example.test/{name}"),
        name: name.to_owned(),
        quality: Some("best".to_owned()),
        category_id: None,
        enabled: true,
        recording: rd_core::RecordingPolicy::default(),
    }
}

async fn assert_signed_in(database: &Database, account: &SignedIn) {
    let flow = database
        .auth_flow(account.id)
        .await
        .expect("flow")
        .expect("the sign-in survives");
    assert_eq!(flow.refresh_ref.as_ref(), Some(&account.references[0]));
    assert_eq!(flow.access_ref.as_ref(), Some(&account.references[1]));
    assert_eq!(flow.key_ref.as_ref(), Some(&account.references[2]));
    assert_eq!(
        database
            .auth_flow_part(account.id, "client_secret")
            .await
            .expect("part")
            .as_ref(),
        Some(&account.references[3])
    );
    assert_eq!(
        database.remote_jobs(account.id).await.expect("jobs").len(),
        1
    );
    let vault = database.secret_vault().expect("vault");
    for reference in &account.references {
        assert!(vault.get(reference).await.is_ok(), "{reference} stays");
    }
}

async fn assert_signed_out(database: &Database, account: &SignedIn) {
    assert!(
        database
            .auth_flow(account.id)
            .await
            .expect("flow")
            .is_none()
    );
    assert!(
        database
            .auth_flow_part(account.id, "client_secret")
            .await
            .expect("part")
            .is_none()
    );
    assert!(
        database
            .remote_jobs(account.id)
            .await
            .expect("jobs")
            .is_empty()
    );
    let vault = database.secret_vault().expect("vault");
    for reference in &account.references {
        assert!(vault.get(reference).await.is_err(), "{reference} leaves");
    }
}

#[tokio::test]
async fn an_import_naming_the_same_ids_keeps_sign_ins_jobs_and_schedules() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let account = signed_in_account(&database, "torbox").await;
    let (channel, schedule) = scheduled_channel(&database, "kept").await;
    assert_eq!(vault_entries(directory.path()), 4);

    database
        .replace_config(ConfigReplacement {
            accounts: vec![named_account(&account, "renamed")],
            stream_channels: vec![named_channel(channel, "kept again")],
            ..ConfigReplacement::default()
        })
        .await
        .expect("import");

    assert_signed_in(&database, &account).await;
    assert_eq!(vault_entries(directory.path()), 4);
    let accounts = database.list_accounts().await.expect("accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].label, "renamed", "the bundle's values win");
    let schedules = database.list_stream_schedules().await.expect("schedules");
    assert_eq!(schedules.len(), 1);
    assert_eq!(schedules[0].id, schedule);
    assert_eq!(
        database
            .stream_scheduled_runs(Some(schedule), 10)
            .await
            .expect("runs")
            .len(),
        1
    );
    let channels = database.list_stream_channels().await.expect("channels");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].name, "kept again");
}

#[tokio::test]
async fn an_import_dropping_a_parent_takes_its_children_and_their_vault_entries() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let kept = signed_in_account(&database, "torbox").await;
    let dropped = signed_in_account(&database, "premiumize").await;
    let (kept_channel, kept_schedule) = scheduled_channel(&database, "kept").await;
    let (_, dropped_schedule) = scheduled_channel(&database, "dropped").await;
    assert_eq!(vault_entries(directory.path()), 8);

    database
        .replace_config(ConfigReplacement {
            accounts: vec![named_account(&kept, "kept")],
            stream_channels: vec![named_channel(kept_channel, "kept")],
            ..ConfigReplacement::default()
        })
        .await
        .expect("import");

    assert_signed_in(&database, &kept).await;
    assert_signed_out(&database, &dropped).await;
    assert_eq!(vault_entries(directory.path()), 4);
    let schedules: Vec<StreamScheduleId> = database
        .list_stream_schedules()
        .await
        .expect("schedules")
        .into_iter()
        .map(|schedule| schedule.id)
        .collect();
    assert_eq!(schedules, vec![kept_schedule]);
    assert!(
        database
            .stream_scheduled_runs(Some(dropped_schedule), 10)
            .await
            .expect("runs")
            .is_empty()
    );
}

/// The same id under another provider is another account: its sign-in and remote jobs would
/// mean nothing to the new provider, so they go as if the account had been dropped.
#[tokio::test]
async fn an_import_naming_an_id_for_another_provider_starts_that_account_afresh() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let account = signed_in_account(&database, "torbox").await;

    let mut replacement = named_account(&account, "switched");
    replacement.provider = "premiumize".to_owned();
    database
        .replace_config(ConfigReplacement {
            accounts: vec![replacement],
            ..ConfigReplacement::default()
        })
        .await
        .expect("import");

    assert_signed_out(&database, &account).await;
    let accounts = database.list_accounts().await.expect("accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, account.id);
    assert_eq!(accounts[0].provider, "premiumize");
    assert_eq!(vault_entries(directory.path()), 0);
}

/// A primed feed subscription with one archived item carrying an archive password.
async fn polled_subscription(database: &Database, name: &str) -> rd_core::Subscription {
    let created = database
        .create_subscription(NewSubscription {
            name: name.to_owned(),
            url: format!("https://example.test/{name}.xml")
                .parse()
                .expect("url"),
            kind: rd_core::SubscriptionKind::Feed,
            enabled: true,
            mode: rd_core::SubscriptionMode::Review,
            category_id: None,
            priority: rd_core::DownloadPriority::default(),
            interval_seconds: 3_600,
            filters: rd_core::SubscriptionFilters::default(),
            backlog: rd_core::BacklogPolicy::default(),
            category_map: Vec::new(),
            source_categories: Vec::new(),
            every_release: false,
            view: rd_core::SubscriptionView::List,
            autoplay: false,
            card_ratio: rd_core::SubscriptionCardRatio::TwoOne,
            schedule: None,
            script_arguments: Vec::new(),
            indexer_search: rd_core::IndexerSearch::default(),
            git_release: rd_core::GitReleaseOptions::default(),
            secret_ref: None,
        })
        .await
        .expect("subscription");
    database
        .record_subscription_items(
            created.id,
            vec![NewSubscriptionItem {
                item_key: format!("{name}-1"),
                title: format!("{name} release"),
                url: format!("https://example.test/{name}/1.nzb")
                    .parse()
                    .expect("url"),
                published_at: None,
                duration_seconds: None,
                state: rd_core::SubscriptionItemState::Skipped,
                reason: None,
                source_category: None,
                media_type: None,
                attributes: std::collections::BTreeMap::new(),
                password: Some(format!("{name}-password")),
            }],
        )
        .await
        .expect("items");
    database
        .finish_subscription_run(
            created.id,
            Utc::now(),
            PollResult {
                found: 1,
                accepted: 0,
                skipped: 1,
                error: None,
                next_run_at: Utc::now() + Duration::hours(1),
                consecutive_failures: 0,
                etag: None,
                last_modified: None,
            },
        )
        .await
        .expect("run");
    database
        .subscription(created.id)
        .await
        .expect("read")
        .expect("stored")
}

fn named_subscription(subscription: &rd_core::Subscription, name: &str) -> ReplacementSubscription {
    ReplacementSubscription {
        id: subscription.id,
        name: name.to_owned(),
        url: subscription.url.to_string(),
        kind: subscription.kind,
        enabled: subscription.enabled,
        mode: subscription.mode,
        category_id: subscription.category_id,
        priority: subscription.priority,
        interval_seconds: subscription.interval_seconds,
        filters: subscription.filters.clone(),
        backlog: subscription.backlog,
        category_map: subscription.category_map.clone(),
        source_categories: subscription.source_categories.clone(),
        every_release: subscription.every_release,
        view: subscription.view,
        autoplay: subscription.autoplay,
        card_ratio: subscription.card_ratio,
        schedule: subscription.schedule.clone(),
        script_arguments: subscription.script_arguments.clone(),
        indexer_search: subscription.indexer_search.clone(),
        git_release: subscription.git_release.clone(),
        secret_ref: None,
    }
}

/// RA-DB-04: the archive is a subscription's once-only guarantee. Re-created unprimed and
/// empty under the same id, it took the whole feed for new on the next poll; the archive of a
/// subscription the bundle drops leaves the vault with it.
#[tokio::test]
async fn an_import_naming_a_subscription_again_keeps_its_archive_runs_and_priming() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let kept = polled_subscription(&database, "kept").await;
    let dropped = polled_subscription(&database, "dropped").await;
    assert!(kept.primed && dropped.primed);
    assert_eq!(vault_entries(directory.path()), 2);

    database
        .replace_config(ConfigReplacement {
            subscriptions: vec![named_subscription(&kept, "renamed")],
            ..ConfigReplacement::default()
        })
        .await
        .expect("import");

    let subscriptions = database.list_subscriptions().await.expect("subscriptions");
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0].id, kept.id);
    assert_eq!(subscriptions[0].name, "renamed");
    assert!(subscriptions[0].primed, "the priming survives the import");
    let page = database
        .subscription_item_page(kept.id, None, 50, 0)
        .await
        .expect("archive");
    assert_eq!(page.total, 1, "the archive survives the import");
    assert_eq!(page.run_total, 1, "the runs survive the import");
    assert_eq!(page.items[0].password.as_deref(), Some("kept-password"));

    assert!(
        database
            .subscription(dropped.id)
            .await
            .expect("read")
            .is_none()
    );
    assert_eq!(
        vault_entries(directory.path()),
        1,
        "the dropped subscription's archive password left the vault"
    );
}
