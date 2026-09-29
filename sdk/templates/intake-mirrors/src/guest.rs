//! The component: a mirror list in, LinkGrabber candidates and their source sets out.
//!
//! Everything it reads is in [`crate::list`]; this file is the translation into the WIT
//! vocabulary, once for each of the two interfaces the world exports.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "intake-mirrors-plugin",
});

use exports::rdownloader::plugin::intake::{Guest, IntakeCandidate};
use exports::rdownloader::plugin::mirror_sets::{
    Guest as MirrorSetsGuest, SetSource, SourceSet, StatedHash,
};
use rdownloader::plugin::types::Failure;

use crate::list;

struct Component;

impl Guest for Component {
    /// Answer `true` only for input you can actually read: a parser that claims everything is
    /// handed every paste in the application.
    fn claims(input: String) -> bool {
        list::claims(&input)
    }

    /// One candidate per file, under its most preferred address. The other addresses are the
    /// same bytes from somewhere else: the LinkGrabber lists things to download, not ways to
    /// download them, so the rest travel in `sets` and belong to the transfer.
    fn parse(input: String) -> Result<Vec<IntakeCandidate>, Failure> {
        Ok(list::files(&input)
            .into_iter()
            .filter_map(|file| {
                Some(IntakeCandidate {
                    url: file.sources.into_iter().next()?.url,
                    file_name: file.name,
                    size: file.size,
                    package_hint: None,
                })
            })
            .collect())
    }

    /// Nothing to canonicalise: a list already carries the addresses its author meant.
    fn normalize(_url: String) -> Result<Option<String>, Failure> {
        Ok(None)
    }
}

impl MirrorSetsGuest for Component {
    /// The same files as `parse`, each named by the address `parse` proposed for it — that is
    /// how the host puts a set and its candidate back together.
    fn sets(input: String) -> Result<Vec<SourceSet>, Failure> {
        Ok(list::files(&input)
            .into_iter()
            .filter_map(|file| {
                Some(SourceSet {
                    primary_url: file.sources.first()?.url.clone(),
                    file_name: file.name,
                    size: file.size,
                    sources: file
                        .sources
                        .into_iter()
                        .enumerate()
                        .map(|(index, source)| SetSource {
                            url: source.url,
                            // Lower is preferred, counted from 1 as Metalink does. The list's
                            // order is its preference, so the position is the priority.
                            priority: u32::try_from(index + 1).ok(),
                            location: source.location,
                        })
                        .collect(),
                    hashes: file
                        .sha256
                        .map(|value| StatedHash {
                            algorithm: "sha-256".to_owned(),
                            value,
                        })
                        .into_iter()
                        .collect(),
                    // This format states no piece hashes. A format that does fills
                    // `Some(PieceHashes { .. })`, covering the stated size exactly.
                    pieces: None,
                })
            })
            .collect())
    }
}

export!(Component);
