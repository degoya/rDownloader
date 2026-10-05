//! MD5 checksum sidecars as a post-processing step (RD-090-16).
//!
//! The step itself — reading the `.md5` sidecars, streaming each listed file through the
//! hash, the checkpoint and the codes — is `checksum-postprocess-common`'s, shared with the
//! SHA-256 plugin (RD-1110-04). What is this plugin's own is here: the [`ALGORITHM`] it reads
//! sidecars of and the hash it links. The two stay separate plugins, so each is updated,
//! versioned and switched off on its own. `guest` is the component wrapper and exists only on
//! `wasm32`.

use checksum_postprocess_common::{Checksum, sidecar::Algorithm};
use md5::{Digest, Md5};

/// The sidecars this plugin verifies and the codes it reports under.
pub const ALGORITHM: Algorithm = Algorithm {
    extension: ".md5",
    digest_hex_len: 32,
    slug: "md5_postprocess",
    label: "MD5",
};

/// The hash a `.md5` sidecar records.
pub struct Md5Checksum(Md5);

impl Checksum for Md5Checksum {
    fn new() -> Self {
        Self(Md5::new())
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

    use super::{ALGORITHM, Md5Checksum};

    fn digest(chunks: &[&[u8]]) -> String {
        let mut hash = Md5Checksum::new();
        for chunk in chunks {
            hash.update(chunk);
        }
        to_hex(&hash.finish())
    }

    #[test]
    fn the_hash_is_the_one_a_md5_sidecar_records() {
        assert_eq!(digest(&[]), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(
            digest(&[b"a".as_slice(), b"bc".as_slice()]),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(digest(&[]).len(), ALGORITHM.digest_hex_len);
    }

    #[test]
    fn the_plugin_reads_its_own_sidecars_and_reports_under_its_slug() {
        assert!(ALGORITHM.is_sidecar("Film/release.MD5"));
        assert!(!ALGORITHM.is_sidecar("release.sfv"));
        assert_eq!(ALGORITHM.code("mismatch"), "md5_postprocess.mismatch");
    }
}
