//! Builds and signs `.rdplug` archives (manifest + component + Ed25519 signature).

use std::io::{Cursor, Write};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{
    Signer, SigningKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey, spki::der::pem::LineEnding},
};
use rand::RngCore;

use crate::{
    PluginManifest, PluginVerifier, SandboxEngine, VerifyError, manifest::validate_manifest,
    package_digest, validate_component, validate_locales,
};

/// Freshly generated signing key with its PEM and base64 public representation.
pub struct GeneratedKey {
    pub signing_key: SigningKey,
    pub private_pem: String,
    pub public_base64: String,
}

/// Generates an Ed25519 key pair for plugin releases.
#[must_use]
pub fn generate_signing_key() -> GeneratedKey {
    let mut seed = [0_u8; 32];
    rand::rng().fill_bytes(&mut seed);
    let signing_key = SigningKey::from_bytes(&seed);
    let private_pem = signing_key
        .to_pkcs8_pem(LineEnding::LF)
        .expect("Ed25519 keys always encode as PKCS#8")
        .to_string();
    let public_base64 = STANDARD.encode(signing_key.verifying_key().to_bytes());
    GeneratedKey {
        signing_key,
        private_pem,
        public_base64,
    }
}

/// Parses a PKCS#8 PEM private key as written by `generate_signing_key`.
pub fn load_signing_key_pem(pem: &str) -> Result<SigningKey> {
    SigningKey::from_pkcs8_pem(pem.trim()).context("parse Ed25519 PKCS#8 private key")
}

/// Base64 public key of a signing key (for `--trusted-plugin-key KEY_ID=…`).
#[must_use]
pub fn public_key_base64(key: &SigningKey) -> String {
    STANDARD.encode(key.verifying_key().to_bytes())
}

/// Validates manifest, component and locales, signs the package digest (unless `key` is
/// `None`, which produces a development-only unsigned archive) and re-verifies the result.
///
/// `locales` holds `(language, raw JSON)` pairs; they are covered by the signature, so a
/// translation cannot be swapped after release.
pub fn package_plugin(
    manifest_bytes: &[u8],
    component: &[u8],
    locales: &[(String, Vec<u8>)],
    key: Option<&SigningKey>,
) -> Result<Vec<u8>> {
    let manifest: PluginManifest =
        toml::from_slice(manifest_bytes).context("parse plugin manifest")?;
    validate_manifest(&manifest)?;
    validate_locales(manifest.message_slug(), locales)?;
    validate_component(component)?;
    SandboxEngine::new(manifest.limits)?
        .compile_component(component, &manifest)
        .context("compile and validate plugin component imports")?;
    if key.is_some() && manifest.key_id.trim().is_empty() {
        bail!("manifest key_id must name the signing key");
    }
    if let Some(key) = key
        && manifest.verifying_key()? != key.verifying_key()
    {
        bail!("manifest public_key does not match the signing key");
    }
    let signature = key.map(|key| {
        STANDARD.encode(
            key.sign(&package_digest(manifest_bytes, component, locales))
                .to_bytes(),
        )
    });
    let mut sorted: Vec<&(String, Vec<u8>)> = locales.iter().collect();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));

    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.start_file("manifest.toml", options)?;
    writer.write_all(manifest_bytes)?;
    writer.start_file("component.wasm", options)?;
    writer.write_all(component)?;
    if let Some(signature) = &signature {
        writer.start_file("signature.ed25519", options)?;
        writer.write_all(signature.as_bytes())?;
    }
    for (language, bytes) in sorted {
        writer.start_file(format!("locales/{language}.json"), options)?;
        writer.write_all(bytes)?;
    }
    let archive = writer.finish()?.into_inner();
    // Round trip through the verifier so a broken package never leaves the packager.
    let verifier = PluginVerifier::new(key.is_none());
    if let Some(key) = key {
        verifier.trust_key(manifest.key_id.clone(), key.verifying_key())?;
    }
    verifier
        .verify_bytes(&archive)
        .map_err(|error| match error {
            VerifyError::Other(error) => error.context("packaged plugin failed verification"),
            other => anyhow::anyhow!("packaged plugin failed verification: {other}"),
        })?;
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::{generate_signing_key, load_signing_key_pem, package_plugin, public_key_base64};
    use crate::{PluginVerifier, VerifyError};

    const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";

    fn manifest(key_id: &str, public_key: &str) -> Vec<u8> {
        format!(
            r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-00000000abcd"
name = "Fixture"
version = "1.2.3"
key_id = "{key_id}"
public_key = "{public_key}"
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "A fixture resolver"
author = "Fixture Author"

[provider]
slug = "fixture"
kind = "hoster"
credentials = "api_key"
"#
        )
        .into_bytes()
    }

    #[test]
    fn signed_package_round_trips_and_tampering_is_detected() {
        let generated = generate_signing_key();
        let reloaded = load_signing_key_pem(&generated.private_pem).expect("PEM round trip");
        assert_eq!(reloaded.to_bytes(), generated.signing_key.to_bytes());
        let manifest = manifest("release", &generated.public_base64);
        let archive =
            package_plugin(&manifest, EMPTY_COMPONENT, &[], Some(&reloaded)).expect("package");
        let verifier = PluginVerifier::new(false);
        verifier
            .trust_key_base64("release".to_owned(), &generated.public_base64)
            .expect("trust");
        assert!(verifier.verify_bytes(&archive).is_ok());
        assert!(
            matches!(
                PluginVerifier::new(false).verify_bytes(&archive),
                Err(VerifyError::UntrustedKey { .. })
            ),
            "an unknown key id must surface as a trust decision"
        );
        // Swapping signed content while keeping the signature must be rejected.
        let edited = String::from_utf8(manifest.clone())
            .expect("manifest is UTF-8")
            .replace("A fixture resolver", "A tampered resolver");
        let tampered = repack_with_manifest(&archive, edited.as_bytes());
        assert!(
            verifier.verify_bytes(&tampered).is_err(),
            "a manifest edited after signing must not verify"
        );
    }

    /// Rebuilds a `.rdplug` with a different manifest, keeping every other member as signed.
    fn repack_with_manifest(archive: &[u8], manifest: &[u8]) -> Vec<u8> {
        use std::io::{Cursor, Read, Write};

        let mut source = zip::ZipArchive::new(Cursor::new(archive)).expect("read archive");
        let names: Vec<String> = source.file_names().map(str::to_owned).collect();
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for name in names {
            let mut bytes = Vec::new();
            source
                .by_name(&name)
                .expect("member")
                .read_to_end(&mut bytes)
                .expect("read member");
            writer.start_file(&name, options).expect("member");
            let payload = if name == "manifest.toml" {
                manifest
            } else {
                &bytes
            };
            writer.write_all(payload).expect("write member");
        }
        writer.finish().expect("archive").into_inner()
    }

    #[test]
    fn manifest_public_key_must_match_the_signing_key() {
        let generated = generate_signing_key();
        let other = generate_signing_key();
        let manifest = manifest("release", &other.public_base64);
        let error = package_plugin(
            &manifest,
            EMPTY_COMPONENT,
            &[],
            Some(&generated.signing_key),
        )
        .expect_err("mismatch is refused");
        assert!(error.to_string().contains("public_key"));
    }

    #[test]
    fn locales_are_packaged_and_signed() {
        let generated = generate_signing_key();
        let manifest = manifest("release", &generated.public_base64);
        let locales = vec![
            (
                "en".to_owned(),
                br#"{"name":"Fixture","codes":{"fixture.oops":"Oops"}}"#.to_vec(),
            ),
            ("de".to_owned(), br#"{"name":"Fixture DE"}"#.to_vec()),
        ];
        let archive = package_plugin(
            &manifest,
            EMPTY_COMPONENT,
            &locales,
            Some(&generated.signing_key),
        )
        .expect("package");
        let verifier = PluginVerifier::new(false);
        verifier
            .trust_key_base64("release".to_owned(), &generated.public_base64)
            .expect("trust");
        let package = verifier.verify_bytes(&archive).expect("verify");
        assert_eq!(package.locales.len(), 2);
        assert_eq!(package.locales[0].0, "de", "locales are stored sorted");
        assert_eq!(
            public_key_base64(&generated.signing_key),
            generated.public_base64
        );
    }

    #[test]
    fn a_plugin_cannot_ship_only_a_non_english_locale() {
        let generated = generate_signing_key();
        let manifest = manifest("release", &generated.public_base64);
        let locales = vec![("de".to_owned(), br#"{"name":"Fixture DE"}"#.to_vec())];
        assert!(
            package_plugin(
                &manifest,
                EMPTY_COMPONENT,
                &locales,
                Some(&generated.signing_key)
            )
            .is_err()
        );
    }

    /// The component is the only unbounded member, and `install_bytes` takes up to 64 MiB
    /// of it from anyone who can reach the REST path. Validating it before the signature fed
    /// all of that to wasmparser pre-authentication, so an unsigned package has to be refused
    /// for what it is missing, never for the shape of bytes nobody should have parsed yet.
    #[test]
    fn an_unsigned_package_is_refused_before_its_component_is_parsed() {
        let generated = generate_signing_key();
        let manifest = manifest("release", &generated.public_base64);
        let Err(error) = PluginVerifier::new(false).verify_parts(
            manifest,
            b"this is not a WebAssembly component".to_vec(),
            None,
            Vec::new(),
        ) else {
            panic!("an unsigned package must not verify");
        };
        assert!(
            error
                .to_string()
                .contains("unsigned plugins require development mode"),
            "the signature must decide first, got: {error}"
        );
    }

    /// The same order seen from the other side: a package that proves possession of a key
    /// nobody trusts becomes a trust decision with its component still unparsed. Whether
    /// those bytes are a component at all is asked once the key is trusted, not before.
    #[test]
    fn an_untrusted_key_surfaces_before_the_component_is_parsed() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use ed25519_dalek::Signer;

        let generated = generate_signing_key();
        let manifest = manifest("release", &generated.public_base64);
        let component = b"this is not a WebAssembly component".to_vec();
        let signature = STANDARD.encode(
            generated
                .signing_key
                .sign(&crate::package_digest(&manifest, &component, &[]))
                .to_bytes(),
        );
        let Err(error) = PluginVerifier::new(false).verify_parts(
            manifest,
            component,
            Some(signature.into_bytes()),
            Vec::new(),
        ) else {
            panic!("an unknown key must not verify");
        };
        assert!(
            matches!(error, VerifyError::UntrustedKey { .. }),
            "expected a trust decision, got: {error}"
        );
    }

    #[test]
    fn unsigned_development_package_needs_development_mode() {
        let generated = generate_signing_key();
        let manifest = manifest("release", &generated.public_base64);
        let archive = package_plugin(&manifest, EMPTY_COMPONENT, &[], None).expect("package");
        assert!(PluginVerifier::new(false).verify_bytes(&archive).is_err());
        assert!(PluginVerifier::new(true).verify_bytes(&archive).is_ok());
    }
}
