//! Rapidgator resolver. With an account it uses the pure JSON API (`rapidgator.net/api/v2`),
//! username (login e-mail) + account password: every call issues a fresh `user/login` (JD's
//! `FEATURE.USERNAME_IS_EMAIL`), and the resulting session `token` is a plain per-invocation
//! value, never a secret-store secret and never cached (WASM is stateless).
//!
//! Without an account it runs the website free flow instead ([`page`] + each adapter's `free`
//! submodule): the file page's server-side countdown is started, its reCAPTCHA solved and the
//! countdown waited out through the host, and the resulting download link probed.

pub(crate) mod api;
mod messages;
pub(crate) mod page;
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

/// `rd-provider-registry`'s `rapidgator` row: `secret_reference`.
pub(crate) const PASSWORD_REFERENCE: &str = "rapidgator_password";

#[cfg(not(target_arch = "wasm32"))]
pub use native::RapidgatorResolver;
