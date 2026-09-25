//! Target-independent LinkSnappy API logic: request building, response shapes and error
//! classification. Shared verbatim by the native (`native.rs`) and WebAssembly (`guest.rs`)
//! adapters so both report byte-identical failure codes and messages; neither `rd-core` nor
//! `wit-bindgen`'s generated types are used here, only `serde`/`serde_json`/`url`, which are
//! available on every target.
//!
//! IMPL-VERIFY summary (against JD's `LinkSnappyCom.java`, the living reference —
//! `svn_trunk/src/jd/plugins/hoster/LinkSnappyCom.java`, revision 52502, fetched in full):
//!
//! - **Credential shape (the brief's critical open question)**: the password is **not**
//!   MD5-hashed anywhere in JD's plugin — `loginAPI` sends it as a plain query parameter
//!   (`/api/AUTHENTICATE?username=...&password=...`, both `Encoding.urlEncode`d, nothing else).
//!   Two independent third-party implementations confirm the same: `resolveurl`'s Kodi plugin
//!   (`linksnappy.py`) authenticates the identical way, and pyLoad's `LinksnappyCom.py`
//!   `MultiHoster` plugin sends the *raw* password directly inside the `genLinks` JSON body with
//!   no hashing. `{{secret:linksnappy_password}}`'s raw value is used as-is.
//! - **`genLinks` credential embedding**: JD itself does **not** put `username`/`password` inside
//!   `genLinks` — `handleMultiHost` calls `loginAPI` once (which authenticates via
//!   `/api/AUTHENTICATE` and stores the resulting session cookies on the `Account`), then issues
//!   every subsequent call (`linkgen`, `FILEHOSTS`, `USERDETAILS`) bare, relying on that cookie
//!   session (`resolveurl`'s Kodi plugin does the same: it authenticates once into a cookie jar
//!   file and never repeats credentials afterwards). This plugin cannot replicate that: the host
//!   never hands a freshly-authenticated session's cookies back to the resolver for reuse within
//!   the same call (see `crates/rd-plugin-host/src/native/host.rs` — `cookies_get` only reads a
//!   *pre-provisioned* `cookie_ref` secret set up out of band, the resolver has no way to write
//!   one), and every other plugin in this workspace treats a WASM guest invocation as fully
//!   stateless. pyLoad's `LinksnappyCom.py` proves the alternative actually works against the live
//!   API: it calls `/api/linkgen` with **no** prior `AUTHENTICATE` at all, embedding `username`
//!   and `password` directly in the `genLinks` JSON (`{"link":..,"type":..,"username":..,
//!   "password":..}`) as a single stateless call. This plugin follows pyLoad's proven shape for
//!   `genLinks` (see [`gen_links_json`]), and — absent any confirmation either way — extends the
//!   same stateless per-call credential pattern to `USERDETAILS` (`check_account`), since no
//!   reference confirms or rules out that endpoint accepting direct credentials outside of a
//!   cookie session. **This one part is unverified beyond the pyLoad `genLinks` precedent; flag
//!   for live testing** (per the brief: report `DONE_WITH_CONCERNS`).
//! - **`type` field**: JD always sends `type` (empty string `""` for a premium/Elite account, the
//!   only account type this plugin supports — free accounts use the website, not the API, and
//!   `handleFree` throws `AccountRequiredException` unconditionally). pyLoad instead sends the
//!   target host's domain as `type`. This plugin follows JD (`type=""`, letting the server
//!   auto-detect the host from the `link` field), since JD is the more current/authoritative
//!   reference and an empty `type` is the value JD's own production code sends today.
//! - **`FILEHOSTS` auth**: JD's `LinkSnappyCom.java` never demonstrates an unauthenticated call
//!   to this endpoint. `api_fetchAccountInfo` (the only caller) reads `loginAPI(account, force)`'s
//!   result first, then calls `br.getPage("/api/FILEHOSTS")` bare, relying on the cookie session
//!   `loginAPI` just established (`loginAPI` itself ends with a `USERDETAILS` call, matching the
//!   review finding that FILEHOSTS is only ever reached after `USERDETAILS` succeeds). JD never
//!   shows or documents `/api/FILEHOSTS` working without that prior login. (An earlier draft of
//!   this plugin treated the endpoint as an unauthenticated public catalogue by analogy with
//!   `resolveurl`'s *sibling* `FILEHOSTSREALTIME` endpoint — a different endpoint, not evidence
//!   about `FILEHOSTS` itself, and a weaker signal than JD's own never-called-bare pattern. That
//!   analogy has been dropped.) `hosters()` now sends the same `username`/`password` credential
//!   pair as `USERDETAILS` (this plugin has no cookie session to lean on either) and is
//!   secret-gated exactly like every other authenticated call.
//! - **Host alias field name**: the task brief guessed `Aliases`; JD never parses the field at all
//!   (it resolves aliases through its own internal registry, not from the JSON). `resolveurl`'s
//!   Kodi `get_hosts()` is the only reference that actually reads it — the real key is lowercase
//!   `alias` (`d['alias']`). This plugin uses `alias`, correcting the brief.
//! - **Error classification**: every branch in [`classify_message`] mirrors JD's `handleErrors`
//!   in its exact `if`/`else if` order; the full per-branch enumeration with JD line references
//!   lives in `api/tests.rs`'s module doc, next to the tests that assert each one. One branch
//!   (`HOST_UNSUPPORTED`) has no JD counterpart at all and is flagged as such in its own doc
//!   comment in `messages.rs` — it exists only because the task brief explicitly asked for it.
//!   Two more corrections/deviations from the task brief, both JD-verified:
//!   - The brief guessed the bad-credentials string as "Invalid username or password"; JD's
//!     actual string (`handleErrors`) is **"Incorrect Username or Password"**. [`classify_message`]
//!     matches JD's exact wording, not the brief's guess.
//!   - [`messages::PASSWORD_PROTECTED`] is mapped `Permanent`, while JD's own exception for the
//!     same message (`This file requires password`) is `PluginException(ERROR_RETRY, ...)` — a
//!     retry, not a hard failure. JD can retry because it re-prompts the user for the file's
//!     download password (`getUserInput`) and resubmits; `ResolveRequest` carries no such
//!     password field for this plugin to collect or resend, so a bare retry would just repeat the
//!     same unanswerable request forever. `Permanent` is used instead — a deliberate deviation
//!     from JD's retry behavior, not an oversight.
//! - **`size` field**: the brief asks for a `size` field on a `genLinks` link entry; no reference
//!   (JD, `resolveurl`, or pyLoad) reads such a field, so its existence is unconfirmed. Modeled
//!   here as optional and simply left `None` if absent — same risk profile as leaving it out.
//! - **Known limitation**: `genLinks` is sent as a `GET` query parameter (matching JD and
//!   `resolveurl`, and the brief), not a JSON request body. The host only JSON-string-escapes a
//!   `{{username}}`/`{{secret:...}}` substitution when it lands inside an `application/json`
//!   request *body* (see `crates/rd-plugin-host/src/native/expand.rs`); a query value is expanded
//!   raw and then percent-encoded as a whole. If a username or password itself contains a literal
//!   `"` character, the resulting `genLinks` JSON becomes malformed. This is an inherent
//!   consequence of using a query parameter for a JSON payload and cannot be worked around from
//!   this module.

use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::messages;

/// `rd-provider-registry`'s `linksnappy` row: `secret_reference`.
pub(crate) const PASSWORD_REFERENCE: &str = "linksnappy_password";

pub(crate) const API_BASE: &str = "https://linksnappy.com/api";

/// LinkSnappy is a multihoster: it claims any http(s) URL (mirrors `premiumize`/`debridlink`).
pub(crate) fn matches(scheme: &str) -> bool {
    matches!(scheme, "http" | "https")
}

/// Builds the `genLinks` query value: `{"link":"<url>","type":"","username":"{{username}}",
/// "password":"{{secret:linksnappy_password}}"}`. The `{{username}}`/`{{secret:...}}` markers are
/// literal template text the host expands later (see the module doc's IMPL-VERIFY note on why
/// they are embedded here rather than relying on a cookie session); `link` is the only piece of
/// untrusted text this function itself inserts into the JSON, so it alone needs JSON-escaping.
pub(crate) fn gen_links_json(link: &str) -> String {
    format!(
        r#"{{"link":"{}","type":"","username":"{{{{username}}}}","password":"{{{{secret:{}}}}}"}}"#,
        json_escape(link),
        PASSWORD_REFERENCE
    )
}

/// Escapes `"`, `\` and control characters so a value can be embedded in a JSON string literal.
fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                escaped.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

/// Generic `{"status": "OK"|"ERROR", "error": <string|any>, "return": T}` envelope every
/// LinkSnappy endpoint answers with (JD's `getError()`/`handleErrors()`).
#[derive(Deserialize)]
pub(crate) struct Envelope<T> {
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(default)]
    pub(crate) error: Option<Value>,
    #[serde(rename = "return", default = "Option::default")]
    pub(crate) value: Option<T>,
}

/// `GET /api/linkgen` top-level response; `links` is absent when the call fails before it ever
/// tries to generate a link (e.g. bad credentials) — JD's `handleMultiHost` reads the top-level
/// `status`/`error` pair directly in that case.
#[derive(Deserialize)]
pub(crate) struct GenLinksResponse {
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(default)]
    pub(crate) error: Option<Value>,
    #[serde(default)]
    pub(crate) links: Option<Vec<LinkEntry>>,
}

/// One entry of `links`; carries its own `status`/`error` pair (the same shape as the top-level
/// envelope — JD's `getError()` is called on either interchangeably).
#[derive(Deserialize)]
pub(crate) struct LinkEntry {
    #[serde(default)]
    pub(crate) status: Option<String>,
    #[serde(default)]
    pub(crate) error: Option<Value>,
    #[serde(default)]
    pub(crate) generated: Option<String>,
    #[serde(default)]
    pub(crate) filename: Option<String>,
    /// Unconfirmed by any reference — see the module doc's IMPL-VERIFY note.
    #[serde(default)]
    pub(crate) size: Option<u64>,
}

/// `return` of `GET /api/USERDETAILS`.
#[derive(Default, Deserialize)]
pub(crate) struct UserDetails {
    #[serde(default)]
    pub(crate) expire: Option<Value>,
    #[serde(default)]
    pub(crate) trafficleft: Option<Value>,
}

/// One entry of `return` of `GET /api/FILEHOSTS`: keyed by domain, `alias` is a lowercase JSON
/// key (see the module doc's IMPL-VERIFY note correcting the brief's `Aliases` guess).
#[derive(Deserialize)]
pub(crate) struct HostEntry {
    #[serde(default)]
    pub(crate) alias: Vec<String>,
}

/// Lower-cases, deduplicates and sorts every hoster domain and alias from the `FILEHOSTS` map
/// (mirrors `debridlink::merge_hosters`/`alldebrid::merge_hosters`).
pub(crate) fn merge_hosters(entries: std::collections::HashMap<String, HostEntry>) -> Vec<String> {
    let mut hosters: Vec<String> = entries
        .into_iter()
        .flat_map(|(domain, entry)| std::iter::once(domain).chain(entry.alias))
        .map(|host| host.trim().to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .collect();
    hosters.sort();
    hosters.dedup();
    hosters
}

/// `true` when `expire` indicates an active plan (`"lifetime"`, the lifetime sentinel epoch
/// `"2177388000"`, or any other numeric epoch/number); `false` for the literal string `"expired"`
/// or a missing/unrecognized value. Unlike JD's `fetchAccountInfo`, this does **not** compare the
/// numeric epoch against the current time: neither this crate's WASM guest target (no host clock
/// function in `rdownloader.wit`) nor, for native/guest parity, `native.rs` do that comparison —
/// see `lib.rs`'s module doc for the full reasoning. This can report `premium:
/// true` for a numeric `expire` that has, in reality, already passed; `resolve()`'s own
/// `ACCOUNT_EXPIRED` handling (triggered by LinkSnappy's live "Your Account has Expired" message)
/// is the authoritative signal for that case.
pub(crate) fn is_premium(expire: Option<&Value>) -> bool {
    match expire {
        Some(Value::String(text)) => !text.eq_ignore_ascii_case("expired"),
        Some(Value::Number(_)) => true,
        _ => false,
    }
}

/// What `USERDETAILS`'s `return.expire` field says about the subscription.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Subscription {
    /// The literal `lifetime`: a subscription that never runs out.
    Lifetime,
    /// The literal `expired`.
    Expired,
    /// A running subscription with an expiry the label does not state, since it is an epoch
    /// this plugin cannot compare (see `is_premium`).
    Elite,
    /// Nothing the label can say.
    Unknown,
}

/// Classifies `return.expire` for the account label.
pub(crate) fn subscription(expire: Option<&Value>) -> Subscription {
    match expire {
        Some(Value::String(text)) if text.eq_ignore_ascii_case("lifetime") => {
            Subscription::Lifetime
        }
        Some(Value::String(text)) if text.eq_ignore_ascii_case("expired") => Subscription::Expired,
        Some(_) if is_premium(expire) => Subscription::Elite,
        _ => Subscription::Unknown,
    }
}

/// `return.trafficleft`: `None` for the literal string `"unlimited"` (JD: "mostly \"unlimited\"")
/// or a missing/unrecognized value, otherwise the numeric byte count (negative values clamped to
/// `0`, mirroring JD's `if (trafficleft <= 0) ac.setTrafficLeft(0)`).
pub(crate) fn traffic_left(trafficleft: Option<&Value>) -> Option<u64> {
    match trafficleft {
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

/// Validates the raw download URL string a `genLinks` link entry's `generated` field carries.
/// Shared so a malformed URL from the API produces the exact same `linksnappy.invalid_url`
/// failure on both the native and guest adapters instead of one erroring and the other silently
/// forwarding a URL that later fails to parse deeper in the pipeline.
pub(crate) fn parse_download_url(raw: &str) -> Result<Url, ApiFailure> {
    Url::parse(raw).map_err(|error| ApiFailure {
        kind: ErrorKind::Permanent,
        code: messages::INVALID_URL,
        message: messages::invalid_url(&error),
        params: vec![("error", error.to_string())],
    })
}

/// Failure classification independent of the native (`rd_core::Failure`) and WASM
/// (WIT-generated `Failure`) representations; both adapters convert this into their own type.
#[derive(Debug)]
pub(crate) struct ApiFailure {
    pub(crate) kind: ErrorKind,
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) params: Vec<(&'static str, String)>,
}

/// Mirrors `rd_core::FailureKind` / the WIT `failure-kind` variant, without depending on either.
#[derive(Debug)]
pub(crate) enum ErrorKind {
    Transient(Option<u64>),
    Permanent,
    Offline,
    #[allow(dead_code)] // Raised directly by the adapters' `require_secret` gate, bypassing
    // `classify_message` entirely — no JD-known message maps to this variant.
    AuthRequired,
    AccountInvalid,
    RateLimited(Option<u64>),
    #[allow(dead_code)] // LinkSnappy's JSON API never challenges with a captcha.
    NeedsCaptcha,
    #[allow(dead_code)] // `HOST_UNSUPPORTED` uses `Unsupported`, but `matches()` accepts every
    // http(s) URL, so nothing in this plugin's own request-building rejects a link up front.
    Unsupported,
}

fn coded(kind: ErrorKind, (code, message): (&'static str, &str)) -> ApiFailure {
    ApiFailure {
        kind,
        code,
        message: message.to_owned(),
        params: Vec::new(),
    }
}

/// Classifies a non-`OK` `status`/`error` message. Mirrors JD's `handleErrors` in its exact
/// `if`/`else if` order — see `api/tests.rs`'s module doc for the full per-branch enumeration
/// with JD line references. `has_link` mirrors whether JD's call carries a non-null
/// `DownloadLink` (`true` throughout `resolve()`, `false` for `check_account`/`hosters()`), which
/// changes the retry delay of the final catch-all branch only (see [`messages::API_ERROR`]).
pub(crate) fn classify_message(message: &str, has_link: bool) -> ApiFailure {
    let lower = message.to_ascii_lowercase();
    if lower.contains("two-factor verification required") {
        return coded(
            ErrorKind::RateLimited(Some(300)),
            messages::TWO_FACTOR_REQUIRED,
        );
    }
    if lower.contains("no server available for this filehost") {
        return coded(ErrorKind::Transient(Some(300)), messages::HOST_UNAVAILABLE);
    }
    if lower.contains("you have reached max download request") {
        return coded(
            ErrorKind::RateLimited(Some(300)),
            messages::TOO_MANY_REQUESTS,
        );
    }
    if lower.contains("you have reached max download limit of") {
        return coded(ErrorKind::RateLimited(Some(60)), messages::LIMIT_REACHED);
    }
    if lower.contains("invalid file url format") {
        return coded(ErrorKind::Transient(None), messages::INVALID_LINK_FORMAT);
    }
    if lower.contains("file not found") || lower.contains("file deleted on") {
        return coded(ErrorKind::Offline, messages::FILE_OFFLINE);
    }
    if lower.contains("your account has expired") {
        return coded(ErrorKind::RateLimited(Some(300)), messages::ACCOUNT_EXPIRED);
    }
    if lower == "this file requires password" {
        return coded(ErrorKind::Permanent, messages::PASSWORD_PROTECTED);
    }
    if lower.contains("please upgrade to elite membership") {
        return coded(
            ErrorKind::RateLimited(Some(600)),
            messages::PREMIUM_REQUIRED,
        );
    }
    if lower.contains("incorrect username or password") {
        return coded(ErrorKind::AccountInvalid, messages::BAD_CREDENTIALS);
    }
    if lower.contains("account has exceeded the daily quota") {
        return coded(ErrorKind::RateLimited(Some(300)), messages::LIMIT_REACHED);
    }
    if lower.contains("not supported") {
        return coded(ErrorKind::Unsupported, messages::HOST_UNSUPPORTED);
    }
    ApiFailure {
        kind: if has_link {
            ErrorKind::Transient(Some(300))
        } else {
            ErrorKind::RateLimited(Some(600))
        },
        code: messages::API_ERROR,
        message: messages::api_error(message),
        params: vec![("message", message.to_owned())],
    }
}

/// JD's `getError()`: `None` when `status` is `"OK"` (case-insensitive) or both `status` and
/// `error` are absent; otherwise the error text — the `error` field verbatim if it is a JSON
/// string, else a synthetic `unknown/<status>/<error>` placeholder (JD's own fallback for a
/// non-string `error` value).
fn error_message(status: Option<&str>, error: Option<&Value>) -> Option<String> {
    if let Some(status) = status {
        if status.eq_ignore_ascii_case("OK") {
            return None;
        }
        return Some(error.and_then(Value::as_str).map_or_else(
            || {
                format!(
                    "unknown/{status}/{}",
                    error.map_or_else(|| "null".to_owned(), Value::to_string)
                )
            },
            str::to_owned,
        ));
    }
    error.map(|error| {
        error
            .as_str()
            .map_or_else(|| format!("unknown/null/{error}"), str::to_owned)
    })
}

/// Checks a `status`/`error` pair and classifies any error; `None` for success. `has_link` — see
/// [`classify_message`].
pub(crate) fn error_from_envelope(
    status: Option<&str>,
    error: Option<&Value>,
    has_link: bool,
) -> Option<ApiFailure> {
    error_message(status, error).map(|message| classify_message(&message, has_link))
}

/// Maps a bare HTTP status whose body did not parse as an [`Envelope`]/[`GenLinksResponse`].
/// `429`/`5xx` mirror plugin-common's stated HTTP conventions; `425` reuses JD's
/// `handleDownloadErrors` "still caching, retry" semantics defensively for the JSON API (see the
/// module doc's IMPL-VERIFY note on `SERVER_ERROR`).
pub(crate) fn ensure_http_status(status: u16) -> Result<(), ApiFailure> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(coded(ErrorKind::AccountInvalid, messages::BAD_CREDENTIALS)),
        404 | 410 | 451 => Err(coded(ErrorKind::Offline, messages::FILE_OFFLINE)),
        425 => Err(coded(
            ErrorKind::Transient(Some(60)),
            messages::SERVER_ERROR,
        )),
        429 => Err(coded(ErrorKind::RateLimited(None), messages::RATE_LIMITED)),
        500..=599 => Err(coded(ErrorKind::Transient(None), messages::SERVER_ERROR)),
        other => Err(ApiFailure {
            kind: ErrorKind::Permanent,
            code: messages::HTTP_ERROR,
            message: messages::http_error(other),
            params: vec![("status", other.to_string())],
        }),
    }
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;
