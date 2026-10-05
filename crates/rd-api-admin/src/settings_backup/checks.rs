//! What an imported bundle has to satisfy before it replaces anything, and how it becomes the
//! replacement.

use super::*;

pub(super) fn validate_references(bundle: &SettingsBundle) -> Result<(), ApiError> {
    let roots = unique_set(bundle.storage_roots.iter().map(|value| value.id))?;
    let categories = unique_set(bundle.categories.iter().map(|value| value.id))?;
    let proxies = unique_set(bundle.proxy_profiles.iter().map(|value| value.id))?;
    unique_set(bundle.category_rules.iter().map(|value| value.id))?;
    unique_set(bundle.hotfolders.iter().map(|value| value.id))?;
    unique_set(bundle.stream_channels.iter().map(|value| value.id))?;
    unique_set(bundle.accounts.iter().map(|value| value.id))?;
    unique_set(bundle.usenet_servers.iter().map(|value| value.id))?;
    unique_set(bundle.auth_profiles.iter().map(|value| value.id))?;
    unique_set(bundle.indexers.iter().map(|value| value.id))?;
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

pub(super) fn unique_set<T: Copy + Eq + Hash>(
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

pub(super) fn referenced_slots(bundle: &SettingsBundle) -> BTreeSet<String> {
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
        .chain(crate::settings_backup_indexers::referenced_slots(bundle))
        .collect()
}

pub(super) fn into_replacement(
    bundle: SettingsBundle,
    minted: &BTreeMap<String, String>,
) -> rd_db::ConfigReplacement {
    rd_db::ConfigReplacement {
        auth_profiles: crate::settings_backup_auth::into_replacement(bundle.auth_profiles, minted),
        indexers: crate::settings_backup_indexers::into_replacement(bundle.indexers, minted),
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
                script_arguments: value.script_arguments,
                indexer_search: value.indexer_search,
                git_release: value.git_release,
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

pub(super) fn slot_reference(
    minted: &BTreeMap<String, String>,
    slot: Option<String>,
) -> Option<String> {
    slot.and_then(|slot| minted.get(&slot).cloned())
}
