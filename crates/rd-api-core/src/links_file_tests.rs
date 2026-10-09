use base64::{Engine as _, engine::general_purpose::STANDARD};
use rd_collector::{LinksDocument, LinksEntry, LinksPackage};

use super::{Passphrase, read_links, seal_links};

fn document() -> LinksDocument {
    LinksDocument {
        packages: vec![LinksPackage {
            name: Some("Holiday".to_owned()),
            password: Some("archive secret".to_owned()),
            category: None,
            comment: None,
            links: vec![LinksEntry {
                url: "https://ddownload.com/abc123/holiday.rar"
                    .parse()
                    .expect("address"),
                file_name: Some("holiday.rar".to_owned()),
                size: Some(10),
                checksum: None,
                mirror_group: None,
            }],
        }],
    }
}

fn passphrase(value: &str) -> Passphrase {
    Passphrase::new(value.to_owned())
}

#[tokio::test]
async fn a_sealed_file_opens_with_its_passphrase_and_hides_everything_without_it() {
    let sealed = seal_links(&document(), &passphrase("correct horse"))
        .await
        .expect("sealed");
    let text = String::from_utf8(sealed.clone()).expect("UTF-8");
    for hidden in ["ddownload", "Holiday", "archive secret"] {
        assert!(!text.contains(hidden), "{hidden} readable in {text}");
    }
    let opened = read_links(&sealed, Some(&passphrase("correct horse")))
        .await
        .expect("opened");
    assert_eq!(opened, document());
}

#[tokio::test]
async fn a_wrong_passphrase_is_refused() {
    let sealed = seal_links(&document(), &passphrase("correct horse"))
        .await
        .expect("sealed");
    let error = read_links(&sealed, Some(&passphrase("wrong horse")))
        .await
        .expect_err("refused");
    assert_eq!(error.code(), "rdlinks.passphrase_invalid");
    let error = read_links(&sealed, None).await.expect_err("refused");
    assert_eq!(error.code(), "rdlinks.passphrase_required");
}

/// One flipped byte of the ciphertext fails the tag, exactly as a wrong passphrase does.
#[tokio::test]
async fn a_changed_ciphertext_is_refused() {
    let sealed = seal_links(&document(), &passphrase("correct horse"))
        .await
        .expect("sealed");
    let mut file: serde_json::Value = serde_json::from_slice(&sealed).expect("JSON");
    let mut ciphertext = STANDARD
        .decode(
            file["encryption"]["ciphertext"]
                .as_str()
                .expect("ciphertext"),
        )
        .expect("base64");
    ciphertext[0] ^= 1;
    file["encryption"]["ciphertext"] = STANDARD.encode(ciphertext).into();
    let error = read_links(
        file.to_string().as_bytes(),
        Some(&passphrase("correct horse")),
    )
    .await
    .expect_err("refused");
    assert_eq!(error.code(), "rdlinks.passphrase_invalid");

    file["encryption"]["kdf"]["m_cost"] = 8.into();
    let error = read_links(
        file.to_string().as_bytes(),
        Some(&passphrase("correct horse")),
    )
    .await
    .expect_err("refused");
    assert_eq!(error.code(), "rdlinks.file_invalid");
}

#[tokio::test]
async fn a_short_passphrase_seals_nothing_and_none_is_ever_printed() {
    let error = seal_links(&document(), &passphrase("short"))
        .await
        .expect_err("refused");
    assert_eq!(error.code(), "rdlinks.passphrase_too_short");
    assert_eq!(
        format!("{:?}", passphrase("correct horse")),
        "Passphrase(..)"
    );
    assert!(Passphrase::given(Some(passphrase(""))).is_none());
}

#[tokio::test]
async fn a_readable_file_needs_no_passphrase_and_a_file_too_large_is_refused_first() {
    let written = rd_collector::write_links_file(&document()).expect("written");
    assert_eq!(read_links(&written, None).await.expect("read"), document());
    let oversized = vec![b' '; rd_collector::MAX_RDLINKS_BYTES + 1];
    let error = read_links(&oversized, None).await.expect_err("refused");
    assert_eq!(error.code(), "rdlinks.too_large");
}
