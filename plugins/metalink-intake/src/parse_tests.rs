//! Reading a Metalink document: what is claimed, and which files and sources come out.

use super::{ParsedFile, ParsedPieces, ParsedSource, UNREADABLE, claims, files_in};

const META4: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="example.iso">
    <size>14471447</size>
    <hash type="sha-256">abc</hash>
    <url priority="1">https://mirror.example/example.iso</url>
    <url priority="2">https://other.example/example.iso</url>
  </file>
  <file name="notes.txt">
    <url>https://mirror.example/notes.txt</url>
  </file>
</metalink>"#;

#[test]
fn a_metalink_4_document_yields_its_files() {
    assert!(claims(META4));
    let files = files_in(META4);
    assert_eq!(files.len(), 2);
    assert_eq!(
        files[0],
        ParsedFile {
            name: Some("example.iso".to_owned()),
            size: Some(14_471_447),
            urls: vec![
                "https://mirror.example/example.iso".to_owned(),
                "https://other.example/example.iso".to_owned(),
            ],
            sources: vec![
                ParsedSource {
                    url: "https://mirror.example/example.iso".to_owned(),
                    priority: Some(1),
                    location: None,
                },
                ParsedSource {
                    url: "https://other.example/example.iso".to_owned(),
                    priority: Some(2),
                    location: None,
                },
            ],
            hashes: vec![("sha-256".to_owned(), "abc".to_owned())],
            pieces: None,
        }
    );
    assert_eq!(files[1].name.as_deref(), Some("notes.txt"));
    assert_eq!(files[1].size, None);
}

#[test]
fn the_older_metalink_3_layout_is_read_the_same_way() {
    // Version 3 wraps the entries in <files> and the addresses in <resources>. Neither
    // wrapper carries anything this parser needs, so it reads through them.
    const V3: &str = r#"<metalink version="3.0" xmlns="http://www.metalinker.org/">
  <files>
    <file name="example.tar.gz">
      <size>1024</size>
      <resources>
        <url type="http">http://mirror.example/example.tar.gz</url>
      </resources>
    </file>
  </files>
</metalink>"#;
    assert!(claims(V3));
    let files = files_in(V3);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name.as_deref(), Some("example.tar.gz"));
    assert_eq!(files[0].size, Some(1024));
    assert_eq!(files[0].urls, vec!["http://mirror.example/example.tar.gz"]);
}

#[test]
fn an_entry_without_a_usable_address_is_skipped() {
    // A metalink may list a torrent or an FTP mirror this parser cannot propose. Dropping
    // the entry is right; proposing an address the queue cannot fetch is not.
    const MIXED: &str = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="only-torrent.iso"><url>magnet:?xt=urn:btih:abc</url></file>
  <file name="good.iso"><url>https://mirror.example/good.iso</url></file>
</metalink>"#;
    let files = files_in(MIXED);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name.as_deref(), Some("good.iso"));
}

#[test]
fn escaped_characters_come_back_decoded() {
    const ESCAPED: &str = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="a &amp; b.bin"><url>https://mirror.example/get?a=1&amp;b=2</url></file>
</metalink>"#;
    let files = files_in(ESCAPED);
    assert_eq!(files[0].name.as_deref(), Some("a & b.bin"));
    assert_eq!(files[0].urls, vec!["https://mirror.example/get?a=1&b=2"]);
}

#[test]
fn text_that_is_not_a_metalink_is_not_claimed() {
    assert!(!claims(
        "https://example.com/one.bin\nhttps://example.com/two.bin"
    ));
    assert!(!claims("<html><body>nothing to see</body></html>"));
}

#[test]
fn an_unterminated_document_stops_instead_of_looping() {
    // A truncated download is the realistic way this happens, and a scanner that keeps
    // looking for a closing tag it will never find would hang inside the fuel budget.
    let files = files_in("<metalink><file name=\"x\"><url>https://a.example/x");
    assert!(files.is_empty());
}

#[test]
fn mirrors_come_back_by_priority_with_location_and_every_protocol() {
    const RANKED: &str = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="ranked.iso">
    <size>1000</size>
    <url priority="3" location="us">https://slow.example/ranked.iso</url>
    <url priority="1" location="de">ftp://fast.example/ranked.iso</url>
    <url priority="2" location="fr">https://second.example/ranked.iso</url>
    <metaurl mediatype="torrent" priority="1">https://t.example/ranked.torrent</metaurl>
  </file>
</metalink>"#;
    let files = files_in(RANKED);
    let file = &files[0];
    let order: Vec<_> = file
        .sources
        .iter()
        .map(|source| {
            (
                source.url.as_str(),
                source.priority,
                source.location.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        order,
        [
            ("ftp://fast.example/ranked.iso", Some(1), Some("de")),
            ("https://second.example/ranked.iso", Some(2), Some("fr")),
            ("https://slow.example/ranked.iso", Some(3), Some("us")),
        ]
    );
    // The proposal is the best HTTP mirror; FTP stays a source of the set.
    assert_eq!(file.urls[0], "https://second.example/ranked.iso");
}

#[test]
fn piece_hashes_are_read_apart_from_the_whole_file_hash() {
    const PIECES: &str = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="pieces.bin">
    <size>40000</size>
    <hash type="sha-256">aaaa</hash>
    <pieces length="16384" type="sha-1">
      <hash>1111</hash>
      <hash>2222</hash>
      <hash>3333</hash>
    </pieces>
    <url>https://m.example/pieces.bin</url>
  </file>
</metalink>"#;
    let file = &files_in(PIECES)[0];
    assert_eq!(file.hashes, [("sha-256".to_owned(), "aaaa".to_owned())]);
    assert_eq!(
        file.pieces,
        Some(ParsedPieces {
            algorithm: "sha-1".to_owned(),
            length: 16_384,
            hashes: vec!["1111".to_owned(), "2222".to_owned(), "3333".to_owned()],
        })
    );
}

#[test]
fn metalink_3_preference_and_verification_map_onto_the_same_fields() {
    const V3: &str = r#"<metalink version="3.0" xmlns="http://www.metalinker.org/">
  <files><file name="v3.bin">
    <size>10</size>
    <verification>
      <hash type="sha1">abcd</hash>
      <pieces length="16384" type="sha1"><hash piece="0">eeee</hash></pieces>
    </verification>
    <resources>
      <url type="http" location="uk" preference="10">http://low.example/v3.bin</url>
      <url type="http" preference="100">http://high.example/v3.bin</url>
    </resources>
  </file></files>
</metalink>"#;
    let file = &files_in(V3)[0];
    assert_eq!(file.urls[0], "http://high.example/v3.bin");
    assert_eq!(file.sources[0].priority, Some(1));
    assert_eq!(file.sources[1].priority, Some(91));
    assert_eq!(file.sources[1].location.as_deref(), Some("uk"));
    assert_eq!(file.hashes, [("sha1".to_owned(), "abcd".to_owned())]);
    assert_eq!(
        file.pieces.as_ref().map(|pieces| pieces.hashes.len()),
        Some(1)
    );
}

#[test]
fn a_hostile_mirror_list_is_bounded() {
    // Thousands of mirrors and more piece hashes than the host keeps: the first is cut to
    // the per-file ceiling, the second is refused whole.
    let mut document = String::from(
        r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="big"><size>1</size>"#,
    );
    for index in 0..5_000 {
        document.push_str(&format!("<url>https://m{index}.example/big</url>"));
    }
    document.push_str(r#"<pieces length="16384" type="sha-1">"#);
    for _ in 0..=super::MAX_PIECE_HASHES {
        document.push_str("<hash>00</hash>");
    }
    document.push_str("</pieces></file></metalink>");
    let file = &files_in(&document)[0];
    assert_eq!(file.sources.len(), super::MAX_URLS_PER_FILE);
    assert!(file.pieces.is_none());
}

#[test]
fn traversal_in_a_file_name_is_left_for_the_host_to_sanitise() {
    // The parser reports the name as written; `rd_files::sanitize_file_name` is the one
    // place a name is made safe, and a second, different rule here would only disagree.
    const TRAVERSAL: &str = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="../../etc/passwd"><url>https://m.example/x</url></file>
</metalink>"#;
    assert_eq!(
        files_in(TRAVERSAL)[0].name.as_deref(),
        Some("../../etc/passwd")
    );
}

#[test]
fn mixed_case_markup_and_non_ascii_text_keep_their_offsets() {
    const MIXED: &str = r#"<Metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <FILE NAME="Çalış İst.bin"><URL Priority="1" LOCATION="at">https://m.example/g</URL></FILE>
</Metalink>"#;
    let file = &files_in(MIXED)[0];
    assert_eq!(file.name.as_deref(), Some("Çalış İst.bin"));
    assert_eq!(file.sources[0].location.as_deref(), Some("at"));
    assert_eq!(file.sources[0].priority, Some(1));
}

/// A document this parser claims but that lists no file with an address is the case the
/// guest refuses with [`UNREADABLE`] rather than answering with nothing.
#[test]
fn a_claimed_document_without_a_file_is_the_unreadable_case() {
    const NOTHING: &str = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink"></metalink>"#;
    assert!(claims(NOTHING));
    assert!(
        files_in(NOTHING)
            .into_iter()
            .all(|file| file.urls.is_empty())
    );
}

/// The code the guest sends is one every catalogue translates.
#[test]
fn the_unreadable_code_is_in_every_catalogue() {
    let root = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    for language in ["de", "en", "es", "fr"] {
        let path = root.join("locales").join(format!("{language}.json"));
        let catalogue = std::fs::read_to_string(&path).expect("catalogue");
        assert!(
            catalogue.contains(&format!("\"{UNREADABLE}\"")),
            "{} lacks {UNREADABLE}",
            path.display()
        );
    }
}
