//! What a third party can run against their own package before publishing it.
//!
//! The point is that "it worked on my machine" and "this core will run it" become the same
//! question. Everything checked here is something the host would otherwise only tell the
//! author about through a user's bug report: a manifest field out of bounds, an import the
//! manifest never granted, a world that does not match, a translation that lacks codes English
//! has, a resolver or crawler that claims links on hosts it does not declare.
//!
//! Deliberately not here: whether the plugin resolves a real link. That needs a hoster, an
//! account and a network, none of which belong in a conformance run — and a plugin that
//! passes every check here can still be wrong about its hoster. The report says what was
//! verified, not that the plugin is good.

use serde::Serialize;

use crate::{PluginType, PluginVerifier, locales::REQUIRED_LANGUAGES};

/// Addresses on hosts no plugin declares, which a resolver or a crawler must not claim.
const FOREIGN_LINKS: [&str; 2] = [
    "https://conformance.invalid/some/file.bin",
    "https://cdn.example.org/a/b/c.zip",
];

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

    // That English is there when anything is was proven by `package`; what verification does
    // not ask is whether a German, Spanish or French catalogue the package ships is whole. A
    // missing code falls back to English one string at a time, so the person reads two
    // languages mixed in one dialog.
    report.push(
        "locales_required",
        "Every interface language the package ships (de, en, es, fr) carries every code \
         English does",
        required_locales_complete(manifest.message_slug(), &package.locales),
    );

    // Instantiating against the world the type declares is the only way to find out that the
    // exports line up. A component missing an export compiles fine and fails at the first
    // call, which is to say: on a user's download.
    match instantiate(&package).await {
        Ok(claimer) => {
            report.push(
                "world",
                "The component exports the world its plugin type requires",
                Ok(()),
            );
            match claimer {
                Some(Claimer::Resolver(resolver)) => report.push(
                    "match_url",
                    "The resolver does not claim links belonging to no hoster it declares",
                    does_not_claim_foreign_links(&resolver, manifest).await,
                ),
                Some(Claimer::Crawler(crawler)) => report.push(
                    "claims_url",
                    "The crawler does not claim links on hosts it does not declare",
                    claims_no_foreign_link(async |url| {
                        crawler
                            .claims(url)
                            .await
                            .map_err(|error| format!("claims-url failed: {error:#}"))
                    })
                    .await,
                ),
                None => {}
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

/// Whether every interface language in `locales` carries each code `en` does.
///
/// Only the languages the interface requires (`web/src/locales/languages.json`): one still in
/// progress may carry a subset, as a bundled plugin's may. A language the package does not
/// ship at all is the documented fallback to English, not a gap.
fn required_locales_complete(slug: &str, locales: &[(String, Vec<u8>)]) -> Result<(), String> {
    let parse = |language: &str| {
        locales
            .iter()
            .find(|(tag, _)| tag == language)
            .map(|(_, bytes)| {
                crate::parse_locale(slug, language, bytes).map_err(|error| format!("{error:#}"))
            })
            .transpose()
    };
    let Some(english) = parse("en")? else {
        return Ok(());
    };
    let mut gaps = Vec::new();
    for language in REQUIRED_LANGUAGES.into_iter().filter(|tag| *tag != "en") {
        let Some(locale) = parse(language)? else {
            continue;
        };
        let missing: Vec<&str> = english
            .codes
            .keys()
            .filter(|code| !locale.codes.contains_key(*code))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            gaps.push(format!(
                "locales/{language}.json lacks {}",
                missing.join(", ")
            ));
        }
    }
    if gaps.is_empty() {
        Ok(())
    } else {
        Err(gaps.join("; "))
    }
}

/// What a conformance run asks the built component beyond "it instantiates".
enum Claimer {
    // Both boxed: the two components differ in size by hundreds of bytes (clippy large_enum_variant).
    Resolver(Box<crate::ComponentResolver>),
    Crawler(Box<crate::extension::FolderCrawler>),
}

/// Builds the package as the core would, returning it when it is a resolver or a crawler.
async fn instantiate(package: &crate::VerifiedPackage) -> Result<Option<Claimer>, String> {
    match package.manifest.plugin_type {
        PluginType::Resolver => crate::ComponentResolver::new(
            package.manifest.clone(),
            &package.component,
            refusing_host(),
        )
        .map(|resolver| Some(Claimer::Resolver(Box::new(resolver))))
        .map_err(|error| format!("{error:#}")),
        // A crawler answers `claims-url` from the address alone, like a resolver's
        // `match-url`, and the selection asks it before every paste: over-claiming there takes
        // a link away from the plugin it belongs to.
        PluginType::Crawler => crate::extension::FolderCrawler::new(
            package.manifest.clone(),
            &package.component,
            Some(refusing_host()),
        )
        .map(|crawler| Some(Claimer::Crawler(Box::new(crawler))))
        .map_err(|error| format!("{error:#}")),
        PluginType::Transfer => {
            crate::TransferBackend::new(package.manifest.clone(), &package.component)
                .map(|_| None)
                .map_err(|error| format!("{error:#}"))
        }
        // The other extension types are built through the shared loader, which links the
        // world their manifest names. Nothing is returned: they have no behavioural check
        // beyond "it instantiates".
        PluginType::Intake
        | PluginType::Auth
        | PluginType::OAuth
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
    claims_no_foreign_link(async |url| {
        resolver
            .guest_claims(url)
            .await
            .map_err(|failure| format!("match-url failed: {}", failure.message))
    })
    .await
}

/// Asks `claims` about every [`FOREIGN_LINKS`] address; claiming one fails the check.
async fn claims_no_foreign_link(
    claims: impl AsyncFn(&'static str) -> Result<bool, String>,
) -> Result<(), String> {
    for foreign in FOREIGN_LINKS {
        if claims(foreign).await? {
            return Err(format!(
                "the plugin claims {foreign}, which belongs to no host it declares"
            ));
        }
    }
    Ok(())
}

/// Stands in for the host during a conformance run: nothing here should reach the network,
/// and a plugin that tries during `match-url` is doing something it has no business doing.
struct RefusingHost;

fn refusing_host() -> std::sync::Arc<dyn rd_plugin_api::ResolverHost> {
    std::sync::Arc::new(RefusingHost)
}

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

#[cfg(test)]
#[path = "conformance_tests.rs"]
mod tests;
