//! Target-independent Keep2Share API v2 logic: URL matching, request-body construction, response
//! shapes and error classification. Shared verbatim by the native (`native.rs`) and WebAssembly
//! (`guest.rs`) adapters so both report byte-identical failure codes and messages and send
//! byte-identical request bodies; neither `rd-core` nor `wit-bindgen`'s generated types are used
//! here, only `serde`/`serde_json`/`url`, which are available on every target.
//!
//! IMPL-VERIFY summary (against JD's `Keep2ShareCc.java` + its base class `K2SApi.java`, the
//! living reference — `svn_trunk/src/jd/plugins/hoster/{Keep2ShareCc,K2SApi}.java`, revisions
//! 50178/53214, fetched in full, plus `Keep2ShareCcDecrypter.java` for the domain/URL-pattern
//! question):
//!
//! - **API host (corrects the brief and the pre-existing registry row)**: `K2SApi.getApiUrl()`
//!   is `getProtocol() + getInternalAPIDomain() + "/api/v2"`, and `Keep2ShareCc.getInternalAPIDomain()`
//!   hardcodes `"k2s.cc"` — every API call (`/login`, `/geturl`, `/accountinfo`, `/getfilesinfo`)
//!   targets `k2s.cc` specifically, regardless of which mirror domain (`keep2share.cc`, `k2share.cc`,
//!   ...) the original file link used. The brief's guessed domains (`keep2share.cc`, `api.k2s.cc`)
//!   are never actually contacted by this plugin.
//! - **Endpoint paths are all lowercase** (the brief guessed camelCase for three of the four):
//!   `/login` (brief matches), `/geturl` (brief guessed `/getUrl`), `/accountinfo` (brief guessed
//!   `/accountInfo`), `/getfilesinfo` (brief guessed `/getFilesInfo`). JD wins per plugin-common.
//! - **`/geturl` premium request body**: JD's `handleDownload` only adds `captcha_challenge`/
//!   `captcha_response`/`free_download_key` when `isFree` (`!isPremium(account)` — true both for no
//!   account *and* for a logged-in but non-premium account) — for a logged-in call (`account !=
//!   null`) the body sent to `/geturl` is always exactly `{"file_id":..,"auth_token":..}`, regardless
//!   of whether that account turns out to be premium; a non-premium account simply gets a
//!   `premium_required`-flavored error back from this same request instead of a different request
//!   shape. This plugin only ever calls `/geturl` with a real, logged-in account, so it always sends
//!   this body shape; the brief's speculative "may need `free_download_key`/`captcha_challenge`
//!   fields as null" is not needed and is not sent.
//! - **`resolve()` shape**: the brief's flow section lists only `login` → `/geturl` for `resolve`
//!   (no separate file-info call), matching JD's own two-call sequence for a premium account with
//!   no cached direct-URL (`getAuthToken` then `/geturl`); `/geturl`'s response never carries a
//!   filename or size (only `url`/`free_download_key`/`time_wait`), so `resolve()` here returns
//!   `file_name: None`/`size: None`, exactly like `/geturl`'s own payload.
//! - **`check()` batch body**: the brief asks for `{"ids": [...]}` only; JD's `checkLinks` always
//!   adds `"extended_info": true` (`/getfilesinfo`'s docs: `extended_info` unlocks `video_info`).
//!   This plugin never reads `video_info`, so the field is omitted — a deliberate simplification
//!   with no effect on the `is_available`/`name`/`size` fields this plugin actually parses.
//! - **`check()` needs no login at all (corrects the brief)**: JD's `checkLinks` calls
//!   `postPageRaw(br, "/getfilesinfo", postdata, null)` — the `account` argument is a literal
//!   `null`, and `postdata` never gets an `auth_token` added. `/getfilesinfo` is genuinely
//!   unauthenticated in JD, unlike every other endpoint this plugin calls. The brief's flow
//!   section ("check: login once → ...") is not followed here: `check()` requires neither an
//!   account nor the secret gate.
//! - **`check()` batching**: JD's `checkLinks` chunks at 100 fileIDs per `/getfilesinfo` call and
//!   loops ("Check up to 100 fileIDs with one request", `K2SApi.java:476-485`) — mirrored in
//!   `native.rs`/`guest.rs` (not in this cfg-free module, since it owns the HTTP round-trip loop,
//!   which differs between the native `async` adapter and the guest's synchronous one). A chunk
//!   whose request fails degrades only that chunk's links to `Unknown`, not the whole batch.
//! - **Sister-site question (settled)**: `Keep2ShareCcDecrypter.getPluginDomains()` registers
//!   `domainsK2s`, `domainsFileboom` and `domainsTezfilesAndPublish2` as three *separate*
//!   `List<String[]>` entries — the class doc comment says each entry becomes its own
//!   `PluginForHost` with its own `getHost()`. `fileboom.me`/`fboom.me`/`tezfiles.com`/
//!   `publish2.me` are therefore separate JD hosters with separate accounts, not Keep2Share
//!   aliases. [`MATCH_HOSTS`] only carries `domainsK2s`'s members (minus `keep2.cc`, which JD's
//!   own `getDeadDomains()` excludes).
//! - **`matches()` pattern**: JD's `SUPPORTED_LINKS_PATTERN_FILE` is
//!   `(?i)/(?:f|file|preview)/(?:info/)?([a-z0-9_\-]{13,})(/([^/\?]+))?.*` — a `/f/`, `/file/` or
//!   `/preview/` prefix, an optional `info/` segment, then a 13+ character id (letters/digits/`_`/
//!   `-`). The brief's guess (`/file/<id>[/...]`) covered only one of the three prefixes and
//!   omitted the `info/` segment and the 13-character minimum; [`file_id`] follows JD's full
//!   pattern. The sibling `SUPPORTED_LINKS_PATTERN_FOLDER` (`/folder/...`) is deliberately not
//!   matched here — this plugin resolves single files only, like every other hoster plugin in
//!   this workspace (folder/container expansion is a decrypter's job, not modeled here).
//! - **Error classification**: `handleErrorsAPI`'s full per-branch enumeration (JD lines
//!   1350-1589), with every arm's JD line citation and this module's mapping decision, lives in
//!   `api/tests.rs`'s module doc, next to the tests that assert the ones a premium, already-logged-in
//!   flow can actually reach.
//! - **`check_account` premium/expiry (a documented limitation)**: JD compares `account_expires`
//!   (epoch seconds) against `System.currentTimeMillis()` to detect an *expired* premium account
//!   that still carries an old expiry date. `rdownloader.wit` exposes no host clock function (the
//!   same constraint `plugins/linksnappy`'s `api::is_premium` documents for the identical reason),
//!   so this module cannot replicate that comparison on either target. [`is_premium`] instead
//!   reports `true` whenever `account_expires` is a JSON number at all (JD's own signal for "this
//!   account has ever had premium up to some earlier-or-later date"; a genuinely free account gets
//!   the literal `false`, not a number — see JD's comment above `fetchAccountInfo`'s `account_expires`
//!   read). A past-but-numeric expiry therefore reports `premium: true` here; a live traffic/`/geturl`
//!   call against such an account fails through the normal error path instead (JD itself treats a
//!   truly exhausted account the same way, via `/geturl`'s or `/accountinfo`'s own error response).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::messages;

pub(crate) const API_BASE: &str = "https://k2s.cc/api/v2";

/// `{{username}}`/`{{secret:...}}` bodies must declare this content type for the host to
/// JSON-string-escape the substituted value (see `crates/rd-plugin-host/src/native/expand.rs`).
pub(crate) const CONTENT_TYPE_JSON: &str = "application/json";

/// Bare hostnames (no `www.` prefix) this hoster's file links carry — JD's `domainsK2s`, minus
/// `keep2.cc` (JD's own `getDeadDomains()`); see the module doc's sister-site IMPL-VERIFY note for
/// why `fileboom.me`/`tezfiles.com` are not here.
pub(crate) const MATCH_HOSTS: &[&str] = &["k2s.cc", "keep2share.cc", "k2share.cc", "keep2s.cc"];

const USERNAME_MARKER: &str = "{{username}}";

/// Extracts the file id from a Keep2Share link, e.g. `https://k2s.cc/file/<id>/name.html`,
/// `https://k2s.cc/f/<id>` or `https://k2s.cc/file/info/<id>`. Mirrors JD's
/// `SUPPORTED_LINKS_PATTERN_FILE` — see the module doc's IMPL-VERIFY note.
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
    Url::parse(raw).map_err(|error| ApiFailure {
        kind: ErrorKind::Permanent,
        code: messages::INVALID_URL,
        message: messages::invalid_url(&error),
        params: vec![("error", error.to_string())],
    })
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
/// `free_download_key`/`time_wait` in JD's free-download flow, which this plugin never takes).
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
    /// JD's `parseFileInfo`: available unless explicitly marked unavailable or deleted.
    pub(crate) fn is_online(&self) -> bool {
        self.is_available != Some(false) && self.is_deleted != Some(true)
    }

    /// Whether `fuid` (the id taken from the requested URL) refers to this entry — either
    /// directly or via its `requested_id` alias (JD: some files have multiple/legacy ids).
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

/// Formats a Unix timestamp (seconds) as a UTC `YYYY-MM-DD` date, using Howard Hinnant's
/// `civil_from_days` algorithm (pure integer arithmetic — `chrono` is a native-only dependency in
/// this workspace's plugin convention, and this module must stay usable from the WASM guest too).
fn civil_date(epoch_seconds: i64) -> String {
    let days = epoch_seconds.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// <http://howardhinnant.github.io/date_algorithms.html#civil_from_days>; `z` is a day count
/// relative to the Unix epoch (1970-01-01 = day 0).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

mod errors;

pub(crate) use errors::*;

pub(crate) mod free;

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;
