use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::hash::Hash;

use axum::{Json, extract::State};
use chrono::Utc;

use crate::{
    ApiError, AppState,
    settings_backup_crypto::{decrypt_secrets, encrypt_secrets},
};

pub use crate::settings_backup_dto::{
    BundleAccount, BundleProxyProfile, BundleStreamChannel, BundleUsenetServer,
    ExportSettingsRequest, ImportSettingsRequest, ImportSummaryResponse, SettingsBundle,
};

const BUNDLE_FORMAT: &str = "rdownloader-settings-bundle";
const BUNDLE_VERSION: u32 = 1;

#[utoipa::path(
    post,
    path = "/api/v1/settings/export",
    tag = "system",
    request_body = ExportSettingsRequest,
    responses((status = 200, body = SettingsBundle), (status = 400, body = crate::error::ErrorBody))
)]
pub async fn export_settings(
    State(state): State<AppState>,
    Json(request): Json<ExportSettingsRequest>,
) -> Result<Json<SettingsBundle>, ApiError> {
    let passphrase = if request.include_secrets {
        let passphrase = request
            .passphrase
            .filter(|value| value.chars().count() >= 8);
        Some(passphrase.ok_or_else(passphrase_required)?)
    } else {
        None
    };
    let (
        settings,
        storage_roots,
        categories,
        category_rules,
        hotfolders,
        stream_channels,
        proxy_profiles,
        accounts,
        usenet_servers,
    ) = tokio::try_join!(
        crate::handlers::read_settings(&state),
        async { Ok::<_, ApiError>(state.database.list_storage_roots().await?) },
        async { Ok::<_, ApiError>(state.database.list_categories().await?) },
        async { Ok::<_, ApiError>(state.database.list_category_rules().await?) },
        async { Ok::<_, ApiError>(state.database.list_hotfolders().await?) },
        async { Ok::<_, ApiError>(state.database.list_stream_channels().await?) },
        async { Ok::<_, ApiError>(state.database.list_proxy_profiles().await?) },
        async { Ok::<_, ApiError>(state.database.list_accounts().await?) },
        async { Ok::<_, ApiError>(state.database.list_usenet_servers().await?) },
    )?;

    let mut slots = crate::settings_backup_secrets::ExportSlots::default();
    let include_secrets = request.include_secrets;
    let mut bundled_proxies = Vec::with_capacity(proxy_profiles.len());
    for profile in proxy_profiles {
        let secret_slot = slots
            .add(
                &state,
                include_secrets.then_some(profile.secret_ref).flatten(),
            )
            .await?;
        bundled_proxies.push(BundleProxyProfile {
            id: profile.id,
            name: profile.name,
            kind: profile.kind,
            endpoint: profile.endpoint,
            username: profile.username,
            secret_slot,
        });
    }
    let mut bundled_accounts = Vec::with_capacity(accounts.len());
    for account in accounts {
        let references = if include_secrets {
            state
                .database
                .account_secret_refs(account.id)
                .await?
                .unwrap_or((None, None))
        } else {
            (None, None)
        };
        bundled_accounts.push(BundleAccount {
            id: account.id,
            provider: account.provider,
            label: account.label,
            username: account.username,
            credential_mode: account.credential_mode,
            proxy_profile_id: account.proxy_profile_id,
            enabled: account.enabled,
            secret_slot: slots.add(&state, references.0).await?,
            cookies_slot: slots.add(&state, references.1).await?,
        });
    }
    let mut bundled_subscriptions = Vec::new();
    for subscription in state.database.list_subscriptions().await? {
        let secret_ref = if include_secrets {
            subscription.secret_ref.clone()
        } else {
            None
        };
        bundled_subscriptions.push(crate::settings_backup_dto::BundleSubscription {
            id: subscription.id,
            name: subscription.name,
            url: subscription.url.to_string(),
            kind: subscription.kind,
            enabled: subscription.enabled,
            mode: subscription.mode,
            category_id: subscription.category_id,
            priority: subscription.priority,
            interval_seconds: subscription.interval_seconds,
            filters: subscription.filters,
            backlog: subscription.backlog,
            category_map: subscription.category_map,
            source_categories: subscription.source_categories,
            every_release: subscription.every_release,
            view: subscription.view,
            autoplay: subscription.autoplay,
            card_ratio: subscription.card_ratio,
            schedule: subscription.schedule,
            secret_slot: slots.add(&state, secret_ref).await?,
        });
    }
    let bundled_auth_profiles =
        crate::settings_backup_auth::export(&state, &mut slots, include_secrets).await?;
    let mut bundled_servers = Vec::with_capacity(usenet_servers.len());
    for server in usenet_servers {
        let password_ref = if include_secrets {
            state
                .database
                .usenet_connection_config(server.id)
                .await?
                .and_then(|config| config.password_ref)
        } else {
            None
        };
        bundled_servers.push(BundleUsenetServer {
            id: server.id,
            name: server.name,
            host: server.host,
            port: server.port,
            tls: server.tls,
            username: server.username,
            proxy_profile_id: server.proxy_profile_id,
            priority: server.priority,
            max_connections: server.max_connections,
            enabled: server.enabled,
            password_slot: slots.add(&state, password_ref).await?,
        });
    }
    let encrypted = match passphrase {
        Some(passphrase) => Some(encrypt_secrets(&passphrase, &slots.values).await?),
        None => None,
    };
    Ok(Json(SettingsBundle {
        format: BUNDLE_FORMAT.to_owned(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        settings,
        storage_roots,
        categories,
        category_rules,
        hotfolders,
        stream_channels: stream_channels
            .into_iter()
            .map(|channel| BundleStreamChannel {
                id: channel.id,
                url: channel.url,
                name: channel.name,
                quality: channel.quality,
                category_id: channel.category_id,
                enabled: channel.enabled,
                recording: channel.recording,
            })
            .collect(),
        subscriptions: bundled_subscriptions,
        proxy_profiles: bundled_proxies,
        accounts: bundled_accounts,
        usenet_servers: bundled_servers,
        auth_profiles: bundled_auth_profiles,
        secrets: encrypted,
    }))
}

/// Replaces all configuration tables atomically after validation. Secret-store writes made before
/// the database swap are removed on failure. A process crash after the swap but before settings
/// persistence can temporarily leave new tables with old settings; the next settings save heals it.
#[utoipa::path(
    post,
    path = "/api/v1/settings/import",
    tag = "system",
    request_body = ImportSettingsRequest,
    responses((status = 200, body = ImportSummaryResponse), (status = 400, body = crate::error::ErrorBody))
)]
pub async fn import_settings(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Json(request): Json<ImportSettingsRequest>,
) -> Result<Json<ImportSummaryResponse>, ApiError> {
    let mut bundle = request.bundle;
    validate_header(&bundle)?;
    validate_references(&bundle)?;
    crate::handlers::validate_settings(&mut bundle.settings)?;

    let secrets_included = bundle.secrets.is_some();
    let secret_values = match &bundle.secrets {
        Some(encrypted) => {
            let passphrase = request
                .passphrase
                .as_deref()
                .filter(|value| !value.is_empty())
                .ok_or_else(passphrase_required)?;
            decrypt_secrets(passphrase, encrypted).await?
        }
        None => BTreeMap::new(),
    };
    let needed_slots = referenced_slots(&bundle);
    crate::settings_backup_secrets::validate_secret_slots(&bundle, &secret_values, &needed_slots)?;
    let old_account_ids = state
        .database
        .list_accounts()
        .await?
        .into_iter()
        .map(|account| account.id)
        .collect::<Vec<_>>();
    let old_references = crate::settings_backup_secrets::current_secret_references(&state).await?;
    let minted = crate::settings_backup_secrets::mint_secret_references(
        &state,
        &secret_values,
        &needed_slots,
    )
    .await?;
    let minted_for_cleanup = minted.values().cloned().map(Some).collect::<Vec<_>>();
    let summary = ImportSummaryResponse::from_bundle(&bundle);
    let imported_account_ids = bundle
        .accounts
        .iter()
        .map(|account| account.id)
        .collect::<Vec<_>>();
    let settings = bundle.settings.clone();
    let replacement = into_replacement(bundle, &minted);
    if let Err(error) = state.database.replace_config(replacement).await {
        crate::config_handlers::cleanup_secrets(&state.secrets, minted_for_cleanup).await;
        crate::audit::record(
            &state,
            crate::audit::AuditEvent::failure(rd_core::AuditAction::BackupRestored)
                .by(&audit)
                .target("backup", "configuration"),
        )
        .await;
        return Err(error.into());
    }
    crate::config_handlers::cleanup_secrets(&state.secrets, old_references).await;
    for id in old_account_ids.into_iter().chain(imported_account_ids) {
        crate::hosters::forget(id);
    }
    crate::handlers::apply_settings(&state, settings).await?;
    // The one action that replaces the whole configuration in a single request, and the one
    // the audit log most has to survive: `replace_config` clears configuration tables and
    // `audit_records` is not among them, so the record of a restore outlives the restore.
    // Counts only, never the contents: the bundle holds accounts, proxies and Usenet servers.
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::BackupRestored)
            .by(&audit)
            .target("backup", "configuration")
            .detail("storage_roots", summary.storage_roots)
            .detail("categories", summary.categories)
            .detail("accounts", summary.accounts)
            .detail("usenet_servers", summary.usenet_servers)
            .detail("with_secrets", secrets_included),
    )
    .await;
    Ok(Json(summary))
}

fn validate_header(bundle: &SettingsBundle) -> Result<(), ApiError> {
    if bundle.format != BUNDLE_FORMAT {
        return Err(invalid_bundle(
            "The selected file is not an rDownloader settings bundle",
        ));
    }
    if bundle.version != BUNDLE_VERSION {
        return Err(ApiError::bad_request(
            "settings.backup_version_unsupported",
            format!(
                "Settings bundle version {} is not supported",
                bundle.version
            ),
        ));
    }
    Ok(())
}

fn validate_references(bundle: &SettingsBundle) -> Result<(), ApiError> {
    let roots = unique_set(bundle.storage_roots.iter().map(|value| value.id))?;
    let categories = unique_set(bundle.categories.iter().map(|value| value.id))?;
    let proxies = unique_set(bundle.proxy_profiles.iter().map(|value| value.id))?;
    unique_set(bundle.category_rules.iter().map(|value| value.id))?;
    unique_set(bundle.hotfolders.iter().map(|value| value.id))?;
    unique_set(bundle.stream_channels.iter().map(|value| value.id))?;
    unique_set(bundle.accounts.iter().map(|value| value.id))?;
    unique_set(bundle.usenet_servers.iter().map(|value| value.id))?;
    unique_set(bundle.auth_profiles.iter().map(|value| value.id))?;
    let valid = bundle
        .categories
        .iter()
        .all(|value| roots.contains(&value.storage_root_id))
        && bundle
            .category_rules
            .iter()
            .all(|value| categories.contains(&value.category_id))
        && bundle
            .hotfolders
            .iter()
            .all(|value| value.category_id.is_none_or(|id| categories.contains(&id)))
        && bundle
            .stream_channels
            .iter()
            .all(|value| value.category_id.is_none_or(|id| categories.contains(&id)))
        && bundle.accounts.iter().all(|value| {
            value
                .proxy_profile_id
                .is_none_or(|id| proxies.contains(&id))
        })
        && bundle.usenet_servers.iter().all(|value| {
            value
                .proxy_profile_id
                .is_none_or(|id| proxies.contains(&id))
        })
        && bundle
            .settings
            .global_proxy_profile_id
            .is_none_or(|id| proxies.contains(&id));
    if !valid {
        return Err(ApiError::bad_request(
            "settings.backup_reference_invalid",
            "The settings bundle contains a dangling configuration reference",
        ));
    }
    Ok(())
}

fn unique_set<T: Copy + Eq + Hash>(
    values: impl IntoIterator<Item = T>,
) -> Result<HashSet<T>, ApiError> {
    let mut result = HashSet::new();
    for value in values {
        if !result.insert(value) {
            return Err(invalid_bundle(
                "The settings bundle contains duplicate identifiers",
            ));
        }
    }
    Ok(result)
}

fn referenced_slots(bundle: &SettingsBundle) -> BTreeSet<String> {
    bundle
        .proxy_profiles
        .iter()
        .filter_map(|value| value.secret_slot.clone())
        .chain(
            bundle
                .accounts
                .iter()
                .filter_map(|value| value.secret_slot.clone()),
        )
        .chain(
            bundle
                .accounts
                .iter()
                .filter_map(|value| value.cookies_slot.clone()),
        )
        .chain(
            bundle
                .usenet_servers
                .iter()
                .filter_map(|value| value.password_slot.clone()),
        )
        .chain(
            bundle
                .subscriptions
                .iter()
                .filter_map(|value| value.secret_slot.clone()),
        )
        .chain(crate::settings_backup_auth::referenced_slots(bundle))
        .collect()
}

fn into_replacement(
    bundle: SettingsBundle,
    minted: &BTreeMap<String, String>,
) -> rd_db::ConfigReplacement {
    rd_db::ConfigReplacement {
        auth_profiles: crate::settings_backup_auth::into_replacement(bundle.auth_profiles, minted),
        storage_roots: bundle.storage_roots,
        categories: bundle.categories,
        category_rules: bundle.category_rules,
        hotfolders: bundle.hotfolders,
        stream_channels: bundle
            .stream_channels
            .into_iter()
            .map(|value| rd_db::ReplacementStreamChannel {
                id: value.id,
                url: value.url,
                name: value.name,
                quality: value.quality,
                category_id: value.category_id,
                enabled: value.enabled,
                recording: value.recording,
            })
            .collect(),
        subscriptions: bundle
            .subscriptions
            .into_iter()
            .map(|value| rd_db::ReplacementSubscription {
                id: value.id,
                name: value.name,
                url: value.url,
                kind: value.kind,
                enabled: value.enabled,
                mode: value.mode,
                category_id: value.category_id,
                priority: value.priority,
                interval_seconds: value.interval_seconds,
                filters: value.filters,
                backlog: value.backlog,
                category_map: value.category_map,
                source_categories: value.source_categories,
                every_release: value.every_release,
                view: value.view,
                autoplay: value.autoplay,
                card_ratio: value.card_ratio,
                schedule: value.schedule,
                secret_ref: slot_reference(minted, value.secret_slot),
            })
            .collect(),
        proxy_profiles: bundle
            .proxy_profiles
            .into_iter()
            .map(|value| rd_db::ReplacementProxyProfile {
                id: value.id,
                name: value.name,
                kind: value.kind,
                endpoint: value.endpoint,
                username: value.username,
                secret_ref: slot_reference(minted, value.secret_slot),
            })
            .collect(),
        accounts: bundle
            .accounts
            .into_iter()
            .map(|value| rd_db::ReplacementAccount {
                id: value.id,
                provider: value.provider,
                label: value.label,
                username: value.username,
                credential_mode: value.credential_mode,
                secret_ref: slot_reference(minted, value.secret_slot),
                cookie_ref: slot_reference(minted, value.cookies_slot),
                proxy_profile_id: value.proxy_profile_id,
                enabled: value.enabled,
            })
            .collect(),
        usenet_servers: bundle
            .usenet_servers
            .into_iter()
            .map(|value| rd_db::ReplacementUsenetServer {
                id: value.id,
                name: value.name,
                host: value.host,
                port: value.port,
                tls: value.tls,
                username: value.username,
                password_ref: slot_reference(minted, value.password_slot),
                proxy_profile_id: value.proxy_profile_id,
                priority: value.priority,
                max_connections: value.max_connections,
                enabled: value.enabled,
            })
            .collect(),
    }
}

fn slot_reference(minted: &BTreeMap<String, String>, slot: Option<String>) -> Option<String> {
    slot.and_then(|slot| minted.get(&slot).cloned())
}

fn passphrase_required() -> ApiError {
    ApiError::bad_request(
        "settings.backup_passphrase_required",
        "A passphrase of at least 8 characters is required for a backup with secrets",
    )
}

pub(crate) fn invalid_bundle(message: impl Into<String>) -> ApiError {
    ApiError::bad_request("settings.backup_invalid", message)
}
