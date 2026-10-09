use super::{
    LinksDocument, LinksEntry, LinksFile, LinksKdf, LinksPackage, MAX_RDLINKS_BYTES,
    MAX_RDLINKS_LINKS, RDLINKS_FORMAT, SealedLinks, read_links_file, read_sealed_plaintext,
    sealed_plaintext, write_links_file, write_sealed_file,
};

fn entry(address: &str) -> LinksEntry {
    LinksEntry {
        url: address.parse().expect("address"),
        file_name: Some("holiday.part1.rar".to_owned()),
        size: Some(1_048_576),
        checksum: Some(rd_core::ExpectedChecksum {
            algorithm: rd_core::ChecksumAlgorithm::Sha256,
            value: "ab".repeat(32),
        }),
        mirror_group: Some("part1".to_owned()),
    }
}

fn document() -> LinksDocument {
    LinksDocument {
        packages: vec![LinksPackage {
            name: Some("Holiday".to_owned()),
            password: Some("secret".to_owned()),
            category: Some("Videos".to_owned()),
            comment: Some("two lines\nof comment".to_owned()),
            links: vec![
                entry("https://ddownload.com/abc123/holiday.part1.rar"),
                entry("https://rapidgator.net/file/abc/holiday.part1.rar.html"),
            ],
        }],
    }
}

#[test]
fn a_readable_document_comes_back_as_it_was_written() {
    let written = write_links_file(&document()).expect("written");
    let text = String::from_utf8(written.clone()).expect("UTF-8");
    assert!(text.contains(RDLINKS_FORMAT));
    assert_eq!(
        read_links_file(&written).expect("read"),
        LinksFile::Plain(document())
    );
}

/// The file carries what makes the links and nothing that binds them to this installation.
#[test]
fn no_plugin_version_account_or_token_is_part_of_the_format() {
    let text = String::from_utf8(write_links_file(&document()).expect("written")).expect("UTF-8");
    for absent in [
        "plugin", "version", "account", "cookie", "token", "resolver",
    ] {
        assert!(!text.contains(absent), "{absent} in {text}");
    }
}

#[test]
fn another_format_version_is_refused() {
    let text = r#"{"format":"rdownloader-links/2","packages":[]}"#;
    let error = read_links_file(text.as_bytes()).expect_err("refused");
    assert!(format!("{error:#}").contains(RDLINKS_FORMAT));
}

#[test]
fn a_document_with_both_bodies_or_neither_is_refused() {
    let sealed = SealedLinks {
        kdf: LinksKdf {
            algorithm: "argon2id".to_owned(),
            m_cost: 1,
            t_cost: 1,
            p_cost: 1,
            salt: "AAAA".to_owned(),
        },
        cipher: "xchacha20poly1305".to_owned(),
        nonce: "AAAA".to_owned(),
        ciphertext: "AAAA".to_owned(),
    };
    let mut both: serde_json::Value =
        serde_json::from_slice(&write_sealed_file(&sealed).expect("sealed")).expect("JSON");
    both["packages"] = serde_json::json!([]);
    assert!(read_links_file(both.to_string().as_bytes()).is_err());
    let neither = format!(r#"{{"format":"{RDLINKS_FORMAT}"}}"#);
    assert!(read_links_file(neither.as_bytes()).is_err());
}

#[test]
fn a_sealed_file_is_handed_on_unopened() {
    let sealed = SealedLinks {
        kdf: LinksKdf {
            algorithm: "argon2id".to_owned(),
            m_cost: 19_456,
            t_cost: 2,
            p_cost: 1,
            salt: "c2FsdA==".to_owned(),
        },
        cipher: "xchacha20poly1305".to_owned(),
        nonce: "bm9uY2U=".to_owned(),
        ciphertext: "Y2lwaGVy".to_owned(),
    };
    let written = write_sealed_file(&sealed).expect("sealed");
    assert_eq!(
        read_links_file(&written).expect("read"),
        LinksFile::Sealed(sealed)
    );
    let plaintext = sealed_plaintext(&document()).expect("plaintext");
    assert_eq!(
        read_sealed_plaintext(&plaintext).expect("opened"),
        document()
    );
}

/// An edited file is refused whole, never imported in part: a link to this machine's files, a
/// script address or a control character in a name is not something an export ever wrote.
#[test]
fn a_manipulated_document_is_refused() {
    for address in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/plain,x",
    ] {
        let mut changed = document();
        changed.packages[0].links[0].url = address.parse().expect("address");
        let text = serde_json::json!({ "format": RDLINKS_FORMAT, "packages": changed.packages });
        assert!(
            read_links_file(text.to_string().as_bytes()).is_err(),
            "{address} accepted"
        );
    }
    let mut changed = document();
    changed.packages[0].name = Some("bell\u{7}".to_owned());
    let text = serde_json::json!({ "format": RDLINKS_FORMAT, "packages": changed.packages });
    assert!(read_links_file(text.to_string().as_bytes()).is_err());
    let mut changed = document();
    changed.packages[0].links[0]
        .checksum
        .as_mut()
        .expect("checksum")
        .value = "not a hash!".to_owned();
    let text = serde_json::json!({ "format": RDLINKS_FORMAT, "packages": changed.packages });
    assert!(read_links_file(text.to_string().as_bytes()).is_err());
    assert!(read_links_file(b"not json at all").is_err());
}

#[test]
fn the_limits_hold_on_both_sides() {
    let mut many = document();
    many.packages[0].links = (0..=MAX_RDLINKS_LINKS)
        .map(|index| entry(&format!("https://example.com/{index}")))
        .collect();
    assert!(write_links_file(&many).is_err(), "written past the limit");
    let text = serde_json::json!({ "format": RDLINKS_FORMAT, "packages": many.packages });
    assert!(read_links_file(text.to_string().as_bytes()).is_err());
    many.packages[0].links.truncate(MAX_RDLINKS_LINKS);
    assert!(write_links_file(&many).is_ok(), "exactly the limit is fine");

    let empty = LinksDocument {
        packages: vec![LinksPackage::default()],
    };
    assert!(write_links_file(&empty).is_err(), "no links, no file");

    let oversized = vec![b' '; MAX_RDLINKS_BYTES + 1];
    let error = read_links_file(&oversized).expect_err("refused");
    assert!(format!("{error:#}").contains("MiB"));
}
