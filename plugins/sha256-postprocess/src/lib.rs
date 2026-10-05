//! SHA-256 checksum sidecars as a post-processing step (RD-090-16).
//!
//! The step itself — reading the `.sha256` sidecars, streaming each listed file through the
//! hash, the checkpoint and the codes — is `checksum-postprocess-common`'s, shared with the
//! MD5 plugin (RD-1110-04). What is this plugin's own is here: the [`ALGORITHM`] it reads
//! sidecars of and the hash it links. The two stay separate plugins, so each is updated,
//! versioned and switched off on its own. `guest` is the component wrapper and exists only on
//! `wasm32`.

use checksum_postprocess_common::{Checksum, sidecar::Algorithm};
use sha2::{Digest, Sha256};

/// The sidecars this plugin verifies and the codes it reports under.
pub const ALGORITHM: Algorithm = Algorithm {
    extension: ".sha256",
    digest_hex_len: 64,
    slug: "sha256_postprocess",
    label: "SHA-256",
};

/// The hash a `.sha256` sidecar records.
pub struct Sha256Checksum(Sha256);

impl Checksum for Sha256Checksum {
    fn new() -> Self {
        Self(Sha256::new())
    }

    fn update(&mut self, bytes: &[u8]) {
        Digest::update(&mut self.0, bytes);
    }

    fn finish(self) -> Vec<u8> {
        self.0.finalize().to_vec()
    }
}

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(test)]
mod tests {
    use checksum_postprocess_common::Checksum;
    use plugin_common::encode::to_hex;

    use super::{ALGORITHM, Sha256Checksum};

    fn digest(chunks: &[&[u8]]) -> String {
        let mut hash = Sha256Checksum::new();
        for chunk in chunks {
            hash.update(chunk);
        }
        to_hex(&hash.finish())
    }

    #[test]
    fn the_hash_is_the_one_a_sha256_sidecar_records() {
        assert_eq!(
            digest(&[]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            digest(&[b"a".as_slice(), b"bc".as_slice()]),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(digest(&[]).len(), ALGORITHM.digest_hex_len);
    }

    #[test]
    fn the_plugin_reads_its_own_sidecars_and_reports_under_its_slug() {
        assert!(ALGORITHM.is_sidecar("Film/release.SHA256"));
        assert!(!ALGORITHM.is_sidecar("release.sfv"));
        assert_eq!(ALGORITHM.code("mismatch"), "sha256_postprocess.mismatch");
    }
}
