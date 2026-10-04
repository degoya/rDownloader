//! Field checks and secret handling shared by every handler that stores a named entry with a
//! credential: accounts, auth profiles, remote logins, object storage profiles, subscriptions.

use crate::ApiError;

/// The longest name of a stored entry with a credential.
pub const MAX_NAME_CHARS: usize = 100;

/// `400 request.name_length` unless the name is between 1 and [`MAX_NAME_CHARS`] characters.
pub fn validate_name(value: &str) -> Result<(), ApiError> {
    crate::input_checks::name_length(value, "request.name_length", MAX_NAME_CHARS)
}

pub fn validate_secret_value(
    value: Option<&str>,
    maximum: usize,
    code: &'static str,
    label: &str,
) -> Result<(), ApiError> {
    if let Some(value) = value
        && (value.is_empty() || value.len() > maximum)
    {
        return Err(ApiError::bad_request(
            code,
            format!("{label} is empty or exceeds the size limit of {maximum} bytes"),
        )
        .with_param("label", label)
        .with_param("max", maximum));
    }
    Ok(())
}

pub async fn store_optional(
    store: &rd_secrets::SecretStore,
    value: Option<String>,
) -> Result<Option<String>, ApiError> {
    match value.filter(|value| !value.is_empty()) {
        Some(value) => Ok(Some(store.put_string(value).await?)),
        None => Ok(None),
    }
}

pub async fn cleanup_secrets(
    store: &rd_secrets::SecretStore,
    references: impl IntoIterator<Item = Option<String>>,
) {
    for reference in references.into_iter().flatten() {
        if let Err(error) = store.remove(&reference).await {
            tracing::warn!(%error, "failed to clean up orphaned secret");
        }
    }
}
