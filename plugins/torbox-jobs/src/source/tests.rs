//! What each source shape is, and what it is known by.

use super::{
    Handed, Kind, address_digest, cache_digest, container_info_hash, identify, is_nzb,
    magnet_info_hash, normalise_info_hash, split_key,
};

/// SHA-1 of nothing, which is the info hash every fixture in the contract test carries.
const EMPTY_SHA1: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";

fn torrent_bytes() -> Vec<u8> {
    b"d8:announce23:http://tracker.invalid/4:infod6:lengthi31e4:name15:Example.Release12:piece lengthi16384e6:pieces20:01234567890123456789ee".to_vec()
}

#[test]
fn a_magnet_is_a_torrent_job_keyed_by_its_info_hash() {
    let magnet = format!(
        "magnet:?xt=urn:btih:{}&dn=Example.Release",
        EMPTY_SHA1.to_uppercase()
    );
    let (kind, key) = identify(Handed::Magnet(&magnet)).expect("a magnet is claimed");
    assert_eq!(kind, Kind::Torrent);
    assert_eq!(key, format!("torrent:{EMPTY_SHA1}"));
}

/// The two spellings of one info hash are one key, which is what makes a magnet copied from
/// two sites one job and not two.
#[test]
fn hex_and_base32_are_the_same_key() {
    let base32 = "magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ&dn=Example.Release";
    let (_, key) = identify(Handed::Magnet(base32)).expect("a base32 magnet is claimed");
    assert_eq!(key, format!("torrent:{EMPTY_SHA1}"));
    assert_eq!(normalise_info_hash(EMPTY_SHA1).as_deref(), Some(EMPTY_SHA1));
    // A magnet that names something other than a BitTorrent info hash is nobody's.
    assert_eq!(magnet_info_hash("magnet:?xt=urn:sha1:ABC"), None);
    assert_eq!(magnet_info_hash("https://example.invalid/x"), None);
}

#[test]
fn a_bencoded_container_is_a_torrent_job_keyed_by_its_info_dictionary() {
    let bytes = torrent_bytes();
    let (kind, key) = identify(Handed::Container(&bytes)).expect("a torrent file is claimed");
    assert_eq!(kind, Kind::Torrent);
    let digest = key.strip_prefix("torrent:").expect("the kind is in front");
    assert_eq!(digest.len(), 40);
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    // Derived, not remembered: the same bytes answer the same way every time.
    assert_eq!(container_info_hash(&bytes).as_deref(), Some(digest));
}

#[test]
fn an_nzb_container_is_a_usenet_job_keyed_by_its_bytes() {
    let nzb = br#"<?xml version="1.0" encoding="iso-8859-1" ?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <file poster="nobody@example.invalid" date="1" subject="Example.Release (1/1)">
    <groups><group>alt.binaries.test</group></groups>
    <segments><segment bytes="10" number="1">part1@example</segment></segments>
  </file>
</nzb>
"#;
    assert!(is_nzb(nzb));
    let (kind, key) = identify(Handed::Container(nzb)).expect("an NZB is claimed");
    assert_eq!(kind, Kind::Usenet);
    let digest = key.strip_prefix("usenet:").expect("the kind is in front");
    assert_eq!(
        digest.len(),
        32,
        "an MD5, which is what TorBox keys an NZB by"
    );
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    // One byte different is one job different.
    let mut altered = nzb.to_vec();
    altered.push(b' ');
    let (_, other) = identify(Handed::Container(&altered)).expect("still an NZB");
    assert_ne!(other, key);
}

#[test]
fn an_address_is_a_web_job_keyed_by_the_address_it_submits() {
    let (kind, key) =
        identify(Handed::Address("https://example.invalid/file.bin")).expect("claimed");
    assert_eq!(kind, Kind::Web);
    let digest = key.strip_prefix("web:").expect("the kind is in front");
    assert_eq!(digest.len(), 32);
    // The address is hashed exactly as submitted: a normalisation here would key an address
    // nobody sent, and the adoption the key exists for would never match.
    assert_ne!(
        address_digest("https://example.invalid/file.bin"),
        address_digest("https://example.invalid/File.bin")
    );
}

/// Nothing outside TorBox's three shapes is claimed, and nothing the host refuses gets a
/// second chance here.
#[test]
fn what_is_not_a_torbox_job_is_not_claimed() {
    assert!(identify(Handed::Magnet("https://example.invalid/x")).is_none());
    assert!(identify(Handed::Container(b"not a container at all")).is_none());
    assert!(identify(Handed::Container(b"")).is_none());
    for refused in [
        "file:///etc/passwd",
        "data:text/plain,hello",
        "ftp://example.invalid/x",
        "https://example.invalid/a b",
        "",
    ] {
        assert!(
            identify(Handed::Address(refused)).is_none(),
            "{refused} must not be claimed"
        );
    }
    // Bounded, so a megabyte pasted into the address box is refused rather than submitted.
    let long = format!(
        "https://example.invalid/{}",
        "a".repeat(super::MAX_ADDRESS_BYTES)
    );
    assert!(identify(Handed::Address(&long)).is_none());
    let huge = vec![b'd'; super::MAX_CONTAINER_BYTES + 1];
    assert!(identify(Handed::Container(&huge)).is_none());
}

/// A bencoded container from a stranger is bounded before it is read.
#[test]
fn a_hostile_container_is_refused_rather_than_followed() {
    let nested: Vec<u8> = std::iter::repeat_n(b'l', 4096).collect();
    assert_eq!(container_info_hash(&nested), None);
    // A length header claiming more than the file holds.
    assert_eq!(container_info_hash(b"d9999999999:info"), None);
}

/// The key is read back out of the host's own row, so it is checked rather than split.
#[test]
fn a_content_key_is_checked_on_the_way_back_in() {
    assert_eq!(
        split_key(&format!("torrent:{EMPTY_SHA1}")),
        Some((Kind::Torrent, EMPTY_SHA1))
    );
    assert_eq!(
        split_key("usenet:0123456789abcdef0123456789abcdef"),
        Some((Kind::Usenet, "0123456789abcdef0123456789abcdef"))
    );
    for refused in [
        "torrent:../../etc/passwd",
        "torrent:",
        "picture:0123456789abcdef0123456789abcdef",
        EMPTY_SHA1,
        "web:zzzz",
    ] {
        assert_eq!(split_key(refused), None, "{refused}");
    }
}

/// RD-130-11: the cache is asked by the kind the host decided, with the digest `identify`
/// derives for the same content, and a kind that cannot be asked about a shape is not asked.
#[test]
fn the_cache_digest_follows_the_kind_and_matches_the_content_key() {
    let magnet = format!("magnet:?xt=urn:btih:{EMPTY_SHA1}");
    assert_eq!(
        cache_digest(Kind::Torrent, Handed::Magnet(&magnet)).as_deref(),
        Some(EMPTY_SHA1)
    );
    // Base32 and hex name one hash, so they ask the cache about one thing.
    assert_eq!(
        cache_digest(
            Kind::Torrent,
            Handed::Magnet("magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ")
        ),
        cache_digest(Kind::Torrent, Handed::Magnet(&magnet))
    );
    let torrent = torrent_bytes();
    assert_eq!(
        cache_digest(Kind::Torrent, Handed::Container(&torrent)),
        container_info_hash(&torrent)
    );

    // An NZB link and a hoster link are both addresses; both are keyed as TorBox keys a link.
    let link = "https://indexer.invalid/api?t=get&id=1";
    assert_eq!(
        cache_digest(Kind::Usenet, Handed::Address(link)),
        address_digest(link)
    );
    assert_eq!(
        cache_digest(Kind::Web, Handed::Address(link)),
        address_digest(link)
    );
    let nzb = b"<?xml version=\"1.0\"?><nzb></nzb>";
    let (_, key) = identify(Handed::Container(nzb)).expect("an NZB");
    assert_eq!(
        cache_digest(Kind::Usenet, Handed::Container(nzb)).as_deref(),
        key.strip_prefix("usenet:")
    );

    // Shapes a kind cannot be asked about.
    assert_eq!(cache_digest(Kind::Web, Handed::Magnet(&magnet)), None);
    assert_eq!(cache_digest(Kind::Usenet, Handed::Magnet(&magnet)), None);
    assert_eq!(cache_digest(Kind::Torrent, Handed::Address(link)), None);
    assert_eq!(cache_digest(Kind::Web, Handed::Container(&torrent)), None);
    assert_eq!(
        cache_digest(Kind::Usenet, Handed::Container(&torrent)),
        None
    );
    assert_eq!(cache_digest(Kind::Torrent, Handed::Container(nzb)), None);
    assert_eq!(
        cache_digest(Kind::Web, Handed::Address("file:///etc/passwd")),
        None
    );
}
