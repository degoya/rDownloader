//! What a person handed over, and the key that stands for it.
//!
//! Entirely local: nothing here makes a request, and nothing here is allowed to. That is the
//! whole point of `identify`. The content key is what keeps a provider whose submit is not
//! idempotent from being asked twice, and a key that needed a request could not be written
//! into the host's row *before* the first one. See
//! <https://github.com/degoya/rDownloader/wiki/plugin-reference#remote-jobs>.
//!
//! For both shapes the key here is the BitTorrent info hash, lower-case hex. A magnet carries
//! it (`xt=urn:btih:`), in hex or in base32; a `.torrent` file is the SHA-1 of the bencoded
//! value of its top-level `info` key, which is the same number arrived at the long way. Being
//! the same number for both shapes is what lets a magnet and the matching file find the same
//! remote job, and what lets `adopt` recognise a job this installation may have created
//! seconds before a crash.
//!
//! If your provider takes something other than torrents, this is the module to replace. Keep
//! the two properties, not the algorithm: derived without a request, and the same for the
//! same content every time.

use crate::digest::{sha1, to_hex};

/// Longest container this plugin will read. A `.torrent` is kilobytes; anything far past that
/// is not one, and scanning it would spend the invocation's whole budget finding that out.
pub const MAX_CONTAINER_BYTES: usize = 4 * 1024 * 1024;

/// Deepest nesting a bencoded container may have before it is refused.
///
/// A container comes from a stranger. Without this, `llllll…` is a stack overflow written in
/// eleven characters.
const MAX_BENCODE_DEPTH: u32 = 32;

/// Longest address this plugin will take. A link is hundreds of characters; past this it is
/// not one, and the check belongs here rather than in a fuel budget.
pub const MAX_ADDRESS_BYTES: usize = 8 * 1024;

/// The content key of a plain address (RD-120-20).
///
/// A third shape needs a third key, and it has to obey the same two properties as the info
/// hash above: derived without a request, and the same for the same content every time. The
/// SHA-1 of the address does both. It carries a `url:` prefix so that an address key can
/// never collide with an info hash — they share one unique index, and two different things
/// that happened to hash alike would silently become one remote job.
///
/// `None` for anything that is not an `http` or `https` address: a provider fetches what it
/// is given, and giving it a scheme nobody checked is how a plugin ends up asking a provider
/// to read a local file.
#[must_use]
pub fn address_key(address: &str) -> Option<String> {
    let trimmed = address.trim();
    if trimmed.len() > MAX_ADDRESS_BYTES {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return None;
    }
    // An address with no host after the scheme is not an address.
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    if rest.is_empty() || rest.starts_with('/') {
        return None;
    }
    Some(format!("url:{}", to_hex(&sha1(trimmed.as_bytes()))))
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
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
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
///
/// The bytes are hashed exactly as they lay in the file rather than re-encoded, because a
/// re-encoding that ordered one key differently would produce a different hash for the same
/// torrent — and the whole value of the key is that it is the same number everybody else has.
#[must_use]
pub fn container_info_hash(bytes: &[u8]) -> Option<String> {
    info_slice(bytes).map(|info| to_hex(&sha1(info)))
}

/// The encoded bytes of the top-level `info` value.
fn info_slice(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() > MAX_CONTAINER_BYTES || bytes.first() != Some(&b'd') {
        return None;
    }
    let mut at = 1;
    while at < bytes.len() && bytes[at] != b'e' {
        let (key, after_key) = read_byte_string(bytes, at)?;
        let end = skip_value(bytes, after_key, 0)?;
        if key == b"info" {
            return bytes.get(after_key..end);
        }
        at = end;
    }
    None
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
    use super::{container_info_hash, magnet_info_hash, normalise_info_hash};

    const HEX: &str = "c12fe1c06bba254a9dc9f519b335aa7c1367a88a";

    /// The bencoded value of one minimal `info` dictionary.
    const INFO: &[u8] = b"d6:lengthi3e4:name1:a12:piece lengthi16384ee";

    /// A minimal but real bencoded torrent, built around [`INFO`] rather than written out,
    /// so the test can hash that value on its own and compare.
    fn container() -> Vec<u8> {
        let mut bytes = b"d8:announce19:http://t.example/aa4:info".to_vec();
        bytes.extend_from_slice(INFO);
        bytes.push(b'e');
        bytes
    }

    #[test]
    fn a_magnet_answers_with_its_info_hash_however_it_is_spelled() {
        assert_eq!(
            magnet_info_hash(&format!("magnet:?xt=urn:btih:{HEX}&dn=x")),
            Some(HEX.to_owned())
        );
        assert_eq!(
            magnet_info_hash(&format!(
                "magnet:?dn=x&xt.1=urn%3Abtih%3A{}",
                HEX.to_uppercase()
            )),
            Some(HEX.to_owned())
        );
        // The same twenty bytes in base32, as half the trackers in the field write them.
        assert_eq!(
            magnet_info_hash("magnet:?xt=urn:btih:YEX6DQDLXISUVHOJ6UM3GNNKPQJWPKEK"),
            Some(HEX.to_owned())
        );
    }

    #[test]
    fn a_source_that_is_not_ours_is_refused_rather_than_guessed() {
        // `claims` is answered from this, so a yes here costs somebody else's plugin the job.
        assert_eq!(magnet_info_hash("https://example.com/a.torrent"), None);
        assert_eq!(magnet_info_hash("magnet:?xt=urn:sha1:ABCDEF"), None);
        assert_eq!(magnet_info_hash("magnet:?dn=no+topic+at+all"), None);
        assert_eq!(normalise_info_hash("not a hash"), None);
        assert_eq!(container_info_hash(b"not bencoded"), None);
        assert_eq!(container_info_hash(b"d8:announce3:abce"), None);
    }

    #[test]
    fn a_container_is_hashed_over_the_bytes_as_they_lay() {
        let bytes = container();
        let hash = container_info_hash(&bytes).expect("a bencoded torrent has an info hash");
        assert_eq!(hash.len(), 40);
        // The `info` value alone hashes to the same thing, which is what "as they lay" means:
        // the dictionary is not re-encoded on the way through.
        assert_eq!(
            hash,
            crate::digest::to_hex(&crate::digest::sha1(INFO)),
            "the info dictionary must be hashed verbatim"
        );
        // And a magnet naming that hash is the same key, so both shapes find one job.
        assert_eq!(
            magnet_info_hash(&format!("magnet:?xt=urn:btih:{hash}")),
            Some(hash)
        );
    }

    #[test]
    fn a_container_that_is_a_trap_is_refused_before_it_costs_anything() {
        // Nesting written in eleven characters, and a length header claiming more than the
        // file holds. Both used to be a way to end an invocation badly.
        assert_eq!(container_info_hash(&b"d4:infol".repeat(64)), None);
        assert_eq!(container_info_hash(b"d4:info999999999:aaaae"), None);
        assert_eq!(
            container_info_hash(&vec![b'd'; super::MAX_CONTAINER_BYTES + 1]),
            None
        );
    }
}
