//! A file name read from an address or from the server's answer (RD-1240-33).

/// The name a download takes when neither its address nor anything else offered one. The
/// worker replaces it with the name a server's `Content-Disposition` declares.
pub const FALLBACK_FILE_NAME: &str = "download.bin";

/// The last segment of an address's path as the name a person reads: percent-decoded as UTF-8,
/// so `Big%20Buck%20Test%20(2026).mkv` names the file `Big Buck Test (2026).mkv` and not the
/// escaped spelling. Bytes that are no UTF-8 become U+FFFD, which the sanitising turns into `_`;
/// an encoded `/` or `\` (`%2F`, `%5C`) becomes `_` here already, so it can never name a folder,
/// whatever the caller does with the name next.
#[must_use]
pub fn decode_path_segment(segment: &str) -> String {
    String::from_utf8_lossy(&percent_decode(segment)).replace(['/', '\\'], "_")
}

/// The file name a `Content-Disposition` value declares (RFC 6266): `filename*=UTF-8''…`
/// (RFC 5987) first, then `filename="…"`; `None` when it names none. Not sanitised — the
/// caller decides where the name goes.
#[must_use]
pub fn disposition_file_name(value: &str) -> Option<String> {
    let mut plain = None;
    for part in value.split(';').map(str::trim) {
        if let Some(rest) = part.strip_prefix("filename*=") {
            let encoded = rest.trim_matches('"');
            let encoded = encoded.splitn(3, '\'').nth(2).unwrap_or(encoded);
            if let Ok(decoded) = String::from_utf8(percent_decode(encoded))
                && !decoded.is_empty()
            {
                return Some(decoded);
            }
        } else if let Some(rest) = part.strip_prefix("filename=") {
            let name = rest.trim_matches(['"', '\'']).trim();
            if !name.is_empty() {
                plain = Some(name.to_owned());
            }
        }
    }
    plain
}

/// `%XX` escapes as the bytes they stand for; anything else, a broken escape included, as
/// written.
fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%'
            && let [high, low, after @ ..] = tail
            && let (Some(high), Some(low)) = (hex_value(*high), hex_value(*low))
        {
            decoded.push((high << 4) | low);
            rest = after;
            continue;
        }
        decoded.push(byte);
        rest = tail;
    }
    decoded
}

fn hex_value(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

#[cfg(test)]
mod tests {
    use super::{decode_path_segment, disposition_file_name};

    #[test]
    fn a_disposition_names_its_file_the_encoded_form_first() {
        assert_eq!(
            disposition_file_name("attachment; filename=\"Big Buck Bunny.mp4\"").as_deref(),
            Some("Big Buck Bunny.mp4")
        );
        assert_eq!(
            disposition_file_name("attachment; filename=x.bin; filename*=UTF-8''Caf%C3%A9.zip")
                .as_deref(),
            Some("Café.zip")
        );
        assert_eq!(disposition_file_name("attachment"), None);
    }

    #[test]
    fn a_percent_encoded_name_reads_as_it_is_meant() {
        assert_eq!(
            decode_path_segment("Big%20Buck%20Test%20%282026%29.mkv"),
            "Big Buck Test (2026).mkv"
        );
        assert_eq!(
            decode_path_segment("Big%20Buck%20Test%20(2026).mkv"),
            "Big Buck Test (2026).mkv"
        );
        assert_eq!(decode_path_segment("Caf%C3%A9.zip"), "Café.zip");
        assert_eq!(decode_path_segment("plain.bin"), "plain.bin");
    }

    /// An encoded separator stays part of the name; a broken escape is kept as written.
    #[test]
    fn an_encoded_separator_never_names_a_folder() {
        assert_eq!(decode_path_segment("..%2Fetc%2Fpasswd"), ".._etc_passwd");
        assert_eq!(decode_path_segment("a%5Cb.txt"), "a_b.txt");
        assert_eq!(decode_path_segment("100%.txt"), "100%.txt");
        assert_eq!(decode_path_segment("%zz%4"), "%zz%4");
        // Not UTF-8: replaced, not dropped.
        assert_eq!(decode_path_segment("a%FFb"), "a\u{FFFD}b");
    }
}
