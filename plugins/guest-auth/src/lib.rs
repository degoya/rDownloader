//! The WebAssembly bindings of the auth world, generated once for every authentication plugin.
//!
//! An authentication guest is its provider's own sign-in — a device flow, a key check, a
//! password derivation — so there is no shared adapter to write as there is for the resolvers in
//! `plugin_guest`. What was the same in every one of them is the glue underneath: the generated
//! bindings and the form body two device flows built alike. They live here, so an auth plugin's
//! `guest.rs` imports them, implements [`Guest`] and ends in [`auth_plugin!`]. The component that
//! comes out exports the same world under the same names; the host cannot tell the difference.

#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "auth-plugin",
    // The bindings live here rather than in each plugin, so the export macro has to be usable
    // from another crate and needs a name that says what it exports.
    pub_export_macro: true,
    export_macro_name: "export_auth",
});

pub use exports::rdownloader::plugin::auth::{AuthState, Guest, UserPrompt};
pub use rdownloader::plugin::{credentials, host, http, key_derivation, types};

use http::RequestQuery;
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

/// A failure carrying a stable translation code, `<slug>.<code>`, and nothing a provider wrote.
///
/// The auth guests compose their codes from their slug, where `plugin_guest_crawler` and
/// `plugin_guest_remote_job` take whole codes from a catalogue; otherwise the same helper
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

/// Exports an authentication plugin: `$component` implements [`Guest`].
#[macro_export]
macro_rules! auth_plugin {
    ($component:ident) => {
        $crate::export_auth!($component with_types_in $crate);
    };
}
