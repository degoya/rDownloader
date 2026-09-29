//! The component: an address in, the address of its ciphertext and the key schedule out.
//!
//! Everything it decides is in [`crate::link`] and [`crate::reply`]; this file is the one
//! request and the translation into the WIT vocabulary.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "stream-transform-plugin",
});

use exports::rdownloader::plugin::stream_transform::{
    ContentTransform, Guest, ResolveRequest, ResolvedDownload, StreamCipher, TransformedDownload,
};
use rdownloader::plugin::{
    http,
    types::{Failure, FailureKind},
};

use crate::{link, reply};

struct Component;

impl Guest for Component {
    /// Reaches nothing: answered from the address alone, before anything is fetched.
    fn claims_url(url: String) -> bool {
        link::file_id(&url).is_some()
    }

    fn resolve(request: ResolveRequest) -> Result<TransformedDownload, Failure> {
        let Some(id) = link::file_id(&request.url) else {
            // `unsupported` says this plugin was wrong to be asked, and nothing about the file.
            return Err(refuse(
                FailureKind::Unsupported,
                "not_mine",
                "the address does not belong to this plugin",
            ));
        };
        let Some(secret) = link::secret(&request.url) else {
            return Err(refuse(
                FailureKind::Permanent,
                "no_key",
                "the address carries no usable key",
            ));
        };
        // The id and nothing else: the fragment stays on this side of the request.
        let response = http::http_request("GET", &link::api_url(id), &[], &[], &[])?;
        match response.status {
            200 => {}
            404 | 410 => {
                return Err(refuse(
                    FailureKind::Offline,
                    "file_gone",
                    "the provider no longer has the file",
                ));
            }
            _ => {
                return Err(refuse(
                    FailureKind::Transient(None),
                    "unavailable",
                    "the provider did not answer the request",
                ));
            }
        }
        let body = String::from_utf8_lossy(&response.body);
        let stored = reply::stored(&body).ok_or_else(|| {
            refuse(
                FailureKind::Permanent,
                "bad_reply",
                "the provider sent a reply this plugin could not read",
            )
        })?;
        Ok(TransformedDownload {
            download: ResolvedDownload {
                url: stored.url,
                file_name: stored.name,
                size: stored.size,
                headers: Vec::new(),
                checksum_algorithm: None,
                checksum_value: None,
                client: request.client,
            },
            transform: ContentTransform {
                cipher: StreamCipher {
                    algorithm: "aes-128-ctr".to_owned(),
                    key: secret.key.to_vec(),
                    nonce: secret.nonce.to_vec(),
                    // The file is encrypted from its first byte.
                    first_block: 0,
                },
                // This provider publishes nothing to check the plaintext against, so the host
                // downloads, decrypts and says it could not verify. A provider that publishes a
                // MAC answers `Some(StreamIntegrity { algorithm: "cbc-mac-chain", .. })` with
                // its chunk boundaries — a wrong key is otherwise indistinguishable from a right
                // one until somebody opens the file.
                integrity: None,
            },
        })
    }
}

/// A failure carrying a stable translation code and nothing the provider wrote.
fn refuse(category: FailureKind, code: &str, message: &str) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(format!("{{PLUGIN_SLUG}}.{code}")),
        params: Vec::new(),
    }
}

export!(Component);
