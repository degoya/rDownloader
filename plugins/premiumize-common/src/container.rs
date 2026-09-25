//! Which container a person handed over, read from the bytes alone.
//!
//! `job-source::container(list<u8>)` carries bytes and no name, and everything else in this
//! project decides a container's format from its extension
//! (`crates/rd-collector/src/container.rs`). The multipart upload
//! `POST /api/transfer/create` takes needs a file name, and Premiumize's documentation says
//! `.dlc`, `.ccf` and `.rsdf` are handled differently from a torrent — so the name is not
//! decoration and cannot be `container.bin`.
//!
//! So the bytes are sniffed, and only for the shapes that can be told apart with certainty:
//!
//! | Format | What says so |
//! | --- | --- |
//! | `.torrent` | a bencoded dictionary carrying a top-level `info` key |
//! | `.nzb` | an XML document carrying an `<nzb` element |
//! | `.rsdf` | text that is entirely hexadecimal, an even number of digits long |
//! | `.dlc` | text that is entirely base64 |
//!
//! `.ccf` is **not** in the table. A CryptLoad container is binary with no marker of its own,
//! so recognising it would mean calling every unrecognised blob a `.ccf` and uploading it as
//! one. An unrecognised container is refused instead, which is a sentence a person can act on.
//! Hex is tested before base64 because every hex digit is also a base64 character, and the
//! reverse is not true.

/// Longest container this plugin reads. A `.torrent`, an `.nzb` or a `.dlc` is kilobytes;
/// anything far past this is not one, and scanning it would spend the invocation's budget
/// finding that out.
pub const MAX_CONTAINER_BYTES: usize = 4 * 1024 * 1024;

/// Shortest container worth looking at. Below this there is nothing to recognise.
const MIN_CONTAINER_BYTES: usize = 16;

/// How far into a document the sniff looks for an XML marker.
const SNIFF_WINDOW: usize = 1024;

/// The name a recognised container is uploaded under.
///
/// A fixed stem, because the person's own file name never crosses the contract and inventing
/// one would put a guess in somebody's cloud. The extension is what Premiumize reads.
#[must_use]
pub fn file_name(bytes: &[u8]) -> Option<&'static str> {
    Some(match extension(bytes)? {
        "torrent" => "source.torrent",
        "nzb" => "source.nzb",
        "rsdf" => "source.rsdf",
        _ => "source.dlc",
    })
}

/// The extension the bytes announce, or `None` for something this table cannot name.
#[must_use]
pub fn extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < MIN_CONTAINER_BYTES || bytes.len() > MAX_CONTAINER_BYTES {
        return None;
    }
    if is_bencoded_torrent(bytes) {
        return Some("torrent");
    }
    if is_nzb(bytes) {
        return Some("nzb");
    }
    let text: Vec<u8> = bytes
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if text.len() >= MIN_CONTAINER_BYTES && text.len().is_multiple_of(2) && is_hex(&text) {
        return Some("rsdf");
    }
    if text.len() >= MIN_CONTAINER_BYTES && is_base64(&text) {
        return Some("dlc");
    }
    None
}

/// A bencoded dictionary that names an `info` key. Not parsed: the transfers plugin never
/// reads inside a torrent, it only has to know what to call the upload.
fn is_bencoded_torrent(bytes: &[u8]) -> bool {
    bytes.first() == Some(&b'd')
        && bytes
            .windows(6)
            .take(SNIFF_WINDOW)
            .any(|window| window == b"4:info")
}

fn is_nzb(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(SNIFF_WINDOW)];
    head.windows(4).any(|window| window == b"<nzb")
}

fn is_hex(text: &[u8]) -> bool {
    text.iter().all(u8::is_ascii_hexdigit)
}

fn is_base64(text: &[u8]) -> bool {
    text.iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

#[cfg(test)]
mod tests {
    use super::{extension, file_name};

    const TORRENT: &[u8] = b"d8:announce23:http://tracker.invalid/4:infod6:lengthi31e4:name15:Example.Release12:piece lengthi16384e6:pieces20:01234567890123456789ee";

    #[test]
    fn a_torrent_is_recognised_by_its_bencoded_info_key() {
        assert_eq!(extension(TORRENT), Some("torrent"));
        assert_eq!(file_name(TORRENT), Some("source.torrent"));
    }

    #[test]
    fn an_nzb_is_recognised_by_its_element() {
        let nzb = br#"<?xml version="1.0" encoding="iso-8859-1" ?><nzb xmlns="http://www.newzbin.com/DTD/2003/nzb"><file></file></nzb>"#;
        assert_eq!(extension(nzb), Some("nzb"));
        assert_eq!(file_name(nzb), Some("source.nzb"));
    }

    /// Hex before base64: every hex digit is a base64 character too, so the order is the rule.
    #[test]
    fn hexadecimal_text_is_an_rsdf_and_base64_text_is_a_dlc() {
        assert_eq!(extension(b"0123456789ABCDEFfedcba9876543210"), Some("rsdf"));
        assert_eq!(
            extension(b"UEsDBBQAAAAIAA+dtVYAAAAA/w==\nUEsDBBQAAAAIAA=="),
            Some("dlc")
        );
        assert_eq!(
            file_name(b"0123456789ABCDEFfedcba9876543210"),
            Some("source.rsdf")
        );
    }

    /// A CryptLoad container has no marker of its own, so it is refused rather than guessed
    /// at. So is anything else this table cannot name, and anything too small or too large.
    #[test]
    fn an_unrecognisable_container_is_refused_rather_than_named() {
        assert_eq!(extension(&[0x00, 0xff, 0x10, 0x9a][..]), None);
        assert_eq!(extension(&[0x9a_u8; 64][..]), None);
        assert_eq!(extension(b"short"), None);
        assert_eq!(extension(&vec![b'a'; 5 * 1024 * 1024][..]), None);
        assert_eq!(file_name(b"<html>not a container</html>"), None);
    }
}
