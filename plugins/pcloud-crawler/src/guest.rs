//! The component: a pCloud folder or public link in, the files behind it out.
//!
//! One invocation walks the whole tree, because a guest is instantiated fresh for every call
//! and remembers nothing of its own. What keeps that from being unbounded is [`crate::walk`]:
//! depth, breadth and cycles are refused there, and the manifest's fuel and time budget sit
//! underneath as the last resort rather than as the plan.
//!
//! The two sources are walked by the same code and differ only in where a folder's entries
//! come from. An own-drive folder is one `listfolder` call per node — pCloud paginates none of
//! it. A public link is **one** `showpublink` call for the whole tree, because that is what
//! pCloud answers a folder link with; the walk then reads nodes out of that document instead
//! of fetching them, and the limits apply exactly the same, since a tree a stranger shared is
//! no more trustworthy for having arrived in one piece.
//!
//! Which of pCloud's two installations the calls go to is settled by the first one and pinned
//! for the rest — see `plugins/pcloud/src/resolver.rs` for why that correction exists at all.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "crawler-plugin",
});

use exports::rdownloader::plugin::crawler::{CrawledLink, Guest};
use pcloud_common::{
    address::{self, Region},
    api as pcloud_api,
    metadata::{self, Metadata},
};
use rdownloader::plugin::{
    host,
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::{
    listing, messages,
    target::{self, Target},
    walk::{Limit, Walk},
};

/// The vault reference the pCloud provider keeps its access token under. The value never
/// reaches this plugin: the host substitutes it into `{{secret:…}}` on the way out, and only
/// towards the hosts the provider declared for that reference.
const TOKEN_REFERENCE: &str = "pcloud_access_token";
/// The name the root of an account's own drive goes by: `folderid` 0 has no name of its own.
const ROOT_NAME: &str = "pCloud";
/// How deep a node is looked for in the tree a public link answered with. Bounded because the
/// tree was built by whoever shared the link.
const MAX_TREE_DEPTH: u32 = 16;

struct Component;

fn refuse((code, message): (&str, &str), category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

/// A refusal carrying pCloud's own decimal number and nothing it wrote.
fn refuse_with_result(
    (code, message): (&str, &str),
    category: FailureKind,
    result: u64,
) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: vec![("result".to_owned(), result.to_string())],
    }
}

fn authorization() -> Vec<RequestHeader> {
    vec![RequestHeader {
        name: "Authorization".to_owned(),
        value_template: format!("Bearer {{{{secret:{TOKEN_REFERENCE}}}}}"),
    }]
}

fn query(pairs: &[(&str, String)]) -> Vec<RequestQuery> {
    pairs
        .iter()
        .map(|(name, value)| RequestQuery {
            name: (*name).to_owned(),
            value_template: value.clone(),
        })
        .collect()
}

/// What one pCloud answer was, when it was not an answer.
enum Rejected {
    Fatal(Failure),
    Refused(u64, Option<u64>),
}

/// One call at one installation.
fn once(
    region: Region,
    method: &str,
    parameters: &[(&str, String)],
    authenticated: bool,
) -> Result<Vec<u8>, Rejected> {
    let headers = if authenticated {
        authorization()
    } else {
        Vec::new()
    };
    let response = http::http_request(
        "GET",
        &format!("{}/{method}", region.api()),
        &query(parameters),
        &headers,
        &[],
    )
    .map_err(Rejected::Fatal)?;
    if !(200..300).contains(&response.status) {
        // pCloud answers 200 to its own refusals, so a status that is not 2xx never came from
        // pCloud's application: it is a gateway or the network.
        return Err(Rejected::Fatal(refuse(
            messages::FOLDER_UNREACHABLE,
            FailureKind::Transient(pcloud_api::retry_after(&response.headers)),
        )));
    }
    let Some(result) = pcloud_api::result_of(&response.body) else {
        return Err(Rejected::Fatal(refuse(
            messages::INVALID_RESPONSE,
            FailureKind::Permanent,
        )));
    };
    if result == pcloud_api::OK {
        return Ok(response.body);
    }
    Err(Rejected::Refused(
        result,
        pcloud_api::retry_after(&response.headers),
    ))
}

/// Turns one pCloud refusal into a failure, carrying its number and nothing else.
fn failed(result: u64, retry_after: Option<u64>) -> Failure {
    match pcloud_api::Category::of(result) {
        pcloud_api::Category::Credential => refuse_with_result(
            messages::SIGN_IN_REQUIRED,
            FailureKind::AuthRequired,
            result,
        ),
        // A crawl is one invocation, so a rate limit ends it as a wait of its own; there is no
        // queue of pCloud links here to hold back, and nothing else is touched.
        pcloud_api::Category::RateLimited => refuse_with_result(
            messages::RATE_LIMITED,
            FailureKind::RateLimited(retry_after),
            result,
        ),
        pcloud_api::Category::Link => {
            refuse_with_result(messages::LINK_UNAVAILABLE, FailureKind::Permanent, result)
        }
        pcloud_api::Category::Unavailable => refuse_with_result(
            messages::FOLDER_UNREACHABLE,
            FailureKind::Transient(retry_after),
            result,
        ),
        _ => refuse_with_result(messages::FOLDER_UNREACHABLE, FailureKind::Permanent, result),
    }
}

/// One call, corrected once if pCloud's own answer says the installation was wrong, returning
/// the installation that actually answered so the rest of the walk can be pinned to it.
fn call(
    start: Region,
    method: &str,
    parameters: &[(&str, String)],
    authenticated: bool,
) -> Result<(Vec<u8>, Region), Failure> {
    let mut carried: Option<Failure> = None;
    for (attempt, region) in start.both_from().into_iter().enumerate() {
        match once(region, method, parameters, authenticated) {
            Ok(body) => return Ok((body, region)),
            Err(Rejected::Fatal(failure)) => return Err(failure),
            Err(Rejected::Refused(result, retry_after)) => {
                let worth_the_other_region =
                    attempt == 0 && pcloud_api::Category::of(result).may_be_the_other_region();
                if !worth_the_other_region {
                    return Err(failed(result, retry_after));
                }
                host::log(
                    "debug",
                    "pcloud refused this at the first data centre; asking the other one",
                );
                carried = Some(failed(result, retry_after));
            }
        }
    }
    Err(carried.unwrap_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent)))
}

/// One call at an installation that is already settled.
fn fixed(
    region: Region,
    method: &str,
    parameters: &[(&str, String)],
    authenticated: bool,
) -> Result<Vec<u8>, Failure> {
    once(region, method, parameters, authenticated).map_err(|rejected| match rejected {
        Rejected::Fatal(failure) => failure,
        Rejected::Refused(result, retry_after) => failed(result, retry_after),
    })
}

/// The crawled folder's own name, which becomes the root of every path below it.
///
/// The name is a stranger's, so it goes through the same guard a child's name does before it
/// becomes the first segment of every package hint.
fn root_name(folder: &Metadata) -> String {
    let named = crate::walk::join("", folder.name());
    if named.is_empty() {
        ROOT_NAME.to_owned()
    } else {
        named
    }
}

/// The folder with this `folderid` in the tree a public link answered with.
fn find_folder<'a>(node: &'a Metadata, folder_id: u64, depth: u32) -> Option<&'a Metadata> {
    if node.folderid == Some(folder_id) && node.isfolder {
        return Some(node);
    }
    if depth >= MAX_TREE_DEPTH {
        return None;
    }
    node.contents
        .iter()
        .find_map(|child| find_folder(child, folder_id, depth + 1))
}

/// Says once, and visibly, that the walk did not reach everything.
fn report(limit: Option<Limit>) {
    if let Some(limit) = limit {
        // Reported rather than silent: somebody who pasted a folder and got 500 of its 900
        // files has to be able to find out which half they are looking at.
        host::log(
            "warn",
            match limit {
                Limit::Depth => "pcloud folder is nested deeper than this crawl walks",
                Limit::Files => "pcloud folder holds more files than this crawl lists",
                Limit::Folders => "pcloud folder holds more subfolders than this crawl reads",
            },
        );
    }
}

/// An empty answer is not a result: handing one back would create a package with nothing in it
/// and nothing in the interface to explain why.
fn some_files(links: Vec<CrawledLink>) -> Result<Vec<CrawledLink>, Failure> {
    if links.is_empty() {
        return Err(refuse(messages::FOLDER_EMPTY, FailureKind::Permanent));
    }
    Ok(links)
}

/// A folder in the account's own drive: one `listfolder` per node.
fn crawl_own(folder_id: u64, start: Region) -> Result<Vec<CrawledLink>, Failure> {
    let (body, region) = call(
        start,
        "listfolder",
        &[("folderid", folder_id.to_string())],
        true,
    )?;
    let root = metadata::item(&body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    if !root.is_folder() {
        // The sibling resolver's address, pasted at the crawler.
        return Err(refuse(messages::NOT_A_FOLDER, FailureKind::Unsupported));
    }
    let name = root_name(&root);
    let mut walk = Walk::start(folder_id);
    // The root has already been fetched; every other node costs one call.
    let mut fetched = Some(root);
    while let Some(mut pending) = walk.next_folder() {
        if pending.depth == 0 {
            pending.path = name.clone();
        }
        let node = match fetched.take() {
            Some(node) => node,
            None => {
                let body = fixed(
                    region,
                    "listfolder",
                    &[("folderid", pending.folder_id.to_string())],
                    true,
                )?;
                metadata::item(&body)
                    .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?
            }
        };
        walk.absorb(&pending, listing::entries(&node));
    }
    report(walk.limit());
    some_files(
        walk.into_files()
            .into_iter()
            .map(|found| CrawledLink {
                // The canonical address, which the sibling resolver claims. Nothing else passes
                // between the two packages — and it carries the installation that answered, so
                // the resolver reaches the same one without paying for the correction again.
                url: address::file_address(region, found.folder_id, found.file_id),
                file_name: Some(found.name).filter(|name| !name.is_empty()),
                size: found.size,
                package_hint: Some(found.path).filter(|path| !path.is_empty()),
            })
            .collect(),
    )
}

/// A public link: one `showpublink` for the whole tree, walked under the same limits.
fn crawl_public(code: &str, start: Region) -> Result<Vec<CrawledLink>, Failure> {
    let (body, region) = call(start, "showpublink", &[("code", code.to_owned())], false)?;
    let root = metadata::item(&body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    // A link to one file is still this plugin's, because no address could have said it was
    // not. It comes back as that one file, spelled with its `fileid` — which is the address
    // the sibling resolver claims.
    if root.is_file() {
        let Some(file_id) = root.fileid else {
            return Err(refuse(messages::INVALID_RESPONSE, FailureKind::Permanent));
        };
        return Ok(vec![CrawledLink {
            url: address::public_file_address(region, code, file_id),
            file_name: Some(root.name().to_owned()).filter(|name| !name.is_empty()),
            size: root.size,
            // One file is not a package.
            package_hint: None,
        }]);
    }
    let Some(root_id) = root.folderid else {
        return Err(refuse(messages::INVALID_RESPONSE, FailureKind::Permanent));
    };
    let name = root_name(&root);
    let mut walk = Walk::start(root_id);
    while let Some(mut pending) = walk.next_folder() {
        if pending.depth == 0 {
            pending.path = name.clone();
        }
        // Read out of the document rather than fetched: pCloud already sent the whole tree.
        // A node deeper than the floor simply yields nothing, which the limits then report.
        let Some(node) = find_folder(&root, pending.folder_id, 0) else {
            walk.note(Limit::Depth);
            continue;
        };
        walk.absorb(&pending, listing::entries(node));
    }
    report(walk.limit());
    some_files(
        walk.into_files()
            .into_iter()
            .map(|found| CrawledLink {
                url: address::public_file_address(region, code, found.file_id),
                file_name: Some(found.name).filter(|name| !name.is_empty()),
                size: found.size,
                package_hint: Some(found.path).filter(|path| !path.is_empty()),
            })
            .collect(),
    )
}

impl Guest for Component {
    /// Reaches nothing: asked of every link a person pastes, answered from the address alone.
    fn claims_url(url: String) -> bool {
        target::claim(&url).is_some()
    }

    fn crawl(url: String) -> Result<Vec<CrawledLink>, Failure> {
        match target::claim(&url) {
            Some(Target::Own { folder_id, region }) => crawl_own(folder_id, region),
            Some(Target::Public { code, region }) => crawl_public(&code, region),
            None => Err(refuse(messages::NOT_A_FOLDER, FailureKind::Unsupported)),
        }
    }
}

export!(Component);
