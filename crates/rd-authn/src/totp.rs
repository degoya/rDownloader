//! Time-based one-time passwords (RFC 6238), and the base32 alphabet authenticator apps use.
//!
//! Implemented here rather than taken as a dependency for one reason: the algorithm is thirty
//! lines and is pinned by published test vectors, so it can be *proved* correct in a way that
//! reviewing a dependency cannot. RFC 4226 and RFC 6238 both ship reference values, and
//! [`tests`] checks against them directly.
//!
//! ## The choices that are not ours to make
//!
//! SHA-1, six digits and a thirty-second step are all interoperability constraints, not
//! preferences: an authenticator app will not scan a QR code that says otherwise, and the
//! result would be a second factor nobody can enrol. SHA-1 is weak against collisions, which
//! is not what HMAC relies on; HMAC-SHA1 remains sound and is what every implementation of
//! this protocol speaks.

use hmac::{Hmac, Mac};
use sha1::Sha1;

/// Seconds per code. Fixed by what authenticator apps assume.
pub const STEP_SECONDS: u64 = 30;
/// Digits per code.
pub const DIGITS: u32 = 6;
/// How many steps either side of now are accepted.
///
/// One step, so a code entered as it rolls over still works — a phone clock a few seconds off,
/// or a person who started typing at second twenty-nine. Wider would multiply an attacker's
/// guessing surface by the same factor for no gain a user would notice.
pub const DRIFT_STEPS: u64 = 1;

/// Bytes in a generated secret. 20 is the RFC 4226 recommendation and what apps expect.
pub const SECRET_BYTES: usize = 20;

/// Generates a fresh secret.
#[must_use]
pub fn generate_secret() -> Vec<u8> {
    use rand::RngCore;
    let mut bytes = vec![0_u8; SECRET_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

/// The code for one time step.
#[must_use]
pub fn code_at_step(secret: &[u8], step: u64) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();

    // RFC 4226 dynamic truncation: the low nibble of the last byte picks the offset.
    let offset = usize::from(digest[digest.len() - 1] & 0x0F);
    let binary = u32::from(digest[offset] & 0x7F) << 24
        | u32::from(digest[offset + 1]) << 16
        | u32::from(digest[offset + 2]) << 8
        | u32::from(digest[offset + 3]);
    let modulus = 10_u32.pow(DIGITS);
    format!("{:0width$}", binary % modulus, width = DIGITS as usize)
}

/// The code for a unix timestamp.
#[must_use]
pub fn code_at(secret: &[u8], unix_seconds: u64) -> String {
    code_at_step(secret, unix_seconds / STEP_SECONDS)
}

/// Whether `candidate` is a valid code at `unix_seconds`, allowing for clock drift.
///
/// Comparison is constant-time over the accepted window. A timing difference here would leak
/// how many leading digits were right, which turns a million-guess space into six thousand.
#[must_use]
pub fn verify(secret: &[u8], candidate: &str, unix_seconds: u64) -> bool {
    accepted_step(secret, candidate, unix_seconds).is_some()
}

/// *Which* time step `candidate` answers at `unix_seconds`, if any.
///
/// The step is what makes a code single-use. Accepting a code without recording its step
/// leaves the same six digits valid for the whole ±1-step window — about ninety seconds — so a
/// code read over a shoulder, captured by a phishing proxy or left in a client log can simply
/// be replayed. The caller persists this number and refuses anything at or below it.
///
/// Same constant-time property as [`verify`]: every step in the window is compared whatever
/// the outcome, because an early exit would leak how many leading digits were right.
#[must_use]
pub fn accepted_step(secret: &[u8], candidate: &str, unix_seconds: u64) -> Option<u64> {
    let candidate = candidate.trim().replace(['-', ' '], "");
    if candidate.len() != DIGITS as usize || !candidate.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let current = unix_seconds / STEP_SECONDS;
    let mut matched = None;
    for step in current.saturating_sub(DRIFT_STEPS)..=current + DRIFT_STEPS {
        // No early exit: every step in the window is compared, whatever the outcome. The
        // latest match wins, so a collision across two steps cannot lower the recorded step
        // and reopen the earlier one.
        let hit = constant_time_eq(code_at_step(secret, step).as_bytes(), candidate.as_bytes());
        matched = hit.then_some(step).or(matched);
    }
    matched
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

/// RFC 4648 base32 without padding — the encoding authenticator apps read.
#[must_use]
pub fn base32_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    for chunk in bytes.chunks(5) {
        let mut buffer = [0_u8; 5];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let bits = u64::from_be_bytes([
            0, 0, 0, buffer[0], buffer[1], buffer[2], buffer[3], buffer[4],
        ]);
        // 8 characters per 5 bytes; a short chunk contributes proportionally fewer.
        let characters = match chunk.len() {
            1 => 2,
            2 => 4,
            3 => 5,
            4 => 7,
            _ => 8,
        };
        for index in 0..characters {
            let shift = 35 - index * 5;
            out.push(char::from(ALPHABET[((bits >> shift) & 0x1F) as usize]));
        }
    }
    out
}

/// The `otpauth://` URL an authenticator app scans.
///
/// The issuer appears twice — once as a label prefix and once as a parameter — because older
/// apps read only one of the two and would otherwise list the entry with no name at all.
#[must_use]
pub fn provisioning_uri(secret: &[u8], account: &str, issuer: &str) -> String {
    let encode = |value: &str| {
        value
            .chars()
            .map(|character| match character {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => character.to_string(),
                other => {
                    let mut buffer = [0_u8; 4];
                    other
                        .encode_utf8(&mut buffer)
                        .bytes()
                        .map(|byte| format!("%{byte:02X}"))
                        .collect()
                }
            })
            .collect::<String>()
    };
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        encode(issuer),
        encode(account),
        base32_encode(secret),
        encode(issuer)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RFC 6238 secret, ASCII "12345678901234567890".
    const RFC_SECRET: &[u8] = b"12345678901234567890";

    /// Published reference values. The point of implementing this rather than depending on it:
    /// correctness is checkable against the specification instead of taken on trust.
    ///
    /// RFC 6238 Appendix B tabulates eight digits; these are the low six of the SHA-1 rows.
    #[test]
    fn the_rfc_6238_reference_values_match() {
        for (unix_seconds, expected) in [
            (59_u64, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
            (20_000_000_000, "353130"),
        ] {
            assert_eq!(
                code_at(RFC_SECRET, unix_seconds),
                expected,
                "at {unix_seconds}"
            );
        }
    }

    /// The step a code belongs to is what lets the caller refuse a replay of it.
    ///
    /// Without it an accepted code stays valid for the whole ±1-step window, which is the one
    /// property a one-time password exists to have.
    #[test]
    fn an_accepted_code_names_the_step_it_belongs_to() {
        let now = 1_700_000_000_u64;
        let current = now / STEP_SECONDS;
        for offset in [-1_i64, 0, 1] {
            let step = current.wrapping_add_signed(offset);
            assert_eq!(
                accepted_step(RFC_SECRET, &code_at_step(RFC_SECRET, step), now),
                Some(step),
                "the code for step {step} was not attributed to it"
            );
        }
        // Two steps out is outside the window and belongs to nothing.
        assert_eq!(
            accepted_step(RFC_SECRET, &code_at_step(RFC_SECRET, current + 2), now),
            None
        );
    }

    /// RFC 4648 test vectors for the encoding an app has to read.
    #[test]
    fn the_rfc_4648_base32_vectors_match() {
        for (input, expected) in [
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
        ] {
            assert_eq!(base32_encode(input.as_bytes()), expected, "{input}");
        }
    }

    #[test]
    fn a_current_code_verifies() {
        let secret = generate_secret();
        let now = 1_700_000_000;
        assert!(verify(&secret, &code_at(&secret, now), now));
    }

    /// A phone whose clock is a little off, or a person who typed slowly.
    #[test]
    fn one_step_of_drift_either_way_is_accepted() {
        let secret = generate_secret();
        let now = 1_700_000_000;
        for offset in [-(STEP_SECONDS as i64), 0, STEP_SECONDS as i64] {
            let entered = (now as i64 + offset) as u64;
            assert!(
                verify(&secret, &code_at(&secret, entered), now),
                "offset {offset}"
            );
        }
    }

    /// …and no further, or the guessing surface grows for nothing.
    #[test]
    fn a_code_further_out_than_the_drift_window_is_refused() {
        let secret = generate_secret();
        let now = 1_700_000_000;
        for offset in [-3 * STEP_SECONDS as i64, 3 * STEP_SECONDS as i64] {
            let entered = (now as i64 + offset) as u64;
            assert!(
                !verify(&secret, &code_at(&secret, entered), now),
                "offset {offset} was accepted"
            );
        }
    }

    #[test]
    fn a_wrong_code_is_refused() {
        let secret = generate_secret();
        let now = 1_700_000_000;
        assert!(!verify(&secret, "000000", now) || code_at(&secret, now) == "000000");
        assert!(!verify(&secret, "12345", now), "too short");
        assert!(!verify(&secret, "1234567", now), "too long");
        assert!(!verify(&secret, "abcdef", now), "not digits");
        assert!(!verify(&secret, "", now), "empty");
    }

    /// Apps and people both space the digits out; refusing that is a support ticket.
    #[test]
    fn spacing_a_code_out_does_not_break_it() {
        let secret = generate_secret();
        let now = 1_700_000_000;
        let code = code_at(&secret, now);
        let spaced = format!("{} {}", &code[..3], &code[3..]);
        let hyphenated = format!("{}-{}", &code[..3], &code[3..]);
        assert!(verify(&secret, &spaced, now));
        assert!(verify(&secret, &hyphenated, now));
    }

    /// Two secrets must not accept each other's codes.
    #[test]
    fn a_code_from_another_secret_is_refused() {
        let now = 1_700_000_000;
        let mine = generate_secret();
        let theirs = generate_secret();
        assert!(!verify(&mine, &code_at(&theirs, now), now));
    }

    #[test]
    fn the_provisioning_uri_carries_what_an_app_needs() {
        let uri = provisioning_uri(RFC_SECRET, "administrator", "rDownloader");
        assert!(
            uri.starts_with("otpauth://totp/rDownloader:administrator?"),
            "{uri}"
        );
        assert!(
            uri.contains("secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"),
            "{uri}"
        );
        assert!(uri.contains("issuer=rDownloader"), "{uri}");
        assert!(
            uri.contains("digits=6") && uri.contains("period=30"),
            "{uri}"
        );
    }

    /// A label with a space or a colon would otherwise produce a URI apps misparse.
    #[test]
    fn the_provisioning_uri_escapes_its_label() {
        let uri = provisioning_uri(RFC_SECRET, "the admin", "rD:Home");
        assert!(!uri["otpauth://totp/".len()..].contains(' '), "{uri}");
        assert!(uri.contains("rD%3AHome"), "{uri}");
        assert!(uri.contains("the%20admin"), "{uri}");
    }

    #[test]
    fn a_generated_secret_has_the_expected_length_and_is_not_constant() {
        let first = generate_secret();
        assert_eq!(first.len(), SECRET_BYTES);
        assert_ne!(first, generate_secret());
    }
}
