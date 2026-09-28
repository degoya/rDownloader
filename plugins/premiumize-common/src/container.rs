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

/// How far into an NZB the release name in its head is looked for.
const HEAD_WINDOW: usize = 4096;

/// Longest release name carried into an upload's file name.
const MAX_RELEASE_NAME: usize = 120;

/// Longest name the person added a container under that is carried into an upload, in
/// characters.
const MAX_SOURCE_NAME: usize = 200;

/// The name a recognised container is uploaded under.
///
/// Premiumize reads the extension, and it names the transfer -- and the cloud folder a
/// finished transfer lands in -- after the whole file name. Until 1.5 every container went up
/// as `source.<extension>`: two NZBs became two transfers called `source.nzb` whose files met
/// in one folder of that name, and both jobs handed the other's files back as their own (owner
/// report, 2026-09-27). A container uploaded by hand gets a folder named after its file, so:
///
/// - `source_name`, the name the person added the container under (`job-context`), made safe
///   for a multipart header and given the extension the bytes announce: `Show.S01.nzb`;
/// - without one, `<release name> [<tag>].<extension>`, the release name an NZB states in its
///   head or `rdownloader`, and `tag` -- derived by the caller from the content -- so that two
///   nameless uploads never share a name and the same container always gets the same one.
#[must_use]
pub fn file_name(bytes: &[u8], source_name: Option<&str>, tag: &str) -> Option<String> {
    let extension = extension(bytes)?;
    if let Some(stem) = source_name.and_then(|name| source_stem(name, extension)) {
        return Some(format!("{stem}.{extension}"));
    }
    let stem = if extension == "nzb" {
        nzb_release_name(bytes)
    } else {
        None
    }
    .unwrap_or_else(|| "rdownloader".to_owned());
    let tag: String = tag.chars().filter(char::is_ascii_alphanumeric).collect();
    Some(if tag.is_empty() {
        format!("{stem}.{extension}")
    } else {
        format!("{stem} [{tag}].{extension}")
    })
}

/// The person's file name as an upload's stem: every character kept but a quote, a
/// backslash, a slash or a control character, which become `_`, and without the container
/// extension, which the bytes decide. `None` when no letter or digit is left.
fn source_stem(name: &str, extension: &str) -> Option<String> {
    let mut stem = String::new();
    for character in name.trim().chars().take(MAX_SOURCE_NAME) {
        if character.is_control() || matches!(character, '"' | '\\' | '/') {
            stem.push('_');
        } else {
            stem.push(character);
        }
    }
    let lowered = stem.to_ascii_lowercase();
    for known in ["torrent", "nzb", "rsdf", "dlc", "ccf", extension] {
        if lowered.ends_with(&format!(".{known}")) {
            stem.truncate(stem.len() - known.len() - 1);
            break;
        }
    }
    let stem = stem.trim_matches([' ', '.']);
    stem.chars()
        .any(char::is_alphanumeric)
        .then(|| stem.to_owned())
}

/// The release name an NZB states in its head: `<meta type="name">Show.S01E01</meta>`.
///
/// Reduced to characters a multipart header carries without escaping, because it becomes part
/// of one. `None` when the head names nothing, or no letter or digit survives the reduction.
#[must_use]
pub fn nzb_release_name(bytes: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(HEAD_WINDOW)]);
    let mut rest = head.as_ref();
    while let Some(start) = rest.find("<meta") {
        rest = &rest[start + "<meta".len()..];
        let attributes_end = rest.find('>')?;
        let attributes = &rest[..attributes_end];
        rest = &rest[attributes_end + 1..];
        let names_release = !attributes.ends_with('/')
            && attributes
                .split_whitespace()
                .any(|attribute| matches!(attribute, "type=\"name\"" | "type='name'"));
        if !names_release {
            continue;
        }
        let text_end = rest.find("</meta>")?;
        let name = header_safe(&decode_entities(&rest[..text_end]));
        if name
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
        {
            return Some(name);
        }
    }
    None
}

/// The five entities XML predefines; `&amp;` last, so `&amp;lt;` stays `&lt;`.
fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// ASCII letters, digits and `. - _ ( ) +` kept, whitespace folded to single spaces, anything
/// else -- a quote, a slash, a non-ASCII letter -- replaced by `_`, and the result clipped.
fn header_safe(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_RELEASE_NAME));
    for character in text.trim().chars() {
        if out.len() >= MAX_RELEASE_NAME {
            break;
        }
        if character.is_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else if character.is_ascii_alphanumeric()
            || matches!(character, '.' | '-' | '_' | '(' | ')' | '+')
        {
            out.push(character);
        } else {
            out.push('_');
        }
    }
    out.trim_matches([' ', '.']).to_owned()
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
    use super::{extension, file_name, nzb_release_name};

    const TORRENT: &[u8] = b"d8:announce23:http://tracker.invalid/4:infod6:lengthi31e4:name15:Example.Release12:piece lengthi16384e6:pieces20:01234567890123456789ee";

    #[test]
    fn a_torrent_is_recognised_by_its_bencoded_info_key() {
        assert_eq!(extension(TORRENT), Some("torrent"));
        assert_eq!(
            file_name(TORRENT, None, "0a1b2c").as_deref(),
            Some("rdownloader [0a1b2c].torrent")
        );
    }

    #[test]
    fn an_nzb_is_recognised_by_its_element() {
        let nzb = br#"<?xml version="1.0" encoding="iso-8859-1" ?><nzb xmlns="http://www.newzbin.com/DTD/2003/nzb"><file></file></nzb>"#;
        assert_eq!(extension(nzb), Some("nzb"));
        assert_eq!(
            file_name(nzb, None, "0a1b2c").as_deref(),
            Some("rdownloader [0a1b2c].nzb")
        );
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
            file_name(b"0123456789ABCDEFfedcba9876543210", None, "0a1b2c").as_deref(),
            Some("rdownloader [0a1b2c].rsdf")
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
        assert_eq!(
            file_name(b"<html>not a container</html>", Some("x.nzb"), "0a1b2c"),
            None
        );
    }

    /// Owner report 2026-09-27: two NZBs uploaded under one fixed name became two transfers
    /// of that name, and their files met in one folder. Two containers never share a name now,
    /// and the release name an NZB states is what the name starts with.
    #[test]
    fn two_nameless_containers_never_share_an_upload_name_and_an_nzb_brings_its_release_name() {
        let first = br#"<?xml version="1.0"?><nzb xmlns="http://www.newzbin.com/DTD/2003/nzb"><head><meta type="title">ignored</meta><meta type="name">ACES.Der.Club.S01E01.GERMAN.1080p</meta></head><file></file></nzb>"#;
        let second = br#"<?xml version="1.0"?><nzb xmlns="http://www.newzbin.com/DTD/2003/nzb"><file subject="test"></file></nzb>"#;
        assert_eq!(
            file_name(first, None, "aaaa1111").as_deref(),
            Some("ACES.Der.Club.S01E01.GERMAN.1080p [aaaa1111].nzb")
        );
        assert_eq!(
            file_name(second, None, "bbbb2222").as_deref(),
            Some("rdownloader [bbbb2222].nzb")
        );
        assert_ne!(
            file_name(second, None, "aaaa1111"),
            file_name(second, None, "bbbb2222")
        );
    }

    /// The release name ends up inside a quoted multipart header, so nothing that could close
    /// the quote or name a path survives, and entities are read as what they stand for.
    #[test]
    fn a_release_name_is_reduced_to_what_a_header_carries() {
        let nzb = "<nzb><head><meta type='name'>  Tom &amp; Jerry: \"\u{dc}bel\"/../x  </meta></head></nzb>";
        assert_eq!(
            nzb_release_name(nzb.as_bytes()).as_deref(),
            Some("Tom _ Jerry_ __bel__.._x")
        );
        assert_eq!(
            nzb_release_name(b"<nzb><head><meta type=\"name\"/></head></nzb>"),
            None
        );
        assert_eq!(
            nzb_release_name(
                "<nzb><head><meta type=\"name\">\"\u{dc}\"</meta></head></nzb>".as_bytes()
            ),
            None
        );
        assert_eq!(nzb_release_name(b"<nzb><file></file></nzb>"), None);
    }

    /// The owner's case: two NZBs added by name go up under those names, as they would by
    /// hand, and Premiumize gives each a folder of its own.
    #[test]
    fn a_container_the_person_named_goes_up_under_that_name() {
        let nzb = br#"<?xml version="1.0"?><nzb xmlns="http://www.newzbin.com/DTD/2003/nzb"><head><meta type="name">Other.Name</meta></head><file></file></nzb>"#;
        assert_eq!(
            file_name(
                nzb,
                Some("ACES.Der.Club.der.Tennisgiganten.S01.nzb"),
                "aaaa1111"
            )
            .as_deref(),
            Some("ACES.Der.Club.der.Tennisgiganten.S01.nzb")
        );
        assert_eq!(
            file_name(nzb, Some("Speedtest.NZB"), "bbbb2222").as_deref(),
            Some("Speedtest.nzb"),
            "the extension is the one the bytes announce"
        );
        assert_eq!(
            file_name(nzb, Some("\u{dc}bel \"Folge\" 1"), "bbbb2222").as_deref(),
            Some("\u{dc}bel _Folge_ 1.nzb"),
            "a letter is kept, a quote is not"
        );
        assert_eq!(
            file_name(TORRENT, Some("renamed.nzb"), "cccc3333").as_deref(),
            Some("renamed.torrent")
        );
        // Nothing usable in the name: the nameless rule applies.
        assert_eq!(
            file_name(nzb, Some(" .nzb "), "dddd4444").as_deref(),
            Some("Other.Name [dddd4444].nzb")
        );
    }
}
