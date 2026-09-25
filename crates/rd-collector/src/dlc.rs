//! DLC link containers, the format JDownloader established and many DDL sites still publish.
//!
//! A DLC is one long base64 string whose last 88 characters are a key blob. That blob only
//! becomes the container key after a round trip to JDownloader's `dlcrypt` service: the
//! service answers with the key encrypted for one registered application, and there is no
//! offline algorithm that replaces the call. This module therefore stops at the format —
//! [`split_dlc_container`] prepares the request, [`decrypt_dlc`] turns the service's answer into
//! links — while the HTTP call itself lives in `rd-api` behind an opt-in setting, so nothing
//! here can reach the network on its own.

use aes::Aes128;
use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding},
};
use quick_xml::{Reader, XmlVersion, events::Event};
use url::Url;

/// Hard upper bound for an imported DLC container. Containers hold links, not payload, so
/// even a very large one stays far below this.
pub const MAX_DLC_BYTES: usize = 8 * 1024 * 1024;

/// Length of the trailing key blob: base64 of 64 bytes.
const KEY_BLOB_CHARS: usize = 88;

/// Application the key is requested for. The service encrypts its answer with the key pair
/// belonging to this identity, so `DLCRYPT_DEST_TYPE`, [`RC_KEY`] and [`RC_IV`] only ever
/// change together — `pylo` is pyLoad's long-published triple.
pub const DLCRYPT_DEST_TYPE: &str = "pylo";
/// AES key the service's answer is encrypted with. See [`DLCRYPT_DEST_TYPE`].
const RC_KEY: &[u8; 16] = b"cb99b5cbc24db398";
/// AES IV the service's answer is encrypted with. See [`DLCRYPT_DEST_TYPE`].
const RC_IV: &[u8; 16] = b"9bc24cb995cb8db3";

/// A container split into the part the service needs and the part it unlocks.
#[derive(Clone, Debug)]
pub struct DlcContainer {
    key_blob: String,
    payload: Vec<u8>,
}

impl DlcContainer {
    /// The blob to hand to the `dlcrypt` service as its `data` parameter.
    #[must_use]
    pub fn key_blob(&self) -> &str {
        &self.key_blob
    }
}

/// Everything a decrypted container carries.
#[derive(Clone, Debug, Default)]
pub struct DlcDocument {
    pub packages: Vec<DlcPackage>,
}

impl DlcDocument {
    /// Whether the container declared no downloadable link at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packages.iter().all(|package| package.files.is_empty())
    }
}

/// One package inside a container; a container may hold several.
#[derive(Clone, Debug, Default)]
pub struct DlcPackage {
    pub name: Option<String>,
    /// First password announced with the package, if any.
    pub password: Option<String>,
    pub comment: Option<String>,
    pub files: Vec<DlcFile>,
}

/// One link inside a package.
#[derive(Clone, Debug)]
pub struct DlcFile {
    pub url: Url,
    pub file_name: Option<String>,
    pub size: Option<u64>,
}

/// Splits a container into its key blob and its encrypted payload.
///
/// Rejects anything that is not the plain base64 body a DLC consists of, so a mistyped upload
/// fails here instead of after a pointless request to the decryption service.
pub fn split_dlc_container(input: &[u8]) -> Result<DlcContainer> {
    if input.len() > MAX_DLC_BYTES {
        bail!("DLC exceeds the 8 MiB input limit");
    }
    let body: Vec<u8> = input
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if body.len() <= KEY_BLOB_CHARS {
        bail!("DLC is too short to contain a key");
    }
    if !body.iter().all(|byte| is_base64_byte(*byte)) {
        bail!("DLC is not a base64 document");
    }
    let (payload, key_blob) = body.split_at(body.len() - KEY_BLOB_CHARS);
    let key_blob = String::from_utf8(key_blob.to_vec()).context("DLC key is not ASCII")?;
    BASE64
        .decode(&key_blob)
        .context("DLC key is not valid base64")?;
    let payload = BASE64
        .decode(payload)
        .context("DLC payload is not valid base64")?;
    if payload.is_empty() || payload.len() % 16 != 0 {
        bail!("DLC payload is not a whole number of AES blocks");
    }
    Ok(DlcContainer { key_blob, payload })
}

/// Unlocks a container with the `dlcrypt` service's answer and parses the links it holds.
///
/// `service_answer` is the raw response body; the `<rc>` element inside it carries the
/// container key encrypted for [`DLCRYPT_DEST_TYPE`].
pub fn decrypt_dlc(container: &DlcContainer, service_answer: &str) -> Result<DlcDocument> {
    let key = container_key(service_answer)?;
    // The container key is used as key *and* IV, which is how the format is defined.
    let plaintext = decrypt_cbc(&key, &key, &container.payload)?;
    // The plaintext is base64 again, followed by however the encoder padded the last block.
    // Padding bytes are control bytes, never base64 characters, so trimming to the last
    // base64 character removes PKCS#7 and zero padding alike.
    let end = plaintext
        .iter()
        .rposition(|byte| is_base64_byte(*byte))
        .map_or(0, |index| index + 1);
    let document = BASE64
        .decode(&plaintext[..end])
        .context("decrypted DLC is not base64 — the container key does not fit")?;
    parse_document(&document)
}

/// Extracts and unwraps the container key from the service's answer.
fn container_key(answer: &str) -> Result<[u8; 16]> {
    let encoded = answer
        .split_once("<rc>")
        .and_then(|(_, rest)| rest.split_once("</rc>"))
        .map_or_else(|| answer.trim(), |(value, _)| value.trim());
    if encoded.is_empty() {
        bail!("the decryption service returned no key");
    }
    let mut wrapped = BASE64
        .decode(encoded)
        .context("the decryption service returned a malformed key")?;
    if wrapped.len() != 16 {
        bail!("the decryption service returned a key of unexpected length");
    }
    // Deliberately unpadded: the answer is exactly one block and carries no padding.
    let key = decrypt_cbc_in_place(RC_KEY, RC_IV, &mut wrapped)?;
    key.try_into()
        .map_err(|_| anyhow::anyhow!("unwrapped DLC key has the wrong length"))
}

fn decrypt_cbc(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>> {
    let mut buffer = data.to_vec();
    let plaintext = decrypt_cbc_in_place(key, iv, &mut buffer)?;
    Ok(plaintext.to_vec())
}

fn decrypt_cbc_in_place<'a>(
    key: &[u8; 16],
    iv: &[u8; 16],
    buffer: &'a mut [u8],
) -> Result<&'a [u8]> {
    Decryptor::<Aes128>::new(key.into(), iv.into())
        .decrypt_padded_mut::<NoPadding>(buffer)
        .map_err(|_| anyhow::anyhow!("DLC ciphertext is not a whole number of AES blocks"))
}

/// Parses the decrypted DLC XML.
///
/// Every value in the document is base64 on top of the encryption, including the package
/// name and each link. A missing `<dlc>` root means the container key was wrong, which is
/// worth a different message than a container that legitimately holds nothing.
fn parse_document(input: &[u8]) -> Result<DlcDocument> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(true);
    let mut document = DlcDocument::default();
    let mut seen_root = false;
    let mut current_package: Option<DlcPackage> = None;
    let mut current_file: Option<PartialFile> = None;
    let mut current_element = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(start) => {
                current_element = start.name().as_ref().to_vec();
                match start.name().as_ref() {
                    b"dlc" => seen_root = true,
                    b"package" => {
                        let mut package = DlcPackage::default();
                        for attribute in start.attributes().with_checks(true) {
                            let attribute = attribute?;
                            let value =
                                decode_text(&attribute.normalized_value(XmlVersion::Implicit1_0)?);
                            match attribute.key.as_ref() {
                                b"name" => package.name = value,
                                b"passwords" | b"password" => {
                                    package.password = value.and_then(first_line);
                                }
                                b"comment" => package.comment = value,
                                _ => {}
                            }
                        }
                        current_package = Some(package);
                    }
                    b"file" => current_file = Some(PartialFile::default()),
                    _ => {}
                }
            }
            Event::Text(text) => {
                let Some(file) = &mut current_file else {
                    continue;
                };
                let value = decode_text(&text.decode()?);
                match current_element.as_slice() {
                    b"url" => file.url = value,
                    b"filename" => file.file_name = value,
                    b"size" => file.size = value,
                    _ => {}
                }
            }
            Event::End(end) => {
                match end.name().as_ref() {
                    b"file" => {
                        if let (Some(package), Some(file)) =
                            (&mut current_package, current_file.take())
                            && let Some(file) = file.into_file()
                        {
                            package.files.push(file);
                        }
                    }
                    b"package" => {
                        if let Some(package) = current_package.take() {
                            document.packages.push(package);
                        }
                    }
                    _ => {}
                }
                current_element.clear();
            }
            // A DLC never declares a doctype; refusing one outright keeps entity expansion
            // out of reach instead of arguing about which declarations are harmless.
            Event::DocType(_) => bail!("DLC must not declare a doctype"),
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_root {
        bail!("decrypted DLC has no <dlc> document");
    }
    Ok(document)
}

/// A `<file>` while its children are still being read.
#[derive(Default)]
struct PartialFile {
    url: Option<String>,
    file_name: Option<String>,
    size: Option<String>,
}

impl PartialFile {
    /// Drops links that do not parse; one broken entry must not fail the whole container.
    fn into_file(self) -> Option<DlcFile> {
        let url = Url::parse(self.url?.trim())
            .ok()
            .map(crate::canonical_url)?;
        Some(DlcFile {
            url,
            file_name: self.file_name.and_then(|name| {
                let name = name.trim().to_owned();
                (!name.is_empty()).then_some(name)
            }),
            size: self.size.and_then(|size| size.trim().parse().ok()),
        })
    }
}

/// Decodes one base64 value, falling back to the literal text when a writer left it plain.
fn decode_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let decoded = BASE64
        .decode(trimmed)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| trimmed.to_owned());
    let decoded = decoded.trim().to_owned();
    (!decoded.is_empty()).then_some(decoded)
}

fn first_line(value: String) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

const fn is_base64_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
}

#[cfg(test)]
mod tests {
    use aes::Aes128;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use cbc::{
        Encryptor,
        cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7},
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
            .encrypt_padded_mut::<Pkcs7>(&mut buffer, plaintext.len())
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
            .encrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(&mut wrapped, 16)
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
            .encrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(&mut wrong, 16)
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
}
