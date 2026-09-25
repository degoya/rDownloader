//! Auth profiles in the settings backup bundle.
//!
//! Kept apart from `settings_backup.rs` so that file stays inside the size budget. The
//! rules are the same as for every other credential-bearing resource: values leave only
//! through anonymous slots, and only when the export asks for secrets.

use std::collections::BTreeMap;

use crate::{
    ApiError, AppState,
    settings_backup_dto::{BundleAuthProfile, SettingsBundle},
};

/// Turns stored profiles into bundle entries, parking each secret in a slot.
pub(crate) async fn export(
    state: &AppState,
    slots: &mut crate::settings_backup_secrets::ExportSlots,
    include_secrets: bool,
) -> Result<Vec<BundleAuthProfile>, ApiError> {
    let profiles = state.database.list_auth_profiles().await?;
    let mut bundled = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let (secret_ref, certificate_ref) = if include_secrets {
            (profile.secret_ref, profile.certificate_ref)
        } else {
            (None, None)
        };
        bundled.push(BundleAuthProfile {
            id: profile.id,
            name: profile.name,
            scope: profile.scope,
            method: profile.method,
            origin: profile.origin,
            enabled: profile.enabled,
            expires_at: profile.expires_at,
            username: profile.username,
            secret_slot: slots.add(state, secret_ref).await?,
            certificate_slot: slots.add(state, certificate_ref).await?,
        });
    }
    Ok(bundled)
}

/// Slot names an imported bundle needs decrypted values for.
pub(crate) fn referenced_slots(bundle: &SettingsBundle) -> impl Iterator<Item = String> + '_ {
    bundle
        .auth_profiles
        .iter()
        .filter_map(|value| value.secret_slot.clone())
        .chain(
            bundle
                .auth_profiles
                .iter()
                .filter_map(|value| value.certificate_slot.clone()),
        )
}

/// References currently held by stored profiles, cleaned up after a successful import.
pub(crate) async fn current_references(state: &AppState) -> Result<Vec<Option<String>>, ApiError> {
    Ok(state
        .database
        .list_auth_profiles()
        .await?
        .into_iter()
        .flat_map(|profile| [profile.secret_ref, profile.certificate_ref])
        .collect())
}

/// Rebuilds the rows to insert, swapping slot names for freshly minted references.
pub(crate) fn into_replacement(
    profiles: Vec<BundleAuthProfile>,
    minted: &BTreeMap<String, String>,
) -> Vec<rd_db::ReplacementAuthProfile> {
    profiles
        .into_iter()
        .map(|value| rd_db::ReplacementAuthProfile {
            id: value.id,
            name: value.name,
            scope: value.scope,
            method: value.method,
            origin: value.origin,
            enabled: value.enabled,
            expires_at: value.expires_at,
            username: value.username,
            secret_ref: value
                .secret_slot
                .and_then(|slot| minted.get(&slot).cloned()),
            certificate_ref: value
                .certificate_slot
                .and_then(|slot| minted.get(&slot).cloned()),
        })
        .collect()
}
