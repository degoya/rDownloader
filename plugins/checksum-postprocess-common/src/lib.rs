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

// The bindings are the post-processing world's, generated once in `plugin-guest-postprocess`
// for every post-processing plugin (RD-1120-10); the export macro reaches them through here, so
// a checksum plugin depends on this crate alone.
#[doc(hidden)]
pub use plugin_guest_postprocess;
pub use plugin_guest_postprocess::{Guest, StepEnd, StepInput};

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

        $crate::plugin_guest_postprocess::postprocess_plugin!(Component);
    };
}
