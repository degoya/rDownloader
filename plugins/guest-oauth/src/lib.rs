//! The WebAssembly bindings of the OAuth world, generated once for every OAuth plugin.
//!
//! What was the same in every OAuth guest lives here: the generated bindings and the request
//! pieces every sign-in builds, the unguessable values it draws, the reading of a token
//! endpoint's answer ([`token`], RD-1110-04) and, for a provider with an RFC 6749 token
//! endpoint, the whole flow ([`redirect`], RD-1120-10): such a plugin's `guest.rs` declares a
//! [`redirect::Provider`] and ends in [`redirect_plugin!`]. A provider whose answer reads
//! differently (pCloud, Put.io) implements [`Guest`] itself and ends in [`oauth_plugin!`]. The
//! component that comes out exports the same world under the same names either way; the host
//! cannot tell the difference.

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

pub mod redirect;
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
    pkce::verifier(&random).ok_or_else(|| {
        refuse(
            slug,
            "no_entropy",
            "the host did not supply the randomness this sign-in needs",
            FailureKind::Permanent,
        )
    })
}

/// A failure carrying a stable translation code, `<slug>.<code>`, and nothing a provider wrote.
///
/// The OAuth and auth guests compose their codes from their slug, where `plugin_guest_crawler`
/// and `plugin_guest_remote_job` take whole codes from a catalogue; otherwise the same helper
/// (RD-1120-10, PL-4).
#[must_use]
pub fn refuse(
    slug: &str,
    code: &str,
    message: impl Into<String>,
    category: FailureKind,
) -> Failure {
    Failure {
        category,
        message: message.into(),
        code: Some(format!("{slug}.{code}")),
        params: Vec::new(),
    }
}

/// Exports an OAuth plugin: `$component` implements [`Guest`].
#[macro_export]
macro_rules! oauth_plugin {
    ($component:ident) => {
        $crate::export_oauth!($component with_types_in $crate);
    };
}

/// Exports an OAuth plugin whose whole flow is a [`redirect::Provider`]: defines the component,
/// implements [`Guest`] by handing every call to the provider, and exports it.
#[macro_export]
macro_rules! redirect_plugin {
    ($provider:expr) => {
        struct Component;

        impl $crate::Guest for Component {
            fn begin(
                account_id: String,
                _credential_ref: Option<String>,
            ) -> Result<$crate::AuthorizationRequest, $crate::types::Failure> {
                $provider.begin(&account_id)
            }

            fn poll(
                account_id: String,
                code: String,
                flow_state: Option<String>,
            ) -> Result<$crate::TokenOutcome, $crate::types::Failure> {
                $provider.poll(&account_id, &code, flow_state)
            }

            fn device_begin(
                _account_id: String,
                _credential_ref: Option<String>,
            ) -> Result<$crate::DeviceAuthorization, $crate::types::Failure> {
                $provider.device_begin()
            }

            fn device_poll(
                account_id: String,
                flow_state: Option<String>,
            ) -> Result<$crate::TokenOutcome, $crate::types::Failure> {
                $provider.device_poll(&account_id, flow_state)
            }

            fn refresh(
                account_id: String,
                credential_ref: Option<String>,
            ) -> Result<$crate::TokenOutcome, $crate::types::Failure> {
                $provider.refresh(&account_id, credential_ref)
            }
        }

        $crate::oauth_plugin!(Component);
    };
}
