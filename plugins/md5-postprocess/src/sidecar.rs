//! Reading a `.md5` sidecar.
//!
//! The format is the one `md5sum` writes: one line per file, `<hex>  <name>`. A leading
//! `*` on the name marks binary mode and means nothing here — every read is binary.
//!
//! Deliberately duplicated in the SHA-256 plugin rather than shared. The two are separate plugins
//! so each can be updated, versioned and switched off on its own; a shared crate would quietly
//! make them one thing again, and thirty lines is a cheap price for keeping them apart.

/// The extension that marks a sidecar of this kind.
pub const EXTENSION: &str = ".md5";
/// Length of the hex digest MD5 produces.
pub const DIGEST_HEX_LEN: usize = 32;

/// One `<hex>  <name>` line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub digest: String,
    pub file: String,
}

/// Whether a package file is a sidecar of this kind.
#[must_use]
pub fn is_sidecar(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(EXTENSION)
}

/// Parses a sidecar's text.
///
/// A malformed line is skipped rather than failing the file: a stray comment or a blank line
/// is no reason to refuse to check the entries that *are* well formed. A digest of the wrong
/// length is skipped for the same reason — it cannot match anything, and treating it as a
/// mismatch would report a file as corrupt on the strength of a typo.
#[must_use]
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((digest, file)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let digest = digest.trim().to_ascii_lowercase();
        if digest.len() != DIGEST_HEX_LEN || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        // `*name` is binary mode; the marker is not part of the name.
        let file = file.trim().trim_start_matches('*').trim();
        if file.is_empty() {
            continue;
        }
        entries.push(Entry {
            digest,
            file: file.to_owned(),
        });
    }
    entries
}

/// Lower-case hex of a digest, for comparing with what a sidecar recorded.
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Entry, is_sidecar, parse, to_hex};

    /// MD5 of the empty input, used only for its shape.
    const DIGEST: &str = "d41d8cd98f00b204e9800998ecf8427e";

    #[test]
    fn a_sidecar_is_recognised_by_its_extension() {
        assert!(is_sidecar("release.md5"));
        assert!(is_sidecar("RELEASE.MD5"));
        assert!(!is_sidecar("release.sfv"));
        assert!(!is_sidecar("md5"));
    }

    #[test]
    fn the_usual_two_space_form_is_read() {
        assert_eq!(
            parse(&format!("{DIGEST}  release.bin\n")),
            vec![Entry {
                digest: DIGEST.to_owned(),
                file: "release.bin".to_owned(),
            }]
        );
    }

    #[test]
    fn binary_mode_and_upper_case_digests_are_accepted() {
        let entries = parse(&format!("{}  *release.bin", DIGEST.to_uppercase()));
        assert_eq!(entries[0].digest, DIGEST);
        assert_eq!(entries[0].file, "release.bin");
    }

    #[test]
    fn a_malformed_line_costs_only_itself() {
        let text = format!("# a comment\n\nnothex  release.bin\n{DIGEST}  good.bin\n");
        let entries = parse(&text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file, "good.bin");
    }

    #[test]
    fn hex_is_lower_case_and_zero_padded() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
    }
}
