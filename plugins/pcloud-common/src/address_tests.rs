//! Reading pCloud addresses: hosts, regions, both spellings of a public link and the own drive.

use super::{
    Address, Region, file_address, parameter, parse, pcloud_host, public_file_address,
    region_is_certain, route_and_parameters, split, valid_code, valid_digest, valid_id, valid_name,
};

#[test]
fn only_pclouds_own_hosts_are_pclouds_and_each_names_a_region() {
    assert_eq!(
        pcloud_host("my.pcloud.com"),
        Some(("my.pcloud.com", Region::Us))
    );
    assert_eq!(
        pcloud_host("E.PCLOUD.LINK."),
        Some(("e.pcloud.link", Region::Eu))
    );
    assert_eq!(pcloud_host("pcloud.com.evil.test"), None);
    assert_eq!(pcloud_host("evil-pcloud.com"), None);
    assert_eq!(pcloud_host("api.pcloud.com"), None);
}

/// The two installations, and the one host that does not say which it is.
#[test]
fn a_region_is_a_fact_on_pclouds_regional_hosts_and_a_guess_on_the_shared_one() {
    assert!(region_is_certain("e.pcloud.link"));
    assert!(region_is_certain("u.pcloud.link"));
    assert!(region_is_certain("e.pcloud.com"));
    // pCloud documents the link it issues as `my.pcloud.com/#page=publink&code=…` for both
    // installations, so this host is where a lookup starts and never what it concludes.
    assert!(!region_is_certain("my.pcloud.com"));
    assert_eq!(Region::Us.other(), Region::Eu);
    assert_eq!(Region::Eu.other(), Region::Us);
    assert_eq!(Region::Us.both_from(), [Region::Us, Region::Eu]);
}

#[test]
fn an_authority_carrying_credentials_is_refused() {
    assert_eq!(
        split("https://x@my.pcloud.com/#/filemanager?folder=1"),
        None
    );
    assert_eq!(split("ftp://my.pcloud.com/#/filemanager?folder=1"), None);
    assert_eq!(
        split("https://my.pcloud.com:443/#/filemanager?folder=1"),
        Some(("my.pcloud.com", "#/filemanager?folder=1"))
    );
}

/// The fragment is where pCloud keeps the address, so it is read rather than dropped.
#[test]
fn the_route_and_the_parameters_come_out_of_the_fragment() {
    assert_eq!(
        route_and_parameters("#/filemanager?folder=1&fileid=2"),
        ("/filemanager", "folder=1&fileid=2")
    );
    assert_eq!(
        route_and_parameters("#page=publink&code=XZabc"),
        ("", "page=publink&code=XZabc")
    );
    assert_eq!(
        route_and_parameters("publink/show?code=XZabc"),
        ("publink/show", "code=XZabc")
    );
    assert_eq!(parameter("folder=1&fileid=2", "fileid"), Some("2"));
    assert_eq!(parameter("folder=1", "fileid"), None);
}

#[test]
fn identifiers_that_are_not_ones_are_refused() {
    assert!(valid_code("XZabc-1_2"));
    assert!(!valid_code("a/b"));
    assert!(!valid_code(""));
    assert!(!valid_code(&"x".repeat(129)));
    assert_eq!(valid_id("123"), Some(123));
    assert_eq!(valid_id("0"), Some(0));
    assert_eq!(valid_id("12a"), None);
    assert_eq!(valid_id("-1"), None);
    assert_eq!(valid_id(""), None);
    assert_eq!(valid_id(&"9".repeat(21)), None);
    assert!(valid_name("Season 1"));
    assert!(!valid_name(".."));
    assert!(!valid_name("a\u{0}b"));
    assert!(valid_digest(&"a".repeat(64), 64));
    assert!(!valid_digest(&"a".repeat(63), 64));
    assert!(!valid_digest("not a digest at all, of any length!!", 35));
}

/// Every spelling pCloud itself uses for a public link, read to the same code.
#[test]
fn every_spelling_of_a_public_link_is_read() {
    // The address `getfilepublink` documents, which names no region.
    assert_eq!(
        parse("https://my.pcloud.com/#page=publink&code=XZabc"),
        Some(Address::Public {
            code: "XZabc".to_owned(),
            file_id: None,
            region: Region::Us,
            region_certain: false,
        })
    );
    // The short spellings, which do.
    assert_eq!(
        parse("https://e.pcloud.link/publink/show?code=XZabc"),
        Some(Address::Public {
            code: "XZabc".to_owned(),
            file_id: None,
            region: Region::Eu,
            region_certain: true,
        })
    );
    assert_eq!(
        parse("https://u.pcloud.link/publink/show?code=XZabc&fileid=42"),
        Some(Address::Public {
            code: "XZabc".to_owned(),
            file_id: Some(42),
            region: Region::Us,
            region_certain: true,
        })
    );
    // A code that is not one is not a link.
    assert_eq!(
        parse("https://u.pcloud.link/publink/show?code=../etc/passwd"),
        None
    );
    assert_eq!(parse("https://u.pcloud.link/publink/show"), None);
}

#[test]
fn an_address_in_the_accounts_own_drive_is_read() {
    assert_eq!(
        parse("https://my.pcloud.com/#/filemanager?folder=12345"),
        Some(Address::Own {
            folder_id: 12345,
            file_id: None,
            region: Region::Us,
            region_certain: false,
        })
    );
    assert_eq!(
        parse("https://e.pcloud.com/#/filemanager?folder=0&fileid=7"),
        Some(Address::Own {
            folder_id: 0,
            file_id: Some(7),
            region: Region::Eu,
            region_certain: true,
        })
    );
    // A `fileid` that is not one does not quietly become the folder it sits in: the two
    // belong to different plugins.
    assert_eq!(
        parse("https://my.pcloud.com/#/filemanager?folder=1&fileid=x"),
        None
    );
    assert_eq!(parse("https://my.pcloud.com/#/filemanager"), None);
}

#[test]
fn an_address_belonging_to_somebody_else_is_never_read() {
    assert_eq!(
        parse("https://pcloud.com.evil.test/publink/show?code=X"),
        None
    );
    assert_eq!(parse("https://x@my.pcloud.com/#page=publink&code=X"), None);
    assert_eq!(parse("https://ddownload.com/f/abc"), None);
    assert_eq!(parse("https://my.pcloud.com/"), None);
    assert_eq!(parse("https://api.pcloud.com/getfilelink?fileid=1"), None);
}

/// The addresses the crawler hands the resolver are ones the resolver reads back to the
/// same file, in the same region.
#[test]
fn the_canonical_addresses_round_trip_and_keep_their_region() {
    let own = file_address(Region::Eu, 10, 20);
    assert_eq!(
        own,
        "https://e.pcloud.com/#/filemanager?folder=10&fileid=20"
    );
    assert_eq!(
        parse(&own),
        Some(Address::Own {
            folder_id: 10,
            file_id: Some(20),
            region: Region::Eu,
            region_certain: true,
        })
    );
    let folder = "https://my.pcloud.com/#/filemanager?folder=0";
    assert_eq!(parse(folder).and_then(|a| a.file_id()), None);

    let shared = public_file_address(Region::Eu, "XZabc", 42);
    assert_eq!(
        shared,
        "https://e.pcloud.link/publink/show?code=XZabc&fileid=42"
    );
    assert_eq!(
        parse(&shared),
        Some(Address::Public {
            code: "XZabc".to_owned(),
            file_id: Some(42),
            region: Region::Eu,
            region_certain: true,
        })
    );
}
