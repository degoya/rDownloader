use super::{candidate_url, split_candidate_url};

fn stored(address: &str) -> String {
    candidate_url(&address.parse().expect("an address")).into()
}

/// **A provider whose key is a link component keeps it -- in the vault** (RD-110-38).
///
/// This test used to state the opposite, as a recorded collision: MEGA encrypts on the
/// client and puts the file key in the fragment, RD-109-32 drops every fragment because
/// nothing distinguishes a password from an anchor, and ADR 0011 was accepted on "the key
/// travels in the link fragment the person already has". Neither could be bent on the way
/// past, so the address that reached a row was one nothing could decrypt.
///
/// The resolution keeps both promises: the stored address is shortened exactly as it was,
/// and the fragment comes back out of this call as something to put in the vault under a
/// reference. The row still never sees it.
#[test]
fn a_link_whose_key_is_its_fragment_keeps_it_out_of_the_row_and_in_the_vault() {
    let file = "https://mega.nz/file/yuZ0QJ6J#jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc"
        .parse()
        .expect("an address");
    let (stored, secret) = split_candidate_url(&file, true);
    assert_eq!(String::from(stored), "https://mega.nz/file/yuZ0QJ6J");
    assert_eq!(
        secret.as_deref(),
        Some("jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc")
    );

    // A folder address, and a file named inside one: the fragment carries the share key
    // and, for the child form, the path to the node as well. Both are kept whole.
    let folder = "https://mega.nz/folder/e4diDZ7T#iJnegBO_m6OXBQp27lHCrg"
        .parse()
        .expect("an address");
    let (stored, secret) = split_candidate_url(&folder, true);
    assert_eq!(String::from(stored), "https://mega.nz/folder/e4diDZ7T");
    assert_eq!(secret.as_deref(), Some("iJnegBO_m6OXBQp27lHCrg"));

    let child = "https://mega.nz/folder/e4diDZ7T#iJnegBO_m6OXBQp27lHCrg/file/KlVgwR4B"
        .parse()
        .expect("an address");
    let (stored, secret) = split_candidate_url(&child, true);
    assert_eq!(String::from(stored), "https://mega.nz/folder/e4diDZ7T");
    assert_eq!(
        secret.as_deref(),
        Some("iJnegBO_m6OXBQp27lHCrg/file/KlVgwR4B")
    );
}

/// Without the declaration nothing moves: every other address behaves exactly as it did
/// under RD-109-32, and no fragment is handed out to be stored anywhere.
#[test]
fn a_link_no_provider_declared_still_loses_its_fragment_outright() {
    for address in [
        "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4#s3cret",
        "https://mega.nz/file/yuZ0QJ6J#jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc",
    ] {
        let url = address.parse().expect("an address");
        let (shortened, secret) = split_candidate_url(&url, false);
        assert_eq!(String::from(shortened), stored(address));
        assert_eq!(secret, None, "{address} must hand out no secret");
    }
}

/// An empty fragment is nothing to keep, however loudly a provider declares itself: the
/// vault refuses empty material, and a reference pointing at nothing is worse than none.
#[test]
fn an_empty_fragment_is_never_vaulted() {
    let url = "https://mega.nz/file/yuZ0QJ6J#"
        .parse()
        .expect("an address");
    let (shortened, secret) = split_candidate_url(&url, true);
    assert_eq!(String::from(shortened), "https://mega.nz/file/yuZ0QJ6J");
    assert_eq!(secret, None);
}

/// The password RD-108-07 vaults for a claimed share is the fragment of the same address;
/// for an unclaimed one it is dropped instead, and nothing else about the address moves.
#[test]
fn a_stored_address_never_carries_a_fragment() {
    assert_eq!(
        stored("https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4#s3cret"),
        "https://cloud.exmaple.org/s/QxT7bK2mNp9wZr4"
    );
    // An empty fragment is a fragment: `…/a#` must not become a second, distinct row.
    assert_eq!(stored("https://example.com/a#"), "https://example.com/a");
    // Query, userinfo, port and path are untouched -- this is not a normalizer.
    assert_eq!(
        stored("https://example.com:8443/a/b?t=1&u=2#frag"),
        "https://example.com:8443/a/b?t=1&u=2"
    );
    // An address with nothing to drop comes back byte for byte.
    for address in [
        "https://example.com/a/b?t=1",
        "ftp://user@files.example.com/pub/x.iso",
        "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
    ] {
        assert_eq!(stored(address), address);
    }
}
