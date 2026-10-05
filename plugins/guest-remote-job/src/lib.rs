//! The WebAssembly bindings of the remote-job world, generated once for every remote-job plugin.
//!
//! A remote job's guest is its provider's own API — how a job is submitted, polled and read
//! back — so there is no shared adapter to write as there is for the resolvers in
//! `plugin_guest`. What was the same in every one of them is the glue underneath: the generated
//! bindings and the refusal constructor. They live here, so a remote-job plugin's `guest.rs`
//! imports them, implements [`Guest`] and ends in [`remote_job_plugin!`]. The component that
//! comes out exports the same world under the same names; the host cannot tell the difference.

#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "remote-job-plugin",
    // The bindings live here rather than in each plugin, so the export macro has to be usable
    // from another crate and needs a name that says what it exports.
    pub_export_macro: true,
    export_macro_name: "export_remote_job",
});

pub use exports::rdownloader::plugin::remote_job::{
    CacheAnswer, CacheKind, CacheQuery, CacheState, Guest, JobSource, RemoteArtifact, RemoteEntry,
    RemoteHandle, RemoteProgress, RemoteWork, SubmitRequest,
};
pub use rdownloader::plugin::{host, http, job_context, types};

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

/// Exports a remote-job plugin: `$component` implements [`Guest`].
#[macro_export]
macro_rules! remote_job_plugin {
    ($component:ident) => {
        $crate::export_remote_job!($component with_types_in $crate);
    };
}
