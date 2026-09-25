//! What this plugin answers with, per address.
//!
//! Outside the component so it compiles and is readable on the host target; the guest is a
//! translation of these values into the WIT vocabulary and nothing else.

/// The host this plugin claims, and the only one its manifest grants.
pub const HOST: &str = "transform.example.invalid";

/// The key every described stream is encrypted under. Synthetic, and public on purpose: a
/// reference plugin that carried a real provider's key would be a reference nobody could ship.
pub const KEY: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];

/// The counter prefix of every described stream.
pub const NONCE: [u8; 8] = [0x80, 0x1b, 0x72, 0xfd, 0x96, 0x41, 0xcc, 0xfa];

/// Where the chunks of the described file end. The last one is its size.
pub const BOUNDARIES: [u64; 3] = [131_072, 393_216, 524_288];

/// The condensed integrity value the described file is expected to produce. A fixed eight
/// bytes: this plugin answers about a file nobody downloads, so the value only has to be
/// stable, not right.
pub const EXPECTED: [u8; 8] = [0x42, 0xe9, 0x20, 0xfd, 0x56, 0xfd, 0x5e, 0x0b];

/// One shape of answer, chosen by the path of the address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// A cipher and an integrity value: everything the host can compute.
    Full,
    /// A cipher alone, for a provider that publishes nothing to check against.
    CipherOnly,
    /// A cipher this build does not implement. The host must refuse it, not fall back.
    UnknownCipher,
    /// An integrity algorithm this build does not implement.
    UnknownIntegrity,
    /// A nonce of the wrong length, which is a parameter error rather than an unknown name.
    BadNonce,
}

impl Case {
    /// The case an address asks for, or `None` when this plugin does not claim it.
    #[must_use]
    pub fn of(url: &str) -> Option<Self> {
        let rest = url
            .strip_prefix("https://")
            .and_then(|rest| rest.strip_prefix(HOST))?;
        Some(match rest {
            "/file/known" => Self::Full,
            "/file/plain" => Self::CipherOnly,
            "/file/unknown-cipher" => Self::UnknownCipher,
            "/file/unknown-integrity" => Self::UnknownIntegrity,
            "/file/bad-nonce" => Self::BadNonce,
            _ => return None,
        })
    }

    /// The cipher name this case describes.
    #[must_use]
    pub fn cipher(self) -> &'static str {
        match self {
            Self::UnknownCipher => "rot13-ctr",
            _ => "aes-128-ctr",
        }
    }

    /// The integrity algorithm name, or `None` for a case that describes none.
    #[must_use]
    pub fn integrity(self) -> Option<&'static str> {
        match self {
            Self::CipherOnly => None,
            Self::UnknownIntegrity => Some("sha3-chain"),
            _ => Some("cbc-mac-chain"),
        }
    }

    /// The nonce this case describes.
    #[must_use]
    pub fn nonce(self) -> Vec<u8> {
        match self {
            Self::BadNonce => NONCE[..7].to_vec(),
            _ => NONCE.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Case;

    #[test]
    fn every_case_has_its_own_address() {
        assert_eq!(
            Case::of("https://transform.example.invalid/file/known"),
            Some(Case::Full)
        );
        assert_eq!(
            Case::of("https://transform.example.invalid/file/plain"),
            Some(Case::CipherOnly)
        );
        assert_eq!(
            Case::of("https://transform.example.invalid/file/unknown-cipher"),
            Some(Case::UnknownCipher)
        );
        assert_eq!(Case::of("https://transform.example.invalid/other"), None);
        assert_eq!(Case::of("https://cdn.example.org/a/b.bin"), None);
        assert_eq!(Case::of("not a url"), None);
    }

    #[test]
    fn only_the_refusal_cases_name_something_the_host_cannot_compute() {
        assert_eq!(Case::Full.cipher(), "aes-128-ctr");
        assert_eq!(Case::UnknownCipher.cipher(), "rot13-ctr");
        assert_eq!(Case::CipherOnly.integrity(), None);
        assert_eq!(Case::UnknownIntegrity.integrity(), Some("sha3-chain"));
        assert_eq!(Case::Full.nonce().len(), 8);
        assert_eq!(Case::BadNonce.nonce().len(), 7);
    }
}
