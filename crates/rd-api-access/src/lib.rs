//! Who may use the service (RD-160-06): sign-in and setup, sessions, passwords, passkeys and
//! second factors, signing in through an identity provider, API and capture tokens, the audit log,
//! provider sign-in flows, auth profiles and the browser sessions the extension hands over.

#![warn(unreachable_pub)]

pub mod api_tokens;
pub mod audit_dto;
pub mod audit_handlers;
pub mod auth_flow_handlers;
pub mod auth_profile_handlers;
pub mod browser_session_handlers;
pub mod login_handlers;
pub mod mfa_handlers;
pub mod oidc_handlers;
pub mod oidc_settings_handlers;
pub mod passkey_handlers;
pub mod password_handlers;
pub mod password_login_handlers;
pub mod password_reset_handlers;
pub mod session_handlers;
pub mod setup_handlers;
mod step_up;

// The modules of the crates below, at this crate's root, so that a module here names them as
// `crate::…` exactly as it did while the HTTP surface was one crate (RD-160-06).
use rd_api_core::{
    ApiError, AppState, AuthService, audit, auth, auth_flow_service, browser_session, client,
    config_fields, dto, error, error_codes, hosters, scope_policy,
};
