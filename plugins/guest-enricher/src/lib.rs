//! The WebAssembly bindings of the enricher world, generated once for every enricher plugin.
//!
//! An enricher's guest is its source's own lookup — what it asks, how an answer becomes fields —
//! so there is no shared adapter to write as there is for the resolvers in `plugin_guest`. What
//! was the same in every one of them is the glue underneath: the generated bindings and their
//! export (RD-1120-10). They live here, so an enricher plugin's `guest.rs` imports them,
//! implements [`Guest`] and ends in [`enricher_plugin!`]. The component that comes out exports
//! the same world under the same names; the host cannot tell the difference.

#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "enricher-plugin",
    // The bindings live here rather than in each plugin, so the export macro has to be usable
    // from another crate and needs a name that says what it exports.
    pub_export_macro: true,
    export_macro_name: "export_enricher",
});

pub use exports::rdownloader::plugin::enricher::{EnrichField, EnrichSubject, Guest};
pub use rdownloader::plugin::{host, http, types};

/// Exports an enricher plugin: `$component` implements [`Guest`].
#[macro_export]
macro_rules! enricher_plugin {
    ($component:ident) => {
        $crate::export_enricher!($component with_types_in $crate);
    };
}
