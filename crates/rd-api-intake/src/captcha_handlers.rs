//! Captcha queue and solver configuration.
//!
//! A resolver that hits a captcha it cannot solve parks the download and waits here. Image
//! and click-point captchas are offered to the user in the web interface — the first answered
//! with text, the second with a spot in the picture (RD-110-15); widget captchas (reCAPTCHA,
//! hCaptcha, Turnstile) are bound to the hoster's domain and cannot be rendered here at all,
//! so they are answered by a solver service or by the browser extension in the person's real
//! browser (RD-108-02). The capture-scoped half of this file is that second path: it lists
//! the waiting widgets and takes the token back. A CutCaptcha never reaches this queue: only
//! a solver service answers one.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use rd_api_core::input_checks::{TextLimit, required_text};
use rd_captcha::{
    CaptchaAnswerers, CaptchaSettings, PendingCaptcha, PendingWidget, SolverKind, SubmitOutcome,
};
use rd_core::{Failure, FailureKind};
use rd_plugin_api::ClickPoint;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState, dto::MessageResponse};

/// Longest accepted captcha answer; real ones are a few characters or a widget token.
const MAX_TOKEN_BYTES: usize = 8 * 1024;

/// Largest accepted click coordinate; a captcha image is a few hundred pixels across, and
/// the host refuses one above 512 KiB, so anything beyond this is not a click in a picture.
const MAX_CLICK_COORDINATE: u32 = 16_384;

/// Longest accepted solver API key; real ones are around 32 characters.
const MAX_API_KEY_BYTES: usize = 512;

/// Solver configuration as exposed by the API; the API key itself is never returned.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CaptchaConfigResponse {
    pub solver: SolverKind,
    pub endpoint: String,
    pub has_api_key: bool,
    pub manual_enabled: bool,
    pub manual_timeout_seconds: u64,
    /// Whether the endpoint may be plain http, which sends the API key unencrypted
    /// (RD-1420-01); off unless switched on.
    pub allow_plain_http: bool,
}

/// Partial update of the solver configuration.
#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct UpdateCaptchaConfigRequest {
    #[serde(default)]
    pub solver: Option<SolverKind>,
    #[serde(default)]
    pub endpoint: Option<String>,
    /// New API key; stored in the secret store, never echoed back.
    #[serde(default)]
    #[schema(write_only)]
    pub api_key: Option<String>,
    /// Removes the stored API key.
    #[serde(default)]
    pub clear_api_key: bool,
    #[serde(default)]
    pub manual_enabled: Option<bool>,
    #[serde(default)]
    pub manual_timeout_seconds: Option<u64>,
    /// Allows an `http://` endpoint, for a solver on this machine or in the own network; the
    /// endpoint stored or sent along is checked against the new value.
    #[serde(default)]
    pub allow_plain_http: Option<bool>,
}

/// The answer to one waiting captcha.
#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct SolveCaptchaRequest {
    pub token: String,
}

/// The spot clicked in a click-point captcha, in pixels of the image as the hoster served
/// it — not of the rendering, which the interface may have scaled.
#[derive(Clone, Copy, Debug, Deserialize, ToSchema)]
pub struct ClickCaptchaRequest {
    pub x: u32,
    pub y: u32,
}

/// Credentials to try against the solver service; both fall back to what is stored, so a
/// key can be checked before it is saved.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct TestCaptchaSolverRequest {
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Key to test instead of the stored one; never logged and never echoed back.
    #[serde(default)]
    #[schema(write_only)]
    pub api_key: Option<String>,
    /// Plain http allowed for this test instead of the stored setting, as the form says.
    #[serde(default)]
    pub allow_plain_http: Option<bool>,
}

/// Proof that the solver service answered and accepted the key.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CaptchaSolverTestResponse {
    /// Credit left on the account, as reported by the service.
    pub balance: f64,
}

#[utoipa::path(get, path = "/api/v1/captchas", tag = "captcha", responses((status = 200, body = [PendingCaptcha])))]
pub async fn list_captchas(State(state): State<AppState>) -> Json<Vec<PendingCaptcha>> {
    Json(state.scheduler.captcha().pending())
}

#[utoipa::path(post, path = "/api/v1/captchas/{id}/solution", tag = "captcha", params(("id" = rd_core::CaptchaId, Path)), request_body = SolveCaptchaRequest, responses((status = 200, body = MessageResponse), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)))]
pub async fn solve_captcha(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CaptchaId>,
    Json(request): Json<SolveCaptchaRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let token = captcha_token(&request.token)?;
    submitted(state.scheduler.captcha().submit(id, token))
}

/// Answers a click-point captcha with the spot the person clicked (RD-110-15).
///
/// A separate route rather than a second field on the token request, because the two answer
/// shapes exclude each other: a challenge takes exactly one of them, and the queue refuses
/// the other with `captcha.answer_shape` while the challenge keeps waiting.
#[utoipa::path(post, path = "/api/v1/captchas/{id}/click", tag = "captcha", params(("id" = rd_core::CaptchaId, Path)), request_body = ClickCaptchaRequest, responses((status = 200, body = MessageResponse), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)))]
pub async fn click_captcha(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CaptchaId>,
    Json(request): Json<ClickCaptchaRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    if request.x > MAX_CLICK_COORDINATE || request.y > MAX_CLICK_COORDINATE {
        return Err(ApiError::bad_request(
            "captcha.point_invalid",
            "The clicked spot lies outside any captcha image",
        ));
    }
    let point = ClickPoint {
        x: request.x,
        y: request.y,
    };
    submitted(state.scheduler.captcha().submit_click(id, point))
}

/// Turns the queue's verdict on a person's answer into the route's reply.
fn submitted(outcome: SubmitOutcome) -> Result<Json<MessageResponse>, ApiError> {
    match outcome {
        SubmitOutcome::Delivered => Ok(Json(MessageResponse::new(
            "captcha.solved",
            "Captcha answer submitted",
        ))),
        SubmitOutcome::NotWaiting => Err(not_waiting()),
        // The challenge is still waiting: configuring a solver is the way out, so this is a
        // rejected answer, not a vanished captcha.
        SubmitOutcome::WidgetNeedsSolver => Err(ApiError::bad_request(
            "captcha.widget_needs_solver",
            "This hoster uses a captcha widget: answer it in your browser through the \
             rDownloader extension, or configure a solver service",
        )),
        // Still waiting too, for the answer it actually takes.
        SubmitOutcome::WrongAnswerShape => Err(ApiError::bad_request(
            "captcha.answer_shape",
            "This captcha takes a different kind of answer: a click for a click-point \
             captcha, text for an image captcha",
        )),
    }
}

#[utoipa::path(post, path = "/api/v1/captchas/{id}/skip", tag = "captcha", params(("id" = rd_core::CaptchaId, Path)), responses((status = 200, body = MessageResponse), (status = 404, body = crate::error::ErrorBody)))]
pub async fn skip_captcha(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CaptchaId>,
) -> Result<Json<MessageResponse>, ApiError> {
    if !state.scheduler.captcha().skip(id) {
        return Err(not_waiting());
    }
    Ok(Json(MessageResponse::new(
        "captcha.skipped",
        "Captcha declined",
    )))
}

/// The widget token a browser harvested from the hoster's page — the extension's tab or the
/// desktop agent's WebView.
///
/// Deliberately without `Debug`: this carries a live credential, and the one way it could
/// reach a log is a struct that knows how to print itself.
#[derive(Deserialize, ToSchema)]
pub struct AnswerWidgetCaptchaRequest {
    /// Single-use and short-lived; never stored, never logged, never echoed back.
    #[schema(write_only)]
    pub token: String,
}

/// Which kind of client is polling the waiting widgets.
///
/// The desktop agent and the browser extension read the same list with the same kind of
/// token; the extension names itself so the web interface can tell a person whether one is
/// connected (RD-108-02). Not naming oneself is allowed and changes nothing — an agent older
/// than this parameter keeps working.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptureCaptchaClient {
    BrowserExtension,
}

/// Query of [`list_capture_captchas`].
#[derive(Clone, Copy, Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct ListCaptureCaptchasQuery {
    /// The polling client, when it wants its presence known.
    #[serde(default)]
    pub client: Option<CaptureCaptchaClient>,
}

#[utoipa::path(get, path = "/api/v1/capture/captchas", tag = "capture", params(ListCaptureCaptchasQuery), responses((status = 200, body = [PendingWidget]), (status = 401)))]
pub async fn list_capture_captchas(
    State(state): State<AppState>,
    Query(query): Query<ListCaptureCaptchasQuery>,
) -> Json<Vec<PendingWidget>> {
    let captcha = state.scheduler.captcha();
    if query.client == Some(CaptureCaptchaClient::BrowserExtension) {
        captcha.note_browser_extension();
    }
    Json(captcha.pending_widgets())
}

/// Who is around to answer a widget captcha, for the prompt in the web interface.
///
/// The interface runs in a browser of its own and cannot see whether an extension is
/// installed anywhere; the service can, because the extension's poll names itself. This is
/// the session-scoped surface for that fact — a timestamp and a verdict, no token.
#[utoipa::path(get, path = "/api/v1/captcha-answerers", tag = "captcha", responses((status = 200, body = CaptchaAnswerers)))]
pub async fn get_captcha_answerers(State(state): State<AppState>) -> Json<CaptchaAnswerers> {
    Json(state.scheduler.captcha().answerers())
}

/// Takes a widget token produced on the hoster's own page — in the extension's tab or in
/// the desktop agent's WebView.
///
/// Separate from [`solve_captcha`] because the *source* is what makes the answer plausible:
/// a widget token can only have come from the hoster's own page, so typing one stays refused
/// while this route accepts one. The token goes straight to the waiting resolver — it is
/// neither persisted nor announced over SSE, and nothing here logs it.
#[utoipa::path(post, path = "/api/v1/capture/captchas/{id}/token", tag = "capture", params(("id" = rd_core::CaptchaId, Path)), request_body = AnswerWidgetCaptchaRequest, responses((status = 200, body = MessageResponse), (status = 400, body = crate::error::ErrorBody), (status = 401), (status = 404, body = crate::error::ErrorBody)))]
pub async fn answer_capture_captcha(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CaptchaId>,
    Json(request): Json<AnswerWidgetCaptchaRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let token = captcha_token(&request.token)?;
    match state.scheduler.captcha().submit_from_browser(id, token) {
        SubmitOutcome::Delivered => Ok(Json(MessageResponse::new(
            "captcha.solved",
            "Captcha answer submitted",
        ))),
        // A browser token answers every widget, so the widget refusal is unreachable here;
        // it is mapped rather than ignored so the route keeps answering with a stable code
        // whatever the queue decides later. A token for a picture is the wrong shape.
        SubmitOutcome::NotWaiting | SubmitOutcome::WidgetNeedsSolver => Err(not_waiting()),
        SubmitOutcome::WrongAnswerShape => Err(ApiError::bad_request(
            "captcha.answer_shape",
            "This captcha takes a different kind of answer",
        )),
    }
}

/// The person closed the agent's window or the extension's tab: decline the captcha, exactly
/// as the web interface does, so the download fails with `captcha.skipped` rather than
/// waiting out its timeout.
#[utoipa::path(post, path = "/api/v1/capture/captchas/{id}/skip", tag = "capture", params(("id" = rd_core::CaptchaId, Path)), responses((status = 200, body = MessageResponse), (status = 401), (status = 404, body = crate::error::ErrorBody)))]
pub async fn skip_capture_captcha(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CaptchaId>,
) -> Result<Json<MessageResponse>, ApiError> {
    skip_captcha(State(state), Path(id)).await
}

/// The extension opened the hoster's page for a widget and the page showed none (RD-120-45):
/// end the wait with `captcha.page_without_widget`, so the sign-in or download that asked for
/// it fails with a reason a person can act on instead of running into its timeout.
///
/// Grants a capture token nothing it did not have: it could already decline the same captcha.
#[utoipa::path(post, path = "/api/v1/capture/captchas/{id}/no-widget", tag = "capture", params(("id" = rd_core::CaptchaId, Path)), responses((status = 200, body = MessageResponse), (status = 400, body = crate::error::ErrorBody), (status = 401), (status = 404, body = crate::error::ErrorBody)))]
pub async fn report_capture_captcha_without_widget(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CaptchaId>,
) -> Result<Json<MessageResponse>, ApiError> {
    match state.scheduler.captcha().report_page_without_widget(id) {
        SubmitOutcome::Delivered => Ok(Json(MessageResponse::new(
            "captcha.page_without_widget_reported",
            "The page showed no captcha; the waiting sign-in was ended",
        ))),
        SubmitOutcome::WrongAnswerShape | SubmitOutcome::WidgetNeedsSolver => {
            Err(ApiError::bad_request(
                "captcha.not_a_widget",
                "Only a widget captcha is opened in a browser",
            ))
        }
        SubmitOutcome::NotWaiting => Err(not_waiting()),
    }
}

#[utoipa::path(get, path = "/api/v1/captcha-config", tag = "captcha", responses((status = 200, body = CaptchaConfigResponse)))]
pub async fn get_captcha_config(State(state): State<AppState>) -> Json<CaptchaConfigResponse> {
    Json(config_response(state.scheduler.captcha().settings().await))
}

/// The stored configuration as the API shows it, without the key's reference.
fn config_response(settings: CaptchaSettings) -> CaptchaConfigResponse {
    CaptchaConfigResponse {
        solver: settings.solver,
        endpoint: settings.endpoint,
        has_api_key: settings.api_key_ref.is_some(),
        manual_enabled: settings.manual_enabled,
        manual_timeout_seconds: settings.manual_timeout_seconds,
        allow_plain_http: settings.allow_plain_http,
    }
}

#[utoipa::path(put, path = "/api/v1/captcha-config", tag = "captcha", request_body = UpdateCaptchaConfigRequest, responses((status = 200, body = CaptchaConfigResponse), (status = 400, body = crate::error::ErrorBody)))]
pub async fn update_captcha_config(
    State(state): State<AppState>,
    Json(request): Json<UpdateCaptchaConfigRequest>,
) -> Result<Json<CaptchaConfigResponse>, ApiError> {
    let current = state.scheduler.captcha().settings().await;
    // The endpoint that ends up stored is held to the setting that ends up stored: switching
    // plain http off under an http endpoint is refused too (RD-1420-01).
    let allow_plain_http = request.allow_plain_http.unwrap_or(current.allow_plain_http);
    let endpoint = validated_endpoint(
        request.endpoint.as_deref().unwrap_or(&current.endpoint),
        allow_plain_http,
    )?;
    let previous_key_ref = current.api_key_ref.clone();
    let api_key_ref = match request.api_key {
        Some(value) => Some(state.secrets.put_string(validated_api_key(value)?).await?),
        None if request.clear_api_key => None,
        None => previous_key_ref.clone(),
    };
    let settings = CaptchaSettings {
        solver: request.solver.unwrap_or(current.solver),
        endpoint,
        api_key_ref: api_key_ref.clone(),
        manual_enabled: request.manual_enabled.unwrap_or(current.manual_enabled),
        manual_timeout_seconds: request
            .manual_timeout_seconds
            .unwrap_or(current.manual_timeout_seconds),
        allow_plain_http,
    }
    .sanitized();
    state
        .database
        .set_setting(
            rd_captcha::SETTINGS_KEY.to_owned(),
            serde_json::to_value(&settings).map_err(anyhow::Error::new)?,
        )
        .await?;
    // The old key is unreachable once the settings row no longer references it.
    if previous_key_ref != api_key_ref
        && let Some(stale) = previous_key_ref
        && let Err(error) = state.secrets.remove(&stale).await
    {
        tracing::warn!(%error, "previous captcha solver key was not removed");
    }
    Ok(Json(config_response(settings)))
}

#[utoipa::path(post, path = "/api/v1/captcha-config/test", tag = "captcha", request_body = TestCaptchaSolverRequest, responses((status = 200, body = CaptchaSolverTestResponse), (status = 400, body = crate::error::ErrorBody), (status = 502, body = crate::error::ErrorBody)))]
pub async fn test_captcha_solver(
    State(state): State<AppState>,
    Json(request): Json<TestCaptchaSolverRequest>,
) -> Result<Json<CaptchaSolverTestResponse>, ApiError> {
    // A typed endpoint is checked here against the form's setting, or the stored one; the
    // broker checks the endpoint it ends up with against the same.
    let allow_plain_http = match request.allow_plain_http {
        Some(allowed) => allowed,
        None => state.scheduler.captcha().settings().await.allow_plain_http,
    };
    let endpoint = match request.endpoint {
        Some(value) => Some(validated_endpoint(&value, allow_plain_http)?),
        None => None,
    };
    let api_key = match request.api_key {
        Some(value) => Some(validated_api_key(value)?),
        None => None,
    };
    let balance = state
        .scheduler
        .captcha()
        .test_solver_with(endpoint, api_key, request.allow_plain_http)
        .await
        .map_err(solver_unreachable)?;
    Ok(Json(CaptchaSolverTestResponse { balance }))
}

/// Translates a solver failure while keeping its stable code, so the UI can tell a rejected
/// key from a service that is merely down. The key itself is in neither.
fn solver_unreachable(failure: Failure) -> ApiError {
    let code = match failure.code.as_deref() {
        Some("captcha.solver_key_missing") => "captcha.solver_key_missing",
        Some("captcha.solver_timeout") => "captcha.solver_timeout",
        Some(rd_captcha::CODE_ENDPOINT_PLAIN_HTTP) => rd_captcha::CODE_ENDPOINT_PLAIN_HTTP,
        Some(rd_captcha::CODE_ENDPOINT_INVALID) => rd_captcha::CODE_ENDPOINT_INVALID,
        _ => "captcha.solver_failed",
    };
    // A rejected key or an empty balance is the user's to fix; anything else is the
    // service failing us.
    let mut error = if failure.category == FailureKind::Permanent {
        ApiError::bad_request(code, failure.message)
    } else {
        ApiError::bad_gateway(code, failure.message)
    };
    for (key, value) in failure.params {
        error = error.with_param(&key, value);
    }
    error
}

fn validated_api_key(value: String) -> Result<String, ApiError> {
    required_text(
        &value,
        TextLimit::Bytes(MAX_API_KEY_BYTES),
        "captcha.api_key_invalid",
        "The solver API key is empty or too long",
    )
}

/// A typed or a browser answer: trimmed, present, and at most [`MAX_TOKEN_BYTES`].
fn captcha_token(value: &str) -> Result<String, ApiError> {
    required_text(
        value,
        TextLimit::Bytes(MAX_TOKEN_BYTES),
        "captcha.token_invalid",
        "The captcha answer is empty or too long",
    )
}

/// Solver endpoints are contacted with the user's API key, so only absolute HTTPS URLs are
/// accepted — a typo must not send the key over plain HTTP — and an http one only where the
/// configuration allows it, with a code of its own (`rd_captcha::checked_endpoint`).
fn validated_endpoint(value: &str, allow_plain_http: bool) -> Result<String, ApiError> {
    rd_captcha::checked_endpoint(value, allow_plain_http)
        .map_err(|refusal| ApiError::bad_request(refusal.code(), refusal.message()))
}

fn not_waiting() -> ApiError {
    ApiError::not_found(
        "captcha.not_waiting",
        "This captcha is no longer waiting for an answer",
    )
}

#[cfg(test)]
#[path = "captcha_handlers_tests.rs"]
mod tests;
