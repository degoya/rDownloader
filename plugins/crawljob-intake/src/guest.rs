//! The component: `.crawljob` blocks in, LinkGrabber candidates out.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "intake-plugin",
});

use exports::rdownloader::plugin::intake::{Guest, IntakeCandidate};
use rdownloader::plugin::types::Failure;

use crate::parse;

struct Component;

impl Guest for Component {
    fn claims(input: String) -> bool {
        parse::claims(&input)
    }

    fn parse(input: String) -> Result<Vec<IntakeCandidate>, Failure> {
        let mut candidates = Vec::new();
        for job in parse::jobs_in(&input) {
            let single = job.urls.len() == 1;
            for url in job.urls {
                candidates.push(IntakeCandidate {
                    url,
                    file_name: if single { job.file_name.clone() } else { None },
                    // A hint, not a package: the host decides whether the grouping survives.
                    package_hint: job.package_name.clone(),
                    size: None,
                });
            }
        }
        Ok(candidates)
    }

    /// Nothing to canonicalise: a crawljob carries the addresses its author meant.
    fn normalize(_url: String) -> Result<Option<String>, Failure> {
        Ok(None)
    }
}

export!(Component);
