//! RSDF containers, decrypted locally (pyLoad's `containers/RSDF.py`).
//!
//! Unlike DLC and CCF nothing leaves the machine: the key is fixed and published, so the whole
//! format is openable offline. The layout is the file hex-encoded as a whole, decoding to one
//! base64 line per link, each line AES-192-CFB encrypted.
//!
//! The two initialisation vectors are derived rather than given, which is the only awkward part:
//! the published IV is encrypted once to produce a second, and that second one is used to
//! encrypt the published IV again to produce the one the links actually use. Both steps are a
//! single block, so they reduce to the raw block cipher and need no streaming mode.

use aes::{
    Aes192,
    cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray},
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

use crate::dlc::{DlcDocument, DlcFile, DlcPackage};

/// Same order of magnitude as the DLC cap; an RSDF is a list of links, never a large file.
pub const MAX_RSDF_BYTES: usize = 4 * 1024 * 1024;

/// The published RSDF key, as hex. 24 bytes, so this is AES-192.
const KEY_HEX: &str = "8C35192D964DC3182C6F84F3252239EB4A320D2500000000";
/// The published starting vector: sixteen 0xFF bytes.
const IV_SEED: [u8; 16] = [0xFF; 16];
/// Links carry this prefix inside the container; it names the format, not the address.
const LINK_PREFIX: &str = "CCF: ";

/// Reads an RSDF container into the same shape every other container produces.
///
/// The result is one unnamed package: the format carries links and nothing else, no package
/// names and no passwords, so the import names it after the file.
pub fn decode_rsdf(input: &[u8]) -> Result<DlcDocument> {
    if input.len() > MAX_RSDF_BYTES {
        bail!("RSDF exceeds the {MAX_RSDF_BYTES} byte limit");
    }
    let hex: String = String::from_utf8_lossy(input)
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    if hex.is_empty() {
        bail!("the RSDF is empty");
    }
    let decoded = hex::decode(&hex).context("an RSDF is hex from end to end")?;

    let key = hex::decode(KEY_HEX).expect("the published key is valid hex");
    let cipher = Aes192::new_from_slice(&key).expect("the published key is 24 bytes");
    let vector = link_vector(&cipher);

    let mut files = Vec::new();
    for line in decoded.split(|byte| *byte == b'\n') {
        let line = trim_ascii_whitespace(line);
        if line.is_empty() {
            continue;
        }
        let ciphertext = BASE64
            .decode(line)
            .context("an RSDF line is base64 inside the encryption")?;
        let plaintext = decrypt_cfb128(&cipher, vector, &ciphertext);
        let text = String::from_utf8_lossy(&plaintext);
        let text = text.trim().trim_start_matches(LINK_PREFIX).trim();
        if text.is_empty() {
            continue;
        }
        // A wrong key produces bytes, not addresses. Refusing here is what keeps a container
        // that cannot be read from arriving as a package full of nonsense.
        //
        // The line itself stays out of the message (RD-109-39). `container_handlers` answers
        // with `format!("{error:#}")`, so whatever is named here goes back over REST -- and a
        // container line is a caller-supplied address that may carry a share password in its
        // fragment. It is worthless in the message besides: a container read with the wrong key
        // yields bytes, not an address anyone could act on.
        let url = text
            .parse()
            .context("an RSDF line did not decrypt to an address")?;
        files.push(DlcFile {
            url,
            file_name: None,
            size: None,
        });
    }
    if files.is_empty() {
        bail!("the RSDF holds no links");
    }
    Ok(DlcDocument {
        packages: vec![DlcPackage {
            name: None,
            password: None,
            comment: None,
            files,
        }],
    })
}

/// The vector the links are encrypted with.
///
/// Two derivations, both a single block: the seed is encrypted to give an intermediate vector,
/// and the seed is then encrypted again in CFB with that intermediate. CFB over exactly one
/// block is `plaintext XOR E(vector)`, which is why no streaming mode appears here.
fn link_vector(cipher: &Aes192) -> [u8; 16] {
    let intermediate = encrypt_block(cipher, IV_SEED);
    let keystream = encrypt_block(cipher, intermediate);
    let mut vector = IV_SEED;
    for (byte, key) in vector.iter_mut().zip(keystream) {
        *byte ^= key;
    }
    vector
}

fn encrypt_block(cipher: &Aes192, block: [u8; 16]) -> [u8; 16] {
    let mut block = GenericArray::from(block);
    cipher.encrypt_block(&mut block);
    block.into()
}

/// CFB with full-block feedback: each block is XORed with the encrypted previous ciphertext
/// block, and the ciphertext — not the plaintext — becomes the next vector.
fn decrypt_cfb128(cipher: &Aes192, vector: [u8; 16], ciphertext: &[u8]) -> Vec<u8> {
    let mut register = vector;
    let mut plaintext = Vec::with_capacity(ciphertext.len());
    for chunk in ciphertext.chunks(16) {
        let keystream = encrypt_block(cipher, register);
        for (index, byte) in chunk.iter().enumerate() {
            plaintext.push(byte ^ keystream[index]);
        }
        // A short final chunk ends the stream, so the register is only carried for full blocks.
        if chunk.len() == 16 {
            register.copy_from_slice(chunk);
        }
    }
    plaintext
}

fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = bytes {
        if first.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = bytes {
        if last.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::{
        BASE64, Engine, IV_SEED, KEY_HEX, decode_rsdf, decrypt_cfb128, encrypt_block, link_vector,
    };
    use aes::{Aes192, cipher::KeyInit};

    fn cipher() -> Aes192 {
        Aes192::new_from_slice(&hex::decode(KEY_HEX).expect("hex")).expect("key")
    }

    /// The mirror image of `decrypt_cfb128`, used to build fixtures.
    fn encrypt_cfb128(cipher: &Aes192, vector: [u8; 16], plaintext: &[u8]) -> Vec<u8> {
        let mut register = vector;
        let mut ciphertext = Vec::with_capacity(plaintext.len());
        for chunk in plaintext.chunks(16) {
            let keystream = encrypt_block(cipher, register);
            let block: Vec<u8> = chunk
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ keystream[index])
                .collect();
            if block.len() == 16 {
                register.copy_from_slice(&block);
            }
            ciphertext.extend_from_slice(&block);
        }
        ciphertext
    }

    fn container_of(links: &[&str]) -> Vec<u8> {
        let cipher = cipher();
        let vector = link_vector(&cipher);
        let body = links
            .iter()
            .map(|link| BASE64.encode(encrypt_cfb128(&cipher, vector, link.as_bytes())))
            .collect::<Vec<_>>()
            .join("\n");
        hex::encode(body).into_bytes()
    }

    /// The key is published, not derived, so a typo is the only way to get it wrong — and every
    /// fixture here is built from the same constants, so a typo would round-trip happily and
    /// only fail against a real container. Spelled out separately against pyLoad's RSDF.py.
    #[test]
    fn the_published_key_is_what_the_format_uses() {
        assert_eq!(KEY_HEX, "8C35192D964DC3182C6F84F3252239EB4A320D2500000000");
        assert_eq!(IV_SEED, [0xFF; 16]);
        assert_eq!(
            hex::decode(KEY_HEX).expect("hex").len(),
            24,
            "24 bytes means AES-192, not AES-128 on a truncated key"
        );
    }

    #[test]
    fn cfb_round_trips_across_a_block_boundary() {
        let cipher = cipher();
        let vector = link_vector(&cipher);
        // Longer than one block and not a multiple of it, which is where a feedback mistake
        // would show.
        let plaintext = b"https://example.test/a-rather-long-file-name-here.bin";

        let ciphertext = encrypt_cfb128(&cipher, vector, plaintext);
        let decrypted = decrypt_cfb128(&cipher, vector, &ciphertext);

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn every_link_of_a_container_is_read() {
        let container = container_of(&[
            "https://example.test/a.bin",
            "CCF: https://example.test/b.bin",
        ]);

        let document = decode_rsdf(&container).expect("decode");

        let links: Vec<String> = document.packages[0]
            .files
            .iter()
            .map(|file| file.url.to_string())
            .collect();
        assert_eq!(
            links,
            ["https://example.test/a.bin", "https://example.test/b.bin"],
            "the format prefix is not part of the address"
        );
        assert_eq!(document.packages[0].name, None);
    }

    #[test]
    fn whitespace_in_the_hex_body_is_tolerated() {
        let container = container_of(&["https://example.test/a.bin"]);
        let spaced = String::from_utf8(container)
            .expect("ascii")
            .as_bytes()
            .chunks(8)
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(decode_rsdf(spaced.as_bytes()).is_ok());
    }

    #[test]
    fn a_file_that_is_not_hex_is_refused_before_anything_else() {
        let error = decode_rsdf(b"this is not a container").expect_err("refused");
        assert!(format!("{error:#}").contains("hex"), "{error:#}");
    }

    #[test]
    fn junk_that_decrypts_to_nonsense_is_refused_rather_than_imported() {
        // Valid hex and valid base64, but not encrypted with the key: the plaintext is bytes,
        // not an address. Importing that would produce a package full of nonsense.
        let body = BASE64.encode([0x11_u8; 32]);
        let container = hex::encode(body).into_bytes();

        let error = decode_rsdf(&container).expect_err("refused");
        assert!(format!("{error:#}").contains("address"), "{error:#}");
    }

    /// RD-109-39: the refusal of a container line names no part of that line.
    ///
    /// `container_handlers` answers with `format!("{error:#}")`, so anything this context
    /// names travels back over REST and into every log that keeps the answer. A container line
    /// is a caller-supplied address, and the one that fails to parse is the one most likely to
    /// be a mistyped protected share with its password behind the hash.
    #[test]
    fn a_line_that_is_not_an_address_is_not_quoted_back() {
        let container = container_of(&["cloud.exmaple.org/s/QxT7bK2mNp9wZr4#s3cret"]);

        let error = decode_rsdf(&container).expect_err("refused");
        let shown = format!("{error:#}");
        assert!(shown.contains("address"), "{shown}");
        assert!(
            !shown.contains("s3cret"),
            "the refusal carries the password: {shown}"
        );
        assert!(
            !shown.contains("cloud.exmaple.org") && !shown.contains("QxT7bK2mNp9wZr4"),
            "the refusal carries the address: {shown}"
        );
    }

    #[test]
    fn an_empty_container_says_so() {
        let error = decode_rsdf(b"").expect_err("refused");
        assert!(format!("{error:#}").contains("empty"), "{error:#}");
    }
}
