//! The framing every signed artefact in this project is hashed with.
//!
//! One rule, applied everywhere: **every field is length-prefixed**. Concatenating
//! `"ab" + "c"` and `"a" + "bc"` produces the same bytes, so a hash over a bare
//! concatenation lets an attacker move a member boundary without changing the digest.
//! Prefixing each field with its big-endian `u64` length removes that freedom.
//!
//! This framing is **frozen**. The plugin packages shipped in `dist/plugins` are signed
//! with it and the release public key that verifies them is compiled into the binary, so a
//! change here would invalidate every package already in the field. [`tests`] pins it with
//! byte-literal vectors for exactly that reason.

use sha2::{Digest, Sha256};

/// Accumulates the length-prefixed payload that gets signed.
///
/// Fields are hashed in call order, so two documents that differ only in field order have
/// different digests. Use [`DigestBuilder::named_fields`] where the input has no inherent
/// order — it sorts, so the caller does not have to.
#[derive(Clone, Debug, Default)]
pub struct DigestBuilder {
    hasher: Sha256,
}

impl DigestBuilder {
    /// Starts an empty payload.
    #[must_use]
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
        }
    }

    /// Appends one length-prefixed field.
    pub fn field(&mut self, bytes: &[u8]) -> &mut Self {
        self.hasher.update((bytes.len() as u64).to_be_bytes());
        self.hasher.update(bytes);
        self
    }

    /// Appends a count-prefixed set of `(name, bytes)` pairs, sorted by name.
    ///
    /// Sorting is part of the format rather than the caller's duty: the pairs come from a
    /// directory listing or a map, neither of which has a stable order, and a digest that
    /// depended on iteration order would be reproducible only by accident.
    pub fn named_fields(&mut self, entries: &[(String, Vec<u8>)]) -> &mut Self {
        let mut sorted: Vec<&(String, Vec<u8>)> = entries.iter().collect();
        sorted.sort_by(|left, right| left.0.cmp(&right.0));
        self.hasher.update((sorted.len() as u64).to_be_bytes());
        for (name, bytes) in sorted {
            self.field(name.as_bytes());
            self.field(bytes);
        }
        self
    }

    /// Finishes the payload.
    #[must_use]
    pub fn finish(self) -> [u8; 32] {
        self.hasher.finalize().into()
    }
}

/// Hex SHA-256 of `bytes`, the form every fingerprint in the UI and the logs takes.
#[must_use]
pub fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_sha256_of(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// The framing is frozen: shipped plugin packages are signed with it.
    ///
    /// A byte literal rather than a recomputation — a test that hashes the same way the
    /// implementation does would pass after a change that breaks every installed package.
    #[test]
    fn the_framing_is_pinned_to_its_shipped_bytes() {
        let mut builder = DigestBuilder::new();
        builder.field(b"manifest");
        builder.field(b"component");
        builder.named_fields(&[]);
        assert_eq!(
            hex_sha256_of(&builder.finish()),
            "7e2934c3fbc462ddc323ac3ad1aa91dc25995266e07865ee80f48913e1e141aa"
        );
    }

    /// Length prefixes exist to stop a member boundary from moving unnoticed.
    #[test]
    fn a_moved_field_boundary_changes_the_digest() {
        let mut left = DigestBuilder::new();
        left.field(b"ab").field(b"c");
        let mut right = DigestBuilder::new();
        right.field(b"a").field(b"bc");
        assert_ne!(left.finish(), right.finish());
    }

    /// The order the caller happened to read a directory in must not reach the digest.
    #[test]
    fn named_fields_do_not_depend_on_input_order() {
        let ascending = &[
            ("de".to_owned(), b"{\"a\":1}".to_vec()),
            ("en".to_owned(), b"{\"b\":2}".to_vec()),
        ];
        let descending = &[
            ("en".to_owned(), b"{\"b\":2}".to_vec()),
            ("de".to_owned(), b"{\"a\":1}".to_vec()),
        ];
        let mut left = DigestBuilder::new();
        left.named_fields(ascending);
        let mut right = DigestBuilder::new();
        right.named_fields(descending);
        assert_eq!(left.finish(), right.finish());
    }

    /// An added translation has to change the payload, or translations are unsigned.
    #[test]
    fn named_fields_are_covered_by_the_digest() {
        let mut empty = DigestBuilder::new();
        empty.named_fields(&[]);
        let mut one = DigestBuilder::new();
        one.named_fields(&[("en".to_owned(), b"{}".to_vec())]);
        assert_ne!(empty.finish(), one.finish());
    }
}
