//! What a person handed over, and the number that identifies it.
//!
//! Target-independent and entirely local: nothing here makes a request. That is the point —
//! the content key is what keeps a provider whose submit is not idempotent from being asked
//! twice, and a key that needed a request could not be written down before the first one. See
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md`.
//!
//! For both shapes the number is the BitTorrent info hash, lower-case hex. A magnet carries it
//! (`xt=urn:btih:`), in hex or in base32; a `.torrent` file is the SHA-1 of the bencoded value
//! of its top-level `info` key, which is the same number arrived at the long way. Being the
//! same number for both shapes is what lets a magnet and the matching file find the same
//! remote job — and what lets `adopt` recognise it in what Seedr says about a transfer.
//!
//! **A container is submitted as a magnet.** Seedr documents `POST /rest/transfer/file`, a
//! multipart upload, beside `POST /rest/transfer/magnet`. This plugin takes the second for
//! both: [`container_magnet`] reads the container once, locally, and writes the magnet it is
//! equivalent to — the same info hash, the torrent's own name, and every tracker the file
//! listed. What that costs is a torrent carrying neither trackers nor peers reachable over
//! DHT, which Seedr then cannot start; what it buys is one request shape instead of two, one
//! body builder instead of a multipart writer, and a content key that is unchanged by which of
//! the two shapes somebody pasted. `plugins/putio-transfers/` reached the same answer for the
//! same reason.

use sha1::{Digest, Sha1};

/// Longest container this plugin will read. A `.torrent` is kilobytes; anything far past that
/// is not one, and scanning it would spend the invocation's whole budget finding that out.
pub const MAX_CONTAINER_BYTES: usize = 4 * 1024 * 1024;

/// Deepest nesting a bencoded container may have before it is refused.
///
/// A container comes from a stranger. Without this, `llllll…` is a stack overflow written in
/// eleven characters.
const MAX_BENCODE_DEPTH: u32 = 32;

/// Most trackers carried over into a reconstructed magnet. A tracker list is attacker-supplied
/// and an address has to fit in a request body; thirty is more than any real torrent lists.
const MAX_TRACKERS: usize = 30;

/// Longest torrent name carried over as `dn`. It is a display name and nothing depends on it,
/// so it is bounded rather than trusted.
const MAX_NAME_BYTES: usize = 255;

/// What a container says about itself, beyond the number that identifies it.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Container {
    /// The info hash, lower-case hex.
    pub info_hash: String,
    /// `info.name`, when it is readable text.
    pub name: Option<String>,
    /// `announce` and every entry of `announce-list`, in the order they were written, without
    /// repeats.
    pub trackers: Vec<String>,
}

/// The info hash a magnet address names, as lower-case hex.
///
/// `None` for anything that is not a magnet, and for a magnet whose `xt` is not a BitTorrent
/// info hash — `urn:sha1:` and `urn:ed2k:` are legal in a magnet and mean something else.
#[must_use]
pub fn magnet_info_hash(address: &str) -> Option<String> {
    let rest = address
        .strip_prefix("magnet:?")
        .or_else(|| address.strip_prefix("MAGNET:?"))?;
    for pair in rest.split('&') {
        let (name, value) = pair.split_once('=')?;
        // `xt.1` and friends: a magnet may name several topics, and each is a candidate.
        if name != "xt" && !name.starts_with("xt.") {
            continue;
        }
        let Some(raw) = value
            .strip_prefix("urn:btih:")
            .or_else(|| value.strip_prefix("urn%3Abtih%3A"))
        else {
            continue;
        };
        if let Some(hash) = normalise_info_hash(raw) {
            return Some(hash);
        }
    }
    None
}

/// The info hash any text carries, wherever in it the `btih` sits.
///
/// Used by `adopt` rather than by intake: what Seedr says about a running transfer is a
/// `torrent_hash`, and other clients of the same API have seen the whole magnet there instead.
/// The question being asked of either shape is only ever "is this our twenty bytes".
#[must_use]
pub fn info_hash_within(text: &str) -> Option<String> {
    if let Some(hash) = normalise_info_hash(text) {
        return Some(hash);
    }
    let lower = text.to_ascii_lowercase();
    let start = lower.find("btih:")? + "btih:".len();
    let rest = text.get(start..)?;
    let end = rest
        .find(['&', '/', '?', '#'])
        .unwrap_or_else(|| rest.len().min(40));
    normalise_info_hash(rest.get(..end)?)
}

/// Accepts a 40-character hex or a 32-character base32 info hash and answers lower-case hex.
///
/// Both spellings are in the field and both mean the same twenty bytes, so normalising here
/// means a magnet copied from two different sites produces one remote job and not two.
#[must_use]
pub fn normalise_info_hash(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.len() == 40 && raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(raw.to_ascii_lowercase());
    }
    if raw.len() == 32 {
        return decode_base32(raw).map(|bytes| to_hex(&bytes));
    }
    None
}

/// The info hash of a bencoded container: SHA-1 over the *encoded* value of its `info` key.
#[must_use]
pub fn container_info_hash(bytes: &[u8]) -> Option<String> {
    read_container(bytes).map(|container| container.info_hash)
}

/// Everything a container says that the magnet below needs.
///
/// The bytes of `info` are hashed exactly as they lay in the file rather than re-encoded,
/// because a re-encoding that ordered one key differently would produce a different hash for
/// the same torrent — and the whole value of the key is that it is the same number everybody
/// else has.
#[must_use]
pub fn read_container(bytes: &[u8]) -> Option<Container> {
    let root = Dictionary::at(bytes, 0)?;
    let info = root.value(b"info")?;
    let mut hasher = Sha1::new();
    hasher.update(bytes.get(info.clone())?);
    let info_hash = to_hex(&hasher.finalize());
    let name = Dictionary::at(bytes, info.start)
        .and_then(|info| info.value(b"name"))
        .and_then(|range| text_of(bytes, range))
        .map(|name| bounded_name(&name))
        .filter(|name| !name.is_empty());
    let mut trackers = Vec::new();
    if let Some(range) = root.value(b"announce")
        && let Some(tracker) = text_of(bytes, range)
    {
        push_tracker(&mut trackers, tracker);
    }
    // `announce-list` is a list of *tiers*, each a list of addresses. Both levels are walked,
    // because a torrent that puts its only working tracker in the second tier is ordinary.
    if let Some(range) = root.value(b"announce-list") {
        for tier in list_items(bytes, range.start) {
            for entry in list_items(bytes, tier.start) {
                if let Some(tracker) = text_of(bytes, entry) {
                    push_tracker(&mut trackers, tracker);
                }
            }
        }
    }
    Some(Container {
        info_hash,
        name,
        trackers,
    })
}

/// The magnet address a container is equivalent to, or `None` when the bytes are not one.
///
/// This is what `submit` hands to Seedr. It names the same twenty bytes as the file, so the
/// remote job it creates is the same job a magnet for that torrent would have created — which
/// is exactly what the content key promised and what `adopt` relies on after a restart.
#[must_use]
pub fn container_magnet(bytes: &[u8]) -> Option<String> {
    let container = read_container(bytes)?;
    let mut magnet = format!("magnet:?xt=urn:btih:{}", container.info_hash);
    if let Some(name) = &container.name {
        magnet.push_str("&dn=");
        magnet.push_str(&percent_encode(name));
    }
    for tracker in container.trackers.iter().take(MAX_TRACKERS) {
        magnet.push_str("&tr=");
        magnet.push_str(&percent_encode(tracker));
    }
    Some(magnet)
}

/// Percent-encodes everything outside the unreserved set, so a value cannot end the query it
/// sits in — a torrent name containing `&tr=` would otherwise add a tracker of its choosing.
#[must_use]
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => {
                use std::fmt::Write;
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

/// `application/x-www-form-urlencoded` body for `POST /rest/transfer/magnet`.
///
/// Encoded by hand rather than with a URL crate for one field, because that field is a magnet
/// — full of `&`, `=` and `:` — and a body that did not encode them would submit a truncated
/// address and create a transfer for something nobody asked for.
#[must_use]
pub fn add_magnet_body(magnet: &str) -> Vec<u8> {
    format!("magnet={}", percent_encode(magnet)).into_bytes()
}

/// A tracker address, if it is one, added once.
fn push_tracker(trackers: &mut Vec<String>, candidate: String) {
    let candidate = candidate.trim().to_owned();
    let usable = matches!(
        candidate.split_once("://").map(|(scheme, _)| scheme),
        Some("http" | "https" | "udp" | "ws" | "wss")
    ) && candidate.len() <= 255
        && !candidate.bytes().any(|byte| byte.is_ascii_control());
    if usable && !trackers.contains(&candidate) {
        trackers.push(candidate);
    }
}

/// A torrent name, cut to whole characters at the byte bound and stripped of controls.
fn bounded_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_control())
        .fold(String::new(), |mut out, character| {
            if out.len() + character.len_utf8() <= MAX_NAME_BYTES {
                out.push(character);
            }
            out
        })
        .trim()
        .to_owned()
}

/// One bencoded dictionary, as a map from key to the byte range of its value.
struct Dictionary {
    entries: Vec<(Vec<u8>, std::ops::Range<usize>)>,
}

impl Dictionary {
    /// Reads the dictionary starting at `at`, or `None` when there is not one there.
    fn at(bytes: &[u8], at: usize) -> Option<Self> {
        if bytes.len() > MAX_CONTAINER_BYTES || bytes.get(at) != Some(&b'd') {
            return None;
        }
        let mut entries = Vec::new();
        let mut cursor = at + 1;
        while *bytes.get(cursor)? != b'e' {
            let (key, after_key) = read_byte_string(bytes, cursor)?;
            let end = skip_value(bytes, after_key, 0)?;
            entries.push((key.to_vec(), after_key..end));
            cursor = end;
        }
        Some(Self { entries })
    }

    fn value(&self, key: &[u8]) -> Option<std::ops::Range<usize>> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, range)| range.clone())
    }
}

/// The byte ranges of the items of the bencoded list at `at`.
fn list_items(bytes: &[u8], at: usize) -> Vec<std::ops::Range<usize>> {
    let mut items = Vec::new();
    if bytes.get(at) != Some(&b'l') {
        return items;
    }
    let mut cursor = at + 1;
    while bytes.get(cursor).is_some_and(|byte| *byte != b'e') {
        let Some(end) = skip_value(bytes, cursor, 0) else {
            break;
        };
        items.push(cursor..end);
        cursor = end;
    }
    items
}

/// The text of the byte string at the start of `range`, when it is one and it is UTF-8.
fn text_of(bytes: &[u8], range: std::ops::Range<usize>) -> Option<String> {
    let (value, _) = read_byte_string(bytes, range.start)?;
    std::str::from_utf8(value).ok().map(str::to_owned)
}

/// Reads `<length>:<bytes>` at `at`, answering the bytes and the offset just past them.
fn read_byte_string(bytes: &[u8], at: usize) -> Option<(&[u8], usize)> {
    let colon = bytes.iter().skip(at).position(|byte| *byte == b':')? + at;
    let digits = bytes.get(at..colon)?;
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    // A length is bounded by the container, so a header claiming more than it holds is
    // refused rather than trusted into an overflow.
    let length: usize = std::str::from_utf8(digits).ok()?.parse().ok()?;
    let start = colon.checked_add(1)?;
    let end = start.checked_add(length)?;
    Some((bytes.get(start..end)?, end))
}

/// The offset just past the bencoded value starting at `at`.
fn skip_value(bytes: &[u8], at: usize, depth: u32) -> Option<usize> {
    if depth > MAX_BENCODE_DEPTH {
        return None;
    }
    match bytes.get(at)? {
        b'i' => {
            let end = bytes.iter().skip(at).position(|byte| *byte == b'e')? + at;
            Some(end + 1)
        }
        b'l' => {
            let mut at = at + 1;
            while *bytes.get(at)? != b'e' {
                at = skip_value(bytes, at, depth + 1)?;
            }
            Some(at + 1)
        }
        b'd' => {
            let mut at = at + 1;
            while *bytes.get(at)? != b'e' {
                let (_, after_key) = read_byte_string(bytes, at)?;
                at = skip_value(bytes, after_key, depth + 1)?;
            }
            Some(at + 1)
        }
        byte if byte.is_ascii_digit() => read_byte_string(bytes, at).map(|(_, end)| end),
        _ => None,
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        use std::fmt::Write;
        // Writing into a String cannot fail; the result is discarded rather than unwrapped.
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// RFC 4648 base32 without padding, as a magnet spells an info hash.
fn decode_base32(text: &str) -> Option<Vec<u8>> {
    let mut accumulator: u64 = 0;
    let mut bits = 0_u32;
    let mut bytes = Vec::with_capacity(20);
    for character in text.chars() {
        let value = match character.to_ascii_uppercase() {
            letter @ 'A'..='Z' => u64::from(letter as u8 - b'A'),
            digit @ '2'..='7' => u64::from(digit as u8 - b'2') + 26,
            _ => return None,
        };
        accumulator = (accumulator << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            let byte = u8::try_from((accumulator >> bits) & 0xff).ok()?;
            bytes.push(byte);
            // The consumed bits are dropped rather than left in the accumulator. Without
            // this it grows by five bits per character and overflows on the thirteenth,
            // which in a debug build is a panic and in a release build a wrong answer.
            accumulator &= (1_u64 << bits) - 1;
        }
    }
    (bytes.len() == 20).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        add_magnet_body, container_info_hash, container_magnet, info_hash_within, magnet_info_hash,
        normalise_info_hash, read_container,
    };

    /// SHA-1 of nothing, which is the info hash of the container below is not — it is simply
    /// the placeholder every fixture in this provider uses.
    const HEX: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
    /// The same twenty bytes, spelled in base32 as some sites do.
    const BASE32: &str = "3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ";

    /// One minimal bencoded torrent: an announce address and a one-key `info` dictionary.
    fn container() -> Vec<u8> {
        b"d8:announce31:http://tracker.invalid/announce4:infod4:name7:Example6:lengthi31eee"
            .to_vec()
    }

    #[test]
    fn both_spellings_of_one_info_hash_are_one_number() {
        assert_eq!(
            magnet_info_hash(&format!("magnet:?xt=urn:btih:{}", HEX.to_uppercase())).as_deref(),
            Some(HEX)
        );
        assert_eq!(
            magnet_info_hash(&format!("magnet:?xt=urn:btih:{BASE32}&dn=Example")).as_deref(),
            Some(HEX)
        );
        assert_eq!(normalise_info_hash(BASE32).as_deref(), Some(HEX));
    }

    /// A magnet may name a topic that is not a BitTorrent info hash at all, and those mean
    /// something else entirely: claiming one would submit a job nobody could run.
    #[test]
    fn a_magnet_that_names_something_else_is_not_ours() {
        for foreign in [
            "magnet:?xt=urn:sha1:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709",
            "magnet:?xt=urn:ed2k:31d6cfe0d16ae931b73c59d7e0c089c0",
            "magnet:?dn=Example",
            "https://example.invalid/x.torrent",
            "",
        ] {
            assert_eq!(magnet_info_hash(foreign), None, "{foreign}");
        }
    }

    /// The whole value of the key is that it is the same number everybody else has, so the
    /// bytes of `info` are hashed exactly as they lay in the file rather than re-encoded.
    #[test]
    fn a_container_and_its_magnet_are_one_content_key() {
        let hash = container_info_hash(&container()).expect("an info hash");
        assert_eq!(hash.len(), 40);
        let magnet = container_magnet(&container()).expect("a magnet");
        assert_eq!(magnet_info_hash(&magnet).as_deref(), Some(hash.as_str()));
        // The name and the tracker travel with it, which is what lets Seedr start the transfer
        // at all for a torrent whose peers are not reachable over DHT alone.
        assert!(magnet.contains("&dn=Example"), "{magnet}");
        assert!(
            magnet.contains("&tr=http%3A%2F%2Ftracker.invalid%2Fannounce"),
            "{magnet}"
        );
        let read = read_container(&container()).expect("a container");
        assert_eq!(read.name.as_deref(), Some("Example"));
        assert_eq!(read.trackers.len(), 1);
    }

    /// A container comes from a stranger, so every refusal below is a bound rather than a
    /// parse error: nesting, a length header claiming more than the file holds, and bytes that
    /// are not bencoded at all.
    #[test]
    fn bytes_that_are_not_a_torrent_are_refused_rather_than_read() {
        for bad in [
            &b""[..],
            b"not bencoded at all",
            b"d4:infod4:name99:tooshortee",
            &b"l".repeat(200),
        ] {
            assert_eq!(container_info_hash(bad), None);
            assert_eq!(container_magnet(bad), None);
        }
    }

    #[test]
    fn an_info_hash_is_found_wherever_a_provider_wrote_it() {
        assert_eq!(info_hash_within(HEX).as_deref(), Some(HEX));
        assert_eq!(
            info_hash_within(&format!("magnet:?xt=urn:btih:{HEX}&dn=Example")).as_deref(),
            Some(HEX)
        );
        assert_eq!(info_hash_within("no hash here").as_deref(), None);
    }

    /// A magnet is full of the separators of the document it lands in, and a body that did not
    /// encode them would submit a truncated address.
    #[test]
    fn the_form_body_encodes_the_magnet_whole() {
        let body = String::from_utf8(add_magnet_body(
            "magnet:?xt=urn:btih:da39&dn=A B&tr=udp://t.invalid",
        ))
        .expect("utf-8");
        assert!(
            body.starts_with("magnet=magnet%3A%3Fxt%3Durn%3Abtih%3Ada39"),
            "{body}"
        );
        assert_eq!(body.matches('&').count(), 0, "{body}");
    }
}
