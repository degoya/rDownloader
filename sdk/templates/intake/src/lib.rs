//! A scaffold intake parser. It compiles, packages and passes conformance as it is.
//!
//! Two things the host guarantees, which shape how this is written:
//!
//! - Everything you propose goes through the LinkGrabber review, the domain blocklist and
//!   the routing rules, exactly as a pasted link does. You propose; the application decides.
//! - A rewrite from `normalize` is discarded if it changes the host or the scheme. A
//!   normalizer tidies an address; it cannot redirect one.
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
        input.contains("example.com/list/")
    }

    /// Return the links you found. An empty list means "nothing here", not an error; save
    /// the failure for something that actually went wrong.
    fn parse(input: String) -> Result<Vec<IntakeCandidate>, Failure> {
        let candidates = input
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("https://example.com/list/"))
            .map(|line| IntakeCandidate {
                url: line.to_owned(),
                file_name: None,
                size: None,
                package_hint: None,
            })
            .collect();
        Ok(candidates)
    }

    /// Return `None` when there is nothing to change, so the caller can tell "unchanged"
    /// from "rewritten to the same thing".
    fn normalize(_url: String) -> Result<Option<String>, Failure> {
        Ok(None)
    }
}

export!(Component);
