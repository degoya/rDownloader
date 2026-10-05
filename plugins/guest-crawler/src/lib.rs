//! The WebAssembly bindings of the crawler world, generated once for every crawler plugin.
//!
//! A crawler's guest is its own logic — which addresses it claims, how it walks a folder — so
//! there is no shared adapter to write as there is for the resolvers in `plugin_guest`. What was
//! the same in every one of them is the glue underneath: the generated bindings and the two
//! small constructors every crawler wrote out again. They live here, so a crawler's `guest.rs`
//! imports them, implements [`Guest`] and ends in [`crawler_plugin!`]. The component that comes
//! out exports the same world under the same names; the host cannot tell the difference.

#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "crawler-plugin",
    // The bindings live here rather than in each plugin, so the export macro has to be usable
    // from another crate and needs a name that says what it exports.
    pub_export_macro: true,
    export_macro_name: "export_crawler",
});

pub use exports::rdownloader::plugin::crawler::{CrawledLink, Guest};
pub use rdownloader::plugin::{captcha, cookies, host, http, key_derivation, types};

use http::RequestQuery;
use types::{Failure, FailureKind};

/// A failure carrying a stable translation code and its English fallback, and nothing else.
#[must_use]
pub fn refuse((code, message): (&str, &str), category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

/// One query parameter taken literally: no `{{secret:…}}` marker is ever built from it.
#[must_use]
pub fn query(name: &str, value: &str) -> RequestQuery {
    RequestQuery {
        name: name.to_owned(),
        value_template: value.to_owned(),
    }
}

/// Exports a crawler plugin: `$component` implements [`Guest`].
#[macro_export]
macro_rules! crawler_plugin {
    ($component:ident) => {
        $crate::export_crawler!($component with_types_in $crate);
    };
}
