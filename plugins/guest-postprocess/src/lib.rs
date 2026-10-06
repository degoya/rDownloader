//! The WebAssembly bindings of the post-processing world, generated once for every
//! post-processing plugin.
//!
//! A post-processing guest is its step's own run — what it reads of a package, what it writes
//! back — so there is no shared adapter to write as there is for the resolvers in
//! `plugin_guest`. What was the same in every one of them is the glue underneath: the generated
//! bindings and their export (RD-1120-10). They live here, so a post-processing plugin's
//! `guest.rs` — or `checksum-postprocess-common`, for the two checksum steps — imports them,
//! implements [`Guest`] and ends in [`postprocess_plugin!`]. The component that comes out
//! exports the same world under the same names; the host cannot tell the difference.

#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "postprocess-plugin",
    // The bindings live here rather than in each plugin, so the export macro has to be usable
    // from another crate and needs a name that says what it exports.
    pub_export_macro: true,
    export_macro_name: "export_postprocess",
});

pub use exports::rdownloader::plugin::postprocess::{Guest, StepComplete, StepEnd, StepInput};
pub use rdownloader::plugin::{host, source, types};

/// Exports a post-processing plugin: `$component` implements [`Guest`].
#[macro_export]
macro_rules! postprocess_plugin {
    ($component:ident) => {
        $crate::export_postprocess!($component with_types_in $crate);
    };
}
