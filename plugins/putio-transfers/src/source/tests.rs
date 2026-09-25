//! The key a source is known by, and the magnet a container becomes.

use super::{
    add_body, container_info_hash, container_magnet, info_hash_within, magnet_info_hash,
    normalise_info_hash, read_container,
};

/// A bencoded byte string, built rather than counted out by hand: the lengths in a `.torrent`
/// are the thing this module reads, and a fixture with a miscounted one would be testing the
/// parser against a file no client would ever produce.
fn bencode(value: &str) -> String {
    format!("{}:{value}", value.len())
}

/// A minimal but real torrent: one `announce`, an `announce-list` of two tiers, and an `info`
/// dictionary. `announce-list` repeats the `announce` entry, exactly as a real one does.
fn container() -> Vec<u8> {
    torrent("Example.Release")
}

/// The same, with a name of the caller's choosing.
fn torrent(name: &str) -> Vec<u8> {
    let first = bencode("http://tracker.invalid/annce");
    let second = bencode("udp://tracker2.invalid/annce");
    let mut out = String::from("d");
    out.push_str(&bencode("announce"));
    out.push_str(&first);
    out.push_str(&bencode("announce-list"));
    out.push_str(&format!("ll{first}el{second}ee"));
    out.push_str(&bencode("info"));
    out.push('d');
    out.push_str(&bencode("length"));
    out.push_str("i31e");
    out.push_str(&bencode("name"));
    out.push_str(&bencode(name));
    out.push_str(&bencode("piece length"));
    out.push_str("i16384e");
    out.push_str(&bencode("pieces"));
    out.push_str(&bencode("01234567890123456789"));
    out.push_str("ee");
    out.into_bytes()
}

/// The same twenty bytes in the two spellings sites actually use. One key, not two, is what
/// makes a magnet copied from two places one remote job.
#[test]
fn both_spellings_of_one_info_hash_are_one_key() {
    let hex = "magnet:?xt=urn:btih:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709&dn=Example";
    let base32 = "magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ&dn=Example";
    let expected = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
    assert_eq!(magnet_info_hash(hex).as_deref(), Some(expected));
    assert_eq!(magnet_info_hash(base32).as_deref(), Some(expected));
    // The percent-encoded spelling a browser produces, and an upper-case scheme.
    assert_eq!(
        magnet_info_hash("magnet:?xt=urn%3Abtih%3ADA39A3EE5E6B4B0D3255BFEF95601890AFD80709")
            .as_deref(),
        Some(expected)
    );
}

#[test]
fn a_magnet_that_names_no_torrent_has_no_key() {
    for address in [
        // Legal in a magnet, and not a BitTorrent info hash.
        "magnet:?xt=urn:sha1:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709",
        "magnet:?xt=urn:ed2k:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709",
        // Right prefix, wrong length.
        "magnet:?xt=urn:btih:DA39A3EE",
        "magnet:?dn=Example",
        "https://example.invalid/file.torrent",
        "",
    ] {
        assert_eq!(magnet_info_hash(address), None, "{address}");
    }
    assert_eq!(normalise_info_hash("not a hash"), None);
}

/// The key of a container is the key of the matching magnet: the same twenty bytes, arrived at
/// the long way. That is what lets one restart adopt the job the other shape created.
#[test]
fn a_container_is_keyed_by_the_same_number_as_its_magnet() {
    let key = container_info_hash(&container()).expect("an info hash");
    assert_eq!(key.len(), 40);
    assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        magnet_info_hash(&container_magnet(&container()).expect("a magnet")).as_deref(),
        Some(key.as_str())
    );
    // And it is the same number every time; an unstable key would defeat the guard it exists
    // for.
    assert_eq!(
        container_info_hash(&container()).as_deref(),
        Some(key.as_str())
    );
}

/// What `submit` actually sends for a `.torrent`: the torrent's own name and every tracker it
/// listed, in both `announce` and `announce-list`, each once.
#[test]
fn a_container_becomes_a_magnet_carrying_its_name_and_trackers() {
    let read = read_container(&container()).expect("a container");
    assert_eq!(read.name.as_deref(), Some("Example.Release"));
    assert_eq!(
        read.trackers,
        vec![
            "http://tracker.invalid/annce".to_owned(),
            "udp://tracker2.invalid/annce".to_owned(),
        ],
        "announce-list repeats the announce entry, and a repeat is not a second tracker"
    );
    let magnet = container_magnet(&container()).expect("a magnet");
    assert!(magnet.contains("&dn=Example.Release"), "{magnet}");
    assert!(
        magnet.contains("&tr=http%3A%2F%2Ftracker.invalid%2Fannce"),
        "{magnet}"
    );
    assert!(
        magnet.contains("&tr=udp%3A%2F%2Ftracker2.invalid%2Fannce"),
        "{magnet}"
    );
}

/// A name is attacker-supplied text going into a query. Without encoding it, a torrent called
/// `x&tr=udp://mine.invalid` would add a tracker of its author's choosing to the address Put.io
/// is handed.
#[test]
fn a_hostile_torrent_name_cannot_add_anything_to_the_magnet() {
    let magnet = container_magnet(&torrent("x&tr=udp://mine.invalid/a")).expect("a magnet");
    assert!(!magnet.contains("mine.invalid/a"), "{magnet}");
    assert!(magnet.contains("&dn=x%26tr%3Dudp"), "{magnet}");
    // Two trackers, both of them the torrent's own announce entries and neither the name's.
    assert_eq!(magnet.matches("&tr=").count(), 2, "{magnet}");
}

/// A tracker entry that is not an address is dropped rather than repeated into the magnet.
#[test]
fn only_tracker_shaped_entries_are_carried_over() {
    let mut odd = String::from("d");
    odd.push_str(&bencode("announce"));
    odd.push_str(&bencode("javascript:alert(1)"));
    odd.push_str(&bencode("info"));
    odd.push('d');
    odd.push_str(&bencode("length"));
    odd.push_str("i1e");
    odd.push_str(&bencode("name"));
    odd.push_str(&bencode("a"));
    odd.push_str("ee");
    let odd = odd.into_bytes();
    let read = read_container(&odd).expect("a container");
    assert!(read.trackers.is_empty(), "{:?}", read.trackers);
}

#[test]
fn bytes_that_are_not_a_container_are_not_one() {
    for bytes in [
        b"not bencoded at all".to_vec(),
        // A dictionary with no `info` key.
        b"d8:announce5:a://bee".to_vec(),
        Vec::new(),
        // Eleven characters that would be a stack overflow without the depth bound.
        b"llllllllllllllllllllllllllllllllllllllllllll".to_vec(),
        // A length header claiming more than the file holds.
        b"d4:infod6:lengthi1e4:name999:xee".to_vec(),
    ] {
        assert_eq!(container_info_hash(&bytes), None);
        assert_eq!(container_magnet(&bytes), None);
    }
}

/// What `adopt` asks of the three different shapes Put.io states one fact in.
#[test]
fn an_info_hash_is_found_in_whatever_shape_put_io_states_it() {
    let expected = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
    // The `hash` field, bare.
    assert_eq!(
        info_hash_within("DA39A3EE5E6B4B0D3255BFEF95601890AFD80709").as_deref(),
        Some(expected)
    );
    // The `magneturi` field.
    assert_eq!(
        info_hash_within("magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709&dn=x")
            .as_deref(),
        Some(expected)
    );
    // The `source` field, base32 as some sites write it.
    assert_eq!(
        info_hash_within("magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ").as_deref(),
        Some(expected)
    );
    // Somebody else's transfer, and a transfer that is not a torrent at all.
    assert_eq!(
        info_hash_within("magnet:?xt=urn:btih:ffffffffffffffffffffffffffffffffffffffff").as_deref(),
        Some("ffffffffffffffffffffffffffffffffffffffff")
    );
    assert_eq!(info_hash_within("https://example.invalid/file.bin"), None);
    assert_eq!(info_hash_within(""), None);
}

#[test]
fn the_submit_body_is_one_encoded_field() {
    let body = String::from_utf8(add_body("magnet:?xt=urn:btih:da39a3ee&dn=x")).expect("utf-8");
    assert_eq!(body, "url=magnet%3A%3Fxt%3Durn%3Abtih%3Ada39a3ee%26dn%3Dx");
}
