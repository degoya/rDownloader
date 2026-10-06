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
    cipher::{BlockModeDecrypt, KeyIvInit, block_padding::NoPadding},
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
        .decrypt_padded::<NoPadding>(buffer)
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
    let mut current_element = String::new();
    loop {
        match reader.read_event()? {
            Event::Start(start) => {
                current_element = start.name().as_ref().to_owned();
                match start.name().as_ref() {
                    "dlc" => seen_root = true,
                    "package" => {
                        let mut package = DlcPackage::default();
                        for attribute in start.attributes().with_checks(true) {
                            let attribute = attribute?;
                            let value =
                                decode_text(&attribute.normalized_value(XmlVersion::Implicit1_0)?);
                            match attribute.key.as_ref() {
                                "name" => package.name = value,
                                "passwords" | "password" => {
                                    package.password = value.and_then(first_line);
                                }
                                "comment" => package.comment = value,
                                _ => {}
                            }
                        }
                        current_package = Some(package);
                    }
                    "file" => current_file = Some(PartialFile::default()),
                    _ => {}
                }
            }
            Event::Text(text) => {
                let Some(file) = &mut current_file else {
                    continue;
                };
                let value = decode_text(&text);
                match current_element.as_str() {
                    "url" => file.url = value,
                    "filename" => file.file_name = value,
                    "size" => file.size = value,
                    _ => {}
                }
            }
            Event::End(end) => {
                match end.name().as_ref() {
                    "file" => {
                        if let (Some(package), Some(file)) =
                            (&mut current_package, current_file.take())
                            && let Some(file) = file.into_file()
                        {
                            package.files.push(file);
                        }
                    }
                    "package" => {
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
#[path = "dlc_tests.rs"]
mod tests;
