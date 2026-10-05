use std::path::{Component, Path, PathBuf};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::StatusCode,
};
use rd_api_core::input_checks::{TextLimit, required_text};
use rd_db::StoreErrorKind;
use regex::Regex;

use crate::{
    ApiError, AppState,
    config_fields::{cleanup_secrets, store_optional, validate_name, validate_secret_value},
    dto::{
        AccountTestResponse, CreateAccountRequest, CreateCategoryRequest,
        CreateCategoryRuleRequest, CreateHotFolderRequest, CreateProxyProfileRequest,
        CreateStorageRootRequest, UpdateAccountRequest,
    },
    error_codes::{store_duplicate, store_error},
    protected_roots::{protected_directories, refuse_protected, refuse_protected_hotfolder},
};

mod accounts;
mod categories;
mod hotfolders;
mod proxy_profiles;
mod storage_roots;

pub use accounts::*;
pub use categories::*;
pub use hotfolders::*;
pub use proxy_profiles::*;
pub use storage_roots::*;

/// Rejects an account provider not in the registry, one that omits a required username, and
/// one whose credential mode does not match what the provider offers.
fn validate_provider(
    provider: &str,
    username: Option<&str>,
    mode: Option<rd_provider_registry::CredentialMode>,
) -> Result<(), ApiError> {
    let spec = rd_provider_registry::by_slug(provider).ok_or_else(|| {
        ApiError::bad_request(
            "account.provider_unsupported",
            "This provider is not supported",
        )
    })?;
    let offered = spec.credential_modes();
    match mode {
        Some(mode) if !offered.contains(&mode) => {
            return Err(ApiError::bad_request(
                "account.credential_mode_unsupported",
                "This provider does not offer that credential mode",
            ));
        }
        // Leaving it out would silently fall back to the provider's first mode, which decides
        // whether the stored secret is treated as a password or as an API key. Too consequential
        // to guess: the form asks, so the request carries it.
        None if !offered.is_empty() => {
            return Err(ApiError::bad_request(
                "account.credential_mode_required",
                "This provider requires a credential mode",
            ));
        }
        _ => {}
    }
    let has_username = username
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let username_required =
        spec.username_required || mode == Some(rd_provider_registry::CredentialMode::Login);
    if username_required && !has_username {
        return Err(ApiError::bad_request(
            "account.username_required",
            "This provider requires a username",
        ));
    }
    Ok(())
}

fn validate_relative(value: &str, code: &'static str, label: &str) -> Result<(), ApiError> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(ApiError::bad_request(
            code,
            format!("{label} must be relative and must not contain path traversal"),
        )
        .with_param("label", label));
    }
    Ok(())
}

fn valid_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

async fn cleanup_replaced_secret(
    store: &rd_secrets::SecretStore,
    old: Option<String>,
    new: &Option<String>,
) {
    if old
        .as_ref()
        .is_some_and(|reference| Some(reference) != new.as_ref())
    {
        cleanup_secrets(store, [old]).await;
    }
}

async fn validate_proxy_selection(
    state: &AppState,
    proxy_id: Option<rd_core::ProxyProfileId>,
) -> Result<(), ApiError> {
    if let Some(proxy_id) = proxy_id
        && !state
            .database
            .list_proxy_profiles()
            .await?
            .iter()
            .any(|profile| profile.id == proxy_id)
    {
        return Err(ApiError::bad_request(
            "proxy.not_found",
            "Proxy profile not found",
        ));
    }
    Ok(())
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "config_handlers_check_tests.rs"]
mod check_tests;
