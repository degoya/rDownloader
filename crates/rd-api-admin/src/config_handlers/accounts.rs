//! Provider accounts: list, create, change, test and delete.

use super::*;

#[utoipa::path(get, path = "/api/v1/accounts", tag = "configuration", responses((status = 200, body = [rd_core::Account])))]
pub async fn list_accounts(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::Account>>, ApiError> {
    Ok(Json(state.database.list_accounts().await?))
}

#[utoipa::path(put, path = "/api/v1/accounts/{id}", tag = "configuration", params(("id" = rd_core::AccountId, Path)), request_body = UpdateAccountRequest, responses((status = 200, body = rd_core::Account), (status = 404)))]
pub async fn update_account(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::AccountId>,
    Json(request): Json<UpdateAccountRequest>,
) -> Result<Json<rd_core::Account>, ApiError> {
    crate::hosters::forget(id);
    validate_name(&request.provider)?;
    validate_name(&request.label)?;
    // An account whose plugin is no longer installed has to stay editable. Since RD-101-13 a
    // provider exists only while its plugin does, so uninstalling one would otherwise freeze
    // every account that used it — it could not even be switched off, which is the one thing
    // somebody would want to do with it. The provider is therefore validated when it is still
    // known, and when the request changes it; an unchanged unknown one is left alone. Creating
    // an account for an unknown provider stays refused.
    let keeps_its_provider = state
        .database
        .list_accounts()
        .await?
        .into_iter()
        .any(|account| account.id == id && account.provider == request.provider);
    if rd_provider_registry::by_slug(&request.provider).is_some() || !keeps_its_provider {
        validate_provider(
            &request.provider,
            request.username.as_deref(),
            request.credential_mode,
        )?;
    }
    validate_secret_value(
        request.secret.as_deref(),
        16 * 1024,
        "account.secret_invalid",
        "Secret",
    )?;
    validate_secret_value(
        request.cookies.as_deref(),
        4 * 1024 * 1024,
        "account.cookies_invalid",
        "Cookies",
    )?;
    validate_proxy_selection(&state, request.proxy_profile_id).await?;
    let (old_secret_ref, old_cookie_ref) = state
        .database
        .account_secret_refs(id)
        .await?
        .ok_or_else(crate::error_codes::account_not_found)?;

    let replaces_secret = request.secret.is_some();
    let new_secret_ref = match request.secret {
        Some(value) => Some(state.secrets.put_string(value).await?),
        None if request.clear_secret => None,
        None => old_secret_ref.clone(),
    };
    let replaces_cookie = request.cookies.is_some();
    let new_cookie_ref = match request.cookies {
        Some(value) => match state.secrets.put_string(value).await {
            Ok(reference) => Some(reference),
            Err(error) => {
                if replaces_secret {
                    cleanup_secrets(&state.secrets, [new_secret_ref]).await;
                }
                return Err(error.into());
            }
        },
        None if request.clear_cookies => None,
        None => old_cookie_ref.clone(),
    };
    let stored_secret = replaces_secret.then(|| new_secret_ref.clone()).flatten();
    let stored_cookie = replaces_cookie.then(|| new_cookie_ref.clone()).flatten();
    let result = state
        .database
        .update_account(
            id,
            rd_db::UpdateAccount {
                provider: request.provider.trim().to_ascii_lowercase(),
                label: request.label.trim().to_owned(),
                username: optional_text(request.username),
                credential_mode: request.credential_mode,
                secret_ref: new_secret_ref.clone(),
                cookie_ref: new_cookie_ref.clone(),
                proxy_profile_id: request.proxy_profile_id,
                enabled: request.enabled,
            },
        )
        .await;
    let value = match result {
        Ok(value) => value,
        Err(error) => {
            cleanup_secrets(&state.secrets, [stored_secret, stored_cookie]).await;
            return Err(error.into());
        }
    };
    cleanup_replaced_secret(&state.secrets, old_secret_ref, &new_secret_ref).await;
    cleanup_replaced_secret(&state.secrets, old_cookie_ref, &new_cookie_ref).await;
    Ok(Json(value))
}

#[utoipa::path(post, path = "/api/v1/accounts/{id}/test", tag = "configuration", params(("id" = rd_core::AccountId, Path)), responses((status = 200, body = AccountTestResponse), (status = 404), (status = 502)))]
pub async fn test_account(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::AccountId>,
) -> Result<Json<AccountTestResponse>, ApiError> {
    let account = state
        .database
        .list_accounts()
        .await?
        .into_iter()
        .find(|account| account.id == id)
        .ok_or_else(crate::error_codes::account_not_found)?;
    if !account.enabled {
        return Err(ApiError::bad_request(
            "account.disabled",
            "Provider account is disabled",
        ));
    }
    // What the check found about the account's sign-in or its premium end is announced as
    // well (RD-190-19); the notices are keyed, so checking again does not repeat them.
    let today = chrono::Utc::now().date_naive();
    let status = match state.scheduler.check_account(id).await {
        Ok(status) => status,
        Err(failure) => {
            if let Some(notice) =
                rd_api_core::notify_notice::account_failure_notice(&account, &failure, today)
            {
                rd_api_core::notify_notice::announce(&state.database, notice).await;
            }
            return Err(account_check_failed(failure));
        }
    };
    for notice in rd_api_core::notify_notice::account_check_notices(&account, &status, today) {
        rd_api_core::notify_notice::announce(&state.database, notice).await;
    }
    Ok(Json(AccountTestResponse {
        valid: status.valid,
        premium: status.premium,
        label: status.label.into_iter().map(Into::into).collect(),
        traffic_left: status.traffic_left,
    }))
}

/// Failure codes an account check passes through as they are, rather than quoting them.
///
/// A check otherwise answers `account.check_failed` with the provider's English reason, which is
/// all a plugin's own wording can be. A failure the service coded itself and the interface
/// translates is worth more than that: `captcha.page_without_widget` says what to do in the
/// reader's language (RD-120-45), where the quoted reason could only say it in English. So does
/// `plugin.installed_not_running`: the account's plugin is installed and runs after a restart
/// (RD-170-12), which is what the person needs to hear right after installing it.
pub(super) const PASSED_THROUGH_CHECK_CODES: &[&str] = &[
    "captcha.page_without_widget",
    "plugin.installed_not_running",
];

pub(super) fn account_check_failed(failure: rd_core::Failure) -> ApiError {
    if let Some(code) = failure.code.as_deref().and_then(|code| {
        PASSED_THROUGH_CHECK_CODES
            .iter()
            .find(|known| **known == code)
    }) {
        return failure.params.iter().fold(
            ApiError::bad_gateway(code, failure.message.clone()),
            |error, (key, value)| error.with_param(key, value),
        );
    }
    ApiError::bad_gateway(
        "account.check_failed",
        format!("Provider account check failed: {}", failure.message),
    )
    .with_param("reason", failure.message)
}

#[utoipa::path(delete, path = "/api/v1/accounts/{id}", tag = "configuration", params(("id" = rd_core::AccountId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_account(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::AccountId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    let references = state.database.delete_account(id).await.map_err(|error| {
        store_error(
            &error,
            "account.not_found",
            "Provider account not found",
            StoreErrorKind::InUse,
            "account.in_use",
            "The provider account is still used by unfinished downloads",
        )
    })?;
    cleanup_secrets(&state.secrets, [references.0, references.1]).await;
    crate::hosters::forget(id);
    Ok(Json(crate::dto::MessageResponse::new(
        "account.deleted",
        "Provider account deleted",
    )))
}

#[utoipa::path(post, path = "/api/v1/accounts", tag = "configuration", request_body = CreateAccountRequest, responses((status = 201, body = rd_core::Account)))]
pub async fn create_account(
    State(state): State<AppState>,
    Json(request): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<rd_core::Account>), ApiError> {
    validate_name(&request.provider)?;
    validate_name(&request.label)?;
    validate_provider(
        &request.provider,
        request.username.as_deref(),
        request.credential_mode,
    )?;
    validate_secret_value(
        request.secret.as_deref(),
        16 * 1024,
        "account.secret_invalid",
        "Secret",
    )?;
    validate_secret_value(
        request.cookies.as_deref(),
        4 * 1024 * 1024,
        "account.cookies_invalid",
        "Cookies",
    )?;
    validate_proxy_selection(&state, request.proxy_profile_id).await?;
    let secret_ref = store_optional(&state.secrets, request.secret).await?;
    let cookie_ref = match store_optional(&state.secrets, request.cookies).await {
        Ok(reference) => reference,
        Err(error) => {
            cleanup_secrets(&state.secrets, [secret_ref]).await;
            return Err(error);
        }
    };
    let result = state
        .database
        .create_account(rd_db::NewAccount {
            provider: request.provider.trim().to_ascii_lowercase(),
            label: request.label.trim().to_owned(),
            username: optional_text(request.username),
            credential_mode: request.credential_mode,
            secret_ref: secret_ref.clone(),
            cookie_ref: cookie_ref.clone(),
            proxy_profile_id: request.proxy_profile_id,
            enabled: request.enabled,
        })
        .await;
    let value = match result {
        Ok(value) => value,
        Err(error) => {
            cleanup_secrets(&state.secrets, [secret_ref, cookie_ref]).await;
            return Err(error.into());
        }
    };
    Ok((StatusCode::CREATED, Json(value)))
}
