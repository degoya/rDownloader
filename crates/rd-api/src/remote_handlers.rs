//! CRUD and the live test action for stored FTP/SFTP logins, plus the SSH host-key trust
//! store.
//!
//! Credential values enter through write-only request fields, are validated, and are then
//! handed straight to the secret store. Nothing here ever returns a stored value or its
//! `vault://` reference.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rd_core::{
    MAX_REMOTE_HOST, MAX_REMOTE_KEY, MAX_REMOTE_SECRET, RemoteAuthMode, RemoteCredential,
    RemoteCredentialId, RemoteProtocol, SshHostKey,
};
use rd_db::StoreErrorKind;

use crate::{
    ApiError, AppState,
    config_handlers::{cleanup_secrets, store_optional, validate_name, validate_secret_value},
    dto::{
        CreateRemoteCredentialRequest, MessageResponse, RemoteCredentialTestResponse,
        TrustSshHostKeyRequest, UpdateRemoteCredentialRequest,
    },
};

#[utoipa::path(get, path = "/api/v1/remote-credentials", tag = "configuration", responses((status = 200, body = [rd_core::RemoteCredential])))]
pub async fn list_remote_credentials(
    State(state): State<AppState>,
) -> Result<Json<Vec<RemoteCredential>>, ApiError> {
    Ok(Json(state.database.list_remote_credentials().await?))
}

#[utoipa::path(post, path = "/api/v1/remote-credentials", tag = "configuration", request_body = CreateRemoteCredentialRequest, responses((status = 201, body = rd_core::RemoteCredential), (status = 400), (status = 409)))]
pub async fn create_remote_credential(
    State(state): State<AppState>,
    Json(request): Json<CreateRemoteCredentialRequest>,
) -> Result<(StatusCode, Json<RemoteCredential>), ApiError> {
    validate_name(&request.name)?;
    let host = parse_host(&request.host)?;
    let port = request
        .port
        .unwrap_or_else(|| request.protocol.default_port());
    validate_port(port)?;
    let username = normalized(request.username);
    let secret = normalized(request.secret);
    let private_key = normalized(request.private_key);
    let passphrase = normalized(request.passphrase);
    validate_auth(
        request.protocol,
        request.auth_mode,
        username.as_deref(),
        secret.as_deref(),
        private_key.as_deref(),
        true,
    )?;
    validate_sizes(
        secret.as_deref(),
        private_key.as_deref(),
        passphrase.as_deref(),
    )?;

    let secret_ref = store_optional(&state.secrets, secret).await?;
    let key_ref = store_optional(&state.secrets, private_key).await?;
    let passphrase_ref = store_optional(&state.secrets, passphrase).await?;
    let result = state
        .database
        .create_remote_credential(rd_db::NewRemoteCredential {
            name: request.name.trim().to_owned(),
            protocol: request.protocol,
            host,
            port,
            username,
            auth_mode: request.auth_mode,
            passive: request.passive,
            enabled: request.enabled,
            secret_ref: secret_ref.clone(),
            key_ref: key_ref.clone(),
            passphrase_ref: passphrase_ref.clone(),
        })
        .await;
    match result {
        Ok(credential) => Ok((StatusCode::CREATED, Json(credential))),
        Err(error) => {
            cleanup_secrets(&state.secrets, [secret_ref, key_ref, passphrase_ref]).await;
            Err(map_store_error(&error))
        }
    }
}

#[utoipa::path(put, path = "/api/v1/remote-credentials/{id}", tag = "configuration", params(("id" = rd_core::RemoteCredentialId, Path)), request_body = UpdateRemoteCredentialRequest, responses((status = 200, body = rd_core::RemoteCredential), (status = 400), (status = 404), (status = 409)))]
pub async fn update_remote_credential(
    State(state): State<AppState>,
    Path(id): Path<RemoteCredentialId>,
    Json(request): Json<UpdateRemoteCredentialRequest>,
) -> Result<Json<RemoteCredential>, ApiError> {
    validate_name(&request.name)?;
    let host = parse_host(&request.host)?;
    let port = request
        .port
        .unwrap_or_else(|| request.protocol.default_port());
    validate_port(port)?;
    let stored = state
        .database
        .remote_credential(id)
        .await?
        .ok_or_else(not_found)?;
    let username = normalized(request.username);
    let secret = normalized(request.secret);
    let private_key = normalized(request.private_key);
    let passphrase = normalized(request.passphrase);
    validate_sizes(
        secret.as_deref(),
        private_key.as_deref(),
        passphrase.as_deref(),
    )?;

    // An omitted credential keeps the stored one, but only while the mode still uses the
    // same kind of credential; switching mode invalidates it rather than, say, offering a
    // password where a key is expected.
    let same_mode = stored.auth_mode == request.auth_mode;
    let keeps_secret = secret.is_none() && same_mode;
    let keeps_key = private_key.is_none() && same_mode && !request.clear_private_key;
    validate_auth(
        request.protocol,
        request.auth_mode,
        username.as_deref(),
        secret.as_deref().or(keeps_secret.then_some("kept")),
        private_key.as_deref().or(keeps_key.then_some("kept")),
        false,
    )?;

    let secret_ref = match secret {
        Some(value) => Some(state.secrets.put_string(value).await?),
        None if keeps_secret => stored.secret_ref.clone(),
        None => None,
    };
    let key_ref = match private_key {
        Some(value) => Some(state.secrets.put_string(value).await?),
        None if keeps_key => stored.key_ref.clone(),
        None => None,
    };
    let passphrase_ref = match passphrase {
        Some(value) => Some(state.secrets.put_string(value).await?),
        // The passphrase belongs to the key; dropping the key drops it too.
        None if keeps_key => stored.passphrase_ref.clone(),
        None => None,
    };

    let result = state
        .database
        .update_remote_credential(
            id,
            rd_db::UpdateRemoteCredential {
                name: request.name.trim().to_owned(),
                protocol: request.protocol,
                host,
                port,
                username,
                auth_mode: request.auth_mode,
                passive: request.passive,
                enabled: request.enabled,
                secret_ref: secret_ref.clone(),
                key_ref: key_ref.clone(),
                passphrase_ref: passphrase_ref.clone(),
            },
        )
        .await;
    match result {
        Ok((credential, orphaned)) => {
            cleanup_secrets(&state.secrets, orphaned.into_iter().map(Some)).await;
            Ok(Json(credential))
        }
        Err(error) => {
            // Only references minted in this request may be dropped; the stored ones are
            // still in use by the unchanged row.
            let minted = [
                secret_ref.filter(|value| Some(value) != stored.secret_ref.as_ref()),
                key_ref.filter(|value| Some(value) != stored.key_ref.as_ref()),
                passphrase_ref.filter(|value| Some(value) != stored.passphrase_ref.as_ref()),
            ];
            cleanup_secrets(&state.secrets, minted).await;
            Err(map_store_error(&error))
        }
    }
}

#[utoipa::path(delete, path = "/api/v1/remote-credentials/{id}", tag = "configuration", params(("id" = rd_core::RemoteCredentialId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn delete_remote_credential(
    State(state): State<AppState>,
    Path(id): Path<RemoteCredentialId>,
) -> Result<Json<MessageResponse>, ApiError> {
    let orphaned = state
        .database
        .delete_remote_credential(id)
        .await
        .map_err(|_| not_found())?;
    cleanup_secrets(&state.secrets, orphaned.into_iter().map(Some)).await;
    Ok(Json(MessageResponse::new(
        "remote.credential_deleted",
        "Remote login deleted",
    )))
}

#[utoipa::path(post, path = "/api/v1/remote-credentials/{id}/test", tag = "configuration", params(("id" = rd_core::RemoteCredentialId, Path)), responses((status = 200, body = RemoteCredentialTestResponse), (status = 404)))]
pub async fn test_remote_credential(
    State(state): State<AppState>,
    Path(id): Path<RemoteCredentialId>,
) -> Result<Json<RemoteCredentialTestResponse>, ApiError> {
    let credential = state
        .database
        .remote_credential(id)
        .await?
        .ok_or_else(not_found)?;
    // A failure here is an answer, not an error: "unknown host key" in particular is the
    // normal first result and carries the fingerprint the user then confirms.
    let failure = match credential.protocol.family() {
        rd_core::RemoteFamily::Ftp => state.ftp.test_credential(&credential).await?,
        rd_core::RemoteFamily::Sftp => state.sftp.test_credential(&credential).await?,
        rd_core::RemoteFamily::Webdav => {
            return Err(ApiError::bad_request(
                "remote.webdav_uses_auth_profiles",
                "WebDAV shares authenticate through auth profiles, not remote logins",
            ));
        }
    };
    Ok(Json(match failure {
        None => RemoteCredentialTestResponse {
            reachable: true,
            authenticated: true,
            code: None,
            params: rd_core::MessageParams::new(),
        },
        Some(failure) => RemoteCredentialTestResponse {
            // Anything other than a connect failure means the server answered.
            reachable: !failure
                .code
                .as_deref()
                .is_some_and(|code| code.ends_with(".connect_failed")),
            authenticated: false,
            code: failure.code,
            params: failure.params,
        },
    }))
}

#[utoipa::path(get, path = "/api/v1/remote-credentials/ssh-hosts", tag = "configuration", responses((status = 200, body = [rd_core::SshHostKey])))]
pub async fn list_ssh_host_keys(
    State(state): State<AppState>,
) -> Result<Json<Vec<SshHostKey>>, ApiError> {
    Ok(Json(state.database.list_ssh_host_keys().await?))
}

#[utoipa::path(post, path = "/api/v1/remote-credentials/ssh-hosts", tag = "configuration", request_body = TrustSshHostKeyRequest, responses((status = 200, body = MessageResponse), (status = 400)))]
pub async fn trust_ssh_host_key(
    State(state): State<AppState>,
    Json(request): Json<TrustSshHostKeyRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let host = parse_host(&request.host)?;
    validate_port(request.port)?;
    let fingerprint = request.fingerprint.trim();
    // Only the OpenSSH SHA-256 form is accepted, because that is what the blocking failure
    // reports and what a user can compare against `ssh-keyscan`. Accepting anything else
    // would let a typo be stored as a trusted key that never matches.
    if !fingerprint.starts_with("SHA256:") || fingerprint.len() > 128 {
        return Err(ApiError::bad_request(
            "remote.fingerprint_invalid",
            "The fingerprint must be the SHA256: form reported by the failed connection",
        ));
    }
    let algorithm = request.algorithm.trim();
    if algorithm.is_empty() || algorithm.len() > 64 {
        return Err(ApiError::bad_request(
            "remote.algorithm_invalid",
            "The host key algorithm is not valid",
        ));
    }
    state
        .database
        .trust_ssh_host_key(SshHostKey {
            host,
            port: request.port,
            algorithm: algorithm.to_owned(),
            fingerprint: fingerprint.to_owned(),
            first_seen: chrono::Utc::now(),
        })
        .await?;
    Ok(Json(MessageResponse::new(
        "remote.host_key_trusted",
        "SSH host key confirmed",
    )))
}

#[utoipa::path(delete, path = "/api/v1/remote-credentials/ssh-hosts/{host}/{port}/{algorithm}", tag = "configuration", params(("host" = String, Path), ("port" = u16, Path), ("algorithm" = String, Path)), responses((status = 200, body = MessageResponse)))]
pub async fn forget_ssh_host_key(
    State(state): State<AppState>,
    Path((host, port, algorithm)): Path<(String, u16, String)>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .database
        .forget_ssh_host_key(parse_host(&host)?, port, algorithm)
        .await?;
    Ok(Json(MessageResponse::new(
        "remote.host_key_forgotten",
        "SSH host key removed",
    )))
}

/// Normalises a host entry, accepting either a bare host or a full URL.
fn parse_host(input: &str) -> Result<String, ApiError> {
    let trimmed = input.trim().trim_end_matches('.');
    if trimmed.is_empty() || trimmed.len() > MAX_REMOTE_HOST {
        return Err(invalid_host());
    }
    // Going through `Url` supplies IDNA/punycode encoding and lowercasing, so the stored
    // host agrees with the one seen when a link is matched against it.
    let candidate = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("ftp://{trimmed}")
    };
    let url = url::Url::parse(&candidate).map_err(|_| invalid_host())?;
    let host = url.host_str().ok_or_else(invalid_host)?;
    Ok(host.trim_end_matches('.').to_ascii_lowercase())
}

fn validate_port(port: u16) -> Result<(), ApiError> {
    if port == 0 {
        return Err(ApiError::bad_request(
            "remote.port_invalid",
            "The port must be between 1 and 65535",
        ));
    }
    Ok(())
}

/// Checks that the chosen mode has the credential it needs and is valid for the protocol.
fn validate_auth(
    protocol: RemoteProtocol,
    mode: RemoteAuthMode,
    username: Option<&str>,
    secret: Option<&str>,
    private_key: Option<&str>,
    require_credential: bool,
) -> Result<(), ApiError> {
    if !mode.is_valid_for(protocol) {
        return Err(ApiError::bad_request(
            "remote.auth_mode_unsupported",
            "This authentication method is not available for the selected protocol",
        )
        .with_param("protocol", protocol.as_str())
        .with_param("mode", mode.as_str()));
    }
    match mode {
        RemoteAuthMode::Anonymous | RemoteAuthMode::Agent => Ok(()),
        RemoteAuthMode::Password => {
            if username.is_none() {
                return Err(missing(
                    "remote.username_required",
                    "A user name is required",
                ));
            }
            if require_credential && secret.is_none() {
                return Err(missing(
                    "remote.password_required",
                    "A password is required",
                ));
            }
            Ok(())
        }
        RemoteAuthMode::PrivateKey => {
            if username.is_none() {
                return Err(missing(
                    "remote.username_required",
                    "A user name is required",
                ));
            }
            if require_credential && private_key.is_none() {
                return Err(missing(
                    "remote.private_key_required",
                    "A private key is required",
                ));
            }
            Ok(())
        }
    }
}

fn validate_sizes(
    secret: Option<&str>,
    private_key: Option<&str>,
    passphrase: Option<&str>,
) -> Result<(), ApiError> {
    validate_secret_value(
        secret,
        MAX_REMOTE_SECRET,
        "remote.password_length",
        "password",
    )?;
    validate_secret_value(
        private_key,
        MAX_REMOTE_KEY,
        "remote.private_key_length",
        "private key",
    )?;
    validate_secret_value(
        passphrase,
        MAX_REMOTE_SECRET,
        "remote.passphrase_length",
        "passphrase",
    )
}

fn normalized(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn invalid_host() -> ApiError {
    ApiError::bad_request("remote.host_invalid", "The host name is not valid")
}

fn missing(code: &'static str, message: &'static str) -> ApiError {
    ApiError::bad_request(code, message)
}

fn not_found() -> ApiError {
    ApiError::not_found("remote.credential_not_found", "Remote login not found")
}

/// The unique index turns a duplicate endpoint into a conflict rather than a 500.
fn map_store_error(error: &anyhow::Error) -> ApiError {
    if rd_db::store_kind(error) == Some(StoreErrorKind::Duplicate) {
        return ApiError::conflict(
            "remote.credential_duplicate",
            "A login for this server, port and user already exists",
        );
    }
    ApiError::from(anyhow::anyhow!(error.to_string()))
}

#[cfg(test)]
mod tests {
    use rd_core::{RemoteAuthMode, RemoteProtocol};

    use super::{parse_host, validate_auth, validate_port};

    #[test]
    fn hosts_normalise_the_way_links_are_matched() {
        assert_eq!(
            parse_host("Files.EXAMPLE.com.").expect("host"),
            "files.example.com"
        );
        assert_eq!(
            parse_host("ftp://files.example.com/pub").expect("host"),
            "files.example.com"
        );
        // Unicode has to become punycode here, or a link would never match the entry.
        assert_eq!(
            parse_host("\u{e9}xample.fr").expect("host"),
            "xn--xample-9ua.fr"
        );
        assert!(parse_host("").is_err());
        assert!(parse_host("   ").is_err());
    }

    #[test]
    fn keys_and_agents_are_refused_for_ftp() {
        for mode in [RemoteAuthMode::PrivateKey, RemoteAuthMode::Agent] {
            assert!(
                validate_auth(
                    RemoteProtocol::Ftp,
                    mode,
                    Some("bob"),
                    None,
                    Some("k"),
                    true
                )
                .is_err()
            );
        }
        assert!(
            validate_auth(
                RemoteProtocol::Sftp,
                RemoteAuthMode::PrivateKey,
                Some("bob"),
                None,
                Some("k"),
                true
            )
            .is_ok()
        );
    }

    #[test]
    fn a_mode_without_its_credential_is_refused_on_create() {
        assert!(
            validate_auth(
                RemoteProtocol::Ftp,
                RemoteAuthMode::Password,
                Some("bob"),
                None,
                None,
                true
            )
            .is_err()
        );
        // On update the stored credential stands in for the missing field.
        assert!(
            validate_auth(
                RemoteProtocol::Ftp,
                RemoteAuthMode::Password,
                Some("bob"),
                Some("kept"),
                None,
                false
            )
            .is_ok()
        );
    }

    #[test]
    fn anonymous_needs_nothing_at_all() {
        assert!(
            validate_auth(
                RemoteProtocol::Ftp,
                RemoteAuthMode::Anonymous,
                None,
                None,
                None,
                true
            )
            .is_ok()
        );
    }

    #[test]
    fn port_zero_is_refused() {
        assert!(validate_port(0).is_err());
        assert!(validate_port(21).is_ok());
    }
}
