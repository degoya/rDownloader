//! The two primitives every stored credential goes through: its digest, and the comparison
//! against one (audit 1.9.1, API-11).
//!
//! Both used to be written out where they were needed -- the hex SHA-256 eight times across the
//! HTTP surface, the byte-by-byte comparison twice here -- so one copy could drift from the
//! others without anyone noticing.

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Lowercase hex SHA-256 of `bytes`: the stored form of a bearer, a session token or a
/// recovery code, and the content hash of an upload.
#[must_use]
pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}

/// Compares two byte strings in time that depends only on their lengths.
///
/// A length mismatch answers `false` at once: lengths are not secret here, the stored side is a
/// fixed-width digest or code.
#[must_use]
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    bool::from(left.ct_eq(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_lowercase_hex_of_sha256() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex("abc"),
            sha256_hex(String::from("abc").into_bytes())
        );
    }

    #[test]
    fn equal_bytes_match_and_anything_else_does_not() {
        assert!(constant_time_eq(b"123456", b"123456"));
        assert!(!constant_time_eq(b"123456", b"123457"));
        assert!(!constant_time_eq(b"123456", b"12345"));
        assert!(constant_time_eq(b"", b""));
    }
}
