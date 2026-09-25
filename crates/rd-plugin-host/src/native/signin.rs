//! A sign-in's session beside the account's own credential, and computing over the half of
//! it that is key material (RD-120-30).
//!
//! Two halves of one decision. Where a value is **written** decides where it may later be
//! **read**, and where it is read decides which derivations the host admits over it:
//!
//! | value | written by | kept in | a chain over it begins with |
//! | --- | --- | --- | --- |
//! | what a person typed | the accounts form | `accounts.secret_ref` | `pbkdf2-hmac-sha512` |
//! | a sign-in's token | `store-token`, via [`NativeHost::store_session`] | `auth_flows.access_ref` | -- (a token is sent, not computed with) |
//! | a sign-in's key | `store-token`, via [`NativeHost::store_session`] | `auth_flows.key_ref` | `aes-ecb-decrypt` |
//!
//! Nothing but [`NativeHost::store_session`] writes `key_ref`, and it writes only what a
//! guest handed to `store-token` -- a value that guest already held. So the one thing the
//! relaxed first step computes over is something no person typed, and a plugin has no way to
//! put a person's credential there: the WIT `secret-handle` is a name, the origin is decided
//! here from the provider's own slot table, and a slot declared `filled_by = "flow"` is read
//! from the flow row and never from the account's credential.

use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{ClientIdentity, DerivationStep};
use secrecy::ExposeSecret;

use super::{
    expand::{account_missing, reference_active_for_account, secret_target_not_allowed},
    host::NativeHost,
};
use crate::{
    keyderive::{self, SecretOrigin},
    session::{self, FlowValue},
};

fn secret_missing() -> Failure {
    Failure::coded(
        FailureKind::AuthRequired,
        "plugin.provider_secret_missing",
        "Provider secret is missing",
    )
}

impl NativeHost {
    /// Runs a derivation over the credential behind `reference`, with the first step the
    /// credential's origin admits.
    ///
    /// Three gates before the vault is touched: the reference has to be one the account's
    /// provider owns and has active for the account's mode; the chain has to satisfy
    /// [`keyderive::validate`] for the origin **this function** determines; and a sign-in
    /// slot has to actually hold key material, because a bearer token is not a key.
    pub(super) async fn derive_over(
        &self,
        client: &ClientIdentity,
        reference: &str,
        steps: &[DerivationStep],
    ) -> Result<Vec<u8>, Failure> {
        keyderive::validate_shape(steps)?;
        let account_id = client.account_id.ok_or_else(account_missing)?;
        let (provider, mode) = super::account_credentials(&self.database, account_id).await?;
        if !reference_active_for_account(&provider, reference, mode) {
            return Err(secret_target_not_allowed());
        }
        let origin = if filled_by_flow(&provider, reference) {
            SecretOrigin::SignIn
        } else {
            SecretOrigin::Person
        };
        keyderive::validate(steps, origin)?;
        match origin {
            SecretOrigin::Person => {
                let (stored, _) = self
                    .database
                    .account_secret_refs(account_id)
                    .await
                    .map_err(super::permanent)?
                    .ok_or_else(secret_missing)?;
                let stored = stored.ok_or_else(secret_missing)?;
                let secret = self.secrets.get(&stored).await.map_err(super::permanent)?;
                keyderive::run(secret.expose_secret().as_bytes(), steps)
            }
            SecretOrigin::SignIn => {
                // `key_ref`, never `access_ref`: the token beside it is a bearer credential
                // that goes into requests, and computing with it is not what it is for.
                let stored = self
                    .database
                    .auth_flow(account_id)
                    .await
                    .map_err(super::permanent)?
                    .and_then(|flow| flow.key_ref)
                    .ok_or_else(secret_missing)?;
                let text = self.secrets.get(&stored).await.map_err(super::permanent)?;
                let key = session::decode_key(text.expose_secret())?;
                keyderive::run(&key, steps)
            }
        }
    }

    /// Stores a sign-in's session beside the account's own credential, for a provider whose
    /// slots say a flow fills one (RD-120-30).
    ///
    /// Before this, `store-token` wrote over `accounts.secret_ref` whatever the provider --
    /// and for MEGA that meant the session took the password's place, so the next sign-in
    /// computed over a session identifier and failed. Now the person's credential stays
    /// where they put it, and the flow's value is split by [`session::parse`]: a token goes to
    /// `access_ref`, key material to `key_ref`. Both new references are recorded in one
    /// statement before either old one is dropped, the order that survives an interruption.
    pub(super) async fn store_session(
        &self,
        account_id: AccountId,
        value: &str,
    ) -> Result<(), Failure> {
        let (token, key) = match session::parse(value)? {
            FlowValue::Token(token) => (token, None),
            FlowValue::Keyed { token, key } => (token, Some(key)),
        };
        let previous = self
            .database
            .auth_flow(account_id)
            .await
            .map_err(super::permanent)?;
        let new_token = self
            .secrets
            .put_string(token)
            .await
            .map_err(super::permanent)?;
        let new_key = match key {
            Some(key) => match self.secrets.put_string(key.to_string()).await {
                Ok(reference) => Some(reference),
                Err(error) => {
                    let _ = self.secrets.remove(&new_token).await;
                    return Err(super::permanent(error));
                }
            },
            None => None,
        };
        if let Err(error) = self
            .database
            .set_auth_flow_session(account_id, new_token.clone(), new_key.clone())
            .await
        {
            // Nothing references the entries just written, so they are ours to take back.
            let _ = self.secrets.remove(&new_token).await;
            if let Some(orphan) = &new_key {
                let _ = self.secrets.remove(orphan).await;
            }
            return Err(super::permanent(error));
        }
        if let Some(previous) = previous {
            for old in [previous.access_ref, previous.key_ref]
                .into_iter()
                .flatten()
            {
                if old != new_token && Some(&old) != new_key.as_ref() {
                    let _ = self.secrets.remove(&old).await;
                }
            }
        }
        Ok(())
    }
}

/// Whether `reference` names the slot `provider`'s sign-in fills.
///
/// Read from the provider table the host built out of the installed manifests. A manifest can
/// *declare* a slot flow-filled; what that changes is only where the host reads the value
/// from -- the flow row -- and never moves anything a person typed there.
pub(super) fn filled_by_flow(provider: &str, reference: &str) -> bool {
    rd_provider_registry::by_slug(provider).is_some_and(|spec| {
        spec.secret_slot(reference)
            .is_some_and(rd_provider_registry::SecretSlot::is_filled_by_flow)
    })
}

#[cfg(test)]
#[path = "signin_tests.rs"]
mod signin_tests;
