//! The component: pasted text in, LinkGrabber candidates out.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "intake-plugin",
});

use exports::rdownloader::plugin::intake::{Guest, IntakeCandidate};
use rdownloader::plugin::types::Failure;

struct Component;

impl Guest for Component {
    /// Answer `true` only for input you can actually do something with.
    ///
    /// A parser that claims everything is handed every paste in the application in full,
    /// which is slower for the person pasting and no use to you.
    fn claims(input: String) -> bool {
        crate::claims(&input)
    }

    /// Return the links you found. An empty list means "nothing here", not an error; save
    /// the failure for something that actually went wrong.
    fn parse(input: String) -> Result<Vec<IntakeCandidate>, Failure> {
        Ok(crate::parse(&input)
            .into_iter()
            .map(|candidate| IntakeCandidate {
                url: candidate.url,
                // Unknown until the link is checked; the online check fills both in.
                file_name: None,
                size: None,
                package_hint: candidate.package,
            })
            .collect())
    }

    /// Return `None` when there is nothing to change, so the caller can tell "unchanged"
    /// from "rewritten to the same thing".
    fn normalize(url: String) -> Result<Option<String>, Failure> {
        Ok(crate::normalize(&url))
    }
}

export!(Component);
