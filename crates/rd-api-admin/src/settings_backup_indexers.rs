//! Newznab indexers in the settings backup bundle (RD-190-22).
//!
//! Kept apart from `settings_backup.rs` so that file stays inside the size budget. The API key
//! follows the rule of every other credential-bearing resource: its value leaves only through an
//! anonymous slot, and only when the export asks for secrets.

use std::collections::BTreeMap;

use crate::{
    ApiError, AppState,
    settings_backup_dto::{BundleIndexer, SettingsBundle},
};

/// Turns stored indexers into bundle entries, parking each key in a slot.
pub(crate) async fn export(
    state: &AppState,
    slots: &mut crate::settings_backup_secrets::ExportSlots,
    include_secrets: bool,
) -> Result<Vec<BundleIndexer>, ApiError> {
    let indexers = state.database.list_indexers().await?;
    let mut bundled = Vec::with_capacity(indexers.len());
    for indexer in indexers {
        let secret_ref = include_secrets.then_some(indexer.secret_ref).flatten();
        bundled.push(BundleIndexer {
            id: indexer.id,
            name: indexer.name,
            url: indexer.url,
            categories: indexer.categories,
            enabled: indexer.enabled,
            secret_slot: slots.add(state, secret_ref).await?,
            list_style: indexer.list_style,
        });
    }
    Ok(bundled)
}

/// Slot names an imported bundle needs decrypted values for.
pub(crate) fn referenced_slots(bundle: &SettingsBundle) -> impl Iterator<Item = String> + '_ {
    bundle
        .indexers
        .iter()
        .filter_map(|value| value.secret_slot.clone())
}

/// References currently held by stored indexers, cleaned up after a successful import.
pub(crate) async fn current_references(state: &AppState) -> Result<Vec<Option<String>>, ApiError> {
    Ok(state
        .database
        .list_indexers()
        .await?
        .into_iter()
        .map(|indexer| indexer.secret_ref)
        .collect())
}

/// Rebuilds the rows to insert, swapping slot names for freshly minted references.
pub(crate) fn into_replacement(
    indexers: Vec<BundleIndexer>,
    minted: &BTreeMap<String, String>,
) -> Vec<rd_db::ReplacementIndexer> {
    indexers
        .into_iter()
        .map(|value| rd_db::ReplacementIndexer {
            id: value.id,
            name: value.name,
            url: value.url,
            secret_ref: value
                .secret_slot
                .and_then(|slot| minted.get(&slot).cloned()),
            categories: value.categories,
            enabled: value.enabled,
            list_style: value.list_style,
        })
        .collect()
}
