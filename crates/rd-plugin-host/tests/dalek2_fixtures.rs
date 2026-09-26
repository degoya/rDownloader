//! Plugin packages and signing keys from the `ed25519-dalek` 2 era still work (RD-140-12).
//!
//! - **A released package verifies.** The archive is the reference stream transform exactly as
//!   release 1.3 signed it with the plugin release key. Only the trust path is exercised — the
//!   digest framing and the Ed25519 signature against the compiled-in root — not the component
//!   compile, which a frozen component would fail the first time the WIT contract moves.
//! - **Both PKCS#8 layouts load.** The keys in `~/.config/rdownloader/` come in two shapes:
//!   version 2 with the public key embedded, as `generate_signing_key` wrote it, and version 1
//!   without, as `openssl genpkey` writes it. The fixtures are throw-away keys from fixed seeds
//!   in exactly those two layouts. Ed25519 signing is deterministic, so the signatures pinned
//!   below are what the old version produced and the new one has to reproduce byte for byte.

use std::io::{Cursor, Read};

use rd_plugin_host::{
    format_package_digest, load_signing_key_pem, package_digest, public_key_base64,
};
use rd_sign::{EMBEDDED_KEYS, PLUGIN_RELEASE_KEY_ID, VerifyingKey};
use sha2::{Digest, Sha256};

const PACKAGE: &[u8] = include_bytes!("fixtures/dalek2/example-stream-transform-0.1.3.rdplug");

/// The content digest of that package, pinned: a moved framing fails here by name.
const PACKAGE_DIGEST: &str = "066f032438a8292b0a78f05c3f4bb4fab7844ccca540421a4aa436273c87db4b";

type Archive<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

struct Parts {
    manifest: Vec<u8>,
    component: Vec<u8>,
    signature: Vec<u8>,
    locales: Vec<(String, Vec<u8>)>,
}

fn member(archive: &mut Archive<'_>, name: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    archive
        .by_name(name)
        .unwrap_or_else(|error| panic!("{name}: {error}"))
        .read_to_end(&mut bytes)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    bytes
}

fn parts() -> Parts {
    let mut archive = Archive::new(Cursor::new(PACKAGE)).expect("the fixture is a zip archive");
    let languages: Vec<String> = archive
        .file_names()
        .filter_map(|name| name.strip_prefix("locales/")?.strip_suffix(".json"))
        .map(str::to_owned)
        .collect();
    assert!(!languages.is_empty(), "the fixture carries its locales");
    let locales = languages
        .into_iter()
        .map(|language| {
            let bytes = member(&mut archive, &format!("locales/{language}.json"));
            (language, bytes)
        })
        .collect();
    Parts {
        manifest: member(&mut archive, "manifest.toml"),
        component: member(&mut archive, "component.wasm"),
        signature: member(&mut archive, "signature.ed25519"),
        locales,
    }
}

fn release_key() -> VerifyingKey {
    let root = EMBEDDED_KEYS
        .iter()
        .find(|key| key.key_id == PLUGIN_RELEASE_KEY_ID)
        .expect("the plugin release root is compiled in");
    rd_sign::decode_public_key(root.public_key).expect("the release root decodes")
}

#[test]
fn a_package_signed_by_ed25519_dalek_2_still_verifies() {
    let parts = parts();
    let digest = package_digest(&parts.manifest, &parts.component, &parts.locales);
    assert_eq!(format_package_digest(&digest), PACKAGE_DIGEST);
    rd_sign::verify_detached_bytes(&release_key(), &digest, &parts.signature)
        .expect("the release 1.3 package verifies against the release root");
}

/// The test above is not vacuous: one changed locale byte and the signature no longer holds.
#[test]
fn the_same_package_with_one_locale_byte_changed_does_not_verify() {
    let mut parts = parts();
    parts.locales[0].1.push(b' ');
    let digest = package_digest(&parts.manifest, &parts.component, &parts.locales);
    assert!(rd_sign::verify_detached_bytes(&release_key(), &digest, &parts.signature).is_err());
}

fn assert_key_loads(pem: &str, public_key: &str, signature: &str) {
    let key = load_signing_key_pem(pem).expect("the PKCS#8 key loads");
    assert_eq!(public_key_base64(&key), public_key);
    let digest: [u8; 32] = Sha256::digest(b"RD-140-12").into();
    assert_eq!(rd_sign::sign_detached(&key, &digest), signature);
}

#[test]
fn a_pkcs8_v2_key_with_its_public_key_still_loads_and_signs_the_same() {
    assert_key_loads(
        include_str!("fixtures/dalek2/test-key-pkcs8-v2.pem"),
        "A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=",
        "hYvIX5P3A/uJLmVgqH09Cjbyi3ocGahwYI2wbkxQm40IXTLRdRfic2wwaGJePwHZCZMuik5YTwf/frxWi6JTBA==",
    );
}

#[test]
fn a_pkcs8_v1_key_without_its_public_key_still_loads_and_signs_the_same() {
    assert_key_loads(
        include_str!("fixtures/dalek2/test-key-pkcs8-v1.pem"),
        "Kay64UG8yvCyLhqU000LxzYeUm0L/hLIl5S8kyKWbdc=",
        "Z8o91/yYWpeYA4l+oRNOTQxt2KqLKRRUWyiZ8WJSfU9susYB0fJ3L5RdGBZARLfFOBKnhSc8vD98tX/CxD5hCA==",
    );
}
