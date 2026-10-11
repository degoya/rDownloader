//! Target-independent Keep2Share API v2 logic: URL matching, request-body construction, response
//! shapes and error classification. Shared verbatim by the native (`native.rs`) and WebAssembly
//! (`guest.rs`) adapters so both report byte-identical failure codes and messages and send
//! byte-identical request bodies; neither `rd-core` nor `wit-bindgen`'s generated types are used
//! here, only `serde`/`serde_json`/`url`, which are available on every target.
//!
//! IMPL-VERIFY summary (against the known handling of the API, plus the domain/URL-pattern
//! question):
//!
//! - **API host (corrects the brief and the pre-existing registry row)**: the API lives at
//!   `https://k2s.cc/api/v2` — every API call (`/login`, `/geturl`, `/accountinfo`,
//!   `/getfilesinfo`) targets `k2s.cc` specifically, regardless of which mirror domain
//!   (`keep2share.cc`, `k2share.cc`, ...) the original file link used. The brief's guessed domains
//!   (`keep2share.cc`, `api.k2s.cc`) are never actually contacted by this plugin.
//! - **Endpoint paths are all lowercase** (the brief guessed camelCase for three of the four):
//!   `/login` (brief matches), `/geturl` (brief guessed `/getUrl`), `/accountinfo` (brief guessed
//!   `/accountInfo`), `/getfilesinfo` (brief guessed `/getFilesInfo`). The known handling wins per
//!   plugin-common.
//! - **`/geturl` premium request body**: `captcha_challenge`/`captcha_response`/
//!   `free_download_key` are only added for a free download (no account *or* a logged-in but
//!   non-premium account) — for a logged-in call the body sent to `/geturl` is always exactly
//!   `{"file_id":..,"auth_token":..}`, regardless of whether that account turns out to be premium;
//!   a non-premium account simply gets a `premium_required`-flavored error back from this same
//!   request instead of a different request shape. This plugin only ever calls `/geturl` with a
//!   real, logged-in account, so it always sends this body shape; the brief's speculative "may need
//!   `free_download_key`/`captcha_challenge` fields as null" is not needed and is not sent.
//! - **`resolve()` shape**: the brief's flow section lists only `login` → `/geturl` for `resolve`
//!   (no separate file-info call), the two-call sequence for a premium account with no cached
//!   direct URL (an auth token, then `/geturl`); `/geturl`'s response never carries a filename or
//!   size (only `url`/`free_download_key`/`time_wait`), so `resolve()` here returns
//!   `file_name: None`/`size: None`, exactly like `/geturl`'s own payload.
//! - **`check()` batch body**: the brief asks for `{"ids": [...]}` only; `"extended_info": true` is
//!   also accepted (`/getfilesinfo`'s docs: `extended_info` unlocks `video_info`). This plugin
//!   never reads `video_info`, so the field is omitted — a deliberate simplification with no
//!   effect on the `is_available`/`name`/`size` fields this plugin actually parses.
//! - **`check()` needs no login at all (corrects the brief)**: `/getfilesinfo` answers without an
//!   account and without an `auth_token` in the body — it is genuinely unauthenticated, unlike
//!   every other endpoint this plugin calls. The brief's flow section ("check: login once → ...")
//!   is not followed here: `check()` requires neither an account nor the secret gate.
//! - **`check()` batching**: at most 100 fileIDs go into one `/getfilesinfo` call, looped over —
//!   in `native.rs`/`guest.rs` (not in this cfg-free module, since it owns the HTTP round-trip
//!   loop, which differs between the native `async` adapter and the guest's synchronous one). A
//!   chunk whose request fails degrades only that chunk's links to `Unknown`, not the whole batch.
//! - **Sister-site question (settled)**: the Keep2Share domains, the Fileboom domains and the
//!   Tezfiles/Publish2 domains are three *separate* hosters with separate accounts, not Keep2Share
//!   aliases (`fileboom.me`/`fboom.me`/`tezfiles.com`/`publish2.me`). [`MATCH_HOSTS`] only carries
//!   the Keep2Share domains (minus the dead `keep2.cc`).
//! - **`matches()` pattern**: a file link is
//!   `(?i)/(?:f|file|preview)/(?:info/)?([a-z0-9_\-]{13,})(/([^/\?]+))?.*` — a `/f/`, `/file/` or
//!   `/preview/` prefix, an optional `info/` segment, then a 13+ character id (letters/digits/`_`/
//!   `-`). The brief's guess (`/file/<id>[/...]`) covered only one of the three prefixes and
//!   omitted the `info/` segment and the 13-character minimum; [`file_id`] follows the full
//!   pattern. Folder links (`/folder/...`) are deliberately not matched here — this plugin
//!   resolves single files only, like every other hoster plugin in this workspace
//!   (folder/container expansion is a decrypter's job, not modeled here).
//! - **Error classification**: the full per-branch enumeration of the API's error codes, with
//!   this module's mapping decision for each, lives in `api/errors/tests.rs`'s module doc, next to
//!   the tests that assert the ones a premium, already-logged-in flow can actually reach.
//! - **`check_account` premium/expiry (a documented limitation)**: an *expired* premium account
//!   can still carry an old expiry date, which only a comparison of `account_expires` (epoch
//!   seconds) with the current time would catch. `rdownloader.wit` exposes no host clock function
//!   (the same constraint `plugins/linksnappy`'s `api::is_premium` documents for the identical
//!   reason), so this module cannot make that comparison on either target. [`is_premium`] instead
//!   reports `true` whenever `account_expires` is a JSON number at all (the signal for "this
//!   account has ever had premium up to some earlier-or-later date"; a genuinely free account gets
//!   the literal `false`, not a number). A past-but-numeric expiry therefore reports
//!   `premium: true` here; a live traffic/`/geturl` call against such an account fails through the
//!   normal error path instead (a truly exhausted account answers the same way, via `/geturl`'s or
//!   `/accountinfo`'s own error response).

use plugin_common::label::civil_date;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

pub(crate) const API_BASE: &str = "https://k2s.cc/api/v2";

/// `{{username}}`/`{{secret:...}}` bodies must declare this content type for the host to
/// JSON-string-escape the substituted value (see `crates/rd-plugin-host/src/native/expand.rs`).
pub(crate) const CONTENT_TYPE_JSON: &str = "application/json";

/// Bare hostnames (no `www.` prefix) this hoster's file links carry, minus the dead `keep2.cc`;
/// see the module doc's sister-site IMPL-VERIFY note for why `fileboom.me`/`tezfiles.com` are
/// not here.
pub(crate) const MATCH_HOSTS: &[&str] = &["k2s.cc", "keep2share.cc", "k2share.cc", "keep2s.cc"];

const USERNAME_MARKER: &str = "{{username}}";

/// Extracts the file id from a Keep2Share link, e.g. `https://k2s.cc/file/<id>/name.html`,
/// `https://k2s.cc/f/<id>` or `https://k2s.cc/file/info/<id>`, by the file-link pattern
/// — see the module doc's IMPL-VERIFY note.
pub(crate) fn file_id(url: &Url) -> Option<&str> {
    let host_str = url.host_str()?;
    let host = host_str.strip_prefix("www.").unwrap_or(host_str);
    if !MATCH_HOSTS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(host))
    {
        return None;
    }
    let mut segments = url.path().trim_start_matches('/').split('/');
    let prefix = segments.next()?;
    if !["f", "file", "preview"]
        .iter()
        .any(|candidate| prefix.eq_ignore_ascii_case(candidate))
    {
        return None;
    }
    let mut candidate = segments.next()?;
    if candidate.eq_ignore_ascii_case("info") {
        candidate = segments.next()?;
    }
    let is_valid_id = candidate.len() >= 13
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    is_valid_id.then_some(candidate)
}

/// Validates the raw download URL string `/geturl`'s `url` field carries. Shared so a malformed
/// URL from the API produces the exact same `keep2share.invalid_url` failure on both adapters
/// instead of one erroring and the other silently forwarding a URL that later fails to parse
/// deeper in the pipeline (or not at all, on the guest side, where `ResolvedDownload.url` is a
/// bare `String`).
pub(crate) fn parse_download_url(raw: &str) -> Result<Url, ApiFailure> {
    Url::parse(raw).map_err(|error| invalid_url(&error))
}

#[derive(Serialize)]
struct LoginRequest {
    username: &'static str,
    password: String,
}

/// `POST /login {"username":"{{username}}","password":"{{secret:keep2share_password}}"}` — the
/// literal template markers are the field values; the host expands and JSON-string-escapes them
/// (see `crates/rd-plugin-host/src/native/expand.rs`). Built with `serde_json` rather than hand
/// formatting so neither adapter can drift on escaping.
pub(crate) fn login_body() -> Vec<u8> {
    let payload = LoginRequest {
        username: USERNAME_MARKER,
        password: format!("{{{{secret:{}}}}}", crate::PASSWORD_REFERENCE),
    };
    serde_json::to_vec(&payload).expect("LoginRequest always serializes")
}

#[derive(Serialize)]
struct GetUrlRequest<'a> {
    file_id: &'a str,
    auth_token: &'a str,
}

/// `POST /geturl {"file_id":"<id>","auth_token":"<token>"}` — see the module doc's IMPL-VERIFY
/// note on why no captcha/free-download fields are sent.
pub(crate) fn geturl_body(file_id: &str, auth_token: &str) -> Vec<u8> {
    serde_json::to_vec(&GetUrlRequest {
        file_id,
        auth_token,
    })
    .expect("GetUrlRequest always serializes")
}

#[derive(Serialize)]
struct AccountInfoRequest<'a> {
    auth_token: &'a str,
}

/// `POST /accountinfo {"auth_token":"<token>"}`.
pub(crate) fn accountinfo_body(auth_token: &str) -> Vec<u8> {
    serde_json::to_vec(&AccountInfoRequest { auth_token })
        .expect("AccountInfoRequest always serializes")
}

#[derive(Serialize)]
struct GetFilesInfoRequest<'a> {
    ids: &'a [&'a str],
}

/// `POST /getfilesinfo {"ids":["<id1>",...]}` — see the module doc's IMPL-VERIFY note on the
/// omitted `extended_info` field.
pub(crate) fn getfilesinfo_body(ids: &[&str]) -> Vec<u8> {
    serde_json::to_vec(&GetFilesInfoRequest { ids }).expect("GetFilesInfoRequest always serializes")
}

/// `/login`'s success payload.
#[derive(Deserialize)]
pub(crate) struct LoginResult {
    #[serde(default)]
    pub(crate) auth_token: Option<String>,
}

/// `/geturl`'s success payload (only the field this plugin reads; also carries
/// `free_download_key`/`time_wait` in the free-download flow, which this plugin never takes).
#[derive(Deserialize)]
pub(crate) struct GetUrlResult {
    #[serde(default)]
    pub(crate) url: Option<String>,
}

/// `/accountinfo`'s success payload (only the fields this plugin reads).
#[derive(Deserialize)]
pub(crate) struct AccountInfoResult {
    #[serde(default)]
    pub(crate) available_traffic: Option<Value>,
    #[serde(default)]
    pub(crate) account_expires: Option<Value>,
}

/// `/getfilesinfo`'s success payload.
#[derive(Deserialize)]
pub(crate) struct GetFilesInfoResult {
    #[serde(default)]
    pub(crate) files: Vec<FileEntry>,
}

/// One `/getfilesinfo` (or `/getfilestatus`) file entry.
#[derive(Deserialize)]
pub(crate) struct FileEntry {
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) requested_id: Option<String>,
    #[serde(default)]
    pub(crate) is_available: Option<bool>,
    #[serde(default, rename = "isDeleted")]
    pub(crate) is_deleted: Option<bool>,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) size: Option<i64>,
}

impl FileEntry {
    /// Available unless explicitly marked unavailable or deleted.
    pub(crate) fn is_online(&self) -> bool {
        self.is_available != Some(false) && self.is_deleted != Some(true)
    }

    /// Whether `fuid` (the id taken from the requested URL) refers to this entry — either
    /// directly or via its `requested_id` alias (some files have multiple/legacy ids).
    pub(crate) fn matches_fuid(&self, fuid: &str) -> bool {
        self.id.as_deref() == Some(fuid) || self.requested_id.as_deref() == Some(fuid)
    }
}

/// `true` whenever `account_expires` is a JSON number — see the module doc's IMPL-VERIFY note on
/// why this does not compare against the current time.
pub(crate) fn is_premium(account_expires: Option<&Value>) -> bool {
    matches!(account_expires, Some(Value::Number(_)))
}

/// The subscription end the account label states, from `/accountinfo`'s `account_expires`
/// field: its civil date, and only while the account is premium.
pub(crate) fn premium_until(premium: bool, account_expires: Option<&Value>) -> Option<String> {
    account_expires
        .and_then(Value::as_i64)
        .filter(|_| premium)
        .map(civil_date)
}

/// `/accountinfo`'s `available_traffic`: the numeric byte count, negative values clamped to `0`.
pub(crate) fn traffic_left(available_traffic: Option<&Value>) -> Option<u64> {
    match available_traffic {
        Some(Value::Number(number)) => Some(
            number
                .as_i64()
                .map(|value| u64::try_from(value).unwrap_or(0))
                .or_else(|| number.as_u64())
                .unwrap_or(0),
        ),
        _ => None,
    }
}

mod errors;

pub(crate) use errors::*;

pub(crate) mod free;

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;
