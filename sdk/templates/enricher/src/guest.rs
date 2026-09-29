//! The component: a link in, the fields [`crate::fields`] finds in its name out.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "enricher-plugin",
});

use exports::rdownloader::plugin::enricher::{EnrichField, EnrichSubject, Guest};
use rdownloader::plugin::types::Failure;

struct Component;

impl Guest for Component {
    /// Return the fields you know about this subject.
    ///
    /// `subject.known` carries what the core already resolved, as JSON, so you can skip work
    /// it has done. An empty list means "nothing to add", which is not a failure.
    fn enrich(subject: EnrichSubject) -> Result<Vec<EnrichField>, Failure> {
        let Some(file_name) = subject.file_name else {
            return Ok(Vec::new());
        };
        Ok(crate::fields(&file_name)
            .into_iter()
            .map(|(name, value)| EnrichField { name, value })
            .collect())
    }
}

export!(Component);
