//! Building the addresses one upload needs.
//!
//! Three of them: the collection the package goes into, the file inside it, and the same file
//! again for the verification `PROPFIND`. They are built here rather than inline so the
//! percent-encoding and the slash handling have somewhere to be tested.

/// Characters that must be escaped in a path segment. Deliberately an allowlist: a file name
/// comes from a release somebody else made, and guessing which characters a given server
/// tolerates is how an upload ends up at an address nobody meant.
fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The collection this package writes into: the configured destination plus the package name.
#[must_use]
pub fn collection(destination: &str, package: &str) -> String {
    format!(
        "{}/{}",
        destination.trim_end_matches('/'),
        encode_segment(package)
    )
}

/// The address of one file inside the collection.
#[must_use]
pub fn file_url(collection: &str, file_name: &str) -> String {
    // A file name may carry a directory part when the package has subfolders; each segment is
    // encoded on its own so the separators survive and nothing else does.
    let path = file_name
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/");
    format!("{}/{path}", collection.trim_end_matches('/'))
}

/// The `PROPFIND` body that asks only for a file's length.
///
/// Depth 0 and one property: a listing of the whole collection would be a larger answer for
/// no more information, and the response budget is not generous.
pub const PROPFIND_BODY: &str = concat!(
    r#"<?xml version="1.0" encoding="utf-8"?>"#,
    r#"<d:propfind xmlns:d="DAV:"><d:prop><d:getcontentlength/></d:prop></d:propfind>"#
);

/// The content length a `PROPFIND` response reports, if it reports one.
#[must_use]
pub fn content_length(body: &str) -> Option<u64> {
    let start = body.find("getcontentlength")?;
    let rest = &body[start..];
    let open = rest.find('>')? + 1;
    let close = rest[open..].find('<')? + open;
    rest[open..close].trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{collection, content_length, file_url};

    #[test]
    fn a_package_gets_its_own_collection() {
        assert_eq!(
            collection("https://cloud.example/dav/files/me/Downloads", "My Release"),
            "https://cloud.example/dav/files/me/Downloads/My%20Release"
        );
        // A trailing slash on the configured address changes nothing.
        assert_eq!(
            collection("https://cloud.example/dav/", "Set"),
            "https://cloud.example/dav/Set"
        );
    }

    #[test]
    fn a_file_name_is_encoded_but_its_folders_survive() {
        assert_eq!(
            file_url("https://cloud.example/dav/Set", "sub dir/a&b.bin"),
            "https://cloud.example/dav/Set/sub%20dir/a%26b.bin"
        );
    }

    #[test]
    fn a_traversal_attempt_in_a_name_goes_nowhere() {
        // The host offers only files it listed, so this should never arrive — which is
        // exactly why it is worth being sure it would go nowhere if it did.
        assert_eq!(
            file_url("https://cloud.example/dav/Set", "../../etc/passwd"),
            "https://cloud.example/dav/Set/etc/passwd"
        );
    }

    #[test]
    fn the_length_is_read_out_of_a_propfind_answer() {
        let body = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:"><d:response>
            <d:href>/dav/Set/a.bin</d:href><d:propstat><d:prop>
            <d:getcontentlength>14471447</d:getcontentlength>
            </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
        assert_eq!(content_length(body), Some(14_471_447));
        assert_eq!(content_length("<d:multistatus/>"), None);
    }
}
