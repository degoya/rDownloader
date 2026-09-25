//! Second-factor credentials and recovery codes.
//!
//! The secret behind a credential is not here: `material_ref` points into the encrypted secret
//! store, the same way an account's password does. This table holds only what the inventory
//! shows — kind, label, and when it was created, confirmed and last used.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{MfaCredential, MfaCredentialId, MfaKind};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::parse_id;

pub(crate) async fn create_credential(
    connection: &mut SqliteConnection,
    id: MfaCredentialId,
    kind: MfaKind,
    label: String,
    material_ref: String,
) -> Result<MfaCredential> {
    let credential = MfaCredential {
        id,
        kind,
        label,
        created_at: Utc::now(),
        confirmed_at: None,
        last_used_at: None,
    };
    sqlx::query(
        "INSERT INTO mfa_credentials (id, kind, label, material_ref, created_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(credential.id.to_string())
    .bind(kind.as_str())
    .bind(&credential.label)
    .bind(material_ref)
    .bind(credential.created_at)
    .execute(connection)
    .await
    .context("insert MFA credential")?;
    Ok(credential)
}

/// Every credential, newest first.
pub(crate) async fn list_credentials(pool: &SqlitePool) -> Result<Vec<MfaCredential>> {
    let rows = sqlx::query_as::<_, CredentialRow>(
        "SELECT id, kind, label, created_at, confirmed_at, last_used_at \
         FROM mfa_credentials ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await
    .context("list MFA credentials")?;
    rows.into_iter().map(MfaCredential::try_from).collect()
}

/// The vault references of every credential that has been confirmed.
///
/// Only confirmed ones: an enrolment somebody abandoned halfway must not be able to answer a
/// challenge, and must not make the account look protected when it is not.
pub(crate) async fn confirmed_material(
    pool: &SqlitePool,
    kind: MfaKind,
) -> Result<Vec<(MfaCredentialId, String)>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT id, material_ref FROM mfa_credentials \
         WHERE kind = ? AND confirmed_at IS NOT NULL",
    )
    .bind(kind.as_str())
    .fetch_all(pool)
    .await
    .context("read MFA material")?;
    rows.into_iter()
        .map(|(id, reference)| Ok((parse_id(&id)?, reference)))
        .collect()
}

/// The vault reference of one credential, confirmed or not.
pub(crate) async fn material_of(pool: &SqlitePool, id: MfaCredentialId) -> Result<Option<String>> {
    sqlx::query_scalar::<_, String>("SELECT material_ref FROM mfa_credentials WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
        .context("read MFA material")
}

/// Marks a credential as proven to work.
pub(crate) async fn confirm_credential(
    connection: &mut SqliteConnection,
    id: MfaCredentialId,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE mfa_credentials SET confirmed_at = ?, last_used_at = ? \
         WHERE id = ? AND confirmed_at IS NULL",
    )
    .bind(Utc::now())
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(connection)
    .await
    .context("confirm MFA credential")?;
    Ok(result.rows_affected() > 0)
}

/// Repoints a credential at fresh material, and notes that it answered a challenge.
///
/// Both in one statement because they happen together: a passkey's stored state is rewritten
/// precisely when it has just been used. Splitting them would leave a window in which the
/// credential is updated but not counted as used, or the reverse.
pub(crate) async fn repoint_material(
    connection: &mut SqliteConnection,
    id: MfaCredentialId,
    material_ref: String,
) -> Result<bool> {
    let result =
        sqlx::query("UPDATE mfa_credentials SET material_ref = ?, last_used_at = ? WHERE id = ?")
            .bind(material_ref)
            .bind(Utc::now())
            .bind(id.to_string())
            .execute(connection)
            .await
            .context("repoint MFA material")?;
    Ok(result.rows_affected() > 0)
}

/// Records the time step an accepted TOTP code belonged to. Refuses a replay of it.
///
/// Check and write in one statement, on the serialized writer: `last_totp_step < ?` is what
/// makes a code single-use. Reading the step and writing it back separately would leave the
/// window two logins need to both be accepted with the same six digits — which is exactly the
/// replay this column exists to stop.
///
/// `false` means the step was already spent, and the caller must treat the code as wrong.
pub(crate) async fn accept_totp_step(
    connection: &mut SqliteConnection,
    id: MfaCredentialId,
    step: i64,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE mfa_credentials SET last_totp_step = ?, last_used_at = ? \
         WHERE id = ? AND (last_totp_step IS NULL OR last_totp_step < ?)",
    )
    .bind(step)
    .bind(Utc::now())
    .bind(id.to_string())
    .bind(step)
    .execute(connection)
    .await
    .context("record accepted TOTP step")?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn touch_credential(
    connection: &mut SqliteConnection,
    id: MfaCredentialId,
) -> Result<()> {
    sqlx::query("UPDATE mfa_credentials SET last_used_at = ? WHERE id = ?")
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(connection)
        .await
        .context("touch MFA credential")?;
    Ok(())
}

/// Removes a credential, returning its vault reference so the caller can delete the secret.
///
/// Returning it rather than deleting it here keeps this module free of the secret store: the
/// row and the material are owned by different components, and leaving an orphaned secret
/// behind is a smaller problem than this module reaching across that line.
pub(crate) async fn delete_credential(
    connection: &mut SqliteConnection,
    id: MfaCredentialId,
) -> Result<Option<String>> {
    let mut transaction = connection.begin().await?;
    let reference =
        sqlx::query_scalar::<_, String>("SELECT material_ref FROM mfa_credentials WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *transaction)
            .await
            .context("read MFA material")?;
    if reference.is_some() {
        sqlx::query("DELETE FROM mfa_credentials WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await
            .context("delete MFA credential")?;
    }
    transaction.commit().await?;
    Ok(reference)
}

/// Replaces the recovery codes with a fresh set.
///
/// Replacing rather than appending: a new set is issued when the old one is regenerated or
/// when the factor is set up again, and in both cases the old codes must stop working.
pub(crate) async fn replace_recovery_codes(
    connection: &mut SqliteConnection,
    digests: Vec<String>,
) -> Result<()> {
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM mfa_recovery_codes")
        .execute(&mut *transaction)
        .await
        .context("clear recovery codes")?;
    for digest in digests {
        sqlx::query("INSERT INTO mfa_recovery_codes (digest, created_at) VALUES (?, ?)")
            .bind(digest)
            .bind(Utc::now())
            .execute(&mut *transaction)
            .await
            .context("insert recovery code")?;
    }
    transaction.commit().await?;
    Ok(())
}

/// The digests of every unspent code.
pub(crate) async fn unused_recovery_digests(pool: &SqlitePool) -> Result<Vec<String>> {
    sqlx::query_scalar::<_, String>(
        "SELECT digest FROM mfa_recovery_codes WHERE used_at IS NULL ORDER BY created_at",
    )
    .fetch_all(pool)
    .await
    .context("read recovery codes")
}

/// Spends one code. Returns whether it was still unspent.
///
/// The `used_at IS NULL` in the statement is what makes a code single-use even if two requests
/// arrive at once: the second finds nothing to update.
pub(crate) async fn spend_recovery_code(
    connection: &mut SqliteConnection,
    digest: &str,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE mfa_recovery_codes SET used_at = ? WHERE digest = ? AND used_at IS NULL",
    )
    .bind(Utc::now())
    .bind(digest)
    .execute(connection)
    .await
    .context("spend recovery code")?;
    Ok(result.rows_affected() > 0)
}

/// Removes every credential of one kind, and every recovery code.
///
/// Scoped to a kind because "turn two-factor sign-in off" is about the authenticator app.
/// A passkey is an alternative way to sign in rather than a gate in front of the password,
/// so switching the app off must not silently take the passkeys with it.
pub(crate) async fn clear_all(
    connection: &mut SqliteConnection,
    kind: MfaKind,
) -> Result<Vec<String>> {
    let mut transaction = connection.begin().await?;
    let references =
        sqlx::query_scalar::<_, String>("SELECT material_ref FROM mfa_credentials WHERE kind = ?")
            .bind(kind.as_str())
            .fetch_all(&mut *transaction)
            .await
            .context("read MFA material")?;
    sqlx::query("DELETE FROM mfa_credentials WHERE kind = ?")
        .bind(kind.as_str())
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM mfa_recovery_codes")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(references)
}

#[derive(FromRow)]
struct CredentialRow {
    id: String,
    kind: String,
    label: String,
    created_at: DateTime<Utc>,
    confirmed_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
}

impl TryFrom<CredentialRow> for MfaCredential {
    type Error = anyhow::Error;

    fn try_from(row: CredentialRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            kind: MfaKind::parse(&row.kind)
                .ok_or_else(|| anyhow::anyhow!("unknown MFA kind `{}`", row.kind))?,
            label: row.label,
            created_at: row.created_at,
            confirmed_at: row.confirmed_at,
            last_used_at: row.last_used_at,
        })
    }
}
