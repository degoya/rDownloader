//! CRUD and the live test for object storage profiles (RD-150-04, RD-150-05).
//!
//! The secret — an S3 secret key, an Azure account key or shared access signature, a Google
//! service account key — and the S3 session token enter through write-only request fields,
//! are validated and go straight to the secret store. Nothing here returns a stored value or
//! its `vault://` reference; a profile shows only whether one is stored.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rd_core::{
    ObjectAddressing, ObjectCredentialSource, ObjectStorageProfile, ObjectStorageProfileId,
    ObjectStorageProvider,
};
use rd_db::StoreErrorKind;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState, config_fields::cleanup_secrets, dto::MessageResponse};

#[path = "object_storage_fields.rs"]
mod fields;

use fields::{Draft, Fields, secret_required, validate_secrets};
use rd_api_core::input_checks::optional_text;

/// A new object storage profile. The key fields are write-only.
#[derive(Deserialize, ToSchema)]
pub struct CreateObjectStorageProfileRequest {
    pub name: String,
    #[serde(default)]
    pub provider: ObjectStorageProvider,
    /// `http(s)://host[:port][/path]` of a compatible service or an emulator; empty for the
    /// provider's own service (AWS S3 in `region`, the Azure account's blob host, Google).
    pub endpoint: Option<String>,
    /// S3 only.
    pub region: Option<String>,
    /// Binds the profile to one bucket (an Azure container): links into it use this profile.
    pub bucket: Option<String>,
    /// S3 only. Defaults to virtual hosts without an endpoint and to paths with one.
    pub addressing: Option<ObjectAddressing>,
    pub credential_source: ObjectCredentialSource,
    /// S3 only.
    pub access_key_id: Option<String>,
    /// The Azure storage account; required for Azure, ignored otherwise.
    pub account: Option<String>,
    /// The secret of a `static` or `shared_access_signature` source: the S3 secret access
    /// key, the Azure account key or shared access signature, the Google service account key
    /// (the JSON key file's content).
    #[schema(write_only)]
    pub secret_access_key: Option<String>,
    /// S3 only.
    #[schema(write_only)]
    pub session_token: Option<String>,
    #[serde(default = "default_true")]
    pub checksums: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Editable profile fields. An empty secret or session token keeps the stored one while the
/// provider and the credential source stay what they were.
#[derive(Deserialize, ToSchema)]
pub struct UpdateObjectStorageProfileRequest {
    pub name: String,
    #[serde(default)]
    pub provider: ObjectStorageProvider,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub bucket: Option<String>,
    pub addressing: Option<ObjectAddressing>,
    pub credential_source: ObjectCredentialSource,
    pub access_key_id: Option<String>,
    pub account: Option<String>,
    #[schema(write_only)]
    pub secret_access_key: Option<String>,
    #[schema(write_only)]
    pub session_token: Option<String>,
    /// Drops a stored session token without replacing it.
    #[serde(default)]
    pub clear_session_token: bool,
    #[serde(default = "default_true")]
    pub checksums: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_true() -> bool {
    true
}

/// Redaction-safe result of a live profile check.
#[derive(Serialize, ToSchema)]
pub struct ObjectStorageTestResponse {
    pub reachable: bool,
    pub authenticated: bool,
    /// Stable failure code when the check did not succeed.
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: rd_core::MessageParams,
}

#[utoipa::path(get, path = "/api/v1/object-storage/profiles", tag = "configuration", responses((status = 200, body = [rd_core::ObjectStorageProfile])))]
pub async fn list_object_storage_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<ObjectStorageProfile>>, ApiError> {
    Ok(Json(state.database.list_object_storage_profiles().await?))
}

#[utoipa::path(post, path = "/api/v1/object-storage/profiles", tag = "configuration", request_body = CreateObjectStorageProfileRequest, responses((status = 201, body = rd_core::ObjectStorageProfile), (status = 400), (status = 409)))]
pub async fn create_object_storage_profile(
    State(state): State<AppState>,
    Json(request): Json<CreateObjectStorageProfileRequest>,
) -> Result<(StatusCode, Json<ObjectStorageProfile>), ApiError> {
    let fields = Fields::validate(Draft {
        name: &request.name,
        provider: request.provider,
        endpoint: request.endpoint,
        region: request.region,
        bucket: request.bucket,
        addressing: request.addressing,
        source: request.credential_source,
        access_key_id: request.access_key_id,
        account: request.account,
    })?;
    let secret = optional_text(request.secret_access_key);
    let token = optional_text(request.session_token);
    validate_secrets(&fields, secret.as_deref(), token.as_deref())?;
    let (secret, token) = if fields.source.stores_secret() {
        if secret.is_none() {
            return Err(secret_required());
        }
        (secret, token.filter(|_| fields.takes_session_token()))
    } else {
        // Nothing is stored for a source that does not sign with a stored key.
        (None, None)
    };
    let secret_ref = store(&state, secret).await?;
    let token_ref = store(&state, token).await?;
    let result = state
        .database
        .create_object_storage_profile(fields.into_input(
            secret_ref.clone(),
            token_ref.clone(),
            request.checksums,
            request.enabled,
        ))
        .await;
    match result {
        Ok(profile) => Ok((StatusCode::CREATED, Json(profile))),
        Err(error) => {
            cleanup_secrets(&state.secrets, [secret_ref, token_ref]).await;
            Err(map_store_error(&error))
        }
    }
}

#[utoipa::path(put, path = "/api/v1/object-storage/profiles/{id}", tag = "configuration", params(("id" = rd_core::ObjectStorageProfileId, Path)), request_body = UpdateObjectStorageProfileRequest, responses((status = 200, body = rd_core::ObjectStorageProfile), (status = 400), (status = 404), (status = 409)))]
pub async fn update_object_storage_profile(
    State(state): State<AppState>,
    Path(id): Path<ObjectStorageProfileId>,
    Json(request): Json<UpdateObjectStorageProfileRequest>,
) -> Result<Json<ObjectStorageProfile>, ApiError> {
    let stored = state
        .database
        .object_storage_profile(id)
        .await?
        .ok_or_else(not_found)?;
    let fields = Fields::validate(Draft {
        name: &request.name,
        provider: request.provider,
        endpoint: request.endpoint,
        region: request.region,
        bucket: request.bucket,
        addressing: request.addressing,
        source: request.credential_source,
        access_key_id: request.access_key_id,
        account: request.account,
    })?;
    let secret = optional_text(request.secret_access_key);
    let token = optional_text(request.session_token);
    validate_secrets(&fields, secret.as_deref(), token.as_deref())?;

    let signs = fields.source.stores_secret();
    // An omitted secret keeps the stored one only while the profile keeps signing with it:
    // an S3 key is no Google key, and an Azure account key is no shared access signature.
    let keeps =
        signs && stored.credential_source == fields.source && stored.provider == fields.provider;
    if signs && secret.is_none() && !(keeps && stored.secret_ref.is_some()) {
        return Err(secret_required());
    }
    let secret_ref = match secret.filter(|_| signs) {
        Some(value) => Some(state.secrets.put_string(value).await?),
        None if keeps => stored.secret_ref.clone(),
        None => None,
    };
    let takes_token = fields.takes_session_token();
    let token_ref = match token.filter(|_| takes_token) {
        Some(value) => Some(state.secrets.put_string(value).await?),
        None if keeps && takes_token && !request.clear_session_token => {
            stored.session_token_ref.clone()
        }
        None => None,
    };
    let result = state
        .database
        .update_object_storage_profile(
            id,
            fields.into_input(
                secret_ref.clone(),
                token_ref.clone(),
                request.checksums,
                request.enabled,
            ),
        )
        .await;
    match result {
        Ok((profile, orphaned)) => {
            cleanup_secrets(&state.secrets, orphaned.into_iter().map(Some)).await;
            Ok(Json(profile))
        }
        Err(error) => {
            // Only references minted in this request may go; the stored ones are still used.
            let minted = [
                secret_ref.filter(|value| Some(value) != stored.secret_ref.as_ref()),
                token_ref.filter(|value| Some(value) != stored.session_token_ref.as_ref()),
            ];
            cleanup_secrets(&state.secrets, minted).await;
            Err(map_store_error(&error))
        }
    }
}

#[utoipa::path(delete, path = "/api/v1/object-storage/profiles/{id}", tag = "configuration", params(("id" = rd_core::ObjectStorageProfileId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn delete_object_storage_profile(
    State(state): State<AppState>,
    Path(id): Path<ObjectStorageProfileId>,
) -> Result<Json<MessageResponse>, ApiError> {
    let profile = state
        .database
        .object_storage_profile(id)
        .await?
        .ok_or_else(not_found)?;
    // While the credentials still exist: an unfinished upload left behind is billed as stored
    // parts that nothing lists.
    if let Err(error) = state.object_storage.abort_profile_uploads(&profile).await {
        tracing::warn!(%error, "unfinished uploads of a deleted profile could not be aborted");
    }
    let orphaned = state
        .database
        .delete_object_storage_profile(id)
        .await
        .map_err(|_| not_found())?;
    cleanup_secrets(&state.secrets, orphaned.into_iter().map(Some)).await;
    Ok(Json(MessageResponse::new(
        "object_storage.profile_deleted",
        "Object storage profile deleted",
    )))
}

#[utoipa::path(post, path = "/api/v1/object-storage/profiles/{id}/test", tag = "configuration", params(("id" = rd_core::ObjectStorageProfileId, Path)), responses((status = 200, body = ObjectStorageTestResponse), (status = 404)))]
pub async fn test_object_storage_profile(
    State(state): State<AppState>,
    Path(id): Path<ObjectStorageProfileId>,
) -> Result<Json<ObjectStorageTestResponse>, ApiError> {
    let profile = state
        .database
        .object_storage_profile(id)
        .await?
        .ok_or_else(not_found)?;
    let failure = state.object_storage.test_profile(&profile).await?;
    Ok(Json(match failure {
        None => ObjectStorageTestResponse {
            reachable: true,
            authenticated: true,
            code: None,
            params: rd_core::MessageParams::new(),
        },
        Some(failure) => ObjectStorageTestResponse {
            reachable: !matches!(
                failure.code.as_deref(),
                Some(
                    rd_object_storage::error::CONNECT_FAILED
                        | rd_object_storage::ENDPOINT_INVALID
                        | rd_object_storage::TEST_NEEDS_BUCKET
                        | rd_object_storage::PROVIDER_UNSUPPORTED
                )
            ),
            authenticated: false,
            code: failure.code,
            params: failure.params,
        },
    }))
}

async fn store(state: &AppState, value: Option<String>) -> Result<Option<String>, ApiError> {
    match value {
        Some(value) => Ok(Some(state.secrets.put_string(value).await?)),
        None => Ok(None),
    }
}

fn not_found() -> ApiError {
    ApiError::not_found(
        "object_storage.profile_not_found",
        "Object storage profile not found",
    )
}

fn map_store_error(error: &anyhow::Error) -> ApiError {
    match rd_db::store_kind(error) {
        Some(StoreErrorKind::Duplicate) => ApiError::conflict(
            "object_storage.profile_duplicate",
            "An object storage profile with this name already exists",
        ),
        Some(StoreErrorKind::NotFound) => not_found(),
        _ => ApiError::from(anyhow::anyhow!(error.to_string())),
    }
}

#[cfg(test)]
#[path = "object_storage_handlers_tests.rs"]
mod tests;
