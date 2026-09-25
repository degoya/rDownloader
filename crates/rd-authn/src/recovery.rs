//! Recovery codes: the way back in when the second factor is gone.
//!
//! The phone is lost, wiped, or simply replaced without moving the authenticator across. Every
//! deployment of a second factor needs an answer to that, and for a self-hosted single-user
//! service there is nobody to appeal to — no support desk, no account recovery. So the answer
//! has to be issued in advance, at enrolment, and it has to be the *only* other way in.
//!
//! ## Why these are hashed like passwords are not
//!
//! A recovery code carries ~62 bits of entropy from a random generator, unlike a password. It
//! therefore does not need a memory-hard hash: the attack a slow hash defends against —
//! guessing the input — is already infeasible. SHA-256 is used instead, which keeps
//! verification cheap enough that comparing against every stored code costs nothing.
//!
//! ## Single use, and a count the user can see
//!
//! Each code works once and is then spent. A code that still works after being used turns one
//! screenshot, one printout left in a drawer, into a permanent bypass of the second factor.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// How many codes are issued at enrolment.
///
/// Ten is enough that losing a few to a mistyped entry does not matter, and few enough that a
/// person will actually store the list rather than skip past it.
pub const CODE_COUNT: usize = 10;

/// Random bytes per code — 8 bytes, about 62 bits after encoding.
const CODE_BYTES: usize = 8;

/// One newly issued code: the value to show once, and the digest to keep.
#[derive(Clone, Debug)]
pub struct RecoveryCode {
    /// Shown to the user exactly once, at enrolment.
    pub plaintext: String,
    /// What is stored. The plaintext is never recoverable from it.
    pub digest: String,
}

/// Issues a fresh set.
#[must_use]
pub fn generate_codes() -> Vec<RecoveryCode> {
    (0..CODE_COUNT)
        .map(|_| {
            let mut bytes = [0_u8; CODE_BYTES];
            rand::rng().fill_bytes(&mut bytes);
            let plaintext = format_code(&URL_SAFE_NO_PAD.encode(bytes));
            let digest = digest_of(&plaintext);
            RecoveryCode { plaintext, digest }
        })
        .collect()
}

/// The stored form of a code.
///
/// Normalised first, so a code typed without its hyphen, in the wrong case, or with the
/// spacing a person adds while reading it aloud still matches what was stored.
#[must_use]
pub fn digest_of(code: &str) -> String {
    let normalised = normalise(code);
    let digest = Sha256::digest(normalised.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Finds which stored digest a typed code matches, if any.
///
/// Every candidate is compared, with no early exit, so the time taken does not depend on where
/// in the list the match was — or on whether there was one.
#[must_use]
pub fn find_match(candidate: &str, stored: &[String]) -> Option<usize> {
    let digest = digest_of(candidate);
    let mut found = None;
    for (index, entry) in stored.iter().enumerate() {
        if constant_time_eq(entry.as_bytes(), digest.as_bytes()) {
            found = Some(index);
        }
    }
    found
}

/// Groups the code for reading: `abcd-efgh-ijkl`.
fn format_code(encoded: &str) -> String {
    encoded
        .to_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

/// Strips everything a person might add while typing one out.
fn normalise(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_set_is_issued_and_every_code_is_different() {
        let codes = generate_codes();
        assert_eq!(codes.len(), CODE_COUNT);
        let mut seen: Vec<&str> = codes.iter().map(|code| code.plaintext.as_str()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), CODE_COUNT, "two codes came out the same");
    }

    /// The stored form must not be the code.
    #[test]
    fn the_stored_form_does_not_contain_the_code() {
        for code in generate_codes() {
            assert!(!code.digest.contains(&code.plaintext));
            assert_eq!(code.digest.len(), 64, "not a hex sha-256");
            assert!(code.digest.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn a_code_matches_its_own_digest_and_no_other() {
        let codes = generate_codes();
        let stored: Vec<String> = codes.iter().map(|code| code.digest.clone()).collect();
        for (index, code) in codes.iter().enumerate() {
            assert_eq!(find_match(&code.plaintext, &stored), Some(index));
        }
        assert_eq!(find_match("not-a-real-code", &stored), None);
    }

    /// A person reading a code off a printout will not reproduce the punctuation exactly.
    #[test]
    fn a_code_is_recognised_however_it_was_typed() {
        let code = generate_codes().remove(0);
        let stored = vec![code.digest.clone()];
        let bare = code.plaintext.replace('-', "");
        for variant in [
            code.plaintext.clone(),
            bare.clone(),
            bare.to_uppercase(),
            format!("  {}  ", code.plaintext),
            code.plaintext.replace('-', " "),
        ] {
            assert!(
                find_match(&variant, &stored).is_some(),
                "`{variant}` was not recognised"
            );
        }
    }

    #[test]
    fn an_empty_or_absurd_input_matches_nothing() {
        let stored: Vec<String> = generate_codes()
            .into_iter()
            .map(|code| code.digest)
            .collect();
        for candidate in ["", "   ", "-", &"a".repeat(10_000)] {
            assert_eq!(find_match(candidate, &stored), None, "`{candidate}`");
        }
    }

    /// Against an empty list — every code already spent — nothing may match.
    #[test]
    fn nothing_matches_once_every_code_is_spent() {
        let code = generate_codes().remove(0);
        assert_eq!(find_match(&code.plaintext, &[]), None);
    }

    /// The grouping is for reading; it must not change what is stored.
    #[test]
    fn the_digest_ignores_the_grouping() {
        let code = generate_codes().remove(0);
        assert_eq!(
            digest_of(&code.plaintext),
            digest_of(&code.plaintext.replace('-', ""))
        );
    }
}
