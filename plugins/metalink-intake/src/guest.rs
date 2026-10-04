//! The component: Metalink documents in, LinkGrabber candidates out.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "intake-mirrors-plugin",
});

use exports::rdownloader::plugin::intake::{Guest, IntakeCandidate};
use exports::rdownloader::plugin::mirror_sets::{
    Guest as MirrorSetsGuest, PieceHashes, SetSource, SourceSet, StatedHash,
};
use rdownloader::plugin::types::{Failure, FailureKind};

use crate::parse;

struct Component;

impl Guest for Component {
    fn claims(input: String) -> bool {
        parse::claims(&input)
    }

    fn parse(input: String) -> Result<Vec<IntakeCandidate>, Failure> {
        // Only the best mirror of each file is proposed. The rest are the same bytes from
        // somewhere else, and the LinkGrabber is a list of things to download, not of ways
        // to download them — the others travel in `sets` and belong to the transfer.
        let candidates: Vec<IntakeCandidate> = parse::files_in(&input)
            .into_iter()
            .filter_map(|file| {
                Some(IntakeCandidate {
                    url: file.urls.into_iter().next()?,
                    file_name: file.name,
                    size: file.size,
                    package_hint: Some("metalink".to_owned()),
                })
            })
            .collect();
        // Claimed, yet not one file carried an address: say so with the code the catalogues
        // translate, rather than an empty answer that looks like success (PLUG-16).
        if candidates.is_empty() {
            return Err(Failure {
                category: FailureKind::Permanent,
                message: "the Metalink document lists no usable file".to_owned(),
                code: Some(parse::UNREADABLE.to_owned()),
                params: Vec::new(),
            });
        }
        Ok(candidates)
    }

    /// Nothing to canonicalise: a metalink already carries the addresses its author meant.
    fn normalize(_url: String) -> Result<Option<String>, Failure> {
        Ok(None)
    }
}

impl MirrorSetsGuest for Component {
    fn sets(input: String) -> Result<Vec<SourceSet>, Failure> {
        // `parse` proposed `urls[0]` for each file; the same address names the set here, so
        // the host can put the two back together.
        Ok(parse::files_in(&input)
            .into_iter()
            .filter_map(|file| {
                Some(SourceSet {
                    primary_url: file.urls.first()?.clone(),
                    file_name: file.name,
                    size: file.size,
                    sources: file
                        .sources
                        .into_iter()
                        .map(|source| SetSource {
                            url: source.url,
                            priority: source.priority,
                            location: source.location,
                        })
                        .collect(),
                    hashes: file
                        .hashes
                        .into_iter()
                        .map(|(algorithm, value)| StatedHash { algorithm, value })
                        .collect(),
                    pieces: file.pieces.map(|pieces| PieceHashes {
                        algorithm: pieces.algorithm,
                        length: pieces.length,
                        hashes: pieces.hashes,
                    }),
                })
            })
            .collect())
    }
}

export!(Component);
