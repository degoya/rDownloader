//! rd-api integration tests: who may do what: tokens, logins, sessions, second factors, scopes and the audit trail.
//!
//! One test binary per subject, each suite a module of it (RD-150-10). Every binary links the
//! whole service, and one binary per file meant 57 links of ~550 MB each. A new suite is a
//! module here and a row in `scripts/lib/rd-api-tests.map`, which selects suites by these
//! module names.

#[path = "../common/mod.rs"]
mod common;

mod api_tokens;
mod audit;
mod auth;
mod auth_profiles;
mod browser_session;
mod cross_site;
mod grant_ceiling;
mod host_check;
mod media_cookie_profile;
mod mfa;
mod oauth_callback;
mod oidc;
mod passkeys;
mod password_reset;
mod public_surface;
mod reverse_proxy;
mod scope_matrix;
mod sessions;
mod sign_in_doors;
mod step_up;
mod stream_revocation;
mod token_expiry;
