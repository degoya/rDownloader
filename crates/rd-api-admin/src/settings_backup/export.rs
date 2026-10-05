//! Building a settings bundle: every configuration table, its credentials sealed or left out.

use super::*;
use crate::settings_backup_secrets::ExportSlots;

/// How a settings bundle carries its credentials.
pub enum SecretSealing<'a> {
    /// Not at all.
    Omit,
    /// Sealed under a key derived from this passphrase.
    Passphrase(String),
    /// Sealed under the full backup's key (RD-160-01), which the same passphrase derives, so the
    /// bundle inside a full backup opens through the ordinary import.
    BackupKey(&'a rd_backup::BackupKey),
}

/// The settings bundle, as the export route and the full backup build it.
pub async fn build_settings_bundle(
    state: &AppState,
    sealing: SecretSealing<'_>,
) -> Result<SettingsBundle, ApiError> {
    let state = state.clone();
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
        crate::settings_store::read_settings(&state),
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
    let include_secrets = !matches!(sealing, SecretSealing::Omit);
    let bundled_proxies =
        bundle_proxy_profiles(&state, &mut slots, include_secrets, proxy_profiles).await?;
    let bundled_accounts = bundle_accounts(&state, &mut slots, include_secrets, accounts).await?;
    let bundled_subscriptions = bundle_subscriptions(&state, &mut slots, include_secrets).await?;
    let bundled_auth_profiles =
        crate::settings_backup_auth::export(&state, &mut slots, include_secrets).await?;
    let bundled_indexers =
        crate::settings_backup_indexers::export(&state, &mut slots, include_secrets).await?;
    let bundled_servers =
        bundle_usenet_servers(&state, &mut slots, include_secrets, usenet_servers).await?;
    // Only the full backup carries the archive passwords (RD-190-04), sealed with the rest
    // under its key: a settings export is about settings, and packages are not.
    let archive_passwords = bundle_archive_passwords(&state, &mut slots, &sealing).await?;
    let encrypted = match sealing {
        SecretSealing::Passphrase(passphrase) => {
            Some(encrypt_secrets(&passphrase, &slots.values).await?)
        }
        SecretSealing::BackupKey(key) => Some(encrypt_secrets_with_key(key, &slots.values)?),
        SecretSealing::Omit => None,
    };
    Ok(SettingsBundle {
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
        archive_passwords,
        indexers: bundled_indexers,
        secrets: encrypted,
    })
}

/// The proxy profiles, each with the slot its secret went into.
async fn bundle_proxy_profiles(
    state: &AppState,
    slots: &mut ExportSlots,
    include_secrets: bool,
    proxy_profiles: Vec<rd_core::ProxyProfile>,
) -> Result<Vec<BundleProxyProfile>, ApiError> {
    let mut bundled_proxies = Vec::with_capacity(proxy_profiles.len());
    for profile in proxy_profiles {
        let secret_slot = slots
            .add(
                state,
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
    Ok(bundled_proxies)
}

/// The provider accounts, each with the slots of its secret and its cookies.
async fn bundle_accounts(
    state: &AppState,
    slots: &mut ExportSlots,
    include_secrets: bool,
    accounts: Vec<rd_core::Account>,
) -> Result<Vec<BundleAccount>, ApiError> {
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
            secret_slot: slots.add(state, references.0).await?,
            cookies_slot: slots.add(state, references.1).await?,
        });
    }
    Ok(bundled_accounts)
}

/// The subscriptions, each with the slot of its token.
async fn bundle_subscriptions(
    state: &AppState,
    slots: &mut ExportSlots,
    include_secrets: bool,
) -> Result<Vec<crate::settings_backup_dto::BundleSubscription>, ApiError> {
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
            script_arguments: subscription.script_arguments,
            indexer_search: subscription.indexer_search,
            git_release: subscription.git_release,
            secret_slot: slots.add(state, secret_ref).await?,
        });
    }
    Ok(bundled_subscriptions)
}

/// The Usenet servers, each with the slot of its password.
async fn bundle_usenet_servers(
    state: &AppState,
    slots: &mut ExportSlots,
    include_secrets: bool,
    usenet_servers: Vec<rd_core::UsenetServer>,
) -> Result<Vec<BundleUsenetServer>, ApiError> {
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
            password_slot: slots.add(state, password_ref).await?,
        });
    }
    Ok(bundled_servers)
}

/// The archive passwords, readable ones only, for a full backup alone.
async fn bundle_archive_passwords(
    state: &AppState,
    slots: &mut ExportSlots,
    sealing: &SecretSealing<'_>,
) -> Result<Vec<crate::settings_backup_dto::BundleArchivePassword>, ApiError> {
    let mut archive_passwords = Vec::new();
    if matches!(sealing, SecretSealing::BackupKey(_)) {
        for (table, id, reference) in state.database.archive_password_references().await? {
            if let Some(slot) = slots.add_readable(state, reference).await {
                archive_passwords.push(crate::settings_backup_dto::BundleArchivePassword {
                    table: table.to_owned(),
                    id,
                    slot,
                });
            }
        }
    }
    Ok(archive_passwords)
}
