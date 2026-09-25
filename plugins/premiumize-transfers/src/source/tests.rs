use super::{address_key, container_key, magnet_key, normalise_info_hash};

const HEX: &str = "magnet:?xt=urn:btih:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709&dn=Example.Release";
const BASE32: &str = "magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ&dn=Other.Name";
const KEY: &str = "btih:da39a3ee5e6b4b0d3255bfef95601890afd80709";
const TORRENT: &[u8] = b"d8:announce23:http://tracker.invalid/4:infod6:lengthi31e4:name15:Example.Release12:piece lengthi16384e6:pieces20:01234567890123456789ee";

/// One torrent, two spellings, one key -- and the display name does not enter into it.
#[test]
fn both_spellings_of_one_info_hash_are_one_key() {
    assert_eq!(magnet_key(HEX).as_deref(), Some(KEY));
    assert_eq!(magnet_key(BASE32).as_deref(), Some(KEY));
    assert_eq!(
        normalise_info_hash("3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ").as_deref(),
        Some("da39a3ee5e6b4b0d3255bfef95601890afd80709")
    );
}

/// A magnet naming something other than a BitTorrent hash is still a magnet Premiumize may
/// take, so it gets a key of its own rather than being refused.
#[test]
fn a_magnet_without_an_info_hash_is_keyed_by_the_address() {
    let key =
        magnet_key("magnet:?xt=urn:sha1:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709").expect("a key");
    assert!(key.starts_with("magnet:"), "{key}");
    assert_eq!(key.len(), "magnet:".len() + 40);
    assert_eq!(magnet_key("https://example.invalid/file.bin"), None);
}

#[test]
fn a_plain_address_is_keyed_by_itself_and_only_over_http() {
    let key = address_key("https://example.invalid/some/file.bin").expect("a key");
    assert!(key.starts_with("url:"), "{key}");
    assert_eq!(
        address_key("  https://example.invalid/some/file.bin  ").as_deref(),
        Some(key.as_str()),
        "surrounding space is not part of an address"
    );
    assert_ne!(
        address_key("https://example.invalid/other.bin").as_deref(),
        Some(key.as_str())
    );
    for refused in ["ftp://example.invalid/x", "file:///etc/passwd", "https://"] {
        assert_eq!(address_key(refused), None, "{refused}");
    }
}

/// The same bytes are the same key; different bytes are not; and a container this plugin
/// cannot name an upload for gets no key at all.
#[test]
fn a_container_is_keyed_by_its_bytes() {
    let key = container_key(TORRENT).expect("a key");
    assert!(key.starts_with("file:"), "{key}");
    assert_eq!(container_key(TORRENT).as_deref(), Some(key.as_str()));
    let mut other = TORRENT.to_vec();
    other[20] = b'x';
    assert_ne!(container_key(&other).as_deref(), Some(key.as_str()));
    assert_eq!(container_key(&[0x00, 0xff, 0x10, 0x9a][..]), None);
}

/// The cost of keying a container by its bytes, stated as a test so it cannot be forgotten:
/// the same torrent as a file and as a magnet are two keys and therefore two transfers.
#[test]
fn a_torrent_file_and_its_magnet_are_deliberately_not_one_key() {
    assert_ne!(
        container_key(TORRENT).as_deref(),
        magnet_key(HEX).as_deref()
    );
}
