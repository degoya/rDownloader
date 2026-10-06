use super::*;

fn stated(algorithm: &str, value: &str) -> StatedHash {
    StatedHash {
        algorithm: algorithm.to_owned(),
        value: value.to_owned(),
    }
}

#[test]
fn sources_are_ordered_by_priority_then_document_order() {
    let set = SourceSet::checked(
        [
            ("https://c.example/f".to_owned(), None, None),
            (
                "https://b.example/f".to_owned(),
                Some(2),
                Some("DE".to_owned()),
            ),
            (
                "https://a.example/f".to_owned(),
                Some(1),
                Some("usa".to_owned()),
            ),
            ("https://d.example/f".to_owned(), Some(2), None),
        ],
        None,
        &[],
        None,
    )
    .expect("set");
    let hosts: Vec<_> = set
        .sources
        .iter()
        .map(|source| source.url.host_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(hosts, ["a.example", "b.example", "d.example", "c.example"]);
    assert_eq!(set.sources[1].location.as_deref(), Some("de"));
    // Three letters is not an ISO 3166-1 alpha-2 code.
    assert_eq!(set.sources[0].location, None);
}

#[test]
fn unusable_addresses_are_dropped_not_repaired() {
    let set = SourceSet::checked(
        [
            ("magnet:?xt=urn:btih:abc".to_owned(), None, None),
            ("file:///etc/passwd".to_owned(), None, None),
            ("ftp://user:secret@mirror.example/f".to_owned(), None, None),
            ("https://mirror.example/f".to_owned(), None, None),
            ("https://mirror.example/f".to_owned(), Some(1), None),
            ("sftp://mirror.example/f".to_owned(), None, None),
        ],
        None,
        &[],
        None,
    )
    .expect("set");
    assert_eq!(set.sources.len(), 2);
    assert_eq!(set.sources[0].url.as_str(), "https://mirror.example/f");
    assert_eq!(set.sources[1].url.scheme(), "sftp");
    assert!(
        SourceSet::checked(
            [("javascript:alert(1)".to_owned(), None, None)],
            None,
            &[],
            None
        )
        .is_none()
    );
}

#[test]
fn the_strongest_valid_hash_wins_and_malformed_ones_are_ignored() {
    let sha256 = "a".repeat(64);
    let set = SourceSet::checked(
        [("https://m.example/f".to_owned(), None, None)],
        Some(10),
        &[
            stated("md5", &"b".repeat(32)),
            stated("sha-256", &"z".repeat(64)),
            stated("sha-256", &sha256.to_uppercase()),
            stated("sha-512", &"c".repeat(128)),
        ],
        None,
    )
    .expect("set");
    assert_eq!(
        set.checksum,
        Some(ExpectedChecksum {
            algorithm: ChecksumAlgorithm::Sha256,
            value: sha256,
        })
    );
    assert!(set.has_hash_basis());
}

#[test]
fn a_piece_list_must_cover_the_stated_size_exactly() {
    let hash = "0".repeat(40);
    let source = || [("https://m.example/f".to_owned(), None, None)];
    let length = MIN_PIECE_LENGTH;
    let covering = SourceSet::checked(
        source(),
        Some(length * 2 + 1),
        &[],
        Some(("sha-1".to_owned(), length, vec![hash.clone(); 3])),
    )
    .expect("set");
    let pieces = covering.pieces.expect("pieces kept");
    assert_eq!(
        pieces.range(2, length * 2 + 1),
        (length * 2, length * 2 + 1)
    );
    // One hash short, no size, and a piece length below the floor are all refused.
    for (size, length, count) in [
        (Some(length * 2 + 1), length, 2),
        (None, length, 3),
        (Some(30), 10, 3),
    ] {
        let set = SourceSet::checked(
            source(),
            size,
            &[],
            Some(("sha-1".to_owned(), length, vec![hash.clone(); count])),
        )
        .expect("set");
        assert!(set.pieces.is_none());
    }
}

#[test]
fn a_long_mirror_list_is_capped() {
    let many = (0..100).map(|index| (format!("https://m{index}.example/f"), None, None));
    let set = SourceSet::checked(many, None, &[], None).expect("set");
    assert_eq!(set.sources.len(), MAX_SOURCES);
}

#[test]
fn oversized_piece_lists_addresses_and_foreign_schemes_are_refused() {
    let hash = "0".repeat(40);
    let source = || [("https://m.example/f".to_owned(), None, None)];
    // One piece more than the ceiling, covering its size exactly: refused by count alone.
    let pieces = MAX_PIECES + 1;
    let size = MIN_PIECE_LENGTH * pieces as u64;
    let set = SourceSet::checked(
        source(),
        Some(size),
        &[],
        Some((
            "sha-1".to_owned(),
            MIN_PIECE_LENGTH,
            vec![hash.clone(); pieces],
        )),
    )
    .expect("set");
    assert!(set.pieces.is_none());
    // A piece longer than the ceiling.
    let set = SourceSet::checked(
        source(),
        Some(MAX_PIECE_LENGTH + 1),
        &[],
        Some(("sha-1".to_owned(), MAX_PIECE_LENGTH + 1, vec![hash; 1])),
    )
    .expect("set");
    assert!(set.pieces.is_none());
    // An address longer than the ceiling, and schemes no runner speaks.
    let long = format!("https://m.example/{}", "a".repeat(MAX_SOURCE_URL));
    for address in [
        long.as_str(),
        "gopher://m.example/f",
        "dict://m.example:11211/stats",
        "ldap://m.example/o",
        "file://m.example/etc/passwd",
        "data:text/plain,hello",
    ] {
        assert!(
            SourceSet::checked([(address.to_owned(), None, None)], None, &[], None).is_none(),
            "{address}"
        );
    }
}

#[test]
fn a_checked_set_stays_on_public_addresses_until_the_intake_says_otherwise() {
    let set = SourceSet::checked(
        [("http://192.168.1.10/f".to_owned(), None, None)],
        None,
        &[],
        None,
    )
    .expect("set");
    assert!(!set.local_network);
    // A set stored before the field existed reads as the strict one.
    let mut stored = serde_json::to_value(&set).expect("json");
    stored
        .as_object_mut()
        .expect("object")
        .remove("local_network");
    let read: SourceSet = serde_json::from_value(stored).expect("read");
    assert!(!read.local_network);
}

#[test]
fn backoff_doubles_up_to_its_ceiling_and_honours_retry_after() {
    assert_eq!(source_backoff(1, None), Duration::seconds(30));
    assert_eq!(source_backoff(2, None), Duration::seconds(60));
    assert_eq!(source_backoff(40, None), Duration::seconds(1800));
    assert_eq!(source_backoff(1, Some(300)), Duration::seconds(300));
    assert_eq!(source_backoff(1, Some(u64::MAX)), Duration::seconds(1800));
}

#[test]
fn a_proposed_link_becomes_one_source_with_its_reach_and_no_password() {
    let link = Url::parse("ftp://user:secret@mirror.example/f.iso").expect("url");
    let set = SourceSet::of_link(&link, true).expect("set");
    assert_eq!(set.sources.len(), 1);
    assert_eq!(
        set.sources[0].url.as_str(),
        "ftp://user@mirror.example/f.iso"
    );
    assert!(set.local_network);
    assert!(set.size.is_none() && set.checksum.is_none() && set.pieces.is_none());
    let strict =
        SourceSet::of_link(&Url::parse("https://a.example/f").expect("url"), false).expect("set");
    assert!(!strict.local_network);
    assert!(
        SourceSet::of_link(&Url::parse("magnet:?xt=urn:btih:abc").expect("url"), false).is_none()
    );
}

#[test]
fn state_follows_isolation_protocol_and_backoff() {
    let now = Utc::now();
    let mut source = DownloadSource {
        position: 0,
        url: Url::parse("https://m.example/f").expect("url"),
        protocol: SourceProtocol::Https,
        priority: None,
        location: None,
        failures: 0,
        backoff_until: None,
        isolated_code: None,
        last_error_code: None,
        delivered_bytes: 0,
        local_network: false,
    };
    assert_eq!(source.state_at(now), SourceState::Ready);
    source.backoff_until = Some(now + Duration::seconds(10));
    assert_eq!(source.state_at(now), SourceState::BackingOff);
    // An FTP mirror is fetched through its runner and waits out a backoff like any other.
    source.protocol = SourceProtocol::Ftp;
    assert_eq!(source.state_at(now), SourceState::BackingOff);
    source.backoff_until = None;
    assert_eq!(source.state_at(now), SourceState::Ready);
    source.isolated_code = Some(CODE_PIECE_MISMATCH.to_owned());
    assert_eq!(source.state_at(now), SourceState::Isolated);
}
