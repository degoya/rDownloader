//! The administrator password and the credentials a request presents.

use super::*;

/// Writes `password` as the administrator password into `database`, judged by the policy and
/// hashed as every other one; the body of [`AuthService::store_password`].
///
/// A free function because the password reset on the host (RD-190-24) writes the database of a
/// stopped service, where there is no `AppState` to go through.
pub async fn store_admin_password(
    database: &rd_db::Database,
    password: &str,
) -> Result<(), ApiError> {
    validate_password(password)?;
    let hash = hash_password(password).await?;
    database
        .set_setting(PASSWORD_SETTING.to_owned(), serde_json::Value::String(hash))
        .await?;
    Ok(())
}

/// Whether an administrator password is stored in `database` at all.
pub async fn admin_password_configured(database: &rd_db::Database) -> Result<bool, ApiError> {
    Ok(database.get_setting(PASSWORD_SETTING).await?.is_some())
}

/// Whether `password` is the administrator password stored in `database`; the body of
/// [`AuthService::password_matches`].
pub async fn admin_password_matches(
    database: &rd_db::Database,
    password: &str,
) -> Result<bool, ApiError> {
    let Some(value) = database.get_setting(PASSWORD_SETTING).await? else {
        return Ok(false);
    };
    let Some(encoded) = value.as_str().map(str::to_owned) else {
        return Ok(false);
    };
    let password = password.to_owned();
    on_argon2_pool(move || {
        PasswordHash::new(&encoded).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await
}

/// Argon2id over a fresh 16-byte salt, in the PHC string form the setting stores.
pub(super) async fn hash_password(password: &str) -> Result<String, ApiError> {
    let mut salt_bytes = [0_u8; 16];
    rand::rng().fill_bytes(&mut salt_bytes);
    let password = password.to_owned();
    on_argon2_pool(move || {
        Argon2::default()
            .hash_password_with_salt(password.as_bytes(), &salt_bytes)
            .map(|hash| hash.to_string())
    })
    .await?
    .map_err(|error| {
        tracing::error!(%error, "failed to hash password");
        ApiError::bad_request("auth.password_hash_failed", "Password could not be hashed")
    })
}

/// How many Argon2 computations run at once, service-wide.
///
/// Each one is tens of milliseconds of one core and 19 MiB. On the async workers, as many
/// parallel sign-in attempts as cores stalled every other request and event stream, and the
/// sign-in route needs no credential to be asked (audit 2026-10-05, S10). Two keep a sign-in
/// prompt while an attack queues behind them, and leave the other cores to everything else.
pub(super) const ARGON2_CONCURRENCY: usize = 2;

static ARGON2_PERMITS: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(ARGON2_CONCURRENCY);

/// Runs one Argon2 computation on the blocking pool, at most [`ARGON2_CONCURRENCY`] at once.
///
/// The permit moves into the blocking task, so a caller that goes away while it waits for the
/// result does not free a slot the computation still holds.
pub(super) async fn on_argon2_pool<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    let permit = ARGON2_PERMITS
        .acquire()
        .await
        .map_err(|error| anyhow::anyhow!("password hashing is shut down: {error}"))?;
    let result = tokio::task::spawn_blocking(move || {
        let outcome = work();
        drop(permit);
        outcome
    })
    .await
    .map_err(|error| anyhow::anyhow!("password hashing task failed: {error}"))?;
    Ok(result)
}

/// The refusal of a second setup, whichever check caught it.
pub(super) fn setup_completed() -> ApiError {
    ApiError::conflict("auth.setup_completed", "Setup has already been completed")
}

/// The longest password accepted, in characters.
///
/// Far beyond any passphrase a person types or a manager generates, and short enough that the
/// hash a public route computes over it stays cheap: without a ceiling, the only bound was the
/// request body limit (security audit 2026-09-30, finding 8).
pub const MAX_PASSWORD_CHARS: usize = 1024;

/// The password policy. One function, so the first password and every later one are judged
/// by the same rule (RD-120-22).
pub fn validate_password(password: &str) -> Result<(), ApiError> {
    let length = password.chars().count();
    if length < 10 {
        return Err(ApiError::bad_request(
            "auth.password_too_short",
            "The password must be at least 10 characters long",
        )
        .with_param("min", 10));
    }
    if length > MAX_PASSWORD_CHARS {
        return Err(ApiError::bad_request(
            "auth.password_too_long",
            "The password must be at most 1024 characters long",
        )
        .with_param("max", MAX_PASSWORD_CHARS));
    }
    Ok(())
}

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

pub(super) fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{SESSION_COOKIE}=")))
}
