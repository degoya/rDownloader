//! 1fichier resolver: a Bearer-authenticated JSON API (`api.1fichier.com`) for an account with a
//! premium API key, and an account-less website flow for links without one. The API path never
//! runs without the key; the free path never sends it.

pub(crate) mod api;
mod messages;
mod page;
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

/// `rd-provider-registry`'s `1fichier` row: `secret_reference`.
pub(crate) const API_KEY_REFERENCE: &str = "onefichier_api_key";

#[cfg(not(target_arch = "wasm32"))]
pub use native::OneFichierResolver;
