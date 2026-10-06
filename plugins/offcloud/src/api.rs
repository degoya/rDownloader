//! Target-independent Offcloud logic: response shapes, the hoster catalogue merge and the
//! failure classification.
//!
//! Written against the provider's own API documentation, <https://offcloud.com/api> and the
//! repository it points at, <https://github.com/Offcloud/offcloud-api>:
//!
//! - **Auth flavour**: `Authorization: Bearer <api key>`. The published document also offers
//!   `?key=<api key>` as a query parameter, and the header is used instead for one reason: a
//!   query parameter ends up in every redirect chain, every proxy log and every error message
//!   that quotes an address, and a header does not. The plugin never sees the key either way —
//!   it writes `{{secret:offcloud_api_key}}` and the host substitutes the value on the way out,
//!   towards `offcloud.com` and nowhere else.
//! - **Resolve**: `POST /api/instant` with a form body carrying `url`. The answer carries the
//!   address to fetch (`url`), `fileName`, `site` and `status`. A fresh call is what renews a
//!   short-lived address: nothing here is cached, and Offcloud mints the link at the moment it
//!   is asked for.
//! - **Account**: `GET /api/account/info`, carrying `userId`, `isPremium`, `canDownload` and
//!   `expirationDate`. Both spellings of each field are accepted — the published examples and
//!   the field names JDownloader's `OffCloudCom.java` reads out of the same endpoint differ in
//!   case, and guessing wrong would silently report every account as free.
//! - **Hosters**: `GET /api/sites`, the catalogue the account's plan covers.
//! - **Refusals**: `{"error": "<sentence>"}`, and `{"not_available": "<reason>"}` when the
//!   account would need an add-on for this particular link. `NOAUTH` is the one stable word in
//!   the first shape; the reasons in the second are a documented, closed set. Everything else
//!   the provider writes is prose and is dropped rather than forwarded.
//!
//! **Nothing here has been run against a live Offcloud account.** The shapes come from the
//! provider's documentation and from two independent clients of it; `docs/roadmap/jobs/
//! 120-02-offcloud.md` records the run against a real account as open.

use offcloud_common::{BUSY_SECONDS, QUOTA_SECONDS, Words};
pub use offcloud_common::{ErrorEnvelope, error_envelope};
use plugin_common::failure::{ApiFailure, HttpError, HttpWords};
use serde::Deserialize;

use crate::messages;

/// The vault reference the Offcloud provider keeps its API key under. The value never reaches
/// this plugin.
pub const API_KEY_REFERENCE: &str = "offcloud_api_key";

pub const API_BASE: &str = "https://offcloud.com/api";

/// Whether a scheme is one a multihoster could be asked about at all.
#[must_use]
pub fn matches(scheme: &str) -> bool {
    matches!(scheme, "http" | "https")
}

/// `POST /api/instant`.
#[derive(Default, Deserialize)]
pub struct InstantDownload {
    /// The address to fetch. Short-lived; a new one is minted by asking again.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default, alias = "fileName", alias = "filename")]
    pub file_name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// `GET /api/account/info`.
///
/// Every field carries both spellings the two documented clients use. A `serde` alias costs
/// nothing and removes the one failure mode that would not show up as an error: a camel-cased
/// answer read through snake-cased fields is a valid parse in which every account is free.
#[derive(Default, Deserialize)]
pub struct AccountInfo {
    #[serde(default, alias = "userId")]
    pub user_id: Option<String>,
    #[serde(default, alias = "isPremium")]
    pub is_premium: Option<bool>,
    #[serde(default, alias = "canDownload")]
    pub can_download: Option<bool>,
    #[serde(default, alias = "expirationDate")]
    pub expiration_date: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

/// One entry of `GET /api/sites`.
///
/// Offcloud describes a site by a name and the hosts it answers for, and the two documented
/// clients disagree on whether the hosts arrive as one string or as a list. Both are read, and
/// an entry that carries neither is skipped rather than guessed at.
#[derive(Default, Deserialize)]
pub struct SiteEntry {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, alias = "displayName")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub domain: Option<String>,
}

/// How Offcloud's codes name an HTTP status no document in the answer explains: the classes
/// are `plugin_common::http_status`'s, the one mapping every plugin shares (RD-191-07); a `429`
/// or a `5xx` carries the response's `Retry-After` into the wait.
///
/// The waits fall back to the provider's own figures when the response named none: an hour for
/// a `429`, which is also how Offcloud reports an exhausted allowance, and five minutes for a
/// `5xx` (`offcloud_common::QUOTA_SECONDS`, `BUSY_SECONDS`).
pub const HTTP: HttpWords = HttpWords {
    unauthorized: messages::AUTH_INVALID,
    gone: messages::LINK_GONE,
    unavailable: messages::LINK_GONE,
    rate_limited: messages::RATE_LIMITED,
    server_error: messages::SERVER_ERROR,
    rate_limited_wait: Some(QUOTA_SECONDS),
    server_error_wait: Some(BUSY_SECONDS),
    other: HttpError {
        code: messages::HTTP_ERROR.0,
        text: messages::http_error,
    },
};

/// The words this plugin reports a refusal under; the rules are `offcloud_common`'s, shared
/// with `plugins/offcloud-cloud/`.
pub const WORDS: Words = Words {
    http: HTTP,
    auth_invalid: messages::AUTH_INVALID,
    api_error: messages::API_ERROR,
    addon_required: messages::ADDON_REQUIRED,
};

/// The failure an answer describes, or `None` when it describes none: see
/// [`offcloud_common::failure_from`] for the order a word, a status and prose are believed in.
#[must_use]
pub fn failure_from(
    status: u16,
    retry_after: Option<u64>,
    envelope: &ErrorEnvelope,
) -> Option<ApiFailure> {
    offcloud_common::failure_from(status, retry_after, envelope, &WORDS)
}

/// `application/x-www-form-urlencoded` body, as the published API asks for its parameters.
///
/// Percent-encodes by hand rather than pulling a URL crate in for one field: the one value
/// that travels this way is an address, and a body that did not encode its `&` and `=` would
/// submit a truncated one.
#[must_use]
pub fn form_body(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut body = String::new();
    for (name, value) in pairs {
        if !body.is_empty() {
            body.push('&');
        }
        encode_into(&mut body, name);
        body.push('=');
        encode_into(&mut body, value);
    }
    body.into_bytes()
}

fn encode_into(body: &mut String, value: &str) {
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                body.push(char::from(*byte));
            }
            _ => {
                use std::fmt::Write;
                // Writing into a String cannot fail; the result is discarded rather than
                // unwrapped, because `unwrap_used` is denied outside tests.
                let _ = write!(body, "%{byte:02X}");
            }
        }
    }
}

/// The hoster catalogue, flattened to lower-case hosts and deduplicated.
///
/// One site is one row at Offcloud and several domains at the hoster it stands for, and the
/// queue matches on hosts. An entry that names no host at all contributes nothing: a
/// catalogue row is only useful here if it says which addresses it covers.
#[must_use]
pub fn merge_hosters(entries: Vec<SiteEntry>) -> Vec<String> {
    let mut hosts: Vec<String> = entries
        .into_iter()
        .flat_map(|entry| {
            entry
                .hosts
                .into_iter()
                .chain(entry.domains)
                .chain(entry.domain)
                .chain(entry.display_name.filter(|value| value.contains('.')))
                .chain(entry.name.filter(|value| value.contains('.')))
        })
        .filter_map(|host| {
            let host = host.trim().trim_start_matches("www.").to_ascii_lowercase();
            (!host.is_empty() && host.contains('.') && !host.contains('/')).then_some(host)
        })
        .collect();
    hosts.sort_unstable();
    hosts.dedup();
    hosts
}

/// Whether the account may be used for downloading at all.
///
/// Two flags rather than one, because Offcloud has two ways of saying no and they mean
/// different things: a free account has not bought the premium add-on, and an account whose
/// `canDownload` is false has bought it and is still refused — a suspension, an unpaid
/// invoice, an abuse hold. The second is reported separately so the person is not told to buy
/// something they already own.
#[must_use]
pub fn account_state(info: &AccountInfo) -> AccountState {
    match (
        info.is_premium.unwrap_or(false),
        info.can_download.unwrap_or(true),
    ) {
        (false, _) => AccountState::Free,
        (true, false) => AccountState::Blocked,
        (true, true) => AccountState::Usable,
    }
}

/// What `GET /api/account/info` says about an account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountState {
    /// Premium, and allowed to download.
    Usable,
    /// No premium add-on. Offcloud's free tier cannot be used for downloading through the API.
    Free,
    /// Premium, and refused by the provider for a reason it does not name.
    Blocked,
}

/// The name an account is shown under: the address when there is one, the opaque account
/// identifier otherwise. Never the key.
#[must_use]
pub fn account_name(info: &AccountInfo) -> Option<&str> {
    info.email
        .as_deref()
        .or(info.user_id.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;
