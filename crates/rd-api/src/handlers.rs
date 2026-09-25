use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use rd_core::CandidateId;
use rd_db::StoreErrorKind;
use sha2::{Digest, Sha256};

use crate::{
    ApiError, AppState,
    dto::{
        AuthStatus, CapturePairRequest, CapturePairResponse, LoginRequest, MessageResponse,
        NzbImportUpdateRequest, SettingsResponse, SetupRequest,
    },
};

#[utoipa::path(get, path = "/api/v1/health", tag = "system", responses((status = 200, body = Object)))]
pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "service": "rDownloader"
    }))
}

#[utoipa::path(get, path = "/api/v1/auth/status", responses((status = 200, body = AuthStatus)))]
pub async fn auth_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AuthStatus>, ApiError> {
    if state.auth.disabled() {
        return Ok(Json(AuthStatus {
            setup_required: false,
            authenticated: true,
            login_disabled: true,
            passkeys_available: false,
        }));
    }
    let configured = state.auth.is_configured(&state).await?;
    let authenticated = configured && state.auth.authenticated(&state, &headers).await;
    Ok(Json(AuthStatus {
        setup_required: !configured,
        authenticated,
        login_disabled: false,
        // Told to an anonymous caller deliberately: the sign-in screen cannot offer a passkey
        // button it does not know to draw, and the alternative — always offering it and
        // failing — teaches people to ignore the button. What it reveals is that this
        // installation has a passkey, which the challenge endpoint would reveal anyway.
        passkeys_available: crate::passkey_handlers::any_passkey_enrolled(&state).await?,
    }))
}

#[utoipa::path(post, path = "/api/v1/auth/setup", request_body = SetupRequest, responses((status = 200, body = MessageResponse)))]
pub async fn setup(
    State(state): State<AppState>,
    Json(request): Json<SetupRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    state.auth.setup(&state, &request.password).await?;
    Ok(Json(MessageResponse::new(
        "auth.setup_done",
        "Setup completed",
    )))
}

/// One refused sign-in, recorded.
///
/// `reason` names the *stage* that refused, never the value that was wrong: "password",
/// "password+totp", "locked_out". An audit log of failed sign-ins that quoted the attempts
/// would be a dictionary of near-miss passwords, which is the one thing it must not become.
async fn note_failed_login_audit(state: &AppState, client: std::net::IpAddr, reason: &str) {
    crate::audit::record(
        state,
        crate::audit::AuditEvent::failure(rd_core::AuditAction::LoginFailed)
            .actor(crate::audit::Actor::anonymous())
            .client(client)
            .detail("stage", reason),
    )
    .await;
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 429, description = "Too many failed attempts from this address", body = crate::error::ErrorBody),
    )
)]
pub async fn login(
    State(state): State<AppState>,
    client: crate::client::ClientAddress,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    // Checked before the password, and reported as a refusal rather than a wrong password:
    // telling a locked-out caller "invalid credentials" would leave them guessing at why a
    // password they know is right keeps failing.
    if let rd_authn::Decision::Locked { retry_after } = state.auth.throttle_check(client.0).await {
        note_failed_login_audit(&state, client.0, "locked_out").await;
        return Err(ApiError::too_many_requests(
            "auth.too_many_attempts",
            "Too many failed sign-in attempts from this address",
        )
        .with_param("seconds", retry_after.as_secs().max(1).to_string()));
    }
    if let rd_authn::Decision::Proceed { delay } = state.auth.throttle_check(client.0).await
        && !delay.is_zero()
    {
        // The global slow-down. Paid by everyone while an attack is running, and capped low
        // enough that it stays a nuisance rather than an outage.
        tokio::time::sleep(delay).await;
    }
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .and_then(rd_core::truncate_user_agent);

    // The password is checked first and its result held, so a wrong password and a wrong code
    // take the same path and the same time. Refusing early on the password would make the two
    // distinguishable, and an attacker who can tell which half failed has halved the problem.
    let password_ok = state
        .auth
        .password_matches(&state, &request.password)
        .await?;
    // Only the authenticator app gates this path. A passkey is an alternative way in, not an
    // extra step in front of the password — it already carries its own user verification — and
    // counting one here would demand a code from somebody who has no authenticator app at all.
    let second_factor = state.database.list_mfa_credentials().await?;
    let mfa_required = second_factor.iter().any(|credential| {
        credential.kind == rd_core::MfaKind::Totp && credential.confirmed_at.is_some()
    });
    if mfa_required {
        match request
            .code
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            None if password_ok => {
                // Only after the password was right: otherwise this reply would tell an
                // unauthenticated caller that the account exists and uses a second factor.
                return Err(ApiError::unauthorized(
                    "auth.mfa_required",
                    "A code from your authenticator app is required",
                ));
            }
            None => {
                state.auth.note_failed_login(client.0).await;
                note_failed_login_audit(&state, client.0, "password").await;
                return Err(crate::error_codes::invalid_credentials());
            }
            Some(code) => {
                // Two steps on purpose. Working out *what* the code is happens whichever half
                // is wrong — it is the secret-store read, the HMAC over the accepted window and
                // the recovery-digest comparison — so a wrong password does not return
                // measurably sooner than a wrong code and the refusal below is the same either
                // way. Only *spending* what was found waits for the password: consuming first
                // let anyone holding a recovery-code printout and no password burn a code
                // permanently on a login that was refused anyway.
                let matched = crate::mfa_handlers::second_factor_match(&state, code).await?;
                let code_ok = password_ok
                    && crate::mfa_handlers::consume_second_factor(&state, matched).await?;
                if !code_ok {
                    state.auth.note_failed_login(client.0).await;
                    // Deliberately the same reason either way. The refusal does not tell the
                    // caller which half failed, and neither does the record: an audit log a
                    // caller cannot read is still one an attacker who gains it could.
                    note_failed_login_audit(&state, client.0, "password+totp").await;
                    return Err(crate::error_codes::invalid_credentials());
                }
            }
        }
    } else if !password_ok {
        state.auth.note_failed_login(client.0).await;
        note_failed_login_audit(&state, client.0, "password").await;
        return Err(crate::error_codes::invalid_credentials());
    }

    let opened = state
        .auth
        .open_session(&state, client.0, user_agent)
        .await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::LoginSucceeded)
            .actor(crate::audit::Actor::session(opened.id.to_string()))
            .client(client.0)
            .target("session", opened.id)
            .detail(
                "method",
                if mfa_required {
                    "password+totp"
                } else {
                    "password"
                },
            ),
    )
    .await;
    let mut response = Json(MessageResponse::new("auth.logged_in", "Logged in")).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&{
            let proxy = state.proxy.read().await;
            crate::AuthService::cookie(
                &opened.token,
                proxy.cookie_is_secure(),
                proxy.base_path(),
                opened.max_age_seconds,
            )
        })
        .map_err(|_| {
            ApiError::bad_request("auth.session_create_failed", "Session could not be created")
        })?,
    );
    Ok(response)
}

#[utoipa::path(post, path = "/api/v1/capture/pair", tag = "capture", request_body = CapturePairRequest, responses((status = 201, body = CapturePairResponse)))]
pub async fn pair_capture(
    State(state): State<AppState>,
    Json(request): Json<CapturePairRequest>,
) -> Result<(StatusCode, Json<CapturePairResponse>), ApiError> {
    let response = crate::api_tokens::pair_scoped_token(
        &state,
        &request.label,
        rd_core::CAPTURE_SCOPE,
        "capture.label_length",
    )
    .await?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(get, path = "/api/v1/capture/agents", tag = "capture", responses((status = 200, body = [rd_core::CaptureToken])))]
pub async fn list_capture_agents(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::CaptureToken>>, ApiError> {
    Ok(Json(
        state
            .database
            .list_capture_tokens(&[rd_core::CAPTURE_SCOPE])
            .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/capture/agents/{id}", tag = "capture", params(("id" = rd_core::CaptureTokenId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn revoke_capture_agent(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CaptureTokenId>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .database
        .revoke_capture_token(id)
        .await
        .map_err(|error| {
            crate::error_codes::store_not_found(
                &error,
                "capture.not_found",
                "Capture token not found",
            )
        })?;
    Ok(Json(MessageResponse::new(
        "capture.revoked",
        "Capture token revoked",
    )))
}

pub async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(crate::openapi_document())
}

#[utoipa::path(get, path = "/api/v1/packages", tag = "downloads", responses((status = 200, body = [rd_core::DownloadPackage])))]
pub async fn list_packages(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::DownloadPackage>>, ApiError> {
    Ok(Json(state.database.list_packages().await?))
}

#[utoipa::path(get, path = "/api/v1/collector/batches", tag = "collector", responses((status = 200, body = [rd_core::CollectorBatch])))]
pub async fn list_batches(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::CollectorBatch>>, ApiError> {
    Ok(Json(state.database.list_collector_batches().await?))
}

#[utoipa::path(get, path = "/api/v1/collector/candidates", tag = "collector", responses((status = 200, body = [rd_core::LinkCandidate])))]
pub async fn list_candidates(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::LinkCandidate>>, ApiError> {
    Ok(Json(state.database.list_candidates().await?))
}

#[utoipa::path(delete, path = "/api/v1/collector/candidates/{id}", tag = "collector", params(("id" = rd_core::CandidateId, Path)), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_candidate(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CandidateId>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .database
        .delete_candidate(id)
        .await
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::NotFound) => {
                ApiError::not_found("collector.candidate_not_found", "Link not found")
            }
            Some(StoreErrorKind::Busy) => ApiError::conflict(
                "collector.candidate_busy",
                "Link is being enqueued and cannot be deleted",
            ),
            _ => error.into(),
        })?;
    crate::torrent_handlers::prune_checked_torrents(&state).await;
    Ok(Json(MessageResponse::new(
        "collector.candidate_removed",
        "Link removed from the LinkGrabber",
    )))
}

#[utoipa::path(delete, path = "/api/v1/collector/candidates", tag = "collector", responses((status = 200, body = MessageResponse)))]
pub async fn delete_candidates(
    State(state): State<AppState>,
) -> Result<Json<MessageResponse>, ApiError> {
    let count = state.database.delete_candidates().await?;
    crate::torrent_handlers::prune_checked_torrents(&state).await;
    Ok(Json(
        MessageResponse::new(
            "collector.candidates_removed",
            format!("{count} link(s) removed from the LinkGrabber"),
        )
        .with_count(count),
    ))
}

#[utoipa::path(get, path = "/api/v1/nzb/imports", tag = "collector", responses((status = 200, body = [rd_core::NzbImport])))]
pub async fn list_nzb_imports(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::NzbImport>>, ApiError> {
    Ok(Json(state.database.list_nzb_imports().await?))
}

#[utoipa::path(patch, path = "/api/v1/nzb/imports/{id}", tag = "collector", params(("id" = rd_core::NzbImportId, Path)), request_body = NzbImportUpdateRequest, responses((status = 200, body = rd_core::NzbImport), (status = 400), (status = 404), (status = 409)))]
pub async fn update_nzb_import(
    State(state): State<AppState>,
    Path(id): Path<rd_core::NzbImportId>,
    Json(request): Json<NzbImportUpdateRequest>,
) -> Result<Json<rd_core::NzbImport>, ApiError> {
    if let Some(category_id) = request.category_id
        && !state
            .database
            .list_categories()
            .await?
            .iter()
            .any(|category| category.id == category_id)
    {
        return Err(ApiError::bad_request(
            "category.not_found",
            "Category not found",
        ));
    }
    let category_id = if request.clear_category {
        Some(None)
    } else {
        request.category_id.map(Some)
    };
    if category_id.is_none() && request.priority.is_none() {
        return Err(ApiError::bad_request(
            "nzb.no_change",
            "No NZB import change specified",
        ));
    }
    state
        .database
        .update_nzb_import(
            id,
            rd_db::NzbImportChange {
                category_id,
                priority: request.priority,
            },
        )
        .await
        .map(Json)
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::NotFound) => {
                ApiError::not_found("nzb.import_not_found", "NZB import not found")
            }
            Some(StoreErrorKind::WrongState) => ApiError::conflict(
                "nzb.import_active",
                "Enqueued NZB imports must be changed in the download list",
            ),
            _ => error.into(),
        })
}

#[utoipa::path(delete, path = "/api/v1/nzb/imports/{id}", tag = "collector", params(("id" = rd_core::NzbImportId, Path)), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_nzb_import(
    State(state): State<AppState>,
    Path(id): Path<rd_core::NzbImportId>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .database
        .delete_nzb_import(id)
        .await
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::NotFound) => {
                ApiError::not_found("nzb.import_not_found", "NZB import not found")
            }
            Some(StoreErrorKind::WrongState) => {
                ApiError::conflict("nzb.import_active", "Active NZB imports cannot be deleted")
            }
            _ => error.into(),
        })?;
    Ok(Json(MessageResponse::new(
        "nzb.import_removed",
        "NZB import removed from the LinkGrabber",
    )))
}

#[utoipa::path(post, path = "/api/v1/nzb/imports", tag = "collector", request_body(content((Vec<u8> = "multipart/form-data"), (crate::container_upload::ContainerUpload = "application/json"))), responses((status = 201, body = rd_core::NzbImport), (status = 400, description = "The NZB cannot be parsed, a field is invalid, or the JSON content is not base64"), (status = 413, description = "The JSON content decodes to more than 48 MiB, or the body exceeds the service's limit")))]
pub async fn import_nzb(
    State(state): State<AppState>,
    body: crate::container_upload::UploadBody,
) -> Result<(StatusCode, Json<rd_core::NzbImport>), ApiError> {
    let upload = body.read().await?;
    let category_id = match upload.category_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(id) => Some(id.parse::<rd_core::CategoryId>().map_err(|_| {
            ApiError::bad_request("nzb.category_invalid", "Category id is not valid")
        })?),
    };
    let priority = match upload.priority.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(text) => Some(
            serde_json::from_value::<rd_core::DownloadPriority>(serde_json::Value::String(
                text.to_owned(),
            ))
            .map_err(|_| {
                ApiError::bad_request(
                    "nzb.priority_invalid",
                    "Priority must be low, normal or high",
                )
            })?,
        ),
    };
    let file = upload.file.ok_or_else(|| {
        ApiError::bad_request(
            "nzb.file_field_missing",
            "Multipart field 'file' is missing",
        )
    })?;
    if file.bytes.len() > rd_collector::MAX_NZB_BYTES {
        return Err(
            ApiError::bad_request("nzb.too_large", "NZB exceeds the 64 MiB limit")
                .with_param("max_bytes", rd_collector::MAX_NZB_BYTES),
        );
    }
    // A supplied name wins over the uploaded file's name.
    let file_name = match upload.name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => rd_files::sanitize_file_name(name),
        _ => file
            .file_name
            .as_deref()
            .map_or_else(|| "import.nzb".to_owned(), rd_files::sanitize_file_name),
    };
    let content = file.bytes;
    store_nzb_import(
        &state,
        &content,
        &file_name,
        category_id,
        // Somebody uploaded this file through the interface; a rule can target that the same
        // way it targets a hotfolder drop.
        rd_core::IngressSource::Manual,
        priority,
    )
    .await
    .map(|import| (StatusCode::CREATED, Json(import)))
}

/// Parses an NZB and records it as a review-mode import.
///
/// Split out of [`import_nzb`] so the SABnzbd adapter reaches the same parser, the same
/// password convention, the same validation and the same category selection instead of
/// assembling its own import rows.
pub(crate) async fn store_nzb_import(
    state: &AppState,
    content: &[u8],
    file_name: &str,
    category_id: Option<rd_core::CategoryId>,
    source: rd_core::IngressSource,
    priority: Option<rd_core::DownloadPriority>,
) -> Result<rd_core::NzbImport, ApiError> {
    let parsed = rd_collector::parse_nzb(content)
        .map_err(|error| ApiError::bad_request("nzb.parse_failed", error.to_string()))?;
    // SABnzbd convention: `release{{password}}.nzb` carries the archive password.
    let (file_name, marker_password) = rd_files::strip_password_marker(file_name);
    let password = marker_password.or_else(|| parsed.password.clone());
    let import = rd_db::NewNzbImport {
        name: rd_files::sanitize_file_name(&file_name),
        sha256: hex::encode(Sha256::digest(content)),
        category_id,
        source,
        priority,
        import_mode: rd_core::ImportMode::Review,
        source_path: None,
        password,
        // An upload is somebody handing a file over.
        announce_arrival: true,
        files: parsed
            .files
            .into_iter()
            .map(|file| rd_db::NewNzbFile {
                subject: file.subject,
                poster: file.poster,
                groups: file.groups,
                segments: file
                    .segments
                    .into_iter()
                    .map(|segment| rd_db::NewNzbSegment {
                        number: segment.number,
                        bytes: segment.bytes,
                        message_id: segment.message_id,
                    })
                    .collect(),
            })
            .collect(),
    };
    Ok(state.database.add_nzb_import(import).await?)
}

/// The capture agent's NZB drop: the same import, but multipart only — the agent's contract
/// did not change when the interface's route learned JSON (RD-120-31).
pub async fn capture_nzb(
    state: State<AppState>,
    multipart: Multipart,
) -> Result<(StatusCode, Json<rd_core::NzbImport>), ApiError> {
    import_nzb(
        state,
        crate::container_upload::UploadBody::Multipart(multipart),
    )
    .await
}

/// Token check for external clients (browser extension "test connection").
/// `capture_version` tells clients whether structured links with request metadata
/// are accepted; older servers omit it and only understand the text payload.
#[utoipa::path(get, path = "/api/v1/capture/ping", tag = "capture", responses((status = 200, body = Object), (status = 401)))]
pub async fn capture_ping() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "service": "rDownloader",
        "capture_version": rd_core::CAPTURE_CONTRACT_VERSION
    }))
}

#[utoipa::path(post, path = "/api/v1/collector/candidates/{id}/enqueue", tag = "collector", params(("id" = String, Path)), responses((status = 201, body = rd_core::DownloadPackage), (status = 409)))]
pub async fn enqueue_candidate(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<rd_core::DownloadPackage>), ApiError> {
    let id = crate::error_codes::parse_id::<CandidateId>(&id)?;
    let candidate =
        state.database.get_candidate(id).await?.ok_or_else(|| {
            ApiError::not_found("collector.candidate_not_found", "Link not found")
        })?;
    // `LinkCandidateState::ENQUEUEABLE` is the one list; the package path builds its SQL from
    // the same constant. The two used to disagree — a single `unsupported` link was refused
    // while the same link inside a package went through.
    if !candidate.state.is_enqueueable() {
        return Err(ApiError::conflict(
            "collector.candidate_not_enqueueable",
            "This link has already been enqueued or is being processed",
        ));
    }
    // A single link becomes its own package (moved out of its group first).
    let package = state
        .database
        .move_candidates(
            vec![id],
            rd_db::MoveTarget::New {
                // The package name doubles as the folder name, so file extensions are stripped.
                name: candidate.file_name.clone().map_or_else(
                    || candidate.url.host_str().unwrap_or("Link").to_owned(),
                    |name| rd_files::package_name_from_file_name(&name),
                ),
            },
        )
        .await?;
    let created =
        crate::collector_enqueue::enqueue_package(&state, package.id, false, None).await?;
    Ok((StatusCode::CREATED, Json(created.package)))
}

#[utoipa::path(get, path = "/api/v1/settings", tag = "system", responses((status = 200, body = SettingsResponse)))]
pub async fn get_settings(
    State(state): State<AppState>,
) -> Result<Json<SettingsResponse>, ApiError> {
    Ok(Json(read_settings(&state).await?))
}

pub(crate) async fn read_settings(state: &AppState) -> Result<SettingsResponse, ApiError> {
    stored_settings(&state.database).await
}

/// The settings blob for callers that hold a database but no `AppState`, such as the
/// hotfolder watcher.
pub(crate) async fn stored_settings(
    database: &rd_db::Database,
) -> Result<SettingsResponse, ApiError> {
    // Refuses a malformed blob with `settings.invalid`, as before: this is what the settings
    // view reads, and showing silent defaults there would invite saving them back over the
    // stored configuration.
    let value = database.get_setting(rd_db::SERVICE_SETTINGS_KEY).await?;
    value
        .map(|blob| rd_db::parse_service_settings(&blob))
        .transpose()
        .map_err(|error| ApiError::bad_request("settings.invalid", error.to_string()))
        .map(Option::unwrap_or_default)
}

#[utoipa::path(put, path = "/api/v1/settings", tag = "system", request_body = SettingsResponse, responses((status = 200, body = SettingsResponse)))]
pub async fn put_settings(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Json(settings): Json<SettingsResponse>,
) -> Result<Json<SettingsResponse>, ApiError> {
    let holds_admin =
        granted.is_some_and(|axum::Extension(granted)| granted.holds(rd_core::Scope::Admin));
    save_settings(&state, &audit, holds_admin, settings)
        .await
        .map(Json)
}

/// Saves a settings document on a caller's behalf: the privileged-field gate, the apply and
/// the audit record, in the one function both the REST route and the MCP tool call.
///
/// The gate used to live in [`put_settings`] alone, and the MCP tool `update_settings` went
/// straight to [`apply_settings`]. Both cost `api:config`, so a configuration token could
/// switch the administrator login off — and with it gain every scope — over MCP while the
/// same change over REST was refused (RD-130-09). One function is what keeps the two from
/// drifting apart again. `holds_admin` is the caller's own grant, never the service's.
pub(crate) async fn save_settings(
    state: &AppState,
    audit: &crate::audit::AuditContext,
    holds_admin: bool,
    settings: SettingsResponse,
) -> Result<SettingsResponse, ApiError> {
    let current = read_settings(state).await?;
    if !holds_admin && let Some(field) = privileged_change(&current, &settings) {
        // Audited as a failure: a refused attempt to widen what a credential may reach is
        // exactly the thing somebody reads an audit log to find.
        crate::audit::record(
            state,
            crate::audit::AuditEvent::failure(rd_core::AuditAction::SettingsChanged)
                .by(audit)
                .target("settings", "service.settings")
                .detail("refused_field", field)
                .detail("reason", "scope_insufficient"),
        )
        .await;
        return Err(ApiError::forbidden(
            "auth.scope_insufficient",
            "This setting requires the administration scope",
        )
        .with_param("scope", rd_core::Scope::Admin.as_str())
        .with_param("setting", field));
    }
    let applied = apply_settings(state, settings).await?;
    // The *names* of the fields that changed, never their values: the settings document holds
    // secret references, executable paths and proxy addresses, and an audit log that quoted
    // them would be a copy of the configuration with a timestamp on it.
    crate::audit::record(
        state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::SettingsChanged)
            .by(audit)
            .target("settings", "service.settings")
            .detail("fields", changed_field_names(&current, &applied).join(" ")),
    )
    .await;
    Ok(applied)
}

/// The names of the settings fields whose stored value differs between two documents.
///
/// Compared as JSON rather than field by field so a field added later is covered without
/// anybody remembering to add it here, and so nothing in this function ever has to hold a
/// value long enough to log it by accident.
fn changed_field_names(current: &SettingsResponse, next: &SettingsResponse) -> Vec<String> {
    let (Ok(serde_json::Value::Object(before)), Ok(serde_json::Value::Object(after))) =
        (serde_json::to_value(current), serde_json::to_value(next))
    else {
        return Vec::new();
    };
    let mut names: Vec<String> = after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect();
    names.extend(
        before
            .keys()
            .filter(|key| !after.contains_key(*key))
            .cloned(),
    );
    names.sort();
    names
}

/// The first privileged field `next` changes, if any.
///
/// The settings blob is priced `api:config`, but a handful of its fields do not configure
/// downloading at all — they decide who may talk to the service and what it executes.
/// `admin_login_disabled` is the sharpest: setting it makes `granted_scopes` hand every
/// caller, authenticated or not, the full `Scope::API`, so an `api:config` token could mint
/// itself `api:secrets` and `api:admin`. The executable paths and the completion script are
/// the same problem one step removed: they name a program this service runs.
fn privileged_change(current: &SettingsResponse, next: &SettingsResponse) -> Option<&'static str> {
    let fields: [(&'static str, bool); 15] = [
        (
            "admin_login_disabled",
            current.admin_login_disabled != next.admin_login_disabled,
        ),
        (
            "trusted_proxies",
            current.trusted_proxies != next.trusted_proxies,
        ),
        ("external_url", current.external_url != next.external_url),
        (
            "cookie_security",
            current.cookie_security != next.cookie_security,
        ),
        // How long a stolen cookie stays worth something: lengthening either limit widens
        // what a credential reaches in time the way the fields above widen it in scope
        // (RD-130-09).
        (
            "session_idle_hours",
            current.session_idle_hours != next.session_idle_hours,
        ),
        (
            "session_max_hours",
            current.session_max_hours != next.session_max_hours,
        ),
        (
            "completion_action",
            current.completion_action != next.completion_action,
        ),
        (
            "completion_script",
            current.completion_script != next.completion_script,
        ),
        (
            "scripts_directory",
            current.scripts_directory != next.scripts_directory,
        ),
        ("custom_ca_pem", current.custom_ca_pem != next.custom_ca_pem),
        (
            "vendor_directory",
            current.vendor_directory != next.vendor_directory,
        ),
        (
            "rar_executable",
            current.rar_executable != next.rar_executable,
        ),
        (
            "remote_ssh_auto_trust",
            current.remote_ssh_auto_trust != next.remote_ssh_auto_trust,
        ),
        // Pointing the trace export somewhere is telling the service to post to a URL of
        // somebody's choosing on a timer. That is the same family of decision as naming a
        // program it runs, whatever the payload is (RD-110-03).
        (
            "otlp_endpoint",
            current.otlp_endpoint != next.otlp_endpoint
                || current.otlp_enabled != next.otlp_enabled,
        ),
        (
            "media_ytdlp_executable",
            current.media_ytdlp_executable != next.media_ytdlp_executable
                || current.media_ffmpeg_executable != next.media_ffmpeg_executable
                || current.gallery_executable != next.gallery_executable
                || current.record_streamlink_executable != next.record_streamlink_executable,
        ),
    ];
    fields
        .into_iter()
        .find_map(|(name, changed)| changed.then_some(name))
}

/// Restores the built-in runtime defaults without touching accounts, routing or download data.
#[utoipa::path(post, path = "/api/v1/settings/reset", tag = "system", responses((status = 200, body = SettingsResponse)))]
pub async fn reset_settings(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
) -> Result<Json<SettingsResponse>, ApiError> {
    let applied = apply_settings(&state, SettingsResponse::default()).await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::SettingsReset)
            .by(&audit)
            .target("settings", "service.settings"),
    )
    .await;
    Ok(Json(applied))
}

/// Validates, persists and live-applies the full settings blob — the only legal mutation path.
pub(crate) async fn apply_settings(
    state: &AppState,
    mut settings: SettingsResponse,
) -> Result<SettingsResponse, ApiError> {
    let runtime = validate_settings(&mut settings)?;
    state
        .database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::to_value(&settings).map_err(anyhow::Error::new)?,
        )
        .await?;
    state.scheduler.update_runtime_settings(runtime).await?;
    // Every running watcher re-arms its ticker on this; nothing is restarted (RD-110-31).
    state
        .hotfolders
        .set_poll_interval(crate::hotfolder_service::poll_interval_of(&settings));
    // The thresholds live in the same blob, so the capacity service reloads right away
    // instead of waiting for the next supervision tick.
    state.scheduler.reload_capacity_config().await?;
    // The schedule's timezone and default profile ride in the same blob.
    state.scheduler.reload_bandwidth().await?;
    let timezone = rd_limits::parse_timezone(&settings.bandwidth_timezone)
        .unwrap_or(rd_limits::default_timezone());
    state
        .power
        .apply(
            rd_power::PowerSettings {
                quiet_hours: settings.quiet_hours.clone(),
                quiet_hours_defer_postprocess: settings.quiet_hours_defer_postprocess,
                quiet_hours_defer_notifications: settings.quiet_hours_defer_notifications,
                completion_action: settings.completion_action,
                completion_script: settings.completion_script.clone(),
                completion_countdown_seconds: settings.completion_countdown_seconds,
                power_actions_allowed: settings.power_actions_allowed,
                pause_on_battery: settings.pause_on_battery,
                pause_on_metered: settings.pause_on_metered,
                prevent_standby: settings.prevent_standby,
                prevent_display_standby: settings.prevent_display_standby,
            },
            timezone,
        )
        .await;
    state.auth.set_disabled(settings.admin_login_disabled);
    // Every following request is measured against these, the sessions already open included.
    state.auth.set_session_limits(rd_core::SessionLimits {
        idle_hours: settings.session_idle_hours,
        max_hours: settings.session_max_hours,
    });
    // Validated above, so this cannot fail; falling back to the safe default rather than
    // unwrapping keeps a future refactor of the validator from turning into a panic.
    *state.proxy.write().await = rd_authn::ProxyConfig::parse(
        &settings.trusted_proxies,
        settings.external_url.as_deref(),
        settings.cookie_security,
    )
    .unwrap_or_default();
    *state.media_settings.write().await = rd_core::MediaSettings {
        media_ytdlp_executable: settings.media_ytdlp_executable.clone(),
        media_ffmpeg_executable: settings.media_ffmpeg_executable.clone(),
        media_default_variant: settings.media_default_variant.clone(),
        media_default_criteria: settings.media_default_criteria.clone(),
        media_output_template: settings.media_output_template.clone(),
        media_hosts: settings.media_hosts.clone(),
        media_max_parallel: settings.media_max_parallel,
        media_check_timeout_seconds: settings.media_check_timeout_seconds,
        vendor_directory: settings.vendor_directory.clone(),
    };
    *state.gallery_settings.write().await = rd_core::GallerySettings {
        gallery_executable: settings.gallery_executable.clone(),
        gallery_hosts: settings.gallery_hosts.clone(),
        gallery_max_parallel: settings.gallery_max_parallel,
        vendor_directory: settings.vendor_directory.clone(),
    };
    state.tools.apply_settings(rd_core::ManagedToolSettings {
        managed_tools_enabled: settings.managed_tools_enabled,
        managed_tools_manifest_url: settings.managed_tools_manifest_url.clone(),
        tool_compatibility_overrides: settings.tool_compatibility_overrides.clone(),
    });
    *state.remote_settings.write().await = rd_core::RemoteSettings {
        remote_max_parallel: settings.remote_max_parallel,
        remote_timeout_seconds: settings.remote_timeout_seconds,
        remote_ssh_auto_trust: settings.remote_ssh_auto_trust,
    }
    .sanitized();
    *state.stream_settings.write().await = rd_core::StreamSettings {
        record_streamlink_executable: settings.record_streamlink_executable.clone(),
        record_default_quality: settings.record_default_quality.clone(),
        record_poll_interval_seconds: settings.record_poll_interval_seconds,
        record_max_parallel: settings.record_max_parallel,
        vendor_directory: settings.vendor_directory.clone(),
    };
    // Ratio/time/seeding apply live; port and rate limits on the next session start.
    *state.torrent_settings.write().await = rd_core::TorrentSettings {
        torrent_listen_port: settings.torrent_listen_port,
        torrent_seed_ratio: settings.torrent_seed_ratio,
        torrent_seed_time_minutes: settings.torrent_seed_time_minutes,
        torrent_seeding_enabled: settings.torrent_seeding_enabled,
        torrent_sharing_enabled: settings.torrent_sharing_enabled,
        torrent_upload_limit_bytes_per_second: settings.torrent_upload_limit_bytes_per_second,
        torrent_bind_interface: settings.torrent_bind_interface.clone(),
        torrent_kill_switch_enabled: settings.torrent_kill_switch_enabled,
        torrent_ip_blocklist_url: settings.torrent_ip_blocklist_url.clone(),
        torrent_listen_mode: settings.torrent_listen_mode,
        torrent_peer_limit: settings.torrent_peer_limit,
        torrent_download_limit_bytes_per_second: settings.torrent_download_limit_bytes_per_second,
        torrent_proxy_profile_id: settings.torrent_proxy_profile_id,
        torrent_upnp_enabled: settings.torrent_upnp_enabled,
        torrent_announce_port: settings.torrent_announce_port,
        torrent_peer_addresses_visible: settings.torrent_peer_addresses_visible,
        keep_import_history: settings.keep_import_history,
    };
    // Rate limits are applied in place; a changed listen port rebuilds the session. A
    // failed rebuild keeps the previous engine running, so it is reported, not fatal.
    if let Err(error) = state.torrent.reconfigure().await {
        tracing::warn!(%error, "torrent session could not be reconfigured");
    }
    Ok(settings)
}

pub(crate) fn validate_settings(
    settings: &mut SettingsResponse,
) -> Result<rd_scheduler::RuntimeSettings, ApiError> {
    // Parsed here rather than at the point of use so a bad range or URL is refused when it is
    // saved, with a message naming the value, instead of quietly disabling proxy trust later.
    rd_authn::ProxyConfig::parse(
        &settings.trusted_proxies,
        settings.external_url.as_deref(),
        settings.cookie_security,
    )
    .map_err(|error| {
        ApiError::bad_request(
            "settings.proxy_invalid",
            "The proxy configuration is not usable",
        )
        .with_param("reason", error.to_string())
    })?;
    // Refused here rather than clamped: a session limit is a security setting, and quietly
    // storing a different value than the one somebody typed would be a decision they did
    // not take (RD-130-09).
    let (idle_range, max_range) = (
        rd_core::SESSION_IDLE_HOURS_RANGE,
        rd_core::SESSION_MAX_HOURS_RANGE,
    );
    if !idle_range.contains(&settings.session_idle_hours) {
        return Err(ApiError::bad_request(
            "settings.session_idle_invalid",
            format!(
                "The idle limit of a sign-in must be between {} and {} hours",
                idle_range.start(),
                idle_range.end()
            ),
        )
        .with_param("min", *idle_range.start())
        .with_param("max", *idle_range.end()));
    }
    if !max_range.contains(&settings.session_max_hours) {
        return Err(ApiError::bad_request(
            "settings.session_max_invalid",
            format!(
                "The maximum lifetime of a sign-in must be between {} and {} hours",
                max_range.start(),
                max_range.end()
            ),
        )
        .with_param("min", *max_range.start())
        .with_param("max", *max_range.end()));
    }
    if !matches!(settings.byte_display.as_str(), "binary" | "decimal") {
        return Err(ApiError::bad_request(
            "settings.byte_display_invalid",
            "Byte display must be binary or decimal",
        ));
    }
    if !matches!(
        settings.byte_unit.as_str(),
        "auto" | "byte" | "kilo" | "mega" | "giga" | "tera" | "peta"
    ) {
        return Err(ApiError::bad_request(
            "settings.byte_unit_invalid",
            "The byte unit must be auto, byte, kilo, mega, giga, tera or peta",
        ));
    }
    if !rd_core::LOG_RETENTION_RECORDS_RANGE.contains(&settings.log_retention_records)
        || !rd_core::LOG_RETENTION_DAYS_RANGE.contains(&settings.log_retention_days)
    {
        return Err(ApiError::bad_request(
            "settings.log_retention_invalid",
            "Log retention must keep between 1000 and 500000 records for 1 to 365 days",
        ));
    }
    if !rd_core::AUDIT_RETENTION_RECORDS_RANGE.contains(&settings.audit_retention_records)
        || !rd_core::AUDIT_RETENTION_DAYS_RANGE.contains(&settings.audit_retention_days)
    {
        return Err(ApiError::bad_request(
            "settings.audit_retention_invalid",
            "Audit retention must keep between 10000 and 2000000 records for 30 to 3650 days",
        ));
    }
    settings.otlp_endpoint = settings.otlp_endpoint.trim().to_owned();
    if !rd_core::is_valid_otlp_endpoint(&settings.otlp_endpoint)
        || !rd_core::OTLP_TIMEOUT_SECONDS_RANGE.contains(&settings.otlp_timeout_seconds)
    {
        return Err(ApiError::bad_request(
            "settings.otlp_invalid",
            "The OTLP endpoint must be an http or https URL and the timeout 1 to 60 seconds",
        ));
    }
    // Switching the export on with nowhere to send to is a setting that silently does
    // nothing, and a person who ticked the box would reasonably believe it works.
    if settings.otlp_enabled && settings.otlp_endpoint.is_empty() {
        return Err(ApiError::bad_request(
            "settings.otlp_endpoint_required",
            "Exporting traces needs an endpoint",
        ));
    }
    if settings.max_active_files == 0
        || settings.max_chunks_per_file == 0
        || settings.nntp_connections_per_file > 32
        || settings.max_connections_per_host as usize > rd_http::MAX_CONNECTIONS_PER_HOST
    {
        return Err(ApiError::bad_request(
            "settings.concurrency_invalid",
            "Concurrency values must be valid; NNTP connections per file must be between 0 and 32",
        ));
    }
    settings.validate_postprocess()?;
    crate::stats_handlers::validate_stats_settings(settings)?;
    crate::hotfolder_service::validate_hotfolder_settings(settings)?;
    rd_limits::parse_timezone(&settings.bandwidth_timezone).map_err(|_| {
        ApiError::bad_request("bandwidth.timezone_invalid", "Unknown timezone")
            .with_param("timezone", &settings.bandwidth_timezone)
    })?;
    if !(rd_power::MIN_COMPLETION_COUNTDOWN..=rd_power::MAX_COMPLETION_COUNTDOWN)
        .contains(&settings.completion_countdown_seconds)
    {
        return Err(ApiError::bad_request(
            "power.countdown_invalid",
            format!(
                "The countdown must be between {} and {} seconds",
                rd_power::MIN_COMPLETION_COUNTDOWN,
                rd_power::MAX_COMPLETION_COUNTDOWN
            ),
        ));
    }
    if settings.completion_action == rd_power::CompletionAction::Script
        && settings
            .completion_script
            .as_ref()
            .is_none_or(|script| script.trim().is_empty())
    {
        return Err(ApiError::bad_request(
            "power.script_missing",
            "A completion script action needs a script name",
        ));
    }
    if !(1..=rd_core::MAX_UNKNOWN_SIZE_HEADROOM).contains(&settings.storage_unknown_size_headroom) {
        return Err(ApiError::bad_request(
            "settings.storage_headroom_invalid",
            format!(
                "The headroom factor for transfers of unknown size must be between 1 and {}",
                rd_core::MAX_UNKNOWN_SIZE_HEADROOM
            ),
        ));
    }
    settings.dlc_service_endpoint = settings
        .dlc_service_endpoint
        .take()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if let Some(endpoint) = settings.dlc_service_endpoint.as_deref()
        && !url::Url::parse(endpoint)
            .is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
    {
        return Err(ApiError::bad_request(
            "dlc.endpoint_invalid",
            "The DLC decryption service must be an http or https URL",
        ));
    }
    let runtime = rd_scheduler::RuntimeSettings {
        max_active_files: settings.max_active_files as usize,
        max_chunks_per_file: settings.max_chunks_per_file as usize,
        max_connections_per_host: settings.max_connections_per_host as usize,
        external_connections_per_file: settings.nntp_connections_per_file as usize,
        speed_limit_bytes_per_second: settings
            .speed_limit_bytes_per_second
            .map(rd_core::ByteCount::get),
        generate_sha256: settings.generate_sha256,
        global_proxy_profile_id: settings.global_proxy_profile_id,
        custom_ca_pem: settings.custom_ca_pem.clone(),
        max_retries: settings.max_retries,
        pause_during_postprocess: settings.pause_during_postprocess,
        disabled_kinds: service_switches(settings).disabled_kinds(),
    };
    rd_scheduler::SchedulerHandle::validate_runtime_settings(&runtime)?;
    Ok(runtime)
}

/// The service switches this settings document expresses.
pub fn service_switches(settings: &crate::dto::SettingsResponse) -> rd_core::ServiceSwitches {
    rd_core::ServiceSwitches {
        torrent: settings.torrent_service_enabled,
        usenet: settings.usenet_service_enabled,
        media: settings.media_service_enabled,
        gallery: settings.gallery_service_enabled,
        recording: settings.recording_service_enabled,
        remote: settings.remote_service_enabled,
    }
}

#[utoipa::path(get, path = "/api/v1/system/media", tag = "system", responses((status = 200, body = crate::dto::MediaStatusResponse)))]
pub async fn media_status(
    State(state): State<AppState>,
) -> Result<Json<crate::dto::MediaStatusResponse>, ApiError> {
    let settings = state.media_settings.read().await.clone();
    // Falls back to defaults rather than refusing, as before: this is a status read that must
    // still answer when a setting is broken, and the accessor reports what it rejected.
    let postprocess: rd_core::PostprocessSettings =
        state.database.service_settings_or_default().await?;
    let vendor = settings.vendor_directory.as_deref();
    let ytdlp = rd_core::locate_tool(settings.media_ytdlp_executable.as_deref(), vendor, "yt-dlp");
    let ffmpeg = rd_media::FfmpegTools::resolve(&settings);
    // An explicit rar_executable configures whichever tool `rar_tool` names; the other one
    // still falls back to the vendor folders and PATH.
    let explicit_rar = postprocess.rar_executable.as_deref();
    let unrar = rd_core::locate_tool(
        explicit_rar.filter(|_| postprocess.rar_tool == "unrar"),
        vendor,
        "unrar",
    );
    let seven_zip = rd_core::locate_tool(
        explicit_rar.filter(|_| postprocess.rar_tool == "7z"),
        vendor,
        "7z",
    );
    let rclone = rd_core::locate_tool(postprocess.rclone_executable.as_deref(), vendor, "rclone");
    let gallery = state.gallery_settings.read().await.clone();
    let gallery_dl =
        rd_core::locate_tool(gallery.gallery_executable.as_deref(), vendor, "gallery-dl");
    let streamlink = rd_stream::locate_streamlink(&*state.stream_settings.read().await);
    // The executable an apprise target may name is per target, so the status shows what a
    // target without one would run.
    let apprise = rd_core::locate_tool(None, vendor, "apprise");
    let (ytdlp, ffmpeg_status, ffprobe, unrar, seven_zip, rclone, gallery_dl, streamlink, apprise) = tokio::join!(
        rd_media::tool_status("yt-dlp", ytdlp.as_ref()),
        rd_media::tool_status("ffmpeg", ffmpeg.ffmpeg.as_ref()),
        rd_media::tool_status("ffprobe", ffmpeg.ffprobe.as_ref()),
        rd_media::tool_status("unrar", unrar.as_ref()),
        rd_media::tool_status("7z", seven_zip.as_ref()),
        rd_media::tool_status("rclone", rclone.as_ref()),
        rd_media::tool_status("gallery-dl", gallery_dl.as_ref()),
        rd_media::tool_status("streamlink", streamlink.as_ref()),
        rd_media::tool_status("apprise", apprise.as_ref())
    );
    // The managed store answers per tool name, so the two extra fields are a lookup rather
    // than another probe: `managed` says whether this application can manage the tool at all,
    // `active_version` which managed version is currently activated. Both are independent of
    // `source`, which says where the binary that would actually run came from.
    let managed_status = state.tools.status().await;
    let convert = move |status: rd_media::ToolStatus| {
        let managed = managed_status.iter().find(|tool| tool.name == status.name);
        crate::dto::MediaToolStatus {
            managed: rd_tools::is_managed_tool(&status.name),
            active_version: managed.and_then(|tool| tool.active_version.clone()),
            compatibility: status.compatibility.into(),
            name: status.name,
            path: status.path,
            version: status.version,
            source: status.source,
        }
    };
    Ok(Json(crate::dto::MediaStatusResponse {
        ytdlp: convert(ytdlp),
        ffmpeg: convert(ffmpeg_status),
        ffprobe: convert(ffprobe),
        unrar: convert(unrar),
        seven_zip: convert(seven_zip),
        rclone: convert(rclone),
        gallery_dl: convert(gallery_dl),
        streamlink: convert(streamlink),
        apprise: convert(apprise),
        vendor_directories: rd_core::vendor_directories(vendor)
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        hosts: settings.media_hosts,
    }))
}

/// Figures the tray needs to say whether anything is running.
///
/// Same reasoning as the capture event stream (`event_stream::capture_events`): a capture token
/// is a narrow credential, so this answers with counts and byte totals and nothing that names a
/// file, a folder or an account.
#[utoipa::path(get, path = "/api/v1/capture/summary", tag = "capture", responses((status = 200, body = crate::dto::CaptureSummaryResponse)))]
pub async fn capture_summary(
    State(state): State<AppState>,
) -> Result<Json<crate::dto::CaptureSummaryResponse>, ApiError> {
    let downloads = state.database.list_downloads().await?;
    Ok(Json(capture_figures(
        &downloads,
        &state.scheduler.transfer_rates(),
    )))
}

/// Everything the capture summary says, once the queue has been read.
///
/// Split off from the handler so the counting boundary and the remaining time can be tested
/// without standing up a scheduler. The estimate is read out of the same `queue_rate()` value
/// the rate comes from; there is deliberately no second call and no second formula here.
pub(crate) fn capture_figures(
    downloads: &[rd_core::DownloadFile],
    rates: &std::collections::HashMap<rd_core::DownloadId, u64>,
) -> crate::dto::CaptureSummaryResponse {
    use rd_core::DownloadState;
    let count = |predicate: fn(DownloadState) -> bool| {
        u32::try_from(
            downloads
                .iter()
                .filter(|download| predicate(download.state))
                .count(),
        )
        .unwrap_or(u32::MAX)
    };
    let unfinished = |download: &rd_core::DownloadFile| {
        !matches!(
            download.state,
            DownloadState::Completed
                | DownloadState::Cancelled
                | DownloadState::Failed
                | DownloadState::Skipped
        )
    };
    let committed_bytes = downloads
        .iter()
        .filter(|download| unfinished(download))
        .map(|download| download.committed_bytes.get())
        .fold(0u64, u64::saturating_add);
    let total_bytes = downloads
        .iter()
        .filter(|download| unfinished(download))
        .filter_map(|download| download.total_bytes.map(rd_core::ByteCount::get))
        .fold(0u64, u64::saturating_add);
    // One reading of the queue's speed, and both figures it yields. `active` and `queued`
    // count `Downloading` and `Queued` alone, while the estimate follows `is_transferring`
    // and so also covers an entry resolving or waiting on a retry: the remaining time speaks
    // for the whole queue, the counts for two of its states (RD-108-01).
    let queue = crate::download_handlers::queue_rate(rates, downloads);
    crate::dto::CaptureSummaryResponse {
        active: count(|state| matches!(state, DownloadState::Downloading)),
        queued: count(|state| matches!(state, DownloadState::Queued)),
        failed: count(|state| matches!(state, DownloadState::Failed)),
        committed_bytes: rd_core::ByteCount::new(committed_bytes).unwrap_or_default(),
        total_bytes: rd_core::ByteCount::new(total_bytes).unwrap_or_default(),
        // Counts, like everything else here: they say how much and how fast, never what.
        bytes_per_second: queue.bytes_per_second,
        eta_seconds: queue.eta_seconds,
    }
}

#[cfg(test)]
mod capture_summary_tests {
    use super::capture_figures;
    use crate::download_handlers::tests::{file, rates};
    use rd_core::DownloadState;

    /// The tray used to be handed the rate out of `queue_rate` while the estimate sitting in
    /// the same value was dropped. Both travel now, and both come from that one call.
    #[test]
    fn the_capture_summary_carries_the_queue_estimate_beside_the_rate() {
        let running = file(DownloadState::Downloading, 500, Some(1_000));
        let waiting = file(DownloadState::Queued, 0, Some(1_000));
        let table = rates(&[(&running, 250)]);

        let figures = capture_figures(&[running.clone(), waiting], &table);

        assert_eq!(figures.active, 1);
        assert_eq!(figures.queued, 1);
        assert_eq!(figures.bytes_per_second, 250);
        assert_eq!(figures.eta_seconds, Some(6));
    }

    /// An unknown size, a rate of zero and a paused transfer each leave the field empty
    /// rather than putting an invented number in front of the user (RD-104-02).
    #[test]
    fn no_honest_estimate_means_no_field_rather_than_a_placeholder() {
        let running = file(DownloadState::Downloading, 500, Some(1_000));
        let sizeless = file(DownloadState::Queued, 0, None);
        let table = rates(&[(&running, 250)]);
        assert_eq!(
            capture_figures(&[running.clone(), sizeless], &table).eta_seconds,
            None,
            "one entry of unknown size makes the sum a lower bound"
        );

        assert_eq!(
            capture_figures(
                std::slice::from_ref(&running),
                &std::collections::HashMap::new()
            )
            .eta_seconds,
            None,
            "a rate of zero is the service saying nothing is moving"
        );

        let paused = file(DownloadState::Paused, 100, Some(9_000_000));
        let figures = capture_figures(&[paused], &std::collections::HashMap::new());
        assert_eq!(figures.eta_seconds, None);
        assert_eq!(figures.bytes_per_second, 0);
    }

    /// The counts and the estimate draw their line differently, and this is the job that says
    /// so out loud: a resolving entry is in the remaining time and in neither count.
    #[test]
    fn the_estimate_spans_the_queue_while_the_counts_name_two_states() {
        let running = file(DownloadState::Downloading, 0, Some(1_000));
        let resolving = file(DownloadState::Resolving, 0, Some(1_000));
        let table = rates(&[(&running, 100)]);

        let figures = capture_figures(&[running.clone(), resolving], &table);

        assert_eq!(figures.active, 1);
        assert_eq!(figures.queued, 0);
        assert_eq!(
            figures.eta_seconds,
            Some(20),
            "the resolving entry's bytes are in the estimate although no count names it"
        );
    }

    /// A capture token is scoped for handing links in. The summary stays figures only: the
    /// serialized response must name no file, no path and no account.
    #[test]
    fn the_response_names_nothing_that_is_being_downloaded() {
        let running = file(DownloadState::Downloading, 500, Some(1_000));
        let table = rates(&[(&running, 250)]);

        let body = serde_json::to_string(&capture_figures(std::slice::from_ref(&running), &table))
            .expect("serialize the summary");

        assert!(!body.contains("a.bin"), "no file name: {body}");
        assert!(!body.contains("example.test"), "no source: {body}");
        assert!(!body.contains(&running.id.to_string()), "no id: {body}");
        assert!(
            body.contains("\"eta_seconds\":2"),
            "the estimate travels: {body}"
        );
    }
}
