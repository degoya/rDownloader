//! MEGA's command endpoint, as far as an unauthenticated caller reaches it.
//!
//! Three calls carry both plugins: `a=g` for a shared file, the same with `n=` for a file
//! inside a shared folder, and `a=f` for the folder's node list. Measured on 2026-09-21 and
//! again on 2026-09-22 -- see `docs/roadmap/jobs/103-02-mega.md`, section "Messung und
//! Entwurf".
//!
//! Two properties of the endpoint a parser has to know, both measured rather than read:
//! **every answer is `200`**, with the failure as a negative number in the body, and that
//! number arrives either inside the array (`[-9]`) or bare (`-9`).
//!
//! The signed-in account (RD-120-30) uses the same two calls with the session instead of a
//! folder handle: `a=f` for the account's node list, `a=g` with `n=` for one of its files. The
//! session travels as the host's `{{secret:mega_session}}` marker in the `sid` parameter; no
//! plugin ever holds its value. **The account calls follow MEGA's published SDK and were not
//! measured against a live account** -- the recorded answers in the contract test are this
//! project's own, built from the public example files.

use serde_json::Value;

/// Retry later; the caller is asking too quickly.
pub const EAGAIN: i64 = -3;
/// Rate limited.
pub const ERATELIMIT: i64 = -4;
/// No such node. **Deleted and never existed are the same answer** -- measured, not assumed.
pub const ENOENT: i64 = -9;
/// The caller may not see this node.
pub const EACCESS: i64 = -11;
/// A session is required, or the one given is not valid.
pub const ESID: i64 = -15;
/// The account is blocked.
pub const EBLOCKED: i64 = -16;
/// The provider's transfer quota is used up.
pub const EOVERQUOTA: i64 = -17;
/// Temporarily unavailable.
pub const ETEMPUNAVAIL: i64 = -18;

/// MEGA's own status for "you have used your bandwidth", which is not a `Retry-After` case
/// in the usual sense: the number of seconds rides in a header of the provider's own.
pub const STATUS_BANDWIDTH_EXCEEDED: u16 = 509;
/// The header MEGA puts the remaining wait in. Read out of JDownloader's MEGA plugin, which
/// handles it (job file, section 5) -- this project has never provoked a `509` itself, and
/// says so rather than claiming a measurement it does not have. Nothing is lost if MEGA ever
/// stops sending it: the standard header is tried next, and no header at all is `None`.
const MEGA_TIME_LEFT: &str = "x-mega-time-left";

/// How long the provider asked to be left alone, in seconds.
///
/// Its own header first, the standard one second. Only the numeric form of `Retry-After` is
/// read: the date form needs a clock to subtract from, a guest has none of its own, and a
/// wrong guess here would be a wait the scheduler takes literally. An unreadable value is
/// `None`, which means "the caller decides", not "retry at once".
#[must_use]
pub fn retry_after(headers: &[(String, String)]) -> Option<u64> {
    let read = |wanted: &str| {
        headers.iter().find_map(|(name, value)| {
            name.eq_ignore_ascii_case(wanted)
                .then(|| value.trim().parse::<u64>().ok())
                .flatten()
        })
    };
    read(MEGA_TIME_LEFT).or_else(|| read("retry-after"))
}

/// The body of a request for one shared file.
#[must_use]
pub fn file_request(handle: &str) -> Vec<u8> {
    format!(r#"[{{"a":"g","g":1,"ssl":2,"p":{}}}]"#, quote(handle)).into_bytes()
}

/// The body of a request for one file inside a shared folder. The folder handle travels as
/// the `n` query parameter, the node handle as `n` in the command -- measured; `p` answers
/// `-9` for a folder child.
#[must_use]
pub fn folder_child_request(node: &str) -> Vec<u8> {
    format!(r#"[{{"a":"g","g":1,"ssl":2,"n":{}}}]"#, quote(node)).into_bytes()
}

/// The query parameter that carries the signed-in session: MEGA's `sid`, as the host's
/// marker. The host substitutes it on the way out, and only towards the hosts the session
/// slot names.
pub const SESSION_QUERY: (&str, &str) = ("sid", "{{secret:mega_session}}");

/// The reference of the session slot, for the key derivations over its key half.
pub const SESSION_SECRET: &str = "mega_session";

/// The body of the signed-in account's node list: every node, in one answer.
#[must_use]
pub fn account_listing_request() -> Vec<u8> {
    br#"[{"a":"f","c":1,"r":1}]"#.to_vec()
}

/// The body of a download request for one file of the signed-in account. The same command
/// a folder child takes, with the session in the query instead of the folder handle.
#[must_use]
pub fn account_file_request(node: &str) -> Vec<u8> {
    folder_child_request(node)
}

/// The body of a folder listing. `r=1` asks for the whole subtree in one answer, which is
/// what MEGA gives: there is no cursor and no page token.
#[must_use]
pub fn folder_request() -> Vec<u8> {
    br#"[{"a":"f","c":1,"r":1,"ca":1}]"#.to_vec()
}

/// The endpoint for a call, with the folder handle attached when there is one.
#[must_use]
pub fn endpoint(folder: Option<&str>) -> String {
    match folder {
        Some(handle) => format!("{}?n={handle}", crate::address::API),
        None => crate::address::API.to_owned(),
    }
}

/// The one object an answer carries, or the number it failed with.
///
/// `Err(None)` is a body that is neither: an outage page, a truncated answer, anything a
/// caller must not read values out of.
pub fn first_object(body: &[u8]) -> Result<Value, Option<i64>> {
    let text = std::str::from_utf8(body).map_err(|_| None)?;
    let value: Value = serde_json::from_str(text.trim()).map_err(|_| None)?;
    let inner = match &value {
        Value::Array(items) => items.first().cloned().ok_or(None)?,
        other => other.clone(),
    };
    match inner {
        Value::Object(_) => Ok(inner),
        Value::Number(number) => Err(Some(number.as_i64().unwrap_or(0))),
        _ => Err(None),
    }
}

/// One node of a folder listing, as the answer spells it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Node {
    pub handle: String,
    pub parent: String,
    /// The user handle of the account the node belongs to, MEGA's `u`. A node of the
    /// signed-in account files its key under this handle, wrapped under the master key.
    pub owner: String,
    /// `0` a file, `1` a folder, `2`/`3`/`4` the account's own roots.
    pub kind: u8,
    /// The encrypted attribute block.
    pub attributes: String,
    /// `<handle>:<key>` entries, slash separated.
    pub keys: String,
    pub size: u64,
}

impl Node {
    /// Reads the `f` array of an `a=f` answer. Nodes without a handle are dropped rather
    /// than guessed at.
    #[must_use]
    pub fn list(answer: &Value) -> Vec<Self> {
        let Some(items) = answer.get("f").and_then(Value::as_array) else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|item| {
                let handle = item.get("h").and_then(Value::as_str)?.to_owned();
                Some(Self {
                    handle,
                    parent: item
                        .get("p")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    owner: item
                        .get("u")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    kind: u8::try_from(item.get("t").and_then(Value::as_u64).unwrap_or(0))
                        .unwrap_or(0),
                    attributes: item
                        .get("a")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    keys: item
                        .get("k")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    size: item.get("s").and_then(Value::as_u64).unwrap_or(0),
                })
            })
            .collect()
    }

    /// The raw key this node carries under `owner`, still encrypted.
    ///
    /// A node lists one entry per handle it is shared under. Only the entry belonging to the
    /// share root is encrypted with the key from the link's fragment; an entry under the
    /// node's own handle is a different share and cannot be opened from here.
    #[must_use]
    pub fn key_under(&self, owner: &str) -> Option<&str> {
        self.keys.split('/').find_map(|entry| {
            let (handle, key) = entry.split_once(':')?;
            (handle == owner).then_some(key)
        })
    }
}

impl Node {
    /// The key this node carries under its own owner -- for a node of the signed-in account,
    /// the one wrapped under the master key. An entry under any other handle is a share key's
    /// and not the account's to open with it.
    #[must_use]
    pub fn own_key(&self) -> Option<&str> {
        if self.owner.is_empty() {
            return None;
        }
        self.key_under(&self.owner)
    }
}

/// The node every other node hangs off: the one whose parent is not in the listing.
#[must_use]
pub fn share_root(nodes: &[Node]) -> Option<&Node> {
    nodes
        .iter()
        .find(|node| !nodes.iter().any(|other| other.handle == node.parent))
}

/// The storage address an `a=g` answer carries, and the plaintext size beside it.
#[must_use]
pub fn download_target(answer: &Value) -> Option<(String, u64, String)> {
    let url = answer.get("g").and_then(Value::as_str)?.to_owned();
    let size = answer.get("s").and_then(Value::as_u64)?;
    let attributes = answer
        .get("at")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Some((url, size, attributes))
}

/// A JSON string literal. The values quoted here are handles and keys that have already been
/// checked against MEGA's alphabet, but building JSON by hand without escaping is how an
/// injection gets written, so it is escaped anyway.
fn quote(value: &str) -> String {
    Value::String(value.to_owned()).to_string()
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod api_tests;
