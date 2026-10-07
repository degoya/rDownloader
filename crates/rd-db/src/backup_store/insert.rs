//! The inserts of a configuration replacement, one table each, in the order `replace_all` runs
//! them. Tables whose rows own state that is not configuration upsert by id instead.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{Category, CategoryRule, HotFolderConfig, StorageRootConfig};
use sqlx::SqliteConnection;

use super::{
    ReplacementAccount, ReplacementAuthProfile, ReplacementIndexer, ReplacementProxyProfile,
    ReplacementStreamChannel, ReplacementSubscription, ReplacementUsenetServer,
};
use crate::enum_string;

pub(super) async fn insert_storage_roots(
    tx: &mut SqliteConnection,
    values: Vec<StorageRootConfig>,
    now: DateTime<Utc>,
) -> Result<()> {
    for value in values {
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
    Ok(())
}

pub(super) async fn insert_categories(
    tx: &mut SqliteConnection,
    values: Vec<Category>,
    now: DateTime<Utc>,
) -> Result<()> {
    for value in values {
        sqlx::query(
            "INSERT INTO categories (id, name, color, storage_root_id, relative_path, is_default, \
             postprocess_level, script, cleanup_extensions, recursive_unpack, unpack_to_subfolder, \
             direct_unpack, malware_scan, sfv_verify, safe_postproc, delete_par2, \
             upload_enabled, upload_remote, seeding_json, plugin_steps_json, sorting_json, \
             unwrap_package_folder, package_name_rules_json, package_name_regex_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(value.unwrap_package_folder)
        .bind(crate::config_store::package_name_rules_json(
            value.package_name_rules,
        )?)
        .bind(crate::config_store::package_name_regex_json(
            value.package_name_regex.as_ref(),
        )?)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn insert_category_rules(
    tx: &mut SqliteConnection,
    values: Vec<CategoryRule>,
    now: DateTime<Utc>,
) -> Result<()> {
    for value in values {
        sqlx::query(
            "INSERT INTO category_rules (id, name, priority, source, domain, protocol, extension, \
             mime_type, name_regex, name_target, category_id, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(enum_string(value.name_target)?)
        .bind(value.category_id.to_string())
        .bind(value.enabled)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn insert_hotfolders(
    tx: &mut SqliteConnection,
    values: Vec<HotFolderConfig>,
    now: DateTime<Utc>,
) -> Result<()> {
    for value in values {
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
    Ok(())
}

pub(super) async fn insert_stream_channels(
    tx: &mut SqliteConnection,
    values: Vec<ReplacementStreamChannel>,
    now: DateTime<Utc>,
) -> Result<()> {
    for value in values {
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
    Ok(())
}

pub(super) async fn insert_subscriptions(
    tx: &mut SqliteConnection,
    values: Vec<ReplacementSubscription>,
    now: DateTime<Utc>,
) -> Result<()> {
    for value in values {
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
    Ok(())
}

pub(super) async fn insert_auth_profiles(
    tx: &mut SqliteConnection,
    values: Vec<ReplacementAuthProfile>,
    now: DateTime<Utc>,
) -> Result<()> {
    for value in values {
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
    Ok(())
}

pub(super) async fn insert_indexers(
    tx: &mut SqliteConnection,
    values: Vec<ReplacementIndexer>,
    now: DateTime<Utc>,
) -> Result<()> {
    for value in values {
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
    Ok(())
}

pub(super) async fn insert_proxy_profiles(
    tx: &mut SqliteConnection,
    values: Vec<ReplacementProxyProfile>,
    now: DateTime<Utc>,
) -> Result<()> {
    for value in values {
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
    Ok(())
}

pub(super) async fn insert_accounts(
    tx: &mut SqliteConnection,
    values: Vec<ReplacementAccount>,
    now: DateTime<Utc>,
) -> Result<()> {
    for value in values {
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
    Ok(())
}

pub(super) async fn insert_usenet_servers(
    tx: &mut SqliteConnection,
    values: Vec<ReplacementUsenetServer>,
    now: DateTime<Utc>,
) -> Result<()> {
    for value in values {
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
    Ok(())
}
