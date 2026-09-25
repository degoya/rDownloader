//! The component: a MEGA address in, its storage address and key schedule out.
//!
//! Three shapes of address reach it. A file shared on its own carries its whole key in the
//! fragment and costs one call. A file inside a shared folder carries only the folder's key,
//! so its own is read out of the folder's node list -- one extra call, and the reason the
//! crawler emits the folder form rather than inventing an address that carries a node key.
//!
//! A file of the signed-in account (RD-120-30) carries no key at all. Its key sits in the
//! account's node list wrapped under the master key, which only the host holds, so this
//! plugin hands the wrapped bytes to the host's key derivation and gets back the file's own
//! key -- never the master key. The session goes out as the host's marker, never as a value.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "stream-transform-plugin",
});

use exports::rdownloader::plugin::stream_transform::{
    ContentTransform, Guest, ResolveRequest, ResolvedDownload, StreamCipher, StreamIntegrity,
    TransformedDownload,
};
use mega_common::{Target, api, crypto::FileKey};
use rdownloader::plugin::{
    http::{self, RequestHeader, RequestQuery},
    key_derivation::{self, SecretHandle, Step},
    types::{Failure, FailureKind},
};
use serde_json::Value;

use crate::{
    messages,
    plan::{self, Category, Refusal},
};

struct Component;

fn refuse(((code, message), category): Refusal) -> Failure {
    Failure {
        category: match category {
            Category::Unsupported => FailureKind::Unsupported,
            Category::Permanent => FailureKind::Permanent,
            Category::Transient => FailureKind::Transient(None),
            Category::RateLimited => FailureKind::RateLimited(None),
        },
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

/// A refusal this plugin has its own name for, in the shape the failure record wants.
fn named((code, message): (&str, &str)) -> (String, String) {
    (code.to_owned(), message.to_owned())
}

/// Who a call to the command endpoint speaks for.
#[derive(Clone, Copy)]
enum Caller<'a> {
    /// Nobody: a file shared on its own.
    Public,
    /// A shared folder, named by its handle.
    Folder(&'a str),
    /// The signed-in account, by the session the host holds.
    Account,
}

/// One call to MEGA's command endpoint; the object it answered with.
///
/// Every answer is `200` and the failure is a negative number in the body -- measured, not
/// assumed -- so the status is only ever a sign that something other than MEGA answered.
fn call(caller: Caller<'_>, body: Vec<u8>) -> Result<Value, Failure> {
    let headers = vec![RequestHeader {
        name: "Content-Type".to_owned(),
        value_template: "application/json".to_owned(),
    }];
    let (url, query) = match caller {
        Caller::Public => (api::endpoint(None), Vec::new()),
        Caller::Folder(folder) => (api::endpoint(Some(folder)), Vec::new()),
        Caller::Account => (
            api::endpoint(None),
            vec![RequestQuery {
                name: api::SESSION_QUERY.0.to_owned(),
                value_template: api::SESSION_QUERY.1.to_owned(),
            }],
        ),
    };
    let response = http::http_request("POST", &url, &query, &headers, &body)?;
    if !(200..300).contains(&response.status) {
        // How long to wait is the provider's to say, and it says it in a header rather than
        // in the body: `X-MEGA-Time-Left` on a `509`, `Retry-After` otherwise. Passing it on
        // is what turns a bandwidth refusal into a scheduled retry instead of a loop.
        let delay = api::retry_after(&response.headers);
        let (code, message) = match response.status {
            api::STATUS_BANDWIDTH_EXCEEDED => named(messages::QUOTA_EXCEEDED),
            429 => named(messages::RATE_LIMITED),
            status => (
                messages::HTTP_ERROR.to_owned(),
                messages::http_error(status),
            ),
        };
        return Err(Failure {
            category: match response.status {
                429 | api::STATUS_BANDWIDTH_EXCEEDED => FailureKind::RateLimited(delay),
                500..=599 => FailureKind::Transient(delay),
                _ => FailureKind::Permanent,
            },
            message,
            code: Some(code),
            params: vec![("status".to_owned(), response.status.to_string())],
        });
    }
    api::first_object(&response.body).map_err(|number| match number {
        None => refuse((messages::INVALID_RESPONSE, Category::Permanent)),
        Some(code) => match plan::api_refusal(code) {
            Some(known) => refuse(known),
            None => Failure {
                category: FailureKind::Permanent,
                message: messages::api_error(code),
                code: Some(messages::API_ERROR.to_owned()),
                params: vec![("status".to_owned(), code.to_string())],
            },
        },
    })
}

/// The key and the download answer for whichever shape of address this is.
fn lookup(target: &Target) -> Result<(FileKey, Value), Failure> {
    match target {
        Target::File { handle, key } => {
            let key = plan::file_key(key).map_err(refuse)?;
            let answer = call(Caller::Public, api::file_request(handle))?;
            Ok((key, answer))
        }
        Target::FolderChild { folder, key, node } => {
            let share = plan::share_key(key).map_err(refuse)?;
            let listing = call(Caller::Folder(folder), api::folder_request())?;
            let key = plan::node_key(&listing, node, &share).map_err(refuse)?;
            let answer = call(Caller::Folder(folder), api::folder_child_request(node))?;
            Ok((key, answer))
        }
        Target::AccountFile { handle } => {
            let listing = call(Caller::Account, api::account_listing_request())?;
            let wrapped = plan::account_wrapped_key(&listing, handle).map_err(refuse)?;
            // One step, over the key half of the session. What comes back is this file's
            // node key; the master key it was unwrapped under stays on the host.
            let raw = key_derivation::derive(
                &SecretHandle {
                    reference: api::SESSION_SECRET.to_owned(),
                },
                &[Step::AesEcbDecrypt(wrapped)],
            )?;
            let key = FileKey::from_raw(&raw)
                .ok_or_else(|| refuse((messages::KEY_INVALID, Category::Permanent)))?;
            let answer = call(Caller::Account, api::account_file_request(handle))?;
            Ok((key, answer))
        }
        // A folder address is the crawler's, not this plugin's. `claims_url` already says so;
        // these arms exist because the host may ask anyway.
        Target::Folder { .. } | Target::AccountNode { .. } => {
            Err(refuse((messages::NOT_MINE, Category::Unsupported)))
        }
    }
}

impl Guest for Component {
    fn claims_url(url: String) -> bool {
        matches!(
            Target::parse(&url),
            Some(Target::File { .. } | Target::FolderChild { .. } | Target::AccountFile { .. })
        )
    }

    fn resolve(request: ResolveRequest) -> Result<TransformedDownload, Failure> {
        let Some(target) = Target::parse(&request.url) else {
            return Err(refuse((messages::NOT_MINE, Category::Unsupported)));
        };
        let (key, answer) = lookup(&target)?;
        let description = plan::describe(&answer, &key).map_err(refuse)?;
        Ok(TransformedDownload {
            download: ResolvedDownload {
                url: description.url,
                file_name: description.file_name,
                size: Some(description.size),
                // Nothing rides along. A header is replayed on every chunk request and
                // written to the log with it, and this provider's secret is the key.
                headers: Vec::new(),
                // MEGA publishes no digest of its own; the condensed chunk MAC below is the
                // check, and it is the host's to make.
                checksum_algorithm: None,
                checksum_value: None,
                client: request.client,
            },
            transform: ContentTransform {
                cipher: StreamCipher {
                    algorithm: "aes-128-ctr".to_owned(),
                    key: description.key,
                    nonce: description.nonce,
                    first_block: 0,
                },
                integrity: description.integrity.map(|integrity| StreamIntegrity {
                    algorithm: "cbc-mac-chain".to_owned(),
                    boundaries: integrity.boundaries,
                    iv: integrity.iv,
                    expected: integrity.expected,
                }),
            },
        })
    }
}

export!(Component);
