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

mod insert;

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
    let released_secrets = clear_replaced_rows(&mut tx, &replacement).await?;
    insert_replacement(&mut tx, replacement, now).await?;
    clear_dangling_categories(&mut tx).await?;

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

/// Empties the tables the replacement rewrites and answers the vault references the dropped
/// accounts' sign-ins held.
async fn clear_replaced_rows(
    tx: &mut SqliteConnection,
    replacement: &ConfigReplacement,
) -> Result<Vec<String>> {
    // Accounts and stream channels own state that is not configuration -- sign-ins with their
    // vaulted tokens, remote jobs, recording schedules and their runs -- through `ON DELETE
    // CASCADE`, and deferring the foreign keys defers the check, never the cascade (DB-01). So
    // only the rows the bundle no longer names are deleted, taking their children with them;
    // the others are updated in place below and keep theirs.
    let released_secrets = drop_unnamed_accounts(&mut *tx, &replacement.accounts).await?;
    drop_unnamed_stream_channels(&mut *tx, &replacement.stream_channels).await?;
    // Subscriptions the same way (RA-DB-04): their archive is the once-only guarantee, so a
    // named one keeps it; a dropped one's archive passwords reach the sweep through the
    // `subscription_items` delete trigger, and `Database::replace_config` sweeps them.
    drop_unnamed_subscriptions(&mut *tx, &replacement.subscriptions).await?;
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
    Ok(released_secrets)
}

/// Writes every table of the bundle, in the order the import has always used.
async fn insert_replacement(
    tx: &mut SqliteConnection,
    replacement: ConfigReplacement,
    now: DateTime<Utc>,
) -> Result<()> {
    insert::insert_storage_roots(&mut *tx, replacement.storage_roots, now).await?;
    insert::insert_categories(&mut *tx, replacement.categories, now).await?;
    insert::insert_category_rules(&mut *tx, replacement.category_rules, now).await?;
    insert::insert_hotfolders(&mut *tx, replacement.hotfolders, now).await?;
    insert::insert_stream_channels(&mut *tx, replacement.stream_channels, now).await?;
    insert::insert_subscriptions(&mut *tx, replacement.subscriptions, now).await?;
    insert::insert_auth_profiles(&mut *tx, replacement.auth_profiles, now).await?;
    insert::insert_indexers(&mut *tx, replacement.indexers, now).await?;
    insert::insert_proxy_profiles(&mut *tx, replacement.proxy_profiles, now).await?;
    insert::insert_accounts(&mut *tx, replacement.accounts, now).await?;
    insert::insert_usenet_servers(&mut *tx, replacement.usenet_servers, now).await?;
    Ok(())
}

/// Clears the review/queue rows' category references the bundle no longer holds.
async fn clear_dangling_categories(tx: &mut SqliteConnection) -> Result<()> {
    for table in ["link_candidates", "nzb_imports", "collector_packages"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET category_id = NULL WHERE category_id IS NOT NULL \
             AND NOT EXISTS (SELECT 1 FROM categories WHERE categories.id = {table}.category_id)"
        )))
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
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
            "DELETE FROM subscription_item_keys WHERE subscription_id = ?",
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
