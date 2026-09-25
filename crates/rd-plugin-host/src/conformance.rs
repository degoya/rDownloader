//! What a third party can run against their own package before publishing it.
//!
//! The point is that "it worked on my machine" and "this core will run it" become the same
//! question. Everything checked here is something the host would otherwise only tell the
//! author about through a user's bug report: a manifest field out of bounds, an import the
//! manifest never granted, a world that does not match, a plugin that claims none of its own
//! domains.
//!
//! Deliberately not here: whether the plugin resolves a real link. That needs a hoster, an
//! account and a network, none of which belong in a conformance run — and a plugin that
//! passes every check here can still be wrong about its hoster. The report says what was
//! verified, not that the plugin is good.

use serde::Serialize;

use crate::{PluginType, PluginVerifier};

/// One verified property.
#[derive(Debug, Serialize)]
pub struct ConformanceCheck {
    /// Stable identifier, so a CI job can act on a specific failure.
    pub id: &'static str,
    /// What this check is about, in one line.
    pub about: &'static str,
    pub passed: bool,
    /// Why it failed, or what it found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The result of one conformance run.
#[derive(Debug, Serialize)]
pub struct ConformanceReport {
    pub plugin_id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub plugin_type: Option<String>,
    pub passed: bool,
    pub checks: Vec<ConformanceCheck>,
}

impl ConformanceReport {
    fn push(&mut self, id: &'static str, about: &'static str, result: Result<(), String>) {
        let (passed, detail) = match result {
            Ok(()) => (true, None),
            Err(detail) => (false, Some(detail)),
        };
        self.passed &= passed;
        self.checks.push(ConformanceCheck {
            id,
            about,
            passed,
            detail,
        });
    }
}

/// Checks one `.rdplug` and reports every property separately.
///
/// A failing check never stops the run: an author wants the whole list, not the first thing
/// that went wrong.
pub async fn check_package(bytes: &[u8], verifier: &PluginVerifier) -> ConformanceReport {
    let mut report = ConformanceReport {
        plugin_id: None,
        name: None,
        version: None,
        plugin_type: None,
        passed: true,
        checks: Vec::new(),
    };

    // Verification covers the archive layout, the manifest, the declared ABI version, the
    // locales, the signature, the declared limits and the component's imports in one step,
    // which is exactly what installation does — so this is deliberately one check and not
    // six. Reporting those properties separately afterwards was worse than redundant: they
    // ran on a package that had already passed them, so they always said `passed`, and the
    // import line paid for a second full compile to say it. The `about` text names what is
    // covered, because that is what an author loses when a run stops here.
    const PACKAGE_ABOUT: &str = "The package verifies as it would on install: archive, \
        manifest, ABI version, locales, signature, limits, and every import the manifest grants";
    let package = match verifier.verify_bytes(bytes) {
        Ok(package) => {
            report.push("package", PACKAGE_ABOUT, Ok(()));
            package
        }
        Err(error) => {
            report.push("package", PACKAGE_ABOUT, Err(format!("{error}")));
            return report;
        }
    };
    let manifest = &package.manifest;
    report.plugin_id = Some(manifest.id.to_string());
    report.name = Some(manifest.name.clone());
    report.version = Some(manifest.version.clone());
    report.plugin_type = Some(manifest.plugin_type.as_str().to_owned());

    report.push(
        "locales",
        "Translations exist for every language the package ships",
        if package.locales.is_empty() || package.locales.iter().any(|(lang, _)| lang == "en") {
            Ok(())
        } else {
            Err("a localised plugin must ship locales/en.json".to_owned())
        },
    );

    // Instantiating against the world the type declares is the only way to find out that the
    // exports line up. A component missing an export compiles fine and fails at the first
    // call, which is to say: on a user's download.
    match instantiate(&package).await {
        Ok(resolver) => {
            report.push(
                "world",
                "The component exports the world its plugin type requires",
                Ok(()),
            );
            if let Some(resolver) = resolver {
                report.push(
                    "match_url",
                    "The resolver does not claim links belonging to no hoster it declares",
                    does_not_claim_foreign_links(&resolver, manifest).await,
                );
            }
        }
        Err(detail) => report.push(
            "world",
            "The component exports the world its plugin type requires",
            Err(detail),
        ),
    }
    report
}

/// Builds the package as the core would, returning the resolver when it is one.
async fn instantiate(
    package: &crate::VerifiedPackage,
) -> Result<Option<crate::ComponentResolver>, String> {
    match package.manifest.plugin_type {
        PluginType::Resolver => crate::ComponentResolver::new(
            package.manifest.clone(),
            &package.component,
            std::sync::Arc::new(RefusingHost),
        )
        .map(Some)
        .map_err(|error| format!("{error:#}")),
        PluginType::Transfer => {
            crate::TransferBackend::new(package.manifest.clone(), &package.component)
                .map(|_| None)
                .map_err(|error| format!("{error:#}"))
        }
        // The extension types are built through the shared loader, which links the world
        // their manifest names. Nothing is returned: only a resolver has a behavioural
        // check beyond "it instantiates".
        PluginType::Intake
        | PluginType::Auth
        | PluginType::OAuth
        | PluginType::Crawler
        | PluginType::Enricher
        | PluginType::Notifier
        | PluginType::Postprocess
        | PluginType::Storage
        | PluginType::RemoteJob
        | PluginType::StreamTransform => {
            crate::extension::instantiate(&package.manifest, &package.component)
                .map(|()| None)
                .map_err(|error| format!("{error:#}"))
        }
        PluginType::Unknown(ref other) => Err(format!("unknown plugin type `{other}`")),
    }
}

/// A hoster that claims links belonging to nobody breaks every direct download.
///
/// Only this direction is checked, and deliberately so. The opposite — "does it claim its own
/// links" — cannot be asked without a URL the plugin would accept, and real hosters match on a
/// file code in the path rather than on the host alone: ddownload rejects
/// `https://ddownload.com/f/anything` and is right to. Over-claiming, on the other hand, is
/// checkable from the outside and is the failure that hurts: a resolver answering yes to every
/// URL turns each plain HTTP download into "this hoster needs an account".
///
/// Multihosters are exempt: claiming other people's links on an account's behalf is what they
/// are, which is what `match_domains = ["*"]` says.
async fn does_not_claim_foreign_links(
    resolver: &crate::ComponentResolver,
    manifest: &crate::PluginManifest,
) -> Result<(), String> {
    let multihoster = manifest
        .provider
        .as_ref()
        .is_some_and(|provider| provider.kind == crate::ProviderKindManifest::Multihoster);
    if multihoster {
        return Ok(());
    }
    for foreign in [
        "https://conformance.invalid/some/file.bin",
        "https://cdn.example.org/a/b/c.zip",
    ] {
        match resolver.guest_claims(foreign).await {
            Ok(false) => {}
            Ok(true) => {
                return Err(format!(
                    "the plugin claims {foreign}, which belongs to no hoster it declares"
                ));
            }
            Err(failure) => return Err(format!("match-url failed: {}", failure.message)),
        }
    }
    Ok(())
}

/// Stands in for the host during a conformance run: nothing here should reach the network,
/// and a plugin that tries during `match-url` is doing something it has no business doing.
struct RefusingHost;

#[async_trait::async_trait]
impl rd_plugin_api::ResolverHost for RefusingHost {
    async fn http_request(
        &self,
        _client: &rd_plugin_api::ClientIdentity,
        _request: rd_plugin_api::HostHttpRequest,
    ) -> Result<rd_plugin_api::HostHttpResponse, rd_core::Failure> {
        Err(rd_core::Failure::coded(
            rd_core::FailureKind::Unsupported,
            "conformance.no_network",
            "A conformance run does not reach the network",
        ))
    }

    async fn secret_available(&self, _account_id: rd_core::AccountId, _reference: &str) -> bool {
        false
    }
}
