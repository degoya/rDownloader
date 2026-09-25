use super::{
    ADDRESS_PREFIX, MAGNET_PREFIX, MAX_ADDRESS_BYTES, address_key, key_of_original_link,
    magnet_key, normalise_address, normalise_info_hash,
};

const HASH: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
const MAGNET_HEX: &str =
    "magnet:?xt=urn:btih:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709&dn=Example.Release";
/// The same twenty bytes, spelled in base32 as some sites do.
const MAGNET_BASE32: &str =
    "magnet:?xt=urn:btih:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ&dn=Example.Release";

#[test]
fn one_info_hash_in_two_spellings_is_one_key() {
    // What makes a magnet copied from two sites one job and not two. Without it the duplicate
    // guard is a guard against pasting the same *characters* twice, which is not the question
    // anybody is asking.
    let expected = format!("{MAGNET_PREFIX}{HASH}");
    assert_eq!(magnet_key(MAGNET_HEX).as_deref(), Some(expected.as_str()));
    assert_eq!(
        magnet_key(MAGNET_BASE32).as_deref(),
        Some(expected.as_str())
    );
}

#[test]
fn the_key_of_one_source_is_the_same_every_time_it_is_asked() {
    // The property the whole duplicate guard rests on: a key that changed between two calls
    // would defeat the index it is written into.
    for source in [MAGNET_HEX, MAGNET_BASE32] {
        assert_eq!(magnet_key(source), magnet_key(source), "{source}");
    }
    for source in [
        "https://example.invalid/f/abc",
        "http://example.invalid/a%20b?x=1",
    ] {
        assert_eq!(address_key(source), address_key(source), "{source}");
    }
}

#[test]
fn a_magnet_that_names_something_other_than_a_torrent_is_not_ours() {
    // `urn:sha1:` and `urn:ed2k:` are legal in a magnet and mean something else.
    for foreign in [
        "magnet:?xt=urn:sha1:DA39A3EE5E6B4B0D3255BFEF95601890AFD80709",
        "magnet:?xt=urn:ed2k:31D6CFE0D16AE931B73C59D7E0C089C0",
        "magnet:?dn=Example.Release",
        "not a magnet at all",
    ] {
        assert_eq!(magnet_key(foreign), None, "{foreign}");
    }
}

#[test]
fn a_magnet_names_several_topics_and_the_torrent_one_is_found() {
    let several = format!("magnet:?xt.1=urn:sha1:abc&xt.2=urn:btih:{HASH}&dn=Example");
    assert_eq!(
        magnet_key(&several).as_deref(),
        Some(format!("{MAGNET_PREFIX}{HASH}").as_str())
    );
}

#[test]
fn both_written_forms_of_an_info_hash_normalise_to_lower_case_hex() {
    assert_eq!(
        normalise_info_hash("DA39A3EE5E6B4B0D3255BFEF95601890AFD80709").as_deref(),
        Some(HASH)
    );
    assert_eq!(
        normalise_info_hash("3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ").as_deref(),
        Some(HASH)
    );
    assert_eq!(normalise_info_hash("short"), None);
    assert_eq!(normalise_info_hash(&"z".repeat(40)), None);
}

#[test]
fn an_address_is_keyed_and_the_two_key_spaces_never_meet() {
    let key = address_key("https://example.invalid/f/abc").expect("a key");
    assert!(key.starts_with(ADDRESS_PREFIX), "{key}");
    assert!(!key.starts_with(MAGNET_PREFIX), "{key}");
    // The unique index is one space. Without the prefixes an address whose SHA-1 happened to
    // equal some magnet's info hash would be refused as a duplicate of a job it has nothing to
    // do with.
    let magnet = magnet_key(MAGNET_HEX).expect("a key");
    assert_ne!(key, magnet);
    assert_eq!(key.len() - ADDRESS_PREFIX.len(), 40);
}

#[test]
fn only_the_parts_that_never_reach_the_server_are_normalised_away() {
    // A fragment never leaves the client, so two addresses differing only in one are the same
    // download and must be the same job.
    assert_eq!(
        address_key("https://example.invalid/f/abc#part1"),
        address_key("https://example.invalid/f/abc")
    );
    // Scheme and host are case-insensitive by definition.
    assert_eq!(
        address_key("HTTPS://Example.Invalid/f/abc"),
        address_key("https://example.invalid/f/abc")
    );
    // Everything else is left alone: at some hosters these are different files, and merging
    // them would refuse the second as a duplicate of the first.
    assert_ne!(
        address_key("https://example.invalid/f/ABC"),
        address_key("https://example.invalid/f/abc")
    );
    assert_ne!(
        address_key("https://example.invalid/f/abc/"),
        address_key("https://example.invalid/f/abc")
    );
    assert_ne!(
        address_key("https://example.invalid/f/abc?x=1"),
        address_key("https://example.invalid/f/abc?x=2")
    );
}

#[test]
fn an_address_that_could_not_be_fetched_is_not_keyed_at_all() {
    for hostile in [
        "file:///etc/passwd",
        "data:text/plain,hello",
        "ftp://example.invalid/a",
        "https://",
        "https:///f/abc",
        "",
        "   ",
    ] {
        assert_eq!(address_key(hostile), None, "{hostile}");
    }
    // Bounded before it is hashed: a value past this is not an address.
    let huge = format!("https://example.invalid/{}", "a".repeat(MAX_ADDRESS_BYTES));
    assert_eq!(address_key(&huge), None);
}

#[test]
fn normalising_keeps_the_query_and_drops_the_fragment() {
    assert_eq!(
        normalise_address("HTTPS://Example.Invalid:443/A/b?x=1#frag").as_deref(),
        Some("https://example.invalid:443/A/b?x=1")
    );
}

#[test]
fn what_the_provider_recorded_is_keyed_exactly_as_a_fresh_source_would_be() {
    // The adoption check, in one line: the history's `originalLink` goes through the very
    // derivation `identify` used, so a magnet recorded in one spelling is recognised when it
    // was pasted in the other.
    assert_eq!(
        key_of_original_link(MAGNET_BASE32),
        magnet_key(MAGNET_HEX),
        "one job, whichever spelling was pasted"
    );
    assert_eq!(
        key_of_original_link("https://example.invalid/f/abc"),
        address_key("https://example.invalid/f/abc")
    );
    // A recorded link nothing can be derived from finds nothing rather than matching
    // everything.
    assert_eq!(key_of_original_link("ftp://example.invalid/a"), None);
    assert_eq!(key_of_original_link(""), None);
}
