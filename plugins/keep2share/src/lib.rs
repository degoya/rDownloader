//! Keep2Share resolver: JSON API (`k2s.cc/api/v2`), username + account password. Every flow
//! logs in per invocation (`POST /login`) — the resulting `auth_token` is a plain per-invocation
//! value, never a secret-store secret and never cached (WASM is stateless). This is the first
//! plugin to exercise the host's JSON-body `{{username}}`/`{{secret:...}}` expansion (Task 2):
//! the login request is a JSON body carrying the literal template markers, which the host
//! expands and JSON-string-escapes before the request leaves the process.

pub(crate) mod api;
mod messages;
mod resolver;

/// This plugin's own packaging manifest: the single authority for its identity, domains and
/// capability grants. The native build reads it from here, the component build gets the same
/// file out of the package the host installed, so neither can describe the plugin differently.
#[cfg(not(target_arch = "wasm32"))]
pub const MANIFEST: &str = include_str!("../manifest.toml");

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(target_arch = "wasm32")]
mod guest;

/// `rd-provider-registry`'s `keep2share` row: `secret_reference`.
pub(crate) const PASSWORD_REFERENCE: &str = "keep2share_password";

#[cfg(not(target_arch = "wasm32"))]
pub use native::Keep2ShareResolver;
