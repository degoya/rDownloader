//! The file name a container is uploaded under.
//!
//! A provider that names its job after the uploaded file names every job alike under a fixed
//! name. At Premiumize that put two jobs' files into one cloud folder (owner report,
//! 2026-09-27); TorBox uploaded as `upload.nzb` and `upload.torrent` the same way. So:
//!
//! - the name the person added the container under (`job-context`), as an upload by hand would
//!   carry it, with the extension the kind decides;
//! - without one, `<release name> [<tag>].<extension>`: the name a torrent's `info` dictionary
//!   or an NZB's head states, or `rdownloader`, and twelve hex digits of the content's digest,
//!   so two nameless uploads never share a name and the same container keeps its own.
//!
//! The rule is Premiumize's (`plugins/premiumize-common/src/container.rs`), copied rather than
//! shared: the two plugins share no crate, and a new one for forty lines would put a
//! dependency into both for what is a naming convention.

use crate::{
    api,
    source::{self, Kind},
};

/// Longest name the person added a container under that is carried into an upload, in
/// characters.
const MAX_SOURCE_NAME: usize = 200;

/// Longest release name carried into an upload's file name.
const MAX_RELEASE_NAME: usize = 120;

/// How far into an NZB the release name in its head is looked for.
const HEAD_WINDOW: usize = 4096;

/// How many hex digits of the content's digest a nameless upload carries.
const TAG_DIGITS: usize = 12;

/// The file name `bytes`, a container of `kind`, is uploaded under.
#[must_use]
pub fn upload_name(kind: Kind, source_name: Option<&str>, bytes: &[u8]) -> String {
    let extension = match kind {
        Kind::Torrent => "torrent",
        Kind::Usenet => "nzb",
        // Not a container kind; a part still needs a name.
        Kind::Web => return api::container_name(kind).to_owned(),
    };
    if let Some(stem) = source_name.and_then(source_stem) {
        return format!("{stem}.{extension}");
    }
    let release = match kind {
        Kind::Torrent => torrent_name(bytes),
        Kind::Usenet => nzb_release_name(bytes),
        Kind::Web => None,
    }
    .unwrap_or_else(|| "rdownloader".to_owned());
    match source::container_kind_and_digest(bytes) {
        Some((_, digest)) => {
            let tag: String = digest.chars().take(TAG_DIGITS).collect();
            format!("{release} [{tag}].{extension}")
        }
        None => format!("{release}.{extension}"),
    }
}

/// The person's file name as an upload's stem: every character kept but a quote, a backslash,
/// a slash or a control character, which become `_`, and without a container extension, which
/// the kind decides. `None` when no letter or digit is left.
fn source_stem(name: &str) -> Option<String> {
    let mut stem: String = name
        .trim()
        .chars()
        .take(MAX_SOURCE_NAME)
        .map(|character| {
            if character.is_control() || matches!(character, '"' | '\\' | '/') {
                '_'
            } else {
                character
            }
        })
        .collect();
    let lowered = stem.to_ascii_lowercase();
    if let Some(known) = ["torrent", "nzb"]
        .into_iter()
        .find(|known| lowered.ends_with(&format!(".{known}")))
    {
        stem.truncate(stem.len() - known.len() - 1);
    }
    let stem = stem.trim_matches([' ', '.']);
    stem.chars()
        .any(char::is_alphanumeric)
        .then(|| stem.to_owned())
}

/// The `name` a torrent's `info` dictionary states, made header-safe.
#[must_use]
pub fn torrent_name(bytes: &[u8]) -> Option<String> {
    let info = source::info_slice(bytes)?;
    if info.first() != Some(&b'd') {
        return None;
    }
    let mut at = 1;
    while at < info.len() && info[at] != b'e' {
        let (key, after_key) = source::read_byte_string(info, at)?;
        if key == b"name" {
            let (value, _) = source::read_byte_string(info, after_key)?;
            return named(&String::from_utf8_lossy(value));
        }
        at = source::skip_value(info, after_key, 1)?;
    }
    None
}

/// The release name an NZB states in its head: `<meta type="name">Show.S01E01</meta>`.
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
        if let Some(name) = named(&decode_entities(&rest[..text_end])) {
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
/// else replaced by `_`, the result clipped; `None` when no letter or digit survives.
fn named(text: &str) -> Option<String> {
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
    let out = out.trim_matches([' ', '.']);
    out.chars()
        .any(|character| character.is_ascii_alphanumeric())
        .then(|| out.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{nzb_release_name, torrent_name, upload_name};
    use crate::source::{Kind, container_info_hash};

    const TORRENT: &[u8] = b"d8:announce23:http://tracker.invalid/4:infod6:lengthi31e4:name15:Example.Release12:piece lengthi16384e6:pieces20:01234567890123456789ee";
    const NZB: &[u8] = br#"<?xml version="1.0"?><nzb xmlns="http://www.newzbin.com/DTD/2003/nzb"><head><meta type="name">ACES.S01E01.GERMAN</meta></head><file></file></nzb>"#;
    const BARE_NZB: &[u8] = br#"<?xml version="1.0"?><nzb xmlns="http://www.newzbin.com/DTD/2003/nzb"><file></file></nzb>"#;

    /// The owner's case (2026-09-27): a container added under a name goes up under that name,
    /// as it would by hand, with the extension its kind says.
    #[test]
    fn a_container_goes_up_under_the_name_it_was_added_as() {
        assert_eq!(
            upload_name(Kind::Usenet, Some("ACES.Der.Club.S01.nzb"), NZB),
            "ACES.Der.Club.S01.nzb"
        );
        assert_eq!(
            upload_name(Kind::Usenet, Some("Speedtest.NZB"), NZB),
            "Speedtest.nzb"
        );
        assert_eq!(
            upload_name(
                Kind::Torrent,
                Some("\u{dc}bel \"Folge\"\r\n1.torrent"),
                TORRENT
            ),
            "\u{dc}bel _Folge___1.torrent"
        );
        assert_eq!(
            upload_name(Kind::Torrent, Some("renamed.nzb"), TORRENT),
            "renamed.torrent"
        );
    }

    /// Without a name: the release name the container states and a tag from its digest, so
    /// two nameless uploads never share a name -- never the fixed `upload.nzb` again.
    #[test]
    fn a_nameless_container_goes_up_under_its_release_name_and_a_tag() {
        let hash = container_info_hash(TORRENT).expect("an info hash");
        assert_eq!(
            upload_name(Kind::Torrent, None, TORRENT),
            format!("Example.Release [{}].torrent", &hash[..12])
        );
        let named = upload_name(Kind::Usenet, Some(" .nzb "), NZB);
        assert!(named.starts_with("ACES.S01E01.GERMAN ["), "{named}");
        assert!(named.ends_with("].nzb"), "{named}");
        let bare = upload_name(Kind::Usenet, None, BARE_NZB);
        assert!(bare.starts_with("rdownloader ["), "{bare}");
        assert_ne!(bare, upload_name(Kind::Usenet, None, NZB));
        assert_eq!(upload_name(Kind::Web, Some("x.txt"), b""), "upload.bin");
    }

    #[test]
    fn release_names_are_read_and_reduced_to_what_a_header_carries() {
        assert_eq!(torrent_name(TORRENT).as_deref(), Some("Example.Release"));
        assert_eq!(torrent_name(b"d4:infod6:lengthi1eee"), None);
        assert_eq!(nzb_release_name(NZB).as_deref(), Some("ACES.S01E01.GERMAN"));
        assert_eq!(
            nzb_release_name(
                "<nzb><head><meta type='name'>Tom &amp; Jerry: \"\u{dc}bel\"</meta></head></nzb>"
                    .as_bytes()
            )
            .as_deref(),
            Some("Tom _ Jerry_ __bel_")
        );
        assert_eq!(nzb_release_name(BARE_NZB), None);
    }
}
