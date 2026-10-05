//! Authentication profiles, API tokens, second factors and the identity provider, remote
//! credentials, object storage profiles and request replay routes.

use axum::{
    Router,
    routing::{delete, get, post},
};
use utoipa::OpenApi;

use crate::{
    AppState, api_tokens, auth_profile_handlers, mfa_handlers, object_storage_handlers,
    oidc_handlers, oidc_settings_handlers, passkey_handlers, password_handlers,
    password_login_handlers, password_reset_handlers, remote_handlers, remote_listing_handlers,
    replay_handlers, session_handlers,
};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .merge(account_security_routes())
        .merge(credential_routes())
}

/// Request replay, API tokens, second factors, the identity provider, passwords and sessions.
fn account_security_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/collector/candidates/{id}/replay-preview",
            get(replay_handlers::replay_preview),
        )
        .route(
            "/api/v1/collector/candidates/{id}/replay-consent",
            post(replay_handlers::grant_replay_consent)
                .delete(replay_handlers::revoke_replay_consent),
        )
        .route(
            "/api/v1/api-tokens",
            get(api_tokens::list_api_tokens).post(api_tokens::pair_api_token),
        )
        // Registered before the `{id}` route so `scopes` is not swallowed as an id.
        .route(
            "/api/v1/api-tokens/scopes",
            get(api_tokens::list_token_scopes),
        )
        .route(
            "/api/v1/api-tokens/{id}",
            delete(api_tokens::revoke_api_token).patch(api_tokens::update_api_token_scopes),
        )
        .route("/api/v1/mfa", get(mfa_handlers::mfa_status))
        .route("/api/v1/mfa/totp", post(mfa_handlers::enrol_totp))
        .route(
            "/api/v1/mfa/totp/{id}/confirm",
            post(mfa_handlers::confirm_totp),
        )
        .route("/api/v1/mfa/passkey", post(passkey_handlers::enrol_passkey))
        .route(
            "/api/v1/mfa/passkey/confirm",
            post(passkey_handlers::confirm_passkey),
        )
        .route(
            "/api/v1/mfa/credentials/{id}",
            delete(mfa_handlers::delete_credential),
        )
        .route("/api/v1/mfa/disable", post(mfa_handlers::disable_mfa))
        // The identity provider (RD-190-15): configuring it, binding the administrator's
        // identity, and the password sign-in switch. Each takes a session and the password
        // except `password-login/on`, which only the local control token opens.
        .route(
            "/api/v1/auth/oidc",
            get(oidc_settings_handlers::get_oidc_settings)
                .put(oidc_settings_handlers::put_oidc_settings)
                .delete(oidc_settings_handlers::delete_oidc_settings),
        )
        .route(
            "/api/v1/auth/oidc/link",
            post(oidc_settings_handlers::link_oidc_identity),
        )
        .route(
            "/api/v1/auth/oidc/identity",
            delete(oidc_settings_handlers::unlink_oidc_identity),
        )
        .route(
            "/api/v1/auth/password-login/off",
            post(password_login_handlers::switch_password_login_off),
        )
        .route(
            "/api/v1/auth/password-login/on",
            post(password_login_handlers::switch_password_login_on),
        )
        .route(
            "/api/v1/mfa/recovery-codes",
            post(mfa_handlers::regenerate_recovery_codes),
        )
        // Behind the session layer, unlike `/auth/login` and `/auth/setup`: changing the
        // password is something an administrator who is already in does, and the current
        // password on top of that is what makes it a change rather than a second way in.
        .route(
            "/api/v1/auth/password",
            post(password_handlers::change_password),
        )
        // Without the current password, and therefore only for the local control token: the
        // reset on the host (RD-190-24). The handler refuses every other credential.
        .route(
            "/api/v1/auth/password/reset",
            post(password_reset_handlers::reset_password_locally),
        )
        .route("/api/v1/sessions", get(session_handlers::list_sessions))
        .route(
            "/api/v1/sessions/revoke-others",
            post(session_handlers::revoke_other_sessions),
        )
        .route(
            "/api/v1/sessions/{id}",
            delete(session_handlers::revoke_session),
        )
}

/// Authentication profiles, remote credentials, object storage profiles and remote listings.
fn credential_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/auth-profiles",
            get(auth_profile_handlers::list_auth_profiles)
                .post(auth_profile_handlers::create_auth_profile),
        )
        .route(
            "/api/v1/auth-profiles/{id}",
            axum::routing::put(auth_profile_handlers::update_auth_profile)
                .delete(auth_profile_handlers::delete_auth_profile),
        )
        .route(
            "/api/v1/auth-profiles/{id}/enable",
            post(auth_profile_handlers::enable_auth_profile),
        )
        .route(
            "/api/v1/auth-profiles/{id}/disable",
            post(auth_profile_handlers::disable_auth_profile),
        )
        .route(
            "/api/v1/auth-profiles/{id}/test",
            post(auth_profile_handlers::test_auth_profile),
        )
        .route(
            "/api/v1/remote-credentials",
            get(remote_handlers::list_remote_credentials)
                .post(remote_handlers::create_remote_credential),
        )
        // Registered before the `{id}` route so `ssh-hosts` is not swallowed as an id.
        .route(
            "/api/v1/remote-credentials/ssh-hosts",
            get(remote_handlers::list_ssh_host_keys).post(remote_handlers::trust_ssh_host_key),
        )
        .route(
            "/api/v1/remote-credentials/ssh-hosts/{host}/{port}/{algorithm}",
            axum::routing::delete(remote_handlers::forget_ssh_host_key),
        )
        .route(
            "/api/v1/remote-credentials/{id}",
            axum::routing::put(remote_handlers::update_remote_credential)
                .delete(remote_handlers::delete_remote_credential),
        )
        .route(
            "/api/v1/remote-credentials/{id}/test",
            post(remote_handlers::test_remote_credential),
        )
        .route(
            "/api/v1/object-storage/profiles",
            get(object_storage_handlers::list_object_storage_profiles)
                .post(object_storage_handlers::create_object_storage_profile),
        )
        .route(
            "/api/v1/object-storage/profiles/{id}",
            axum::routing::put(object_storage_handlers::update_object_storage_profile)
                .delete(object_storage_handlers::delete_object_storage_profile),
        )
        .route(
            "/api/v1/object-storage/profiles/{id}/test",
            post(object_storage_handlers::test_object_storage_profile),
        )
        .route(
            "/api/v1/collector/candidates/{id}/listing",
            get(remote_listing_handlers::get_candidate_listing),
        )
        .route(
            "/api/v1/collector/candidates/{id}/listing/plan",
            axum::routing::put(remote_listing_handlers::put_candidate_listing_plan),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    api_tokens::pair_api_token,
    api_tokens::list_api_tokens,
    api_tokens::revoke_api_token,
    api_tokens::update_api_token_scopes,
    api_tokens::list_token_scopes,
    mfa_handlers::mfa_status,
    mfa_handlers::enrol_totp,
    mfa_handlers::confirm_totp,
    mfa_handlers::delete_credential,
    mfa_handlers::disable_mfa,
    mfa_handlers::regenerate_recovery_codes,
    passkey_handlers::enrol_passkey,
    passkey_handlers::confirm_passkey,
    passkey_handlers::passkey_challenge,
    passkey_handlers::passkey_login,
    oidc_handlers::oidc_start,
    oidc_handlers::oidc_callback,
    oidc_settings_handlers::get_oidc_settings,
    oidc_settings_handlers::put_oidc_settings,
    oidc_settings_handlers::delete_oidc_settings,
    oidc_settings_handlers::link_oidc_identity,
    oidc_settings_handlers::unlink_oidc_identity,
    password_login_handlers::switch_password_login_off,
    password_login_handlers::switch_password_login_on,
    password_handlers::change_password,
    password_reset_handlers::reset_password_locally,
    session_handlers::list_sessions,
    session_handlers::revoke_session,
    session_handlers::revoke_other_sessions,
    session_handlers::logout,
    replay_handlers::replay_preview,
    replay_handlers::grant_replay_consent,
    replay_handlers::revoke_replay_consent,
    auth_profile_handlers::list_auth_profiles,
    auth_profile_handlers::create_auth_profile,
    auth_profile_handlers::update_auth_profile,
    auth_profile_handlers::enable_auth_profile,
    auth_profile_handlers::disable_auth_profile,
    auth_profile_handlers::delete_auth_profile,
    auth_profile_handlers::test_auth_profile,
    auth_profile_handlers::capture_cookies,
    remote_handlers::list_remote_credentials,
    remote_handlers::create_remote_credential,
    remote_handlers::update_remote_credential,
    remote_handlers::delete_remote_credential,
    remote_handlers::test_remote_credential,
    remote_handlers::list_ssh_host_keys,
    remote_handlers::trust_ssh_host_key,
    remote_handlers::forget_ssh_host_key,
    object_storage_handlers::list_object_storage_profiles,
    object_storage_handlers::create_object_storage_profile,
    object_storage_handlers::update_object_storage_profile,
    object_storage_handlers::delete_object_storage_profile,
    object_storage_handlers::test_object_storage_profile,
    remote_listing_handlers::get_candidate_listing,
    remote_listing_handlers::put_candidate_listing_plan,
))]
pub(crate) struct Doc;
