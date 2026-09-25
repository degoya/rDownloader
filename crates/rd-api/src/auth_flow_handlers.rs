//! Starting, reading and cancelling a provider sign-in flow (RD-090-13).
//!
//! Three endpoints and nothing more. The flow itself is driven by the service's sweep loop,
//! so the interface reads a state rather than polling a provider — which is also why closing
//! the browser in the middle of a sign-in loses nothing.

use axum::{
    Json,
    extract::{Path, Query, State},
    response::Redirect,
};
use rd_plugin_host::OAuthFlowManifest;
use serde::Deserialize;

use crate::{AppState, error::ApiError};

/// Starts a sign-in flow.
///
/// The two ways this fails are told apart, because they ask different things of the person
/// reading the message: no installed plugin can sign this provider in, which is about what is
/// installed, or the plugin tried and the attempt failed, which is about the provider. Both
/// used to arrive as the first one, so a failing flow reported a plugin that was in fact
/// present and claiming the provider.
#[utoipa::path(post, path = "/api/v1/accounts/{id}/auth/begin", tag = "configuration", params(("id" = rd_core::AccountId, Path)), responses((status = 200, body = rd_core::AuthFlow), (status = 400), (status = 404), (status = 502)))]
pub async fn begin_auth(
    State(state): State<AppState>,
    Path(id): Path<rd_core::AccountId>,
) -> Result<Json<rd_core::AuthFlow>, ApiError> {
    let account = account(&state, id).await?;
    if let Some(entrance) = state
        .auth_flows
        .oauth_providers()
        .await
        .preferred_flow(&account.provider)
    {
        // Which way in is the plugin's to state, not this handler's to guess: a manifest
        // naming both is a provider that offers both, and its first entry is the one its
        // author wants people sent through (RD-106-01).
        //
        // What comes back is the same `AuthFlow` either way -- the address to visit is in
        // `verification_url`, the code to type in `user_code` -- so the caller sees one shape
        // and does not have to know which entrance, or which world, ran it.
        let started = match entrance {
            OAuthFlowManifest::Device => {
                state
                    .auth_flows
                    .begin_oauth_device(id, &account.provider)
                    .await
            }
            OAuthFlowManifest::Redirect => {
                state.auth_flows.begin_oauth(id, &account.provider).await
            }
        };
        let flow = started.map_err(|error| auth_failed(&account.provider, &error))?;
        return Ok(Json(flow));
    }
    if !state
        .auth_flows
        .providers()
        .await
        .supports(&account.provider)
    {
        return Err(ApiError::bad_request(
            "account.auth_unavailable",
            format!(
                "no installed plugin can sign in to {provider}",
                provider = account.provider
            ),
        ));
    }
    let flow = state
        .auth_flows
        .begin(id, &account.provider)
        .await
        .map_err(|error| auth_failed(&account.provider, &error))?;
    Ok(Json(flow))
}

/// The wording a failed start reports, kept in one place now that two worlds can produce one.
///
/// One failure is pulled out ahead of the rest, because it asks something different of the
/// person reading it. Everything else here is about the provider — it refused, it could not be
/// reached — and arrives as a bad gateway quoting what the plugin said. An OAuth provider whose
/// installation has registered no client of its own is not that: nothing is wrong at the
/// provider, the sign-in never left the building, and what is needed is a client id in a form
/// field. So it is a bad request with a code of its own, and the translated text carries the
/// steps rather than the diagnosis (RD-106-04).
fn auth_failed(provider: &str, error: &anyhow::Error) -> ApiError {
    if error
        .downcast_ref::<crate::auth_flow_service::ClientNotConfigured>()
        .is_some()
    {
        tracing::info!(%provider, "a sign-in needs an OAuth client this installation has not registered");
        return ApiError::bad_request(
            "oauth.client_not_configured",
            format!("{provider} needs an OAuth client registered for this installation"),
        )
        .with_param("provider", provider.to_owned());
    }
    let reason = format!("{error:#}");
    tracing::warn!(%provider, reason = %reason, "a sign-in flow could not be started");
    ApiError::bad_gateway("account.auth_failed", reason.clone())
        // Bounded: the wording comes from a plugin talking to a foreign service.
        .with_param("reason", reason.chars().take(200).collect::<String>())
}

/// What a provider appends to the redirect it sends the browser back with.
#[derive(Deserialize)]
pub struct OAuthCallback {
    code: String,
    state: String,
}

/// Where a provider sends the browser once the person has agreed (RD-103-00).
///
/// One fixed address for every provider, because a redirect URI has to be registered with the
/// provider before it is ever used and a per-account path could not be. The `state` is what
/// says which account this belongs to, and looking it up is the check: a callback quoting a
/// value no flow claims matches nothing.
///
/// It always ends in a redirect back into the application, success or not. The outcome is not
/// this response's to report -- it is recorded on the flow, which the accounts view is already
/// watching -- and leaving somebody staring at a JSON body in a browser tab would be a worse
/// answer than putting them back where they started.
#[utoipa::path(get, path = "/api/v1/oauth/callback", tag = "configuration", params(("code" = String, Query), ("state" = String, Query)), responses((status = 303)))]
pub async fn oauth_callback(
    State(state): State<AppState>,
    Query(callback): Query<OAuthCallback>,
) -> Redirect {
    if let Err(error) = state
        .auth_flows
        .complete_oauth(&callback.state, &callback.code)
        .await
    {
        tracing::warn!(reason = %format!("{error:#}"), "an oauth callback could not be completed");
    }
    Redirect::to("/")
}

#[utoipa::path(get, path = "/api/v1/accounts/{id}/auth", tag = "configuration", params(("id" = rd_core::AccountId, Path)), responses((status = 200, body = Option<rd_core::AuthFlow>), (status = 404)))]
pub async fn get_auth(
    State(state): State<AppState>,
    Path(id): Path<rd_core::AccountId>,
) -> Result<Json<Option<rd_core::AuthFlow>>, ApiError> {
    account(&state, id).await?;
    Ok(Json(state.auth_flows.flow(id).await?))
}

#[utoipa::path(delete, path = "/api/v1/accounts/{id}/auth", tag = "configuration", params(("id" = rd_core::AccountId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404)))]
pub async fn cancel_auth(
    State(state): State<AppState>,
    Path(id): Path<rd_core::AccountId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    account(&state, id).await?;
    state.auth_flows.cancel(id).await?;
    Ok(Json(crate::dto::MessageResponse::new(
        "account.auth_cancelled",
        "Sign-in cancelled",
    )))
}

/// The account, or a 404 — so a flow can never be started for one that does not exist.
async fn account(state: &AppState, id: rd_core::AccountId) -> Result<rd_core::Account, ApiError> {
    state
        .database
        .list_accounts()
        .await?
        .into_iter()
        .find(|account| account.id == id)
        .ok_or_else(crate::error_codes::account_not_found)
}

#[cfg(test)]
mod tests {
    use super::auth_failed;

    /// An unregistered OAuth client is reported as a configuration problem, not as a provider
    /// failure (RD-106-04).
    ///
    /// The two ask different things of the person reading them, and they used to arrive as one:
    /// a bad gateway quoting whatever a plugin said about a foreign service. Nothing is wrong at
    /// the provider here — the sign-in never left the building — so it is a bad request under a
    /// code whose translation carries the registration steps.
    #[test]
    fn an_unregistered_oauth_client_is_a_bad_request_and_not_a_provider_failure() {
        let error = anyhow::Error::new(crate::auth_flow_service::ClientNotConfigured);
        let reported = auth_failed("google_drive", &error);
        assert_eq!(reported.code(), "oauth.client_not_configured");

        // Everything else keeps the shape it had.
        let other = anyhow::anyhow!("the provider could not be reached");
        assert_eq!(
            auth_failed("google_drive", &other).code(),
            "account.auth_failed"
        );
    }
}
