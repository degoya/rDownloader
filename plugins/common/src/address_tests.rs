//! The shared address readers, once for every plugin that used to carry a copy.

use super::{
    Parts, parameter, query_value, split, valid_hex, valid_label, valid_name, valid_token,
};

#[test]
fn split_keeps_host_and_path_and_refuses_credentials_and_other_schemes() {
    assert_eq!(
        split("https://www.example.test:443/s/abc?dl=0#frag"),
        Some(("www.example.test", "s/abc?dl=0"))
    );
    assert_eq!(split("http://example.test"), Some(("example.test", "")));
    assert_eq!(split("https://x@example.test/s/abc"), None);
    assert_eq!(split("ftp://example.test/s/abc"), None);
    assert_eq!(split("not an address"), None);
}

#[test]
fn a_parameter_is_read_undecoded_and_the_first_one_wins() {
    assert_eq!(
        query_value("s/abc?dl=0&rlkey=k%2D1", "rlkey"),
        Some("k%2D1")
    );
    assert_eq!(query_value("s/abc?dl=0&dl=1", "dl"), Some("0"));
    assert_eq!(query_value("s/abc", "dl"), None);
    assert_eq!(parameter("folder=1&fileid=2", "fileid"), Some("2"));
    assert_eq!(parameter("folder&fileid=2", "folder"), None);
}

#[test]
fn identifiers_names_and_digests_are_held_to_their_shape() {
    assert!(valid_token("Ab-1_z", 6));
    assert!(!valid_token("Ab-1_z", 5));
    assert!(!valid_token("", 5));
    assert!(!valid_token("a/b", 5));
    assert!(!valid_token("a.b", 5));
    assert!(valid_label("contoso-my"));
    assert!(!valid_label("contoso.evil"));
    assert!(!valid_label(&"a".repeat(64)));
    assert!(valid_name("Season 1"));
    assert!(!valid_name(".."));
    assert!(!valid_name("a/b"));
    assert!(!valid_name("a\u{0}b"));
    assert!(!valid_name(&"x".repeat(256)));
    assert!(valid_hex(&"aF09".repeat(10), 40));
    assert!(!valid_hex(&"a".repeat(39), 40));
    assert!(!valid_hex(&"g".repeat(40), 40));
}

#[test]
fn an_address_is_taken_apart_like_a_browser_would_for_a_claim() {
    let parts = Parts::of("  HTTPS://User@WWW.Example.TEST:8443/file/a/../b?x=1&y#top\n")
        .expect("an address");
    assert!(parts.is_http());
    assert!(parts.credentials);
    assert_eq!(parts.host, "WWW.Example.TEST");
    assert_eq!(parts.path, "/file/a/../b");
    assert_eq!(parts.query, Some("x=1&y"));
    // A dot segment is not resolved, so the address is not read at all.
    assert_eq!(parts.segments(), None);

    let bare = Parts::of("https://example.test?key").expect("an address");
    assert_eq!(
        (bare.host, bare.path, bare.query),
        ("example.test", "", Some("key"))
    );
    assert_eq!(bare.segments(), Some(Vec::new()));

    let plain = Parts::of("https://example.test//l/%2E%2Fx/").expect("an address");
    assert!(!plain.credentials);
    assert_eq!(plain.segments(), Some(vec!["l", "%2E%2Fx"]));
    assert_eq!(
        Parts::of("https://example.test/l/%2E%2e/x").and_then(|parts| parts.segments()),
        None
    );

    let other = Parts::of("x-app+v1.0://example.test/l/x").expect("an address");
    assert!(!other.is_http());
    assert_eq!(
        Parts::of("https://[::1]:80/a").map(|parts| parts.host),
        Some("[::1]")
    );
}

#[test]
fn what_is_not_an_absolute_address_is_refused() {
    for url in [
        "not an address",
        "example.test/l/x",
        "1http://example.test/",
        "https://",
        "https://:80/x",
        "https://example.test:http/x",
        "https://example.test:70000/x",
        "https://example.test:+80/x",
        "https://example.test\\l\\x",
        "https://[::1/x",
        "https://[::1]x/y",
    ] {
        assert_eq!(Parts::of(url), None, "{url}");
    }
}
