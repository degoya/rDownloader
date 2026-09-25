use super::{container_info_hash, magnet_info_hash, normalise_info_hash};

/// The `info` value of the fixture below, byte for byte. Its SHA-1 is the info hash.
const INFO: &[u8] =
    b"d6:lengthi1024e4:name8:file.bin12:piece lengthi16384e6:pieces20:01234567890123456789e";

/// A minimal but well-formed `.torrent`: one tracker and the info dictionary.
fn torrent() -> Vec<u8> {
    let mut bytes = b"d8:announce19:http://tracker.test4:info".to_vec();
    bytes.extend_from_slice(INFO);
    bytes.push(b'e');
    bytes
}

/// Independently computed with `sha1sum` over `INFO`.
const HASH: &str = "0893169844df9c7205bdc113070c2ef05c3a0e10";

/// Base32 spelling of the same twenty bytes, as half the magnet links in the field carry it.
const HASH_BASE32: &str = "BCJRNGCE36OHEBN5YEJQODBO6BODUDQQ";

/// The key is what stops a second `addMagnet`, so the two spellings of one info hash have to
/// produce one key. A magnet copied from two sites would otherwise start two remote jobs, and
/// Real-Debrid would charge for both.
#[test]
fn both_spellings_of_one_info_hash_produce_one_key() {
    let hex = format!("magnet:?xt=urn:btih:{HASH}&dn=file.bin");
    let base32 = format!("magnet:?dn=file.bin&xt=urn:btih:{HASH_BASE32}");
    assert_eq!(magnet_info_hash(&hex).as_deref(), Some(HASH));
    assert_eq!(magnet_info_hash(&base32).as_deref(), Some(HASH));
    // Upper-case hex is the same torrent as lower-case hex, and so is an upper-case scheme.
    assert_eq!(
        magnet_info_hash(&format!("MAGNET:?xt=urn:btih:{}", HASH.to_uppercase())).as_deref(),
        Some(HASH)
    );
}

/// And a `.torrent` file of the same content produces that very same key, which is what lets
/// the file and the magnet find one remote job instead of two.
#[test]
fn a_container_produces_the_same_key_as_its_magnet() {
    assert_eq!(container_info_hash(&torrent()).as_deref(), Some(HASH));
    assert_eq!(
        container_info_hash(&torrent()),
        magnet_info_hash(&format!("magnet:?xt=urn:btih:{HASH}"))
    );
}

/// A magnet may legally name a topic that is not a BitTorrent info hash. Answering with one
/// anyway would submit something nobody asked for.
#[test]
fn a_magnet_without_a_bittorrent_topic_is_not_claimed() {
    assert_eq!(magnet_info_hash("magnet:?xt=urn:sha1:ABCDEF"), None);
    assert_eq!(magnet_info_hash("magnet:?dn=only-a-name"), None);
    assert_eq!(magnet_info_hash("https://example.com/x.torrent"), None);
    assert_eq!(magnet_info_hash("magnet:?xt=urn:btih:tooshort"), None);
}

/// A container is a stranger's file, so every malformed shape ends as `None` rather than as a
/// panic, an overflow or a walk off the end of the buffer.
#[test]
fn a_malformed_container_is_refused_rather_than_read() {
    assert_eq!(container_info_hash(b""), None);
    assert_eq!(container_info_hash(b"not bencode at all"), None);
    // A dictionary that never ends.
    assert_eq!(
        container_info_hash(b"d8:announce19:http://tracker.test"),
        None
    );
    // A byte string claiming more bytes than the file holds.
    assert_eq!(container_info_hash(b"d4:info999:short"), None);
    // No `info` key at all.
    assert_eq!(container_info_hash(b"d8:announce4:helle"), None);
    // Nesting deep enough to be a stack overflow if it were followed.
    let deep: Vec<u8> = b"d4:info"
        .iter()
        .copied()
        .chain(std::iter::repeat_n(b'l', 200))
        .collect();
    assert_eq!(container_info_hash(&deep), None);
}

/// The two accepted spellings, and nothing else. A 41-character string is not a hash with a
/// typo, it is something else entirely.
#[test]
fn only_a_real_info_hash_normalises() {
    assert_eq!(normalise_info_hash(HASH).as_deref(), Some(HASH));
    assert_eq!(normalise_info_hash(HASH_BASE32).as_deref(), Some(HASH));
    assert_eq!(normalise_info_hash(&format!("{HASH}0")), None);
    assert_eq!(normalise_info_hash("zzzz"), None);
    // Base32 is a fixed alphabet; `1`, `8`, `9` and `0` are not in it.
    assert_eq!(
        normalise_info_hash("11111111111111111111111111111111"),
        None
    );
}
