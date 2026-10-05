//! The WebAssembly bindings of the OAuth world, generated once for every OAuth plugin.
//!
//! An OAuth guest is its provider's own flow — endpoints, scopes, how a refusal reads — so there
//! is no shared adapter to write as there is for the resolvers in `plugin_guest`. What was the
//! same in every one of them is the glue underneath: the generated bindings and the request
//! pieces every sign-in builds, the unguessable values it draws and the reading of a token
//! endpoint's answer ([`token`], RD-1110-04). They live here, so an OAuth plugin's `guest.rs`
//! imports them, implements [`Guest`] and ends in [`oauth_plugin!`]. The component that comes
//! out exports the same world under the same names; the host cannot tell the difference.

#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "oauth-plugin",
    // The bindings live here rather than in each plugin, so the export macro has to be usable
    // from another crate and needs a name that says what it exports.
    pub_export_macro: true,
    export_macro_name: "export_oauth",
});

pub use exports::rdownloader::plugin::oauth::{
    AuthorizationRequest, DeviceAuthorization, Guest, TokenOutcome,
};
pub use rdownloader::plugin::{credentials, host, http, types};

pub mod token;

use http::{RequestHeader, RequestQuery};
use plugin_common::pkce;
use types::{Failure, FailureKind};

/// A form body's fields, each taken literally: a `{{secret:…}}` marker in a value is the host's
/// to expand, nothing here builds one.
#[must_use]
pub fn form(pairs: &[(&str, &str)]) -> Vec<RequestQuery> {
    pairs
        .iter()
        .map(|(name, value)| RequestQuery {
            name: (*name).to_owned(),
            value_template: (*value).to_owned(),
        })
        .collect()
}

/// The one header every token endpoint is asked with.
#[must_use]
pub fn accept_json() -> Vec<RequestHeader> {
    vec![RequestHeader {
        name: "Accept".to_owned(),
        value_template: "application/json".to_owned(),
    }]
}

/// The `Retry-After` a provider sent, if it sent one.
#[must_use]
pub fn retry_after(headers: &[(String, String)]) -> Option<String> {
    plugin_common::http::header(headers, "retry-after").map(str::to_owned)
}

/// A value nobody can recompute, for a PKCE verifier or a `state`: the host's random bytes,
/// base64url-encoded.
///
/// An empty answer means the host refused, and a sign-in is failed rather than continued with a
/// value the plugin made up: `<slug>.no_entropy`, under the slug of the plugin asking.
///
/// # Errors
///
/// The `no_entropy` failure when the host supplied no randomness.
pub fn unguessable_value(slug: &str) -> Result<String, Failure> {
    let random = host::random_bytes(pkce::VERIFIER_BYTES as u32);
    pkce::verifier(&random).ok_or_else(|| Failure {
        category: FailureKind::Permanent,
        message: "the host did not supply the randomness this sign-in needs".to_owned(),
        code: Some(format!("{slug}.no_entropy")),
        params: Vec::new(),
    })
}

/// Exports an OAuth plugin: `$component` implements [`Guest`].
#[macro_export]
macro_rules! oauth_plugin {
    ($component:ident) => {
        $crate::export_oauth!($component with_types_in $crate);
    };
}
