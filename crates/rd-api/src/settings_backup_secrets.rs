//! Anonymous secret slots for the settings bundle.
//!
//! Credential values never travel under their own name: on export each `vault://`
//! reference is dereferenced once into a numbered slot, and on import the slots are minted
//! back into fresh references. Split out of `settings_backup.rs` to keep both files inside
//! the size budget.
//!
//! **Opt-in, deliberately.** [`ExportSlots::add`] is called per resource rather than sweeping
//! the secret store, so a secret is in a bundle only because somebody wrote a line putting it
//! there. That is what keeps the second factor out: an authenticator seed and its recovery
//! codes live in the same store, and a bundle is a file people copy between machines and hand
//! to each other for support. `crates/rd-api/tests/mfa.rs` fails if that ever changes.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use secrecy::ExposeSecret;

use crate::{
    ApiError, AppState, settings_backup::invalid_bundle, settings_backup_dto::SettingsBundle,
};

#[derive(Default)]
pub(crate) struct ExportSlots {
    references: HashMap<String, String>,
    pub(crate) values: BTreeMap<String, String>,
}

impl ExportSlots {
    pub(crate) async fn add(
        &mut self,
        state: &AppState,
        reference: Option<String>,
    ) -> Result<Option<String>, ApiError> {
        let Some(reference) = reference else {
            return Ok(None);
        };
        if let Some(slot) = self.references.get(&reference) {
            return Ok(Some(slot.clone()));
        }
        let value = state.secrets.get(&reference).await?;
        let slot = format!("s{}", self.values.len());
        self.values
            .insert(slot.clone(), value.expose_secret().to_owned());
        self.references.insert(reference, slot.clone());
        Ok(Some(slot))
    }
}

pub(crate) fn validate_secret_slots(
    bundle: &SettingsBundle,
    values: &BTreeMap<String, String>,
    needed: &BTreeSet<String>,
) -> Result<(), ApiError> {
    if (bundle.secrets.is_none() && !needed.is_empty())
        || needed
            .iter()
            .any(|slot| values.get(slot).is_none_or(String::is_empty))
    {
        return Err(invalid_bundle(
            "The settings bundle contains an invalid secret slot",
        ));
    }
    Ok(())
}

pub(crate) async fn mint_secret_references(
    state: &AppState,
    values: &BTreeMap<String, String>,
    needed: &BTreeSet<String>,
) -> Result<BTreeMap<String, String>, ApiError> {
    let mut minted = BTreeMap::new();
    for slot in needed {
        let result = state.secrets.put_string(values[slot].clone()).await;
        match result {
            Ok(reference) => {
                minted.insert(slot.clone(), reference);
            }
            Err(error) => {
                crate::config_handlers::cleanup_secrets(
                    &state.secrets,
                    minted.into_values().map(Some).collect::<Vec<_>>(),
                )
                .await;
                return Err(error.into());
            }
        }
    }
    Ok(minted)
}

pub(crate) async fn current_secret_references(
    state: &AppState,
) -> Result<Vec<Option<String>>, ApiError> {
    let mut references = state
        .database
        .list_proxy_profiles()
        .await?
        .into_iter()
        .map(|profile| profile.secret_ref)
        .collect::<Vec<_>>();
    for account in state.database.list_accounts().await? {
        if let Some((secret, cookies)) = state.database.account_secret_refs(account.id).await? {
            references.extend([secret, cookies]);
        }
    }
    for server in state.database.list_usenet_servers().await? {
        if let Some(config) = state.database.usenet_connection_config(server.id).await? {
            references.push(config.password_ref);
        }
    }
    references.extend(crate::settings_backup_auth::current_references(state).await?);
    Ok(references)
}
