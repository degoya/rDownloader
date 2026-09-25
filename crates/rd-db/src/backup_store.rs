//! Atomic replacement of all server-side configuration tables.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{
    AccountId, Category, CategoryRule, EventEnvelope, EventKind, HotFolderConfig, ProxyKind,
    ProxyProfileId, StorageRootConfig, StreamChannelId, UsenetServerId,
};
use sqlx::{Connection, SqliteConnection};
use url::Url;

use crate::writer::insert_event;

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
    /// Splitting, remux, sidecars and VOD fallback (RD-080-09).
    pub recording: rd_core::RecordingPolicy,
}

/// Subscription as it travels in a settings bundle (RD-080-07).
///
/// The archive is deliberately *not* part of it: item history is not configuration, and
/// restoring a bundle onto another machine must not make that machine believe it has
/// already downloaded things it has not. A restored subscription therefore starts unprimed
/// and applies its backlog policy afresh — which is the safe direction, because the policy
/// defaults to "nothing that already exists".
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
}

pub(crate) async fn replace_all(
    connection: &mut SqliteConnection,
    mut replacement: ConfigReplacement,
) -> Result<Vec<EventEnvelope>> {
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
    for table in [
        "auth_profiles",
        "accounts",
        "usenet_servers",
        "proxy_profiles",
        "stream_channels",
        "subscription_runs",
        "subscription_items",
        "subscriptions",
        "hotfolders",
        "category_rules",
        "categories",
        "storage_roots",
    ] {
        sqlx::query(&format!("DELETE FROM {table}"))
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
             postprocess_level, script, cleanup_extensions, recursive_unpack, sfv_verify, \
             safe_postproc, delete_par2, \
             upload_enabled, upload_remote, seeding_json, plugin_steps_json, created_at, \
             updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
             VALUES (?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?, ?)",
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
             primed, consecutive_failures, secret_ref, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?, ?, ?)",
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
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        sqlx::query(&format!(
            "UPDATE {table} SET category_id = NULL WHERE category_id IS NOT NULL \
             AND NOT EXISTS (SELECT 1 FROM categories WHERE categories.id = {table}.category_id)"
        ))
        .execute(&mut *tx)
        .await?;
    }

    let events = replacement_events();
    for event in &events {
        insert_event(&mut tx, event).await?;
    }
    tx.commit().await?;
    Ok(events)
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

fn enum_string<T: serde::Serialize>(value: T) -> Result<String> {
    Ok(serde_json::to_string(&value)?.trim_matches('"').to_owned())
}
