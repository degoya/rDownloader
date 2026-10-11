//! The service's half of Web Push (RD-1240-13): the VAPID key, made the first time it is needed,
//! and what a round of pushes to every subscribed browser comes to.
//!
//! The private key lives in the vault, its reference and the public half in `web_push_keys`. A
//! reference that no longer opens — a full backup restored on another machine brings the row but
//! not the vault entry — is replaced by a new key, and the subscriptions made for the old one go
//! with it: a push service refuses a message signed with a key the browser did not subscribe
//! with, so those browsers turn push on again.

use anyhow::Result;
use rd_notify::{Attempt, PushOutcome, VapidKey, WebPushSubscription};

/// One key is made at a time, so two first requests do not race each other into the vault.
static MAKING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The VAPID key in force, made now if there is none or the stored one cannot be read.
pub async fn vapid_key(
    database: &rd_db::Database,
    secrets: &rd_secrets::SecretStore,
) -> Result<VapidKey> {
    let _making = MAKING.lock().await;
    let replacing = match database.web_push_key().await? {
        None => None,
        Some(stored) => match read_key(secrets, &stored.private_key_ref).await {
            Ok(key) => return Ok(key),
            Err(error) => {
                tracing::warn!(%error, "the push signing key is unreadable; a new one is made and browsers subscribe again");
                Some(stored.private_key_ref)
            }
        },
    };
    let (key, pkcs8) = VapidKey::generate()?;
    let reference = secrets.put_bytes(&pkcs8).await?;
    let stored = database
        .store_web_push_key(
            rd_db::WebPushKey {
                private_key_ref: reference.clone(),
                public_key: key.public_key(),
            },
            replacing.clone(),
        )
        .await;
    let in_force = match stored {
        Ok(in_force) => in_force,
        Err(error) => {
            forget(secrets, &reference).await;
            return Err(error);
        }
    };
    if in_force.private_key_ref != reference {
        // Another key was stored first; it is the one browsers subscribe with.
        forget(secrets, &reference).await;
        return read_key(secrets, &in_force.private_key_ref).await;
    }
    if let Some(stale) = replacing {
        forget(secrets, &stale).await;
    }
    Ok(key)
}

async fn read_key(secrets: &rd_secrets::SecretStore, reference: &str) -> Result<VapidKey> {
    VapidKey::from_pkcs8(&secrets.get_bytes(reference).await?)
}

/// Removes a vault entry nothing names; a failure leaves it to the sweep at the next start.
async fn forget(secrets: &rd_secrets::SecretStore, reference: &str) {
    if let Err(error) = secrets.remove(reference).await {
        tracing::warn!(%error, "an unused push key could not be removed from the vault");
    }
}

/// What a round of pushes comes to: the delivery's attempt, and the subscriptions the push
/// services no longer know, which the caller deletes.
///
/// The delivery succeeds when every browser took the message or is gone. A failure names the
/// devices it failed for and is retried when one of them is worth retrying; the retry sends to
/// every browser again, and the notification's tag makes a second copy replace the first.
#[must_use]
pub fn settle_pushes(outcomes: Vec<(&WebPushSubscription, PushOutcome)>) -> (Attempt, Vec<String>) {
    let mut gone = Vec::new();
    let mut failures = Vec::new();
    let mut retryable = false;
    let mut delivered = 0_usize;
    for (subscription, outcome) in outcomes {
        match outcome {
            PushOutcome::Delivered => delivered += 1,
            PushOutcome::Gone => gone.push(subscription.id.clone()),
            PushOutcome::Failed {
                status,
                detail,
                retryable: again,
            } => {
                retryable |= again;
                let status = status
                    .map(|status| format!(" ({status})"))
                    .unwrap_or_default();
                failures.push(format!("{}{status}: {detail}", subscription.device_name));
            }
        }
    }
    let attempt = if !failures.is_empty() {
        Attempt::could_not_deliver(failures.join("; "), retryable)
    } else if delivered == 0 && !gone.is_empty() {
        Attempt::could_not_deliver(
            "every browser's push subscription has expired; turn push on again in the browser",
            false,
        )
    } else {
        Attempt::succeeded()
    };
    (attempt, gone)
}

#[cfg(test)]
#[path = "web_push_tests.rs"]
mod tests;
