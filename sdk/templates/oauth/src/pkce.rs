//! PKCE (RFC 7636) and the small amount of JSON reading an OAuth exchange needs.
//!
//! Plain Rust with no dependencies, so it compiles for `wasm32-unknown-unknown` without WASI
//! and can be unit-tested on the host target as it is.
//!
//! **Where the entropy comes from.** `host.random-bytes` and nowhere else. A guest has no
//! random source of its own — there is no WASI here — and the values this module builds are
//! worth only as much as the bytes they were built from: a verifier or a `state` that anybody
//! can recompute from public inputs defeats the whole point of PKCE.

/// Fewest random bytes a verifier may be built from.
///
/// RFC 7636 asks for 43..=128 unreserved characters, and base64url without padding turns 32
/// bytes into exactly 43 of them. The same 32 bytes are also the width at which guessing stops
/// being a strategy, so the two bounds happen to agree.
pub const VERIFIER_BYTES: usize = 32;

/// Most random bytes that go into one verifier: 96 encode to the 128 characters RFC 7636
/// allows at most, and anything beyond is dropped rather than pushing the value out of range.
const MAX_VERIFIER_BYTES: usize = 96;

/// A verifier built from bytes the host's random source produced — base64url without padding,
/// which is inside RFC 7636's unreserved set.
///
/// `None` when fewer than [`VERIFIER_BYTES`] arrived. The host answers a request it will not
/// serve with nothing at all, and the only correct response to that is to fail the flow:
/// deriving a value from whatever is at hand — a clock, an account id — is what this function
/// was written to replace.
#[must_use]
pub fn verifier(random: &[u8]) -> Option<String> {
    if random.len() < VERIFIER_BYTES {
        return None;
    }
    Some(base64_url(&random[..random.len().min(MAX_VERIFIER_BYTES)]))
}

/// The `S256` challenge for a verifier: base64url, unpadded, of its SHA-256 digest.
#[must_use]
pub fn challenge(verifier: &str) -> String {
    base64_url(&sha256(verifier.as_bytes()))
}

/// Percent-encodes everything outside the unreserved set, so a value cannot end the query it
/// sits in. Applied to every value this plugin puts into an authorization URL.
#[must_use]
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The string value of a JSON field, without pulling in a parser for five fields.
#[must_use]
pub fn string_field(body: &str, name: &str) -> Option<String> {
    let mut rest = value_after(body, name)?.strip_prefix('"')?;
    let mut out = String::new();
    loop {
        let mut chars = rest.chars();
        let character = chars.next()?;
        rest = chars.as_str();
        match character {
            '"' => return Some(out),
            '\\' => {
                let mut escaped = rest.chars();
                match escaped.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    other => out.push(other),
                }
                rest = escaped.as_str();
            }
            other => out.push(other),
        }
    }
}

/// The numeric value of a JSON field.
#[must_use]
pub fn number_field(body: &str, name: &str) -> Option<u64> {
    let rest = value_after(body, name)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn value_after<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("\"{name}\"");
    let mut from = 0;
    while let Some(at) = body[from..].find(&needle) {
        let after = &body[from + at + needle.len()..];
        if let Some(value) = after.trim_start().strip_prefix(':') {
            return Some(value.trim_start());
        }
        from += at + needle.len();
    }
    None
}

/// base64url without padding, as RFC 7636 requires for a challenge.
fn base64_url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let mut block = [0u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let value = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        let symbols = chunk.len() + 1;
        for index in 0..symbols {
            let shift = 18 - index * 6;
            out.push(ALPHABET[((value >> shift) & 0x3F) as usize] as char);
        }
    }
    out
}

const ROUND_CONSTANTS: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256, the only digest PKCE's `S256` method needs.
#[must_use]
pub fn sha256(input: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = input.to_vec();
    let bit_length = (input.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());

    for block in message.as_chunks::<64>().0 {
        let mut schedule = [0u32; 64];
        for (index, word) in block.as_chunks::<4>().0.iter().enumerate() {
            schedule[index] = u32::from_be_bytes(*word);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(ROUND_CONSTANTS[index])
                .wrapping_add(schedule[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut digest = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    #[test]
    fn sha256_matches_the_published_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_challenge_matches_the_example_in_rfc_7636() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_verifier_is_unreserved_and_long_enough() {
        for count in [VERIFIER_BYTES, 64, MAX_VERIFIER_BYTES, 512] {
            let value = verifier(&vec![0xA5; count]).expect("enough bytes");
            assert!(
                (43..=128).contains(&value.len()),
                "{count} bytes gave {} characters",
                value.len()
            );
            assert!(
                value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "{value}"
            );
        }
    }

    /// The host answers a request it will not serve with nothing. A verifier must not be
    /// invented from what little arrived — failing is the point.
    #[test]
    fn too_few_random_bytes_are_a_refusal_and_not_a_shorter_verifier() {
        assert_eq!(verifier(&[]), None);
        assert_eq!(verifier(&[0u8; VERIFIER_BYTES - 1]), None);
        assert!(verifier(&[0u8; VERIFIER_BYTES]).is_some());
    }

    /// Distinct entropy gives distinct values, which is the property the derived version could
    /// not have: it produced the same string for the same account in the same second. That two
    /// *host* calls in one moment differ is proven where the entropy is — in
    /// `rd-plugin-host`'s `random_bytes` and in the OAuth contract test, which calls `begin`
    /// twice.
    #[test]
    fn different_entropy_gives_different_values() {
        let mut first = [0u8; VERIFIER_BYTES];
        first[0] = 1;
        let mut second = [0u8; VERIFIER_BYTES];
        second[VERIFIER_BYTES - 1] = 1;
        assert_ne!(verifier(&first), verifier(&second));
    }

    #[test]
    fn a_value_cannot_break_out_of_the_query_it_sits_in() {
        assert_eq!(percent_encode("a&b=c d"), "a%26b%3Dc%20d");
    }

    #[test]
    fn json_fields_are_read_without_a_parser() {
        let body = r#"{"access_token":"a b","expires_in":3600,"error":"invalid_grant"}"#;
        assert_eq!(string_field(body, "access_token").as_deref(), Some("a b"));
        assert_eq!(number_field(body, "expires_in"), Some(3600));
        assert_eq!(string_field(body, "refresh_token"), None);
    }
}
