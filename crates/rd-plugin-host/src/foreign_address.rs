//! Addresses somebody else made never carry a vault marker into a plugin's request (RD-120-66).
//!
//! RD-120-65 took the braces out of a notification's text. An address cannot be treated that
//! way: rewriting it changes where it points. And addresses are what the rest of the plugins are
//! handed — a link pasted from a web page to resolve or check, a folder to crawl, a magnet or a
//! web address for a remote job, a stream, a transfer. A multihoster puts that link into its
//! own API request beside the account's credential, and the host expanded a marker inside it
//! like one the plugin wrote: LinkSnappy was sent `…?x=<the account password>` and then fetched
//! that address from whoever made it.
//!
//! So two rules, each in one place:
//!
//! - [`carries_marker`]: an address that holds `{{` — as written, or after undoing
//!   percent-encoding as often as it is layered — is not handed to a plugin at all. Every entry
//!   point asks it before the guest runs ([`refused`] is the failure it answers with). A link
//!   with two braces in a row is not a download anybody meant to paste; a refusal with its own
//!   code says what happened, where a rewritten link would fail somewhere else for no visible
//!   reason. Decoding first is what makes it hold for a plugin that decodes: an address that
//!   passes contains no `{{` at any layer, so no amount of decoding can produce one.
//! - [`guest_url`]: in an address a *plugin* wrote, only the `{{secret}}` it wrote literally is
//!   the granted marker. `Url` stores that marker percent-encoded, as `%7B%7Bsecret%7D%7D`,
//!   which is also exactly what a storage plugin makes of a file called `{{secret}}.txt` when it
//!   encodes the name into its path — and the host filled both with the credential. The literal
//!   marker is carried through parsing under a per-request random token nobody else can know;
//!   every other spelling is re-encoded as `%7B%7B%73ecret%7D%7D`, the same bytes to the server
//!   and nothing the host expands.

use rd_core::{Failure, FailureKind};
use url::Url;

/// The marker a plugin writes into an address for the secret its invocation was granted.
const GRANTED: &str = "{{secret}}";
/// Its spelling after `Url` has encoded the braces, which the host also expands.
const GRANTED_ENCODED: &str = "%7B%7Bsecret%7D%7D";
/// The same bytes on the wire, spelled so the host does not recognise them.
const GRANTED_ENCODED_INERT: &str = "%7B%7B%73ecret%7D%7D";

/// Layers of percent-encoding undone before giving up and refusing. A real link has one.
const MAX_LAYERS: usize = 4;

/// Whether `address` holds `{{` at any layer of its percent-encoding.
pub(crate) fn carries_marker(address: &str) -> bool {
    let mut text = address.to_owned();
    for _ in 0..=MAX_LAYERS {
        if text.contains("{{") {
            return true;
        }
        let decoded = percent_decode(&text);
        if decoded == text {
            return false;
        }
        text = decoded;
    }
    // Still changing after that many layers: nothing a person pasted looks like this.
    true
}

/// The refusal for an address that [`carries_marker`].
pub(crate) fn refused() -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "plugin.address_carries_marker",
        "The address contains a credential placeholder and was not handed to the plugin",
    )
}

/// [`refused`] as an error for the entry points that answer with `anyhow`.
pub(crate) fn refused_error() -> anyhow::Error {
    anyhow::Error::new(refused())
}

/// Parses the address a plugin asked for, keeping only its own literal `{{secret}}` a marker.
pub(crate) fn guest_url(raw: &str) -> Result<Url, url::ParseError> {
    let token = raw
        .contains(GRANTED)
        .then(|| format!("rdgranted{:032x}", rand::random::<u128>()));
    let parsed = match &token {
        Some(token) => Url::parse(&raw.replace(GRANTED, token))?,
        None => Url::parse(raw)?,
    };
    // Checked after parsing, because parsing is what finishes a mixed spelling such as
    // `%7B{secret}}` into the marker.
    if token.is_none() && !contains_ignore_case(parsed.as_str(), GRANTED_ENCODED) {
        return Ok(parsed);
    }
    let inert = replace_ignore_case(parsed.as_str(), GRANTED_ENCODED, GRANTED_ENCODED_INERT);
    match &token {
        Some(token) => Url::parse(&inert.replace(token, GRANTED)),
        None => Url::parse(&inert),
    }
}

/// The addresses of a link check a plugin may be asked about, and an `unknown` answer for each
/// one that [`carries_marker`] — a batch goes on without it rather than failing for it.
pub(crate) fn checkable(urls: Vec<Url>) -> (Vec<Url>, Vec<rd_core::LinkCheckResult>) {
    let (marked, clean): (Vec<Url>, Vec<Url>) = urls
        .into_iter()
        .partition(|url| carries_marker(url.as_str()));
    let unknown = marked
        .into_iter()
        .map(|url| rd_core::LinkCheckResult {
            url,
            status: rd_core::LinkStatus::Unknown,
            file_name: None,
            size: None,
            media: None,
        })
        .collect();
    (clean, unknown)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(value) = bytes
                .get(index + 1..index + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(value);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn replace_ignore_case(haystack: &str, needle: &str, replacement: &str) -> String {
    let lower = haystack.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut last = 0;
    for (start, _) in lower.match_indices(&needle) {
        out.push_str(&haystack[last..start]);
        out.push_str(replacement);
        last = start + needle.len();
    }
    out.push_str(&haystack[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::{carries_marker, guest_url};

    #[test]
    fn an_address_with_a_marker_at_any_layer_is_caught() {
        for address in [
            "https://rapidgator.net/file/abc?x={{secret:linksnappy_password}}",
            "https://evil.example/{{username}}",
            "https://evil.example/f?x=%7B%7Bsecret:torbox_api_key%7D%7D",
            "https://evil.example/f?x=%7b%7bbasic:seedr_password%7d%7d",
            "https://evil.example/f?x={%7Bsecret}}",
            "https://evil.example/f?x=%257B%257Bsecret%257D%257D",
            "magnet:?xt=urn:btih:abc&tr=http://t.example/a%3Fk%3D%7B%7Bsecret%7D%7D",
        ] {
            assert!(carries_marker(address), "{address}");
        }
    }

    #[test]
    fn an_ordinary_address_passes() {
        for address in [
            "https://rapidgator.net/file/abc/release.part1.rar.html",
            "https://example.com/a?json=%7B%22a%22:1%7D",
            "https://example.com/{single}/brace",
            "magnet:?xt=urn:btih:0123456789abcdef&dn=Some%20Name",
            "https://example.com/100%25/done",
        ] {
            assert!(!carries_marker(address), "{address}");
        }
    }

    #[test]
    fn only_the_plugins_own_literal_marker_stays_a_marker() {
        // What Telegram and Discord write: the marker, literally, in the path.
        let own = guest_url("https://api.telegram.org/bot{{secret}}/sendMessage").expect("url");
        assert_eq!(
            own.as_str(),
            "https://api.telegram.org/bot%7B%7Bsecret%7D%7D/sendMessage"
        );
        // What a storage plugin makes of a file called `{{secret}}.txt`.
        let encoded = guest_url("https://cloud.example/dav/%7B%7Bsecret%7D%7D.txt").expect("url");
        assert_eq!(
            encoded.as_str(),
            "https://cloud.example/dav/%7B%7B%73ecret%7D%7D.txt"
        );
        // Both at once: the plugin's own survives, the file name does not become one.
        let both = guest_url("https://cloud.example/{{secret}}/%7b%7bsecret%7d%7d").expect("url");
        assert_eq!(
            both.as_str(),
            "https://cloud.example/%7B%7Bsecret%7D%7D/%7B%7B%73ecret%7D%7D"
        );
        // A mixed spelling that `Url` would finish encoding into the marker.
        let mixed = guest_url("https://cloud.example/%7B{secret}}").expect("url");
        assert!(!mixed.as_str().contains("%7B%7Bsecret%7D%7D"), "{mixed}");
        // Nothing to do, nothing done.
        let plain = guest_url("https://example.com/a?b=c").expect("url");
        assert_eq!(plain.as_str(), "https://example.com/a?b=c");
    }
}
