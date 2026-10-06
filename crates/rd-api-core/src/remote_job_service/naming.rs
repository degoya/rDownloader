//! The name a remote job is listed under (RD-1120-02).
//!
//! A container arrives with its file name; a magnet and a plain address carry theirs inside
//! themselves -- the magnet's `dn`, the address's last path segment. Without one the list of
//! remote jobs could show only the content key, an info hash or a digest, and nobody could
//! tell which torrent or NZB a row was. Every name goes through [`source_name`], whichever
//! of the three it came from.

use rd_plugin_host::extension::RemoteJobSource;

/// Longest source name kept, in characters.
const MAX_SOURCE_NAME: usize = 255;

/// The name a source was added under, reduced to a label: the last path segment, without
/// control characters, trimmed and cut. `None` when nothing is left.
///
/// A label and never a path: it names a LinkGrabber package, and the package name goes through
/// the same file-name rules as every other one before anything is written to disk.
#[must_use]
pub fn source_name(raw: Option<&str>) -> Option<String> {
    let last = raw?.rsplit(['/', '\\']).next().unwrap_or_default();
    let name: String = last
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_SOURCE_NAME)
        .collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// The name a magnet or an address carries in itself, as a label: a magnet's `dn`, an
/// address's last non-empty path segment, both decoded. `None` for a container, whose name
/// comes with the request, and for a source that names nothing.
pub(super) fn implied_name(source: &RemoteJobSource) -> Option<String> {
    let raw = match source {
        RemoteJobSource::Magnet(magnet) => display_name(magnet),
        RemoteJobSource::Address(address) => last_segment(address),
        RemoteJobSource::Container(_) => None,
    };
    source_name(raw.as_deref())
}

/// A magnet's `dn` (display name), with its `+` and percent escapes decoded.
fn display_name(magnet: &str) -> Option<String> {
    let parsed = url::Url::parse(magnet).ok()?;
    parsed
        .query_pairs()
        .find(|(key, _)| key == "dn")
        .map(|(_, value)| value.into_owned())
}

/// The last non-empty path segment of an address, percent-decoded.
fn last_segment(address: &str) -> Option<String> {
    let parsed = url::Url::parse(address).ok()?;
    let segment = parsed
        .path_segments()?
        .rev()
        .find(|segment| !segment.is_empty())?;
    Some(
        percent_encoding::percent_decode_str(segment)
            .decode_utf8_lossy()
            .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use rd_plugin_host::extension::RemoteJobSource;

    use super::{MAX_SOURCE_NAME, implied_name, source_name};

    /// The name a container was added under is a label for its package, never a path.
    #[test]
    fn a_source_name_is_the_last_segment_without_control_characters() {
        assert_eq!(
            source_name(Some("Show.S01.nzb")).as_deref(),
            Some("Show.S01.nzb")
        );
        assert_eq!(
            source_name(Some("C:\\Users\\me\\Show.S01.nzb")).as_deref(),
            Some("Show.S01.nzb")
        );
        assert_eq!(
            source_name(Some("../../etc/Show\u{0}\nS01.nzb ")).as_deref(),
            Some("ShowS01.nzb")
        );
        assert_eq!(source_name(Some("  ")), None);
        assert_eq!(source_name(Some("folder/")), None);
        assert_eq!(source_name(None), None);
        let long = "a".repeat(MAX_SOURCE_NAME + 40);
        assert_eq!(
            source_name(Some(long.as_str())).map(|name| name.chars().count()),
            Some(MAX_SOURCE_NAME)
        );
    }

    fn magnet(address: &str) -> Option<String> {
        implied_name(&RemoteJobSource::Magnet(address.to_owned()))
    }

    fn address(address: &str) -> Option<String> {
        implied_name(&RemoteJobSource::Address(address.to_owned()))
    }

    /// A magnet is listed under its `dn`, decoded the way a browser encodes it; one without a
    /// `dn` has no name and the list falls back to the content key.
    #[test]
    fn a_magnet_is_named_by_its_display_name() {
        assert_eq!(
            magnet("magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709&dn=Some.Show.S01")
                .as_deref(),
            Some("Some.Show.S01")
        );
        assert_eq!(
            magnet("magnet:?dn=Some+Show%20S01%C3%A4&xt=urn:btih:da39a3ee").as_deref(),
            Some("Some Show S01\u{e4}")
        );
        // The same reduction as a container's name: no path, no control characters.
        assert_eq!(
            magnet("magnet:?xt=urn:btih:da39a3ee&dn=..%2Fetc%2FShow%0A.S01").as_deref(),
            Some("Show.S01")
        );
        assert_eq!(magnet("magnet:?xt=urn:btih:da39a3ee"), None);
        assert_eq!(magnet("magnet:?xt=urn:btih:da39a3ee&dn=%20"), None);
        assert_eq!(magnet("not a magnet"), None);
    }

    /// An address is listed under its last path segment, decoded; an address with no path
    /// names nothing.
    #[test]
    fn an_address_is_named_by_its_last_path_segment() {
        assert_eq!(
            address("https://hoster.example/f/abc/Some.Show.S01.rar").as_deref(),
            Some("Some.Show.S01.rar")
        );
        assert_eq!(
            address("https://hoster.example/dir/Some%20Show.zip?token=1#part").as_deref(),
            Some("Some Show.zip")
        );
        assert_eq!(
            address("https://hoster.example/folder/").as_deref(),
            Some("folder")
        );
        assert_eq!(
            address("https://hoster.example/a/..%2F..%2Fetc%2Fpasswd").as_deref(),
            Some("passwd")
        );
        assert_eq!(address("https://hoster.example/"), None);
        assert_eq!(address("https://hoster.example"), None);
    }

    /// A container's name is what the request carried; the bytes are not read for one.
    #[test]
    fn a_container_implies_no_name() {
        assert_eq!(
            implied_name(&RemoteJobSource::Container(b"d4:infoe".to_vec())),
            None
        );
    }
}
