//! The WebAssembly bindings of the notifier world, generated once for every notifier plugin.
//!
//! A notifier's guest is its destination's own request — where it posts, how a message is
//! shaped — so there is no shared adapter to write as there is for the resolvers in
//! `plugin_guest`. What was the same in every one of them is the glue underneath: the generated
//! bindings and the reading of the destination's answer (RD-1120-10). They live here, so a
//! notifier plugin's `guest.rs` imports them, implements [`Guest`] and ends in
//! [`notifier_plugin!`]. The component that comes out exports the same world under the same
//! names; the host cannot tell the difference.

#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "notifier-plugin",
    // The bindings live here rather than in each plugin, so the export macro has to be usable
    // from another crate and needs a name that says what it exports.
    pub_export_macro: true,
    export_macro_name: "export_notifier",
});

pub use exports::rdownloader::plugin::notifier::{Guest, Notification};
pub use rdownloader::plugin::{destination_settings, host, http, types};

use types::{Failure, FailureKind};

/// A refusal under the plugin's one code, with an English message for the log.
#[must_use]
pub fn refuse(code: &str, message: &str, category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

/// What a destination's answer means: a `2xx` was delivered, anything else is refused under
/// `code` as "`destination` answered `status`".
///
/// A `429` is the destination's rate limit and a `5xx` its own trouble, so both are worth
/// another try; any other status — a deleted webhook, a wrong token or chat — will not change by
/// repeating it, and repeating it forever helps nobody.
///
/// # Errors
///
/// The refusal, when the status is not a success.
pub fn delivered(status: u16, destination: &str, code: &str) -> Result<(), Failure> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let category = if status >= 500 || status == 429 {
        FailureKind::Transient(None)
    } else {
        FailureKind::Permanent
    };
    Err(refuse(
        code,
        &format!("{destination} answered {status}"),
        category,
    ))
}

/// Exports a notifier plugin: `$component` implements [`Guest`].
#[macro_export]
macro_rules! notifier_plugin {
    ($component:ident) => {
        $crate::export_notifier!($component with_types_in $crate);
    };
}
