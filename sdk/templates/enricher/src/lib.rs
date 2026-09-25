//! A scaffold metadata enricher. It compiles, packages and passes conformance as it is.
//!
//! Two things the host guarantees, which shape how this is written:
//!
//! - Your fields are added, never substituted. A name that collides with something the core
//!   already resolved is dropped and counted, so you cannot rewrite a file name or a size.
//! - You are only asked at all once the person has switched enrichment on. Nothing here runs
//!   for someone who did not ask for it, which is why it is safe for this to reach outwards.
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
        if !subject.url.contains("example.com") {
            return Ok(Vec::new());
        }
        Ok(vec![EnrichField {
            name: "example.rating".to_owned(),
            value: "unrated".to_owned(),
        }])
    }
}

export!(Component);
