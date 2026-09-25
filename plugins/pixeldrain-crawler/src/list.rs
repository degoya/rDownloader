//! What this plugin claims, and what `GET /api/list/{id}` says.
//!
//! No host is touched here, so all of it runs under an ordinary `cargo test` without a
//! WebAssembly toolchain. The answer's shape, measured 2026-09-22 (RD-120-07):
//!
//! ```json
//! {"success":true,"id":"Lm4pQ2","title":"Season 1","date_created":"2026-09-01T10:00:00Z",
//!  "file_count":2,"files":[{"id":"Ab3xY9Zq","name":"a.bin","size":4096}, ...]}
//! ```
//!
//! A refusal uses the same envelope every Pixeldrain endpoint does:
//! `{"success":false,"value":"list_not_found","message":"..."}`. The `value` token is the stable
//! code; the `message` is prose and never travels.

use serde::Deserialize;

pub const API_BASE: &str = "https://pixeldrain.com/api";

/// Most entries one list contributes. A list is user-supplied and a crawler that returned an
/// unbounded number would put an unbounded number of rows into the LinkGrabber; the cap is
/// generous enough that no real album reaches it and finite either way.
pub const MAX_ENTRIES: usize = 5000;

/// One file in a list.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ListEntry {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// `GET /api/list/{id}`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FileList {
    #[serde(default)]
    pub success: Option<bool>,
    /// The stable refusal token, present only on a refusal.
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub files: Vec<ListEntry>,
}

/// One candidate for the LinkGrabber.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Child {
    pub url: String,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    pub package_hint: Option<String>,
}

/// The list identifier in an address, or `None` when this plugin does not claim it.
///
/// Two shapes and no more: the share page `/l/{id}` and the API's own `/api/list/{id}`. A file
/// address belongs to `plugins/pixeldrain/` and is deliberately left alone here, so exactly one
/// of the two plugins answers for any given address.
#[must_use]
pub fn list_id(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed
        .host_str()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host != "pixeldrain.com" && host != "www.pixeldrain.com" {
        return None;
    }
    let segments: Vec<&str> = parsed
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    let id = match segments.as_slice() {
        ["l", id] | ["api", "list", id] => *id,
        _ => return None,
    };
    let valid = !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric());
    valid.then(|| (*id).to_owned())
}

/// The address a list's contents are read from.
#[must_use]
pub fn list_url(id: &str) -> String {
    format!("{API_BASE}/list/{id}")
}

/// The share address of one file, which the sibling resolver claims.
///
/// Deliberately the `/u/` page rather than `/api/file/{id}`: it is the address a person
/// recognises in the LinkGrabber, and the resolver reads the same identifier out of either.
#[must_use]
pub fn file_url(id: &str) -> String {
    format!("https://pixeldrain.com/u/{id}")
}

/// The candidates a list answer describes.
///
/// An entry without an identifier contributes nothing: there is no address to build from it,
/// and inventing one would put a row in the LinkGrabber that can never resolve. A name is
/// carried over when the provider stated one and left empty otherwise -- guessing it from the
/// identifier would put a wrong name in front of the right file.
#[must_use]
pub fn children(list: &FileList) -> Vec<Child> {
    let package_hint = list
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned);
    list.files
        .iter()
        .filter_map(|entry| {
            let id = entry.id.as_deref().map(str::trim)?;
            let valid =
                !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric());
            valid.then(|| Child {
                url: file_url(id),
                file_name: entry
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned),
                size: entry.size,
                package_hint: package_hint.clone(),
            })
        })
        .take(MAX_ENTRIES)
        .collect()
}

/// The stable refusal token an answer carries, held to a code shape.
///
/// It arrives over the network and ends up in front of a person, so it is checked rather than
/// trusted: anything longer or carrying punctuation is dropped whole rather than filtered.
#[must_use]
pub fn refusal_token(list: &FileList) -> Option<String> {
    if list.success != Some(false) {
        return None;
    }
    let value = list.value.as_deref()?.trim();
    let code_shaped = !value.is_empty()
        && value.len() <= 48
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    Some(if code_shaped {
        value.to_ascii_lowercase()
    } else {
        String::new()
    })
}

/// The list an answer describes, or `None` when the body is not a JSON object at all.
///
/// Only an object can be a list or a refusal. Serde reads a struct out of a *sequence* as
/// readily as out of a map, taking elements in field order, so an array answer would otherwise
/// invent `success` and `value` out of its first two elements.
#[must_use]
pub fn parse(body: &[u8]) -> Option<FileList> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .filter(serde_json::Value::is_object)
        .and_then(|value| serde_json::from_value(value).ok())
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
