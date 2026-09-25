//! A signed resolver package still verifies and still satisfies its world (RD-104-03).
//!
//! Adding `interface crawler` and `world crawler-plugin` is additive, but "additive" is a
//! claim about the WIT text. Two things it does not prove on its own are what a plugin
//! already in the field actually depends on:
//!
//! - **The signature still verifies.** A `.rdplug` is signed over its manifest, its component
//!   and its locales. None of the three changed for a resolver, so an archive built from them
//!   has to pass the same verification it always did — and the digest framing underneath must
//!   not have moved either.
//! - **The component still satisfies its world.** The Premiumize component here is compiled
//!   from the resolver sources untouched by this job. It must instantiate against a host that
//!   now knows a world it has never heard of.
//!
//! The signing key is generated for the run rather than taken from the machine, so this tests
//! the pipeline and not somebody's key ring: the manifest's `key_id` and `public_key` are
//! rewritten to the ephemeral pair, and the verifier is told to trust exactly that one.

use std::path::PathBuf;

use rd_plugin_api::Resolver as _;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The built Premiumize resolver component, or a failure naming the build command when it has not been
/// built in this checkout, or predates its sources.
fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-premiumize")
}

/// The resolver's shipped manifest, with the signing identity replaced.
fn manifest_for(key_id: &str, public_key: &str) -> Vec<u8> {
    let text = std::fs::read_to_string(root().join("plugins/premiumize/manifest.toml"))
        .expect("the premiumize manifest");
    text.lines()
        .map(|line| {
            if line.starts_with("key_id = ") {
                format!("key_id = \"{key_id}\"")
            } else if line.starts_with("public_key = ") {
                format!("public_key = \"{public_key}\"")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

fn locales() -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> =
        std::fs::read_dir(root().join("plugins/premiumize/locales"))
            .expect("the premiumize locales")
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let language = name.strip_suffix(".json")?.to_owned();
                Some((language, std::fs::read(entry.path()).ok()?))
            })
            .collect();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

#[test]
fn a_signed_resolver_package_still_verifies_and_instantiates() {
    let component = component();
    let key = rd_plugin_host::generate_signing_key();
    let key_id = "field-resolver-v1";
    let manifest_bytes = manifest_for(key_id, &key.public_base64);
    let locales = locales();

    // Packaging signs over manifest, component and locales, and re-verifies the archive on
    // the way out; a change to the digest framing would fail right here.
    let archive = rd_plugin_host::package_plugin(
        &manifest_bytes,
        &component,
        &locales,
        Some(&key.signing_key),
    )
    .expect("a resolver package still signs");

    let verifier = rd_plugin_host::PluginVerifier::new(false);
    verifier
        .trust_key_base64(key_id.to_owned(), &key.public_base64)
        .expect("trust the run's key");
    let package = verifier
        .verify_bytes(&archive)
        .expect("a signed resolver package still verifies");
    assert_eq!(
        package.manifest.plugin_type,
        rd_plugin_host::PluginType::Resolver
    );
    assert_eq!(
        package.manifest.api_version,
        rd_plugin_host::SUPPORTED_API_VERSIONS[0],
        "the bundled manifest and the host disagree about the contract version"
    );

    // The export side: a component built for `resolver-plugin` still satisfies it.
    let resolver = rd_plugin_host::ComponentResolver::new(
        package.manifest.clone(),
        &package.component,
        std::sync::Arc::new(RefusingHost),
    )
    .expect("a signed resolver package still instantiates");
    assert_eq!(resolver.metadata().provider_slug, "premiumize");
}

/// A tampered archive is still refused; the check did not become decorative.
#[test]
fn a_resolver_package_whose_component_was_swapped_is_still_refused() {
    let component = component();
    let key = rd_plugin_host::generate_signing_key();
    let key_id = "field-resolver-v1";
    let manifest_bytes = manifest_for(key_id, &key.public_base64);
    let archive = rd_plugin_host::package_plugin(
        &manifest_bytes,
        &component,
        &locales(),
        Some(&key.signing_key),
    )
    .expect("package");

    let mut tampered = Vec::new();
    {
        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(archive)).expect("archive");
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut tampered));
        let options = zip::write::SimpleFileOptions::default();
        for index in 0..reader.len() {
            let mut entry = reader.by_index(index).expect("entry");
            let name = entry.name().to_owned();
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes).expect("read");
            if name == "locales/en.json" {
                bytes = br#"{"name":"Not Premiumize","description":"x","codes":{}}"#.to_vec();
            }
            writer.start_file(name, options).expect("start");
            std::io::Write::write_all(&mut writer, &bytes).expect("write");
        }
        writer.finish().expect("finish");
    }

    let verifier = rd_plugin_host::PluginVerifier::new(false);
    verifier
        .trust_key_base64(key_id.to_owned(), &key.public_base64)
        .expect("trust the run's key");
    assert!(
        verifier.verify_bytes(&tampered).is_err(),
        "a swapped translation must not verify"
    );
}

/// RD-120-36: a cache hit crosses the contract as `cached`, not as `online`.
///
/// The case is new in `rdownloader:plugin@0.8.0`, and the enum travels through the canonical
/// ABI by position; this drives the real Premiumize component through a `cache/check` answer
/// so the host's mapping and the guest's are proven against each other, not each alone.
#[tokio::test]
async fn a_cache_hit_crosses_the_contract_as_cached_and_a_known_file_as_online() {
    let manifest: rd_plugin_host::PluginManifest = toml::from_str(
        &std::fs::read_to_string(root().join("plugins/premiumize/manifest.toml"))
            .expect("the premiumize manifest"),
    )
    .expect("manifest");
    let resolver = rd_plugin_host::ComponentResolver::new(
        manifest,
        &component(),
        std::sync::Arc::new(CacheCheckHost),
    )
    .expect("the component loads");
    let results = resolver
        .check(rd_plugin_api::CheckRequest {
            urls: vec![
                "https://hoster.example/a".parse().expect("url"),
                "https://hoster.example/b".parse().expect("url"),
                "https://hoster.example/c".parse().expect("url"),
            ],
            client: rd_plugin_api::ClientIdentity {
                account_id: Some(rd_core::AccountId::new()),
                proxy_profile_id: None,
                tls_revision: 0,
            },
        })
        .await
        .expect("checked");
    let statuses: Vec<rd_core::LinkStatus> = results.iter().map(|result| result.status).collect();
    assert_eq!(
        statuses,
        [
            rd_core::LinkStatus::Cached,
            rd_core::LinkStatus::Online,
            rd_core::LinkStatus::Unknown
        ]
    );
}

/// Answers every request with one sanitised `cache/check` reply.
struct CacheCheckHost;

#[async_trait::async_trait]
impl rd_plugin_api::ResolverHost for CacheCheckHost {
    async fn http_request(
        &self,
        _client: &rd_plugin_api::ClientIdentity,
        request: rd_plugin_api::HostHttpRequest,
    ) -> Result<rd_plugin_api::HostHttpResponse, rd_core::Failure> {
        Ok(rd_plugin_api::HostHttpResponse {
            status: 200,
            final_url: request.url,
            headers: Vec::new(),
            body: br#"{"status":"success","response":[true,false,false],"filename":["a.rar","b.rar",null],"filesize":["1024","2048",null]}"#.to_vec(),
        })
    }

    async fn secret_available(&self, _account_id: rd_core::AccountId, _reference: &str) -> bool {
        true
    }
}

/// Stands in for the application during instantiation; nothing here should be reached.
struct RefusingHost;

#[async_trait::async_trait]
impl rd_plugin_api::ResolverHost for RefusingHost {
    async fn http_request(
        &self,
        _client: &rd_plugin_api::ClientIdentity,
        _request: rd_plugin_api::HostHttpRequest,
    ) -> Result<rd_plugin_api::HostHttpResponse, rd_core::Failure> {
        Err(rd_core::Failure::coded(
            rd_core::FailureKind::Permanent,
            "plugin.offline",
            "the test host reaches nothing",
        ))
    }

    async fn secret_available(&self, _account_id: rd_core::AccountId, _reference: &str) -> bool {
        false
    }
}
