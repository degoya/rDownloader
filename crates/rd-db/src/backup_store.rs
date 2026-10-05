//! Atomic replacement of all server-side configuration tables.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{
    AccountId, Category, CategoryRule, EventEnvelope, EventKind, HotFolderConfig, ProxyKind,
    ProxyProfileId, StorageRootConfig, StreamChannelId, UsenetServerId,
};
use sqlx::{Connection, SqliteConnection};
use url::Url;

use crate::{enum_string, writer::insert_event};

#[derive(Clone, Debug)]
pub struct ReplacementAccount {
    pub id: AccountId,
    pub provider: String,
    pub label: String,
    pub username: Option<String>,
    pub credential_mode: Option<rd_provider_registry::CredentialMode>,
    pub secret_ref: Option<String>,
    pub cookie_ref: Option<String>,
    pub proxy_profile_id: Option<ProxyProfileId>,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct ReplacementProxyProfile {
    pub id: ProxyProfileId,
    pub name: String,
    pub kind: ProxyKind,
    pub endpoint: Url,
    pub username: Option<String>,
    pub secret_ref: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ReplacementUsenetServer {
    pub id: UsenetServerId,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: Option<String>,
    pub password_ref: Option<String>,
    pub proxy_profile_id: Option<ProxyProfileId>,
    pub priority: i32,
    pub max_connections: u16,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct ReplacementStreamChannel {
    pub id: StreamChannelId,
    pub url: String,
    pub name: String,
    pub quality: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
    pub enabled: bool,
    /// Splitting, remux, sidecars and reconnect delay (RD-080-09).
    pub recording: rd_core::RecordingPolicy,
}

/// Subscription as it travels in a settings bundle (RD-080-07).
///
/// The archive is deliberately *not* part of it: item history is not configuration, and
/// restoring a bundle onto another machine must not make that machine believe it has
/// already downloaded things it has not. A subscription the installation does not hold yet
/// therefore starts unprimed and applies its backlog policy afresh — which is the safe
/// direction, because the policy defaults to "nothing that already exists". One it holds under
/// the same id keeps its archive, its runs and its priming, as an edit in the view does
/// (RA-DB-04): the archive is what stops it from queueing the whole feed again.
#[derive(Clone, Debug)]
pub struct ReplacementSubscription {
    pub id: rd_core::SubscriptionId,
    pub name: String,
    pub url: String,
    pub kind: rd_core::SubscriptionKind,
    pub enabled: bool,
    pub mode: rd_core::SubscriptionMode,
    pub category_id: Option<rd_core::CategoryId>,
    pub priority: rd_core::DownloadPriority,
    pub interval_seconds: u32,
    pub filters: rd_core::SubscriptionFilters,
    pub backlog: rd_core::BacklogPolicy,
    /// Both were missing from the bundle until 1.0.1, so a restore quietly dropped every
    /// indexer's category routing and left it pulling the whole feed again.
    pub category_map: Vec<rd_core::CategoryMapping>,
    pub source_categories: Vec<String>,
    /// Keep every release of an episode rather than only the first (RD-110-21).
    pub every_release: bool,
    /// How the LinkGrabber draws the pending hits, and whether the cards turn on their own
    /// (RD-120-37). A presentation choice, but one somebody made, so a restore keeps it.
    pub view: rd_core::SubscriptionView,
    pub autoplay: bool,
    /// The shape of a card's image area (RD-120-42); restored like the view it belongs to.
    pub card_ratio: rd_core::SubscriptionCardRatio,
    /// The cron expression replacing the interval (RD-130-19), when there is one.
    pub schedule: Option<String>,
    /// The arguments a script subscription hands its script (RD-150-08).
    pub script_arguments: Vec<String>,
    /// The search parameters an indexer subscription sends (RD-180-20).
    pub indexer_search: rd_core::IndexerSearch,
    /// Which assets a git-release subscription downloads (RD-190-13).
    pub git_release: rd_core::GitReleaseOptions,
    pub secret_ref: Option<String>,
}

/// Auth profile as it travels in a settings bundle; credentials stay behind their
/// re-minted secret references.
#[derive(Clone, Debug)]
pub struct ReplacementAuthProfile {
    pub id: rd_core::AuthProfileId,
    pub name: String,
    pub scope: rd_core::AuthScope,
    pub method: rd_core::AuthMethod,
    pub origin: rd_core::AuthOrigin,
    pub enabled: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub username: Option<String>,
    pub secret_ref: Option<String>,
    pub certificate_ref: Option<String>,
}

/// Newznab indexer as it travels in a settings bundle (RD-190-22); its API key stays behind a
/// re-minted secret reference like every other credential.
#[derive(Clone, Debug)]
pub struct ReplacementIndexer {
    pub id: rd_core::IndexerId,
    pub name: String,
    pub url: Url,
    pub secret_ref: Option<String>,
    pub categories: Vec<String>,
    pub enabled: bool,
    /// How its search hits are listed (RD-190-16).
    pub list_style: rd_core::IndexerListStyle,
}

#[derive(Clone, Debug, Default)]
pub struct ConfigReplacement {
    pub storage_roots: Vec<StorageRootConfig>,
    pub categories: Vec<Category>,
    pub category_rules: Vec<CategoryRule>,
    pub hotfolders: Vec<HotFolderConfig>,
    pub proxy_profiles: Vec<ReplacementProxyProfile>,
    pub accounts: Vec<ReplacementAccount>,
    pub usenet_servers: Vec<ReplacementUsenetServer>,
    pub stream_channels: Vec<ReplacementStreamChannel>,
    pub subscriptions: Vec<ReplacementSubscription>,
    pub auth_profiles: Vec<ReplacementAuthProfile>,
    pub indexers: Vec<ReplacementIndexer>,
}

/// What a replacement changed beyond the tables it wrote.
#[derive(Debug, Default)]
pub(crate) struct ReplacementOutcome {
    pub(crate) events: Vec<EventEnvelope>,
    /// Vault references of sign-ins whose account the bundle no longer names: their rows went
    /// with the account, so nothing points at the values any more.
    pub(crate) released_secrets: Vec<String>,
}

pub(crate) async fn replace_all(
    connection: &mut SqliteConnection,
    mut replacement: ConfigReplacement,
) -> Result<ReplacementOutcome> {
    // A bundle predates the single-default invariant or was hand-edited: repair it rather than
    // refuse the restore, so an import can never leave the install without a default root.
    crate::config_store::normalize_storage_root_defaults(&mut replacement.storage_roots);
    // The same for categories: zero defaults leaves routing without a fallback for links no
    // rule matched, and two of them are refused outright by `idx_categories_single_default`.
    crate::config_store::normalize_category_defaults(&mut replacement.categories);
    let now = Utc::now();
    let mut tx = connection.begin().await?;

    // A few review/queue tables have nullable category foreign keys. Deferral lets preserved
    // category ids survive the delete/reinsert; references absent from the bundle are cleared
    // below before the transaction is committed.
    sqlx::query("PRAGMA defer_foreign_keys = ON")
        .execute(&mut *tx)
        .await?;
    // Accounts and stream channels own state that is not configuration -- sign-ins with their
    // vaulted tokens, remote jobs, recording schedules and their runs -- through `ON DELETE
    // CASCADE`, and deferring the foreign keys defers the check, never the cascade (DB-01). So
    // only the rows the bundle no longer names are deleted, taking their children with them;
    // the others are updated in place below and keep theirs.
    let released_secrets = drop_unnamed_accounts(&mut tx, &replacement.accounts).await?;
    drop_unnamed_stream_channels(&mut tx, &replacement.stream_channels).await?;
    // Subscriptions the same way (RA-DB-04): their archive is the once-only guarantee, so a
    // named one keeps it; a dropped one's archive passwords reach the sweep through the
    // `subscription_items` delete trigger, and `Database::replace_config` sweeps them.
    drop_unnamed_subscriptions(&mut tx, &replacement.subscriptions).await?;
    for table in [
        "indexers",
        "auth_profiles",
        "usenet_servers",
        "proxy_profiles",
        "hotfolders",
        "category_rules",
        "categories",
        "storage_roots",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table}")))
            .execute(&mut *tx)
            .await?;
    }

    for value in replacement.storage_roots {
        sqlx::query(
            "INSERT INTO storage_roots (id, name, path, is_default, minimum_free_bytes, \
             created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(value.id.to_string())
        .bind(value.name)
        .bind(value.path)
        .bind(value.is_default)
        .bind(
            value
                .minimum_free_bytes
                .map(|bytes| i64::try_from(bytes.get()).unwrap_or(i64::MAX)),
        )
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    for value in replacement.categories {
        sqlx::query(
            "INSERT INTO categories (id, name, color, storage_root_id, relative_path, is_default, \
             postprocess_level, script, cleanup_extensions, recursive_unpack, unpack_to_subfolder, \
             direct_unpack, malware_scan, sfv_verify, safe_postproc, delete_par2, \
             upload_enabled, upload_remote, seeding_json, plugin_steps_json, sorting_json, \
             created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(value.id.to_string())
        .bind(value.name)
        .bind(value.color)
        .bind(value.storage_root_id.to_string())
        .bind(value.relative_path)
        .bind(value.is_default)
        .bind(value.postprocess_level.map(enum_string).transpose()?)
        .bind(value.script)
        .bind(
            value
                .cleanup_extensions
                .map(|items| serde_json::to_string(&items))
                .transpose()?,
        )
        .bind(value.recursive_unpack)
        .bind(value.unpack_to_subfolder)
        .bind(value.direct_unpack)
        .bind(value.malware_scan)
        .bind(value.sfv_verify)
        .bind(value.safe_postproc)
        .bind(value.delete_par2)
        .bind(value.upload_enabled)
        .bind(value.upload_remote)
        .bind(
            value
                .seeding
                .filter(|policy| !policy.is_empty())
                .map(|policy| serde_json::to_string(&policy))
                .transpose()?,
        )
        .bind(
            value
                .plugin_steps
                .map(|steps| serde_json::to_string(&steps))
                .transpose()?,
        )
        .bind(crate::config_store::sorting_json(value.sorting.as_ref())?)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    for value in replacement.category_rules {
        sqlx::query(
            "INSERT INTO category_rules (id, name, priority, source, domain, protocol, extension, \
             mime_type, name_regex, category_id, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(value.id.to_string())
        .bind(value.name)
        .bind(value.priority)
        .bind(value.source.map(enum_string).transpose()?)
        .bind(value.domain)
        .bind(value.protocol)
        .bind(value.extension)
        .bind(value.mime_type)
        .bind(value.name_regex)
        .bind(value.category_id.to_string())
        .bind(value.enabled)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    for value in replacement.hotfolders {
        sqlx::query(
            "INSERT INTO hotfolders (id, name, executor_json, path, recursive, category_id, \
             import_mode, processed_path, failed_path, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(value.id.to_string())
        .bind(value.name)
        .bind(serde_json::to_string(&value.executor)?)
        .bind(value.path)
        .bind(value.recursive)
        .bind(value.category_id.map(|id| id.to_string()))
        .bind(enum_string(value.import_mode)?)
        .bind(value.processed_path)
        .bind(value.failed_path)
        .bind(value.enabled)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    for value in replacement.stream_channels {
        sqlx::query(
            "INSERT INTO stream_channels (id, url, name, quality, category_id, enabled, \
             last_live_at, last_error, recording_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET url = excluded.url, name = excluded.name, \
             quality = excluded.quality, category_id = excluded.category_id, \
             enabled = excluded.enabled, recording_json = excluded.recording_json, \
             updated_at = excluded.updated_at",
        )
        .bind(value.id.to_string())
        .bind(value.url)
        .bind(value.name)
        .bind(value.quality)
        .bind(value.category_id.map(|id| id.to_string()))
        .bind(value.enabled)
        .bind(serde_json::to_string(&value.recording)?)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    for value in replacement.subscriptions {
        sqlx::query(
            "INSERT INTO subscriptions (id, name, url, kind, enabled, mode, category_id, \
             priority, interval_seconds, filters_json, backlog_json, category_map_json, \
             source_categories_json, every_release, view, autoplay, card_ratio, schedule, \
             script_arguments_json, indexer_search_json, git_release_json, primed, \
             consecutive_failures, secret_ref, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, url = excluded.url, \
             kind = excluded.kind, enabled = excluded.enabled, mode = excluded.mode, \
             category_id = excluded.category_id, priority = excluded.priority, \
             interval_seconds = excluded.interval_seconds, filters_json = excluded.filters_json, \
             backlog_json = excluded.backlog_json, \
             category_map_json = excluded.category_map_json, \
             source_categories_json = excluded.source_categories_json, \
             every_release = excluded.every_release, view = excluded.view, \
             autoplay = excluded.autoplay, card_ratio = excluded.card_ratio, \
             script_arguments_json = excluded.script_arguments_json, \
             indexer_search_json = excluded.indexer_search_json, \
             git_release_json = excluded.git_release_json, secret_ref = excluded.secret_ref, \
             etag = CASE WHEN subscriptions.url IS excluded.url \
               AND subscriptions.git_release_json IS excluded.git_release_json \
               THEN subscriptions.etag ELSE NULL END, \
             last_modified = CASE WHEN subscriptions.url IS excluded.url \
               AND subscriptions.git_release_json IS excluded.git_release_json \
               THEN subscriptions.last_modified ELSE NULL END, \
             next_run_at = CASE WHEN subscriptions.schedule IS excluded.schedule \
               THEN subscriptions.next_run_at ELSE NULL END, \
             schedule = excluded.schedule, updated_at = excluded.updated_at",
        )
        .bind(value.id.to_string())
        .bind(value.name)
        .bind(value.url)
        .bind(match value.kind {
            rd_core::SubscriptionKind::Gallery => "gallery",
            rd_core::SubscriptionKind::Feed => "feed",
            rd_core::SubscriptionKind::Indexer => "indexer",
            rd_core::SubscriptionKind::SiteRule => "site_rule",
            rd_core::SubscriptionKind::Script => "script",
            rd_core::SubscriptionKind::GitRelease => "git_release",
            rd_core::SubscriptionKind::Media => "media",
        })
        .bind(value.enabled)
        .bind(match value.mode {
            rd_core::SubscriptionMode::AutoQueue => "auto_queue",
            rd_core::SubscriptionMode::Review => "review",
        })
        .bind(value.category_id.map(|id| id.to_string()))
        .bind(i64::from(value.priority.as_i32()))
        .bind(i64::from(value.interval_seconds))
        .bind(serde_json::to_string(&value.filters)?)
        .bind(serde_json::to_string(&value.backlog)?)
        .bind(serde_json::to_string(&value.category_map)?)
        .bind(serde_json::to_string(&value.source_categories)?)
        .bind(i64::from(value.every_release))
        .bind(value.view.as_str())
        .bind(i64::from(value.autoplay))
        .bind(value.card_ratio.as_str())
        .bind(value.schedule)
        .bind(serde_json::to_string(&value.script_arguments)?)
        .bind(serde_json::to_string(&value.indexer_search)?)
        .bind(serde_json::to_string(&value.git_release)?)
        .bind(value.secret_ref)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    for value in replacement.auth_profiles {
        sqlx::query(
            "INSERT INTO auth_profiles (id, name, host, include_subdomains, path_prefix, \
             method, origin, enabled, expires_at, username, secret_ref, certificate_ref, \
             created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(value.id.to_string())
        .bind(value.name)
        .bind(value.scope.host)
        .bind(value.scope.include_subdomains)
        .bind(value.scope.path_prefix)
        .bind(enum_string(value.method)?)
        .bind(enum_string(value.origin)?)
        .bind(value.enabled)
        .bind(value.expires_at)
        .bind(value.username)
        .bind(value.secret_ref)
        .bind(value.certificate_ref)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    for value in replacement.indexers {
        sqlx::query(
            "INSERT INTO indexers (id, name, url, secret_ref, categories_json, enabled, \
             list_style, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(value.id.to_string())
        .bind(value.name)
        .bind(value.url.as_str())
        .bind(value.secret_ref)
        .bind(serde_json::to_string(&value.categories)?)
        .bind(value.enabled)
        .bind(value.list_style.as_str())
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    for value in replacement.proxy_profiles {
        sqlx::query(
            "INSERT INTO proxy_profiles (id, name, kind, endpoint, username, secret_ref, \
             created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(value.id.to_string())
        .bind(value.name)
        .bind(enum_string(value.kind)?)
        .bind(value.endpoint.as_str())
        .bind(value.username)
        .bind(value.secret_ref)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    for value in replacement.accounts {
        sqlx::query(
            "INSERT INTO accounts (id, provider, label, username, credential_mode, secret_ref, \
             cookie_ref, proxy_profile_id, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET label = excluded.label, \
             username = excluded.username, credential_mode = excluded.credential_mode, \
             secret_ref = excluded.secret_ref, cookie_ref = excluded.cookie_ref, \
             proxy_profile_id = excluded.proxy_profile_id, enabled = excluded.enabled, \
             updated_at = excluded.updated_at",
        )
        .bind(value.id.to_string())
        .bind(value.provider)
        .bind(value.label)
        .bind(value.username)
        .bind(
            value
                .credential_mode
                .map(rd_provider_registry::CredentialMode::as_str),
        )
        .bind(value.secret_ref)
        .bind(value.cookie_ref)
        .bind(value.proxy_profile_id.map(|id| id.to_string()))
        .bind(value.enabled)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    for value in replacement.usenet_servers {
        sqlx::query(
            "INSERT INTO usenet_servers (id, name, host, port, tls, username, password_ref, \
             proxy_profile_id, priority, max_connections, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(value.id.to_string())
        .bind(value.name)
        .bind(value.host)
        .bind(i64::from(value.port))
        .bind(value.tls)
        .bind(value.username)
        .bind(value.password_ref)
        .bind(value.proxy_profile_id.map(|id| id.to_string()))
        .bind(value.priority)
        .bind(i64::from(value.max_connections))
        .bind(value.enabled)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }

    for table in ["link_candidates", "nzb_imports", "collector_packages"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET category_id = NULL WHERE category_id IS NOT NULL \
             AND NOT EXISTS (SELECT 1 FROM categories WHERE categories.id = {table}.category_id)"
        )))
        .execute(&mut *tx)
        .await?;
    }

    let events = replacement_events();
    for event in &events {
        insert_event(&mut tx, event).await?;
    }
    tx.commit().await?;
    Ok(ReplacementOutcome {
        events,
        released_secrets,
    })
}

/// Deletes the accounts the bundle does not name -- or names for another provider, whose
/// sign-in and remote jobs would mean nothing to the new one -- and answers the vault
/// references their sign-ins held, which the cascade leaves without an owner.
async fn drop_unnamed_accounts(
    tx: &mut SqliteConnection,
    kept: &[ReplacementAccount],
) -> Result<Vec<String>> {
    let existing: Vec<(String, String)> = sqlx::query_as("SELECT id, provider FROM accounts")
        .fetch_all(&mut *tx)
        .await?;
    let mut released = Vec::new();
    for (id, provider) in existing {
        let id: AccountId = id.parse()?;
        if kept
            .iter()
            .any(|account| account.id == id && account.provider == provider)
        {
            continue;
        }
        released.extend(crate::auth_flow_store::sign_in_references(&mut *tx, id).await?);
        sqlx::query("DELETE FROM accounts WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
    }
    Ok(released)
}

/// Deletes the stream channels the bundle does not name; their schedules and runs go with them.
async fn drop_unnamed_stream_channels(
    tx: &mut SqliteConnection,
    kept: &[ReplacementStreamChannel],
) -> Result<()> {
    let existing: Vec<String> = sqlx::query_scalar("SELECT id FROM stream_channels")
        .fetch_all(&mut *tx)
        .await?;
    for id in existing {
        if kept.iter().any(|channel| channel.id.to_string() == id) {
            continue;
        }
        sqlx::query("DELETE FROM stream_channels WHERE id = ?")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

/// Deletes the subscriptions the bundle does not name, their archive and runs first so the
/// archive's delete trigger hands every archive password to the sweep.
async fn drop_unnamed_subscriptions(
    tx: &mut SqliteConnection,
    kept: &[ReplacementSubscription],
) -> Result<()> {
    let existing: Vec<String> = sqlx::query_scalar("SELECT id FROM subscriptions")
        .fetch_all(&mut *tx)
        .await?;
    for id in existing {
        if kept
            .iter()
            .any(|subscription| subscription.id.to_string() == id)
        {
            continue;
        }
        for statement in [
            "DELETE FROM subscription_items WHERE subscription_id = ?",
            "DELETE FROM subscription_runs WHERE subscription_id = ?",
            "DELETE FROM subscriptions WHERE id = ?",
        ] {
            sqlx::query(statement).bind(&id).execute(&mut *tx).await?;
        }
    }
    Ok(())
}

fn replacement_events() -> Vec<EventEnvelope> {
    [
        EventKind::CategoryChanged,
        EventKind::HotFolderChanged,
        EventKind::AuthProfileChanged,
        EventKind::AccountChanged,
        EventKind::ProxyChanged,
        EventKind::UsenetChanged,
        EventKind::StreamChanged,
    ]
    .into_iter()
    .map(|kind| {
        EventEnvelope::new(
            kind,
            serde_json::json!({ "resource": "settings_backup_import" }),
        )
    })
    .collect()
}
