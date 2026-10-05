//! Signed resolver package validation and atomic installation.

mod account_label;
#[cfg(any(test, feature = "test-support"))]
pub mod artifact;
mod bundled;
mod bundled_services;
mod compile_cache;
mod component;
mod conformance;
mod diagnostics;
mod engine;
pub mod extension;
mod foreign_address;
mod foreign_text;
pub mod index;
mod install;
mod installed;
pub mod keyderive;
mod locales;
mod manifest;
mod native;
mod own_endpoints;
mod packager;
pub mod preview;
mod registry;
pub mod repository;
mod revocation;
mod runtime;
mod session;
mod siterules;
mod transfer;
mod unsigned_notice;
mod verifier;
mod versions;

pub use bundled::{BundledPolicy, BundledSyncReport, sync_bundled};
pub use bundled_services::{
    BundledPackage, BundledService, BundledText, ServiceCategory, group_services,
};
pub use component::ComponentResolver;
pub use conformance::{ConformanceCheck, ConformanceReport, check_package};
pub use diagnostics::{ExecutionLog, ExecutionOutcome, Invocation};
pub use engine::configure_compile_cache;
pub use install::{PluginInstaller, StartedVersions};
pub use installed::IncompatiblePlugin;
pub use locales::{
    MAX_LOCALE_BYTES, MAX_LOCALE_FILES, PluginLocale, PluginLocaleAccount, locale_member_language,
    parse_locale, valid_language, validate_locales,
};
pub use manifest::{
    Capabilities, CredentialKindManifest, MANIFEST_VERSION, ManifestHeader, ManifestRejection,
    NetHttpCapability, OAuthFlowManifest, PluginManifest, PluginMetadata, PluginType,
    ProviderKindManifest, ProviderManifest, SUPPORTED_API_VERSIONS, SecretFilledByManifest,
    SettingManifest, check_app_version, decode_public_key, provider_spec_from_manifest,
};
pub use native::{
    CLIENT_ID_MARKER, ResolverService, client_not_configured, provider_cookie_scope,
    provider_download_authorization, provider_download_bearer,
    provider_download_carries_credential, provider_token_beside_the_flow,
};
pub use own_endpoints::{CLICK_N_LOAD_PORT, OwnEndpoints, entered_address_policy};
pub use packager::{
    GeneratedKey, generate_signing_key, load_signing_key_pem, package_plugin, public_key_base64,
};
pub use rd_plugin_api::{AccountStatus, LabelPart};
pub use registry::PluginTypeRegistry;
pub use revocation::{RevokedDigests, WithdrawnKeys, format_package_digest, parse_package_digest};
pub use runtime::{PluginStoreState, SandboxEngine};
pub use siterules::{RuleCaptcha, RuleFetcher, RuleNetwork, RuleResolver};
pub use transfer::{
    RemoteFile, TransferBackend, TransferJob, TransferOutcome, TransferState, TransferTarget,
};
pub use verifier::{PluginVerifier, package_digest};
pub use versions::{VersionChoice, VersionChoices, VersionRole, default_version};

use std::path::PathBuf;

use verifier::{read_archive, validate_component};

/// Core version a plugin's `metadata.min_app_version` is compared against.
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_COMPONENT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 1024;

const MANIFEST_MEMBER: &str = "manifest.toml";
/// What an install's staging directory beside the version directories is named after.
const INSTALL_STAGING_PREFIX: &str = ".install-";
const COMPONENT_MEMBER: &str = "component.wasm";
const SIGNATURE_MEMBER: &str = "signature.ed25519";

/// Default instruction budget per invocation. Parsing a single ~200 KiB HTML
/// page costs a few million instructions, so the budget is generous; the epoch
/// timeout remains the wall-clock guard.
pub const DEFAULT_FUEL: u64 = 2_000_000_000;

/// Resource limits applied by the component runtime.
#[derive(Clone, Copy, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct PluginLimits {
    pub memory_bytes: u64,
    pub fuel: u64,
    pub timeout_milliseconds: u64,
    pub max_response_bytes: u64,
    /// Total time a resolver may spend in host-side waits (hoster countdowns) per
    /// invocation. Separate from `timeout_milliseconds`, which bounds compute time only.
    #[serde(default = "default_wait_budget")]
    pub wait_budget_milliseconds: u64,
}

/// Free downloads routinely wait a few minutes; ten minutes covers a countdown plus the
/// captcha round trip without letting a broken plugin occupy a slot indefinitely.
const fn default_wait_budget() -> u64 {
    10 * 60 * 1000
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 64 * 1024 * 1024,
            fuel: DEFAULT_FUEL,
            timeout_milliseconds: 15_000,
            max_response_bytes: 8 * 1024 * 1024,
            wait_budget_milliseconds: default_wait_budget(),
        }
    }
}

/// One installed package: where it landed, and the manifest that was installed.
///
/// Callers need the manifest to register the plugin's provider row, so returning it removes
/// any need to guess which of the installed manifests the new path belongs to.
pub struct InstalledPackage {
    pub path: PathBuf,
    pub manifest: PluginManifest,
}

/// Validated archive contents before installation.
pub struct VerifiedPackage {
    pub manifest: PluginManifest,
    pub manifest_bytes: Vec<u8>,
    pub component: Vec<u8>,
    pub signature: Option<Vec<u8>>,
    /// `(language, raw JSON)` pairs, sorted by language, exactly as signed.
    pub locales: Vec<(String, Vec<u8>)>,
}

/// Why a package was refused, separating the recoverable trust decision from hard errors.
#[derive(Debug)]
pub enum VerifyError {
    /// The package is internally consistent and self-signed by a key the user has not
    /// yet trusted. The caller may show the fingerprint and ask for confirmation.
    UntrustedKey {
        key_id: String,
        /// Base64 Ed25519 key the package declares, ready to be recorded on confirmation.
        public_key: String,
        fingerprint: String,
        name: String,
        version: String,
    },
    Other(anyhow::Error),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UntrustedKey {
                key_id,
                fingerprint,
                ..
            } => write!(
                formatter,
                "untrusted signing key {key_id} (fingerprint {fingerprint})"
            ),
            Self::Other(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for VerifyError {}

impl From<anyhow::Error> for VerifyError {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

pub use rd_sign::key_fingerprint;

/// Checks a target and every redirect against an exact or wildcard domain allowlist.
pub fn domain_allowed(url: &url::Url, domains: &[String]) -> bool {
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && domains.iter().any(|domain| {
            domain == "*"
                || rd_core::host_pattern_matches(domain, &host, rd_core::WildcardApex::Excluded)
        })
}

/// Refuses a widget captcha whose page lies outside the plugin's declared domains.
///
/// A widget challenge is the one host call that ends with a *person* looking at a hoster
/// page, so letting a plugin name any page at all would turn `solve-captcha` into a way to
/// put arbitrary content in front of the user. The boundary is the manifest's, the same one
/// `net_http` is held to (RD-107-03).
pub(crate) fn captcha_target_refused() -> rd_core::Failure {
    rd_core::Failure::coded(
        rd_core::FailureKind::Permanent,
        "plugin.captcha_target_not_allowed",
        "Captcha page is outside the plugin's declared domains",
    )
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};

    use super::{
        PluginInstaller, PluginVerifier, VerifyError, domain_allowed, key_fingerprint,
        package_digest,
    };

    pub(crate) const TEST_PUBLIC_KEY: &str = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY=";

    /// A complete v2 manifest for tests; `extra` is appended to the `[provider]` table.
    pub(crate) fn fixture_manifest(public_key: &str) -> String {
        format!(
            r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.10.0"
id = "019d0000-0000-7000-8000-00000000abcd"
name = "Fixture"
version = "1.2.3"
key_id = "fixture-v1"
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
    }

    #[test]
    fn release_signature_payload_is_stable() {
        let signing = SigningKey::from_bytes(&[7_u8; 32]);
        let digest = package_digest(b"manifest", b"component", &[]);
        let signature = signing.sign(&digest);
        assert!(
            signing
                .verifying_key()
                .verify_strict(&digest, &signature)
                .is_ok()
        );
    }

    #[test]
    fn locale_files_are_covered_by_the_digest() {
        let base = package_digest(b"manifest", b"component", &[]);
        let with_locale = package_digest(
            b"manifest",
            b"component",
            &[("en".to_owned(), b"{}".to_vec())],
        );
        assert_ne!(base, with_locale, "locales must change the signed payload");

        // Order of the input slice must not matter; the digest sorts by language.
        let ascending = package_digest(
            b"manifest",
            b"component",
            &[
                ("de".to_owned(), b"{\"a\":1}".to_vec()),
                ("en".to_owned(), b"{\"b\":2}".to_vec()),
            ],
        );
        let descending = package_digest(
            b"manifest",
            b"component",
            &[
                ("en".to_owned(), b"{\"b\":2}".to_vec()),
                ("de".to_owned(), b"{\"a\":1}".to_vec()),
            ],
        );
        assert_eq!(ascending, descending);
    }

    #[test]
    fn fingerprints_are_stable_hex_sha256() {
        let key = SigningKey::from_bytes(&[3_u8; 32]).verifying_key();
        let fingerprint = key_fingerprint(&key);
        assert_eq!(fingerprint.len(), 64);
        assert!(fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(fingerprint, key_fingerprint(&key));
    }

    #[test]
    fn trust_store_is_shared_between_clones() {
        let verifier = PluginVerifier::new(false);
        let clone = verifier.clone();
        assert!(!clone.is_trusted("fixture-v1").expect("read"));
        verifier
            .trust_key_base64("fixture-v1".to_owned(), TEST_PUBLIC_KEY)
            .expect("trust");
        assert!(
            clone.is_trusted("fixture-v1").expect("read"),
            "runtime trust must reach existing clones"
        );
        assert!(clone.revoke_key("fixture-v1").expect("revoke"));
        assert!(!verifier.is_trusted("fixture-v1").expect("read"));
    }

    #[test]
    fn redirects_must_stay_inside_manifest_domains() {
        let domains = vec!["api.premiumize.me".to_owned(), "*.ddownload.com".to_owned()];
        assert!(domain_allowed(
            &"https://api.premiumize.me/api/account/info"
                .parse()
                .expect("valid URL"),
            &domains
        ));
        assert!(domain_allowed(
            &"https://cdn.ddownload.com/file".parse().expect("valid URL"),
            &domains
        ));
        assert!(!domain_allowed(
            &"https://ddownload.com.attacker.invalid/"
                .parse()
                .expect("valid URL"),
            &domains
        ));
    }

    #[tokio::test]
    async fn installed_manifests_are_discovered_without_loading_components() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manifest_bytes = fixture_manifest(TEST_PUBLIC_KEY);
        let manifest: super::PluginManifest =
            toml::from_str(&manifest_bytes).expect("manifest parses");
        let version = directory
            .path()
            .join(manifest.id.to_string())
            .join(&manifest.version);
        std::fs::create_dir_all(&version).expect("plugin directory");
        std::fs::write(version.join("manifest.toml"), &manifest_bytes).expect("manifest file");
        let installer =
            PluginInstaller::new(directory.path().to_owned(), PluginVerifier::new(false));

        let installed = installer.list_installed().await.expect("installed plugins");
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].id, manifest.id);
        assert_eq!(installed[0].version, "1.2.3");
        assert_eq!(installed[0].message_slug(), "fixture");
    }

    #[test]
    fn untrusted_key_is_reported_separately_from_a_broken_package() {
        let error = VerifyError::UntrustedKey {
            key_id: "third-party".to_owned(),
            public_key: TEST_PUBLIC_KEY.to_owned(),
            fingerprint: "ab".repeat(32),
            name: "Fixture".to_owned(),
            version: "1.0.0".to_owned(),
        };
        assert!(error.to_string().contains("third-party"));
        assert!(matches!(error, VerifyError::UntrustedKey { .. }));
    }
}
