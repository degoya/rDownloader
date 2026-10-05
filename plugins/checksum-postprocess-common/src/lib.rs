//! The checksum steps, once, with the hash as the parameter (RD-1110-04, audit R2).
//!
//! `md5-postprocess` and `sha256-postprocess` were the same 580 lines twice, differing in the
//! hash, the sidecar's extension and the slug of their codes. They stay two plugins, each with
//! its own id, manifest, version and switch, because a person turns one on without the other;
//! what they share is everything else, here. A plugin names its [`sidecar::Algorithm`],
//! implements [`Checksum`] for the hash it links, and ends its `guest.rs` in [`checksum_plugin!`].
//!
//! The sidecar format lives in [`sidecar`], which knows nothing about the plugin contract, so it
//! is unit-tested without a WebAssembly target. [`step`] is the component's run over it.

#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "postprocess-plugin",
    // The bindings live here rather than in each plugin, so the export macro has to be usable
    // from another crate and needs a name that says what it exports.
    pub_export_macro: true,
    export_macro_name: "export_postprocess",
});

pub use exports::rdownloader::plugin::postprocess::{Guest, StepEnd, StepInput};

pub mod sidecar;
pub mod step;

/// An incremental hash: what a plugin brings for the one it links.
pub trait Checksum {
    /// A hash over nothing yet.
    fn new() -> Self;
    /// Feeds the next bytes.
    fn update(&mut self, bytes: &[u8]);
    /// The digest of everything fed.
    fn finish(self) -> Vec<u8>;
}

/// Exports a checksum plugin: `$algorithm` is its [`sidecar::Algorithm`], `$hash` its [`Checksum`].
#[macro_export]
macro_rules! checksum_plugin {
    ($algorithm:path, $hash:ty) => {
        struct Component;

        impl $crate::Guest for Component {
            fn run(input: $crate::StepInput) -> $crate::StepEnd {
                $crate::step::run::<$hash>(&$algorithm, input)
            }
        }

        $crate::export_postprocess!(Component with_types_in $crate);
    };
}
