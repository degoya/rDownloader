use super::{
    bencode, container_info_hash, container_magnet, container_name, info_hash_within, is_magnet,
    magnet_info_hash, normalise_info_hash, read_container, sha1_hex,
};

/// SHA-1 of nothing: not the info hash of the container below, simply the placeholder every
/// fixture of the remote-job plugins uses.
const HEX: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
/// The same twenty bytes, spelled in base32 as some sites do.
const BASE32: &str = "3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ";

/// One minimal bencoded torrent: an announce address and a two-key `info` dictionary.
fn container() -> Vec<u8> {
    b"d8:announce31:http://tracker.invalid/announce4:infod4:name7:Example6:lengthi31eee".to_vec()
}

#[test]
fn both_spellings_of_one_info_hash_are_one_number() {
    assert_eq!(
        magnet_info_hash(&format!("magnet:?xt=urn:btih:{}", HEX.to_uppercase())).as_deref(),
        Some(HEX)
    );
    assert_eq!(
        magnet_info_hash(&format!("magnet:?xt=urn:btih:{BASE32}&dn=Example")).as_deref(),
        Some(HEX)
    );
    assert_eq!(normalise_info_hash(BASE32).as_deref(), Some(HEX));
}

/// PLUG-07: four of six plugins stopped at the first pair without a `=` — an empty pair from
/// `&&` is one — and called a perfectly good magnet "not a torrent". Every such pair is now
/// skipped, wherever it sits.
#[test]
fn an_empty_or_bare_pair_does_not_end_the_magnet() {
    for magnet in [
        format!("magnet:?dn=Example&&xt=urn:btih:{HEX}"),
        format!("magnet:?&xt=urn:btih:{HEX}"),
        format!("magnet:?dn=Example&flag&xt=urn:btih:{HEX}&"),
        format!("magnet:?xt=urn:btih:{HEX}&&&&tr=udp%3A%2F%2Ft.invalid"),
    ] {
        assert_eq!(magnet_info_hash(&magnet).as_deref(), Some(HEX), "{magnet}");
    }
}

/// What people paste is not always tidy: surrounding whitespace, a shouted scheme, spaces
/// around a pair, an encoded `urn`.
#[test]
fn a_magnet_is_read_tolerantly() {
    for magnet in [
        format!("  magnet:?xt=urn:btih:{HEX}\n"),
        format!("MAGNET:?XT=URN:BTIH:{HEX}"),
        format!("Magnet:?dn=x& xt = urn:btih:{HEX} "),
        format!("magnet:?xt=urn%3Abtih%3A{HEX}"),
        format!("magnet:?xt.1=urn:sha1:{HEX}&xt.2=urn:btih:{BASE32}"),
    ] {
        assert_eq!(magnet_info_hash(&magnet).as_deref(), Some(HEX), "{magnet}");
    }
}

/// A magnet may name a topic that is not a BitTorrent info hash at all, and those mean
/// something else entirely: claiming one would submit a job nobody could run.
#[test]
fn a_magnet_that_names_something_else_is_not_a_torrent() {
    for foreign in [
        "magnet:?xt=urn:sha1:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709",
        "magnet:?xt=urn:ed2k:31d6cfe0d16ae931b73c59d7e0c089c0",
        "magnet:?dn=Example",
        "magnet:?xt=urn:btih:tooshort",
        "https://example.invalid/x.torrent",
        "",
    ] {
        assert_eq!(magnet_info_hash(foreign), None, "{foreign}");
    }
    assert!(is_magnet("magnet:?dn=Example"));
    assert!(is_magnet(" MAGNET:?xt=urn:ed2k:x"));
    assert!(!is_magnet("https://example.invalid/?magnet:?"));
    assert!(!is_magnet("magnet"));
}

/// The whole value of the key is that it is the same number everybody else has, so the bytes
/// of `info` are hashed exactly as they lay in the file rather than re-encoded.
#[test]
fn a_container_and_its_magnet_are_one_content_key() {
    let hash = container_info_hash(&container()).expect("an info hash");
    assert_eq!(hash, sha1_hex(b"d4:name7:Example6:lengthi31ee"));
    let magnet = container_magnet(&container()).expect("a magnet");
    assert_eq!(magnet_info_hash(&magnet).as_deref(), Some(hash.as_str()));
    assert!(magnet.contains("&dn=Example"), "{magnet}");
    assert!(
        magnet.contains("&tr=http%3A%2F%2Ftracker.invalid%2Fannounce"),
        "{magnet}"
    );
    let read = read_container(&container()).expect("a container");
    assert_eq!(read.name.as_deref(), Some("Example"));
    assert_eq!(read.trackers.len(), 1);
    assert_eq!(container_name(&container()).as_deref(), Some("Example"));
}

/// A name containing `&tr=` cannot add a tracker of its choosing to the magnet.
#[test]
fn a_hostile_name_stays_inside_its_field() {
    let torrent = b"d4:infod4:name16:x&tr=http://evil6:lengthi1eee";
    let magnet = container_magnet(torrent).expect("a magnet");
    assert_eq!(magnet.matches("&tr=").count(), 0, "{magnet}");
    assert!(
        magnet.contains("&dn=x%26tr%3Dhttp%3A%2F%2Fevil"),
        "{magnet}"
    );
}

/// Both tiers of `announce-list` are read, in order, without repeats, and what is not a
/// tracker address is dropped.
#[test]
fn every_tier_of_the_tracker_list_is_read_once() {
    let torrent = b"d8:announce15:udp://a.invalid13:announce-listll15:udp://a.invalidel16:http://b.invalid10:javascriptee4:infod4:name1:x6:lengthi1eee";
    let read = read_container(torrent).expect("a container");
    assert_eq!(read.trackers, vec!["udp://a.invalid", "http://b.invalid"]);
}

/// A container comes from a stranger, so every refusal below is a bound rather than a parse
/// error: nesting, a length header claiming more than the file holds, and bytes that are not
/// bencoded at all.
#[test]
fn bytes_that_are_not_a_torrent_are_refused_rather_than_read() {
    for bad in [
        &b""[..],
        b"not bencoded at all",
        b"d4:infod4:name99:tooshortee",
        b"d4:name1:xe",
        &b"l".repeat(200),
    ] {
        assert_eq!(container_info_hash(bad), None);
        assert_eq!(container_magnet(bad), None);
        assert_eq!(container_name(bad), None);
    }
    let mut huge = vec![b'd'; bencode::MAX_CONTAINER_BYTES + 1];
    huge.push(b'e');
    assert_eq!(container_info_hash(&huge), None);
}

#[test]
fn an_info_hash_is_found_wherever_a_provider_wrote_it() {
    assert_eq!(info_hash_within(HEX).as_deref(), Some(HEX));
    assert_eq!(
        info_hash_within(&format!("magnet:?xt=urn:btih:{HEX}&dn=Example")).as_deref(),
        Some(HEX)
    );
    assert_eq!(
        info_hash_within(&format!("urn:BTIH:{}", HEX.to_uppercase())).as_deref(),
        Some(HEX)
    );
    assert_eq!(info_hash_within("no hash here"), None);
}
