//! Nitroflare resolver. With an account it uses the JSON API (`nitroflare.com/api/v2`), premium
//! key only: `getDownloadLink`/`getKeyInfo` additionally require the account's username (its
//! Nitroflare login e-mail; JD's `FEATURE.USERNAME_IS_EMAIL`).
//!
//! Without an account it runs the website free flow instead ([`page`] + each adapter's `free`
//! submodule): the file page's countdown is started, its reCAPTCHA solved and the countdown
//! waited out through the host, and the resulting download link probed.

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

/// `rd-provider-registry`'s `nitroflare` row: `secret_reference`.
pub(crate) const PREMIUM_KEY_REFERENCE: &str = "nitroflare_premium_key";

#[cfg(not(target_arch = "wasm32"))]
pub use native::NitroflareResolver;
