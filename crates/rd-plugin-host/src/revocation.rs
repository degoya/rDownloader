//! Package digests that have been withdrawn, and the hex form they are stored and sent in.
//!
//! Key revocation and content revocation answer different questions: revoking a key says
//! "nothing this author signs is acceptable any more", revoking a digest says "this exact
//! version is bad, whoever signed it". Only the second one withdraws a single published
//! version without taking down every other plugin the same author ever signed.
//!
//! The set lives here rather than in `rd_sign::TrustStore` for one reason: withdrawal has to be
//! reversible. A digest is revoked and un-revoked through the API, so the set needs a removal
//! and a wholesale replace for the seeding at startup, and the trust store offers neither —
//! its digest axis is the signed-document one, which is append-only on purpose. What keeps the
//! two in step is that nothing here touches the digest *framing*: `package_digest` stays the
//! payload every released `.rdplug` was signed with.
//!
//! This crate deliberately holds no database dependency for this. The set is seeded with values
//! at startup and mutated with values at runtime; who stores them is the caller's business.

use std::{
    collections::BTreeSet,
    sync::{Arc, RwLock},
};

use anyhow::{Result, bail};

/// Withdrawn package digests, shared between every clone.
///
/// Cloning shares the state for the same reason the trust store does: the verifier is cloned
/// into every subsystem that loads plugins, and a withdrawal confirmed through the API has to
/// be visible to all of them at once rather than to whichever clone happened to receive it.
#[derive(Clone, Debug, Default)]
pub struct RevokedDigests {
    inner: Arc<RwLock<BTreeSet<[u8; 32]>>>,
}

impl RevokedDigests {
    /// An empty set: nothing is withdrawn until a digest is added or seeded.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether this exact package has been withdrawn.
    pub fn contains(&self, digest: &[u8; 32]) -> Result<bool> {
        Ok(self.read()?.contains(digest))
    }

    /// Withdraws one digest; returns whether it was not already withdrawn.
    ///
    /// The caller reports that distinction back to the user, exactly as key revocation does:
    /// "withdrawn" and "was already withdrawn" are different answers to the same request.
    pub fn insert(&self, digest: [u8; 32]) -> Result<bool> {
        Ok(self.write()?.insert(digest))
    }

    /// Takes a withdrawal back; returns whether the digest was withdrawn at all.
    pub fn remove(&self, digest: &[u8; 32]) -> Result<bool> {
        Ok(self.write()?.remove(digest))
    }

    /// Replaces the whole set, for the seeding from persisted state at startup.
    ///
    /// A replace rather than a merge: the stored rows are the truth after a restart, so a
    /// digest deleted while this process was not running must not survive in memory.
    pub fn replace(&self, digests: impl IntoIterator<Item = [u8; 32]>) -> Result<()> {
        *self.write()? = digests.into_iter().collect();
        Ok(())
    }

    /// Every withdrawn digest, in a stable order, for a listing.
    pub fn all(&self) -> Result<Vec<[u8; 32]>> {
        Ok(self.read()?.iter().copied().collect())
    }

    fn read(&self) -> Result<std::sync::RwLockReadGuard<'_, BTreeSet<[u8; 32]>>> {
        self.inner
            .read()
            .map_err(|_| anyhow::anyhow!("plugin revocation list is poisoned"))
    }

    fn write(&self) -> Result<std::sync::RwLockWriteGuard<'_, BTreeSet<[u8; 32]>>> {
        self.inner
            .write()
            .map_err(|_| anyhow::anyhow!("plugin revocation list is poisoned"))
    }
}

/// Renders a package digest as the 64-character lowercase hex the API and the database use.
///
/// The wire and storage form is hex because a digest is something a person compares against a
/// published one, the same reason a key fingerprint is hex rather than base64.
#[must_use]
pub fn format_package_digest(digest: &[u8; 32]) -> String {
    let mut text = String::with_capacity(64);
    for byte in digest {
        text.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        text.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    text
}

/// Parses the hex form back, refusing anything that is not exactly 32 bytes of hex.
///
/// This is the boundary check for the revocation route: a request body is not a digest until
/// it has been through here, and a half-parsed or truncated value must not be able to enter
/// the set — a digest that is silently wrong withdraws nothing and reports success.
pub fn parse_package_digest(text: &str) -> Result<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("a package digest is 64 hexadecimal characters");
    }
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let pair = &text[index * 2..index * 2 + 2];
        *byte = u8::from_str_radix(pair, 16)?;
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::{RevokedDigests, format_package_digest, parse_package_digest};

    #[test]
    fn the_hex_form_round_trips() {
        let mut digest = [0_u8; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(7).wrapping_add(3);
        }
        let text = format_package_digest(&digest);
        assert_eq!(text.len(), 64);
        assert!(
            text.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
        assert_eq!(parse_package_digest(&text).expect("parse"), digest);
        // Upper case is accepted on the way in; a user pastes what they were given.
        assert_eq!(
            parse_package_digest(&text.to_uppercase()).expect("parse"),
            digest
        );
    }

    /// A truncated or non-hex value has to be refused rather than padded: a digest that parses
    /// into something else withdraws a package nobody named and reports success doing it.
    #[test]
    fn a_value_that_is_not_a_digest_is_refused() {
        for bad in ["", "abc", &"z".repeat(64), &"a".repeat(63), &"a".repeat(65)] {
            assert!(
                parse_package_digest(bad).is_err(),
                "must refuse {bad:?} as a digest"
            );
        }
    }

    #[test]
    fn a_withdrawal_reaches_existing_clones_and_can_be_taken_back() {
        let set = RevokedDigests::new();
        let clone = set.clone();
        assert!(set.insert([4; 32]).expect("insert"));
        assert!(!set.insert([4; 32]).expect("insert"), "already withdrawn");
        assert!(clone.contains(&[4; 32]).expect("read"));

        assert!(clone.remove(&[4; 32]).expect("remove"));
        assert!(!set.contains(&[4; 32]).expect("read"));
        assert!(
            !clone.remove(&[4; 32]).expect("remove"),
            "was not withdrawn"
        );
    }

    /// Seeding is what a restart does, so it has to drop what is no longer stored.
    #[test]
    fn seeding_replaces_the_set_rather_than_merging_into_it() {
        let set = RevokedDigests::new();
        set.insert([1; 32]).expect("insert");
        set.replace([[2; 32], [3; 32]]).expect("seed");
        assert!(!set.contains(&[1; 32]).expect("read"));
        assert_eq!(set.all().expect("list"), vec![[2; 32], [3; 32]]);
    }
}
