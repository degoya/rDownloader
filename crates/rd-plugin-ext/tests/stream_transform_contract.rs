//! The twelfth world, driven as a real WebAssembly component (RD-110-33, ADR 0011).
//!
//! `plugins/example-stream-transform/` is built by
//! `cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-example-stream-transform`
//! and by the `components` job in CI. It describes transforms and fetches nothing, which is
//! exactly the shape of the contract: the plugin describes, the host computes.
//!
//! What is asserted here is the boundary, not the arithmetic -- `crates/rd-http/tests/` holds
//! the fixed vectors. Here: that the description survives the crossing unchanged, that the key
//! arrives as key material and never as part of the description, that every primitive this
//! build does not implement is refused at that crossing with a stable code, and that a plugin
//! written against the contract *without* this world still satisfies the world it declares.

use rd_core::{
    CIPHER_AES_128_CTR, CODE_CIPHER_UNKNOWN, CODE_INTEGRITY_UNKNOWN, CODE_PARAMETERS_INVALID,
    FailureKind, INTEGRITY_CBC_MAC_CHAIN,
};
use rd_plugin_api::{ClientIdentity, ResolveRequest};
use rd_plugin_host::{
    PluginManifest, PluginType,
    extension::{StreamTransformProvider, TransformedDownload},
};

const MANIFEST: &str = include_str!("../../../plugins/example-stream-transform/manifest.toml");
const HOST: &str = "https://transform.example.invalid";

fn manifest() -> PluginManifest {
    toml::from_str(MANIFEST).expect("the plugin manifest")
}

/// The plugin component, or a failure naming the build command when it has not been built.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-example-stream-transform")
}

fn provider() -> StreamTransformProvider {
    let bytes = component();
    StreamTransformProvider::new(manifest(), &bytes, None)
        .expect("the component exports the world its type declares")
}

fn identity() -> ClientIdentity {
    ClientIdentity {
        account_id: None,
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

async fn resolve(path: &str) -> Result<TransformedDownload, rd_core::Failure> {
    let request = ResolveRequest {
        url: format!("{HOST}{path}").parse().expect("url"),
        client: identity(),
    };
    provider()
        .resolve(&request)
        .await
        .expect("the guest answered")
}

/// The manifest names the twelfth type, and the type is what picks the world.
#[test]
fn the_manifest_declares_the_new_type_and_one_reachable_host() {
    let manifest = manifest();
    assert_eq!(manifest.plugin_type, PluginType::StreamTransform);
    assert_eq!(manifest.plugin_type.as_str(), "stream-transform");
    assert_eq!(
        manifest.capabilities.domains(),
        ["transform.example.invalid".to_owned()].as_slice()
    );
    assert!(manifest.capabilities.net_stream.is_none());
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    assert_eq!(manifest.api_version, "0.9.0");
}

/// A component built against this contract satisfies the world its type declares.
#[test]
fn the_component_satisfies_the_world_its_type_declares() {
    let bytes = component();
    StreamTransformProvider::new(manifest(), &bytes, None).expect("the world is satisfied");
}

/// The claim is answered from the address alone; nothing is fetched to decide it.
#[tokio::test]
async fn the_plugin_claims_only_the_addresses_it_owns() {
    let provider = provider();
    assert!(
        provider
            .claims(&format!("{HOST}/file/known"))
            .await
            .expect("claims")
    );
    assert!(
        !provider
            .claims("https://cdn.example.org/a/b.bin")
            .await
            .expect("claims")
    );
    assert!(
        !provider
            .claims(&format!("{HOST}/nothing"))
            .await
            .expect("claims")
    );
}

/// The description crosses the boundary intact, and the key crosses it separately.
#[tokio::test]
async fn a_full_description_arrives_with_its_key_held_apart_from_it() {
    let answer = resolve("/file/known").await.expect("a known primitive");
    assert_eq!(answer.download.url.as_str(), format!("{HOST}/file/known"));
    assert_eq!(answer.download.file_name.as_deref(), Some("example.bin"));

    let cipher = &answer.transform.cipher;
    assert_eq!(cipher.algorithm, CIPHER_AES_128_CTR);
    assert_eq!(cipher.nonce.len(), 8);
    assert_eq!(cipher.first_block, 0);
    // The description is what gets written down, and it carries no key. The reference is
    // filled in by the caller once the bytes are in the vault.
    assert!(cipher.key_reference.is_none());

    let integrity = answer
        .transform
        .integrity
        .as_ref()
        .expect("an integrity value");
    assert_eq!(integrity.algorithm, INTEGRITY_CBC_MAC_CHAIN);
    assert_eq!(integrity.boundaries, vec![131_072, 393_216, 524_288]);
    assert_eq!(integrity.iv.len(), 16);
    assert_eq!(integrity.expected.len(), 8);

    // The key arrived as key material, and nothing that can be printed or serialised holds it.
    assert_eq!(answer.key.len(), 16);
    let key_hex: String = answer
        .key
        .expose()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let printed = format!("{:?}", answer.transform);
    assert!(!printed.contains(&key_hex), "{printed}");
    let written = serde_json::to_string(&answer.transform).expect("json");
    assert!(!written.contains(&key_hex), "{written}");
    assert!(format!("{:?}", answer.key).contains("[redacted]"));
}

/// A provider that publishes nothing to check against is described without an integrity value.
#[tokio::test]
async fn a_description_without_an_integrity_value_is_accepted() {
    let answer = resolve("/file/plain").await.expect("a cipher alone");
    assert_eq!(answer.transform.cipher.algorithm, CIPHER_AES_128_CTR);
    assert!(answer.transform.integrity.is_none());
}

/// A primitive this build does not implement is a refusal, not a fallback.
#[tokio::test]
async fn an_unknown_primitive_is_refused_with_a_stable_code() {
    for (path, code) in [
        ("/file/unknown-cipher", CODE_CIPHER_UNKNOWN),
        ("/file/unknown-integrity", CODE_INTEGRITY_UNKNOWN),
        ("/file/bad-nonce", CODE_PARAMETERS_INVALID),
    ] {
        let failure = resolve(path).await.expect_err("refused");
        assert_eq!(failure.code.as_deref(), Some(code), "{path}");
        assert!(matches!(failure.category, FailureKind::Permanent), "{path}");
    }
}

/// A plugin that claimed an address and found it was not its own says so, and says it in a
/// way the selection can act on rather than ending the link.
#[tokio::test]
async fn an_address_the_plugin_does_not_own_comes_back_as_unsupported() {
    let failure = resolve("/somebody/elses/file").await.expect_err("refused");
    assert!(matches!(failure.category, FailureKind::Unsupported));
    assert_eq!(
        failure.code.as_deref(),
        Some("example_stream_transform.not_mine")
    );
}

/// Additive means additive: a plugin built against the contract without this world still
/// builds and still satisfies the world it declares.
///
/// `example-oauth` is the check because it is the other reference plugin in the tree and it
/// is rebuilt from the same WIT: if the twelfth interface had changed anything an existing
/// world names, this instantiation would be where it showed.
#[test]
fn a_plugin_that_does_not_use_the_new_world_is_unaffected() {
    let manifest: PluginManifest =
        toml::from_str(include_str!("../../../plugins/example-oauth/manifest.toml"))
            .expect("the manifest");
    assert_eq!(manifest.plugin_type, PluginType::OAuth);
    let bytes = rd_plugin_host::artifact::component("rd-plugin-example-oauth");
    rd_plugin_host::extension::instantiate(&manifest, &bytes)
        .expect("a world that predates the twelfth still instantiates");
}

/// The canonical contract and every SDK copy of it are the same bytes.
///
/// The twelfth world was additive and left the package version alone; RD-120-20 moved it to
/// `0.7.0`, RD-120-36 to `0.8.0` and RD-130-11 to `0.9.0`, each for a reason of its own
/// (`docs/plugins.md`, "What moves `api_version`"). What this
/// test has always been about is the other half and still is: **eleven copies, one text**. CI
/// diffs them too, and this is the same question asked where a plugin author would notice.
#[test]
fn the_contract_is_copied_verbatim_into_every_template() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let canonical = std::fs::read_to_string(root.join("crates/rd-plugin-api/wit/rdownloader.wit"))
        .expect("wit");
    assert!(canonical.starts_with("package rdownloader:plugin@0.9.0;"));
    assert!(canonical.contains("interface stream-transform {"));
    assert!(canonical.contains("world stream-transform-plugin {"));
    let mut copies = 0;
    for entry in std::fs::read_dir(root.join("sdk/templates")).expect("templates") {
        let path = entry
            .expect("a template")
            .path()
            .join("wit/rdownloader.wit");
        if !path.exists() {
            continue;
        }
        assert_eq!(
            std::fs::read_to_string(&path).expect("a copy"),
            canonical,
            "{} drifted from the canonical contract",
            path.display()
        );
        copies += 1;
    }
    assert_eq!(copies, 11, "every template carries a copy");
}
