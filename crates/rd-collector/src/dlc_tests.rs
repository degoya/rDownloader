use aes::Aes128;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use cbc::{
    Encryptor,
    cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7},
};

use super::{MAX_DLC_BYTES, RC_IV, RC_KEY, decrypt_dlc, split_dlc_container};

const CONTAINER_KEY: &[u8; 16] = b"0123456789abcdef";

const XML: &str = r#"<dlc>
      <header><generator><app>test</app></generator></header>
      <content>
        <package name="UmVsZWFzZSBPbmU=" passwords="c2VjcmV0Cm90aGVy" comment="aGVsbG8=">
          <file>
            <url>aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnJhcg==</url>
            <filename>YS5yYXI=</filename>
            <size>MTA0ODU3Ng==</size>
          </file>
          <file>
            <url>ZnRwOi8vZmlsZXMuZXhhbXBsZS5jb20vcHViL2IuYmlu</url>
            <filename>Yi5iaW4=</filename>
            <size></size>
          </file>
        </package>
        <package name="UmVsZWFzZSBUd28=">
          <file>
            <url>aHR0cHM6Ly9leGFtcGxlLmNvbS9jLnJhcg==</url>
            <filename>Yy5yYXI=</filename>
          </file>
        </package>
      </content>
    </dlc>"#;

fn encrypt(key: &[u8; 16], iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
    let mut buffer = vec![0_u8; plaintext.len() + 16];
    buffer[..plaintext.len()].copy_from_slice(plaintext);
    Encryptor::<Aes128>::new(key.into(), iv.into())
        .encrypt_padded::<Pkcs7>(&mut buffer, plaintext.len())
        .expect("encrypt")
        .to_vec()
}

/// Builds what a real container plus service answer look like for a known key.
fn container_and_answer(xml: &str) -> (String, String) {
    let payload = encrypt(CONTAINER_KEY, CONTAINER_KEY, BASE64.encode(xml).as_bytes());
    let key_blob = BASE64.encode([7_u8; 64]);
    assert_eq!(key_blob.len(), super::KEY_BLOB_CHARS);
    // The service answers with the container key as a single unpadded block.
    let mut wrapped = *CONTAINER_KEY;
    cbc::Encryptor::<Aes128>::new(RC_KEY.into(), RC_IV.into())
        .encrypt_padded::<cbc::cipher::block_padding::NoPadding>(&mut wrapped, 16)
        .expect("wrap key");
    (
        format!("{}{key_blob}", BASE64.encode(&payload)),
        format!("<rc>{}</rc>", BASE64.encode(wrapped)),
    )
}

#[test]
fn decrypts_a_container_and_reads_every_package() {
    let (container, answer) = container_and_answer(XML);
    let parsed = split_dlc_container(container.as_bytes()).expect("split");
    assert_eq!(parsed.key_blob(), BASE64.encode([7_u8; 64]));
    let document = decrypt_dlc(&parsed, &answer).expect("decrypt");

    assert_eq!(document.packages.len(), 2);
    let first = &document.packages[0];
    assert_eq!(first.name.as_deref(), Some("Release One"));
    // Only the first of the announced passwords is carried over.
    assert_eq!(first.password.as_deref(), Some("secret"));
    assert_eq!(first.comment.as_deref(), Some("hello"));
    assert_eq!(first.files.len(), 2);
    assert_eq!(first.files[0].url.as_str(), "https://example.com/a.rar");
    assert_eq!(first.files[0].file_name.as_deref(), Some("a.rar"));
    assert_eq!(first.files[0].size, Some(1_048_576));
    // A transfer-protocol link survives intact instead of being rewritten to https.
    assert_eq!(
        first.files[1].url.as_str(),
        "ftp://files.example.com/pub/b.bin"
    );
    assert_eq!(first.files[1].size, None);
    assert_eq!(document.packages[1].files.len(), 1);
    assert!(!document.is_empty());
}

#[test]
fn a_wrong_container_key_fails_instead_of_yielding_junk() {
    let (container, _) = container_and_answer(XML);
    let parsed = split_dlc_container(container.as_bytes()).expect("split");
    let mut wrong = [0_u8; 16];
    cbc::Encryptor::<Aes128>::new(RC_KEY.into(), RC_IV.into())
        .encrypt_padded::<cbc::cipher::block_padding::NoPadding>(&mut wrong, 16)
        .expect("wrap key");
    let error = decrypt_dlc(&parsed, &format!("<rc>{}</rc>", BASE64.encode(wrong)))
        .expect_err("a wrong key must not parse");
    let message = error.to_string();
    assert!(
        message.contains("container key") || message.contains("<dlc>"),
        "unexpected message: {message}"
    );
}

#[test]
fn an_alias_host_is_canonicalised_like_a_pasted_link() {
    crate::links::tests::register_alias();
    let xml = format!(
        "<dlc><content><package name=\"{}\"><file><url>{}</url></file></package></content></dlc>",
        BASE64.encode("Aliased"),
        BASE64.encode("https://alias.test/ga54c0dlen1e/file.rar")
    );
    let (container, answer) = container_and_answer(&xml);
    let document = decrypt_dlc(
        &split_dlc_container(container.as_bytes()).expect("split"),
        &answer,
    )
    .expect("decrypt");
    assert_eq!(
        document.packages[0].files[0].url.as_str(),
        "https://fixture.test/ga54c0dlen1e/file.rar"
    );
}

#[test]
fn an_unparsable_link_is_skipped_rather_than_failing_the_container() {
    let xml = format!(
        "<dlc><content><package><file><url>{}</url></file><file><url>{}</url></file></package></content></dlc>",
        BASE64.encode("not a url"),
        BASE64.encode("https://example.com/good.rar")
    );
    let (container, answer) = container_and_answer(&xml);
    let document = decrypt_dlc(
        &split_dlc_container(container.as_bytes()).expect("split"),
        &answer,
    )
    .expect("decrypt");
    assert_eq!(document.packages[0].files.len(), 1);
    assert_eq!(
        document.packages[0].files[0].url.as_str(),
        "https://example.com/good.rar"
    );
}

#[test]
fn an_empty_container_parses_but_reports_itself_empty() {
    let (container, answer) = container_and_answer("<dlc><content></content></dlc>");
    let document = decrypt_dlc(
        &split_dlc_container(container.as_bytes()).expect("split"),
        &answer,
    )
    .expect("decrypt");
    assert!(document.is_empty());
}

#[test]
fn a_doctype_is_refused_outright() {
    let (container, answer) = container_and_answer(
        "<!DOCTYPE dlc [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]><dlc><content/></dlc>",
    );
    let error = decrypt_dlc(
        &split_dlc_container(container.as_bytes()).expect("split"),
        &answer,
    )
    .expect_err("a doctype must be refused");
    assert!(error.to_string().contains("doctype"));
}

#[test]
fn malformed_containers_are_rejected_before_any_request() {
    let key_blob = BASE64.encode([7_u8; 64]);
    assert!(split_dlc_container(b"not base64 at all!").is_err());
    assert!(
        split_dlc_container(key_blob.as_bytes()).is_err(),
        "key only"
    );
    assert!(
        split_dlc_container(format!("QUJD{key_blob}").as_bytes()).is_err(),
        "payload is not a whole number of blocks"
    );
    assert!(
        split_dlc_container(&vec![b'A'; MAX_DLC_BYTES + 1]).is_err(),
        "oversized"
    );
}

#[test]
fn whitespace_between_the_base64_lines_is_tolerated() {
    let (container, answer) = container_and_answer(XML);
    let wrapped = container
        .as_bytes()
        .chunks(76)
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect::<Vec<_>>()
        .join("\r\n");
    let document = decrypt_dlc(
        &split_dlc_container(wrapped.as_bytes()).expect("split"),
        &answer,
    )
    .expect("decrypt");
    assert_eq!(document.packages.len(), 2);
}

/// The triple is published, not derived, so the only way to get it wrong is a typo — and
/// every other test here builds its fixture from these same constants, so a typo would
/// round-trip happily and only fail against the real service. Spelled out separately
/// against pyLoad's `containers/DLC.py`, which is where the values come from.
#[test]
fn the_published_key_pair_is_what_the_service_expects() {
    assert_eq!(super::DLCRYPT_DEST_TYPE, "pylo");
    assert_eq!(RC_KEY, b"cb99b5cbc24db398");
    assert_eq!(RC_IV, b"9bc24cb995cb8db3");
}

#[test]
fn a_service_answer_without_a_key_is_reported() {
    let (container, _) = container_and_answer(XML);
    let parsed = split_dlc_container(container.as_bytes()).expect("split");
    assert!(decrypt_dlc(&parsed, "<rc></rc>").is_err());
    assert!(decrypt_dlc(&parsed, "Service temporarily unavailable").is_err());
}
