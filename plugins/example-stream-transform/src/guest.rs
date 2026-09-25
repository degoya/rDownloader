//! The component: an address in, a description of how its bytes become a file out.
//!
//! It fetches nothing. Every value it answers with is in [`crate::plan`], which is also where
//! a reader should start; this file is the translation into the WIT vocabulary.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "stream-transform-plugin",
});

use exports::rdownloader::plugin::stream_transform::{
    ContentTransform, Guest, ResolveRequest, ResolvedDownload, StreamCipher, StreamIntegrity,
    TransformedDownload,
};
use rdownloader::plugin::types::{Failure, FailureKind};

use crate::plan::{BOUNDARIES, Case, EXPECTED, KEY};

struct Component;

impl Guest for Component {
    fn claims_url(url: String) -> bool {
        Case::of(&url).is_some()
    }

    fn resolve(request: ResolveRequest) -> Result<TransformedDownload, Failure> {
        let Some(case) = Case::of(&request.url) else {
            // `unsupported` is the one refusal that says nothing about the file: it says this
            // plugin was wrong to be asked. The host moves on to the next claimer.
            return Err(Failure {
                category: FailureKind::Unsupported,
                message: "this address does not belong to the reference plugin".to_owned(),
                code: Some("example_stream_transform.not_mine".to_owned()),
                params: Vec::new(),
            });
        };
        Ok(TransformedDownload {
            download: ResolvedDownload {
                url: request.url.clone(),
                file_name: Some("example.bin".to_owned()),
                size: Some(BOUNDARIES[BOUNDARIES.len() - 1]),
                headers: Vec::new(),
                checksum_algorithm: None,
                checksum_value: None,
                client: request.client,
            },
            transform: ContentTransform {
                cipher: StreamCipher {
                    algorithm: case.cipher().to_owned(),
                    key: KEY.to_vec(),
                    nonce: case.nonce(),
                    first_block: 0,
                },
                integrity: case.integrity().map(|algorithm| StreamIntegrity {
                    algorithm: algorithm.to_owned(),
                    boundaries: BOUNDARIES.to_vec(),
                    // The provider's own definition: the counter prefix twice over.
                    iv: [crate::plan::NONCE, crate::plan::NONCE].concat(),
                    expected: EXPECTED.to_vec(),
                }),
            },
        })
    }
}

export!(Component);
