//! The component: Metalink documents in, LinkGrabber candidates out.
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
        // Only the first mirror of each file is proposed. The rest are the same bytes from
        // somewhere else, and the LinkGrabber is a list of things to download, not of ways
        // to download them — mirror selection belongs to the transfer, not to intake.
        Ok(parse::files_in(&input)
            .into_iter()
            .filter_map(|file| {
                Some(IntakeCandidate {
                    url: file.urls.into_iter().next()?,
                    file_name: file.name,
                    size: file.size,
                    package_hint: Some("metalink".to_owned()),
                })
            })
            .collect())
    }

    /// Nothing to canonicalise: a metalink already carries the addresses its author meant.
    fn normalize(_url: String) -> Result<Option<String>, Failure> {
        Ok(None)
    }
}

export!(Component);
