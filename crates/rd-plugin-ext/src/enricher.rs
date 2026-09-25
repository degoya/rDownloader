//! Metadata enricher plugins (RD-090-14).
//!
//! An enricher adds fields; it never replaces one. That is the whole rule, and it is enforced
//! here rather than trusted to the plugin: a name that collides with something the core
//! resolved itself is dropped and counted, so a plugin cannot rewrite a file name, a size or
//! a provider by returning a field that happens to be called one of those.

use std::{collections::HashSet, sync::Arc};

use anyhow::Result;
use chrono::Utc;
use rd_core::EnrichmentField;
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{
    PluginInstaller, PluginManifest, PluginType, PluginTypeRegistry, extension::MetadataEnricher,
};

/// Field names the core owns. An enricher offering one of these is offering to replace
/// something the application already knows, which is not what this plugin type is for.
const CORE_FIELDS: &[&str] = &[
    "url",
    "file_name",
    "size",
    "provider",
    "state",
    "error",
    "category_id",
    "priority",
    "package_id",
    "position",
    "media",
    "checked_at",
    "created_at",
];

/// Most fields kept from one plugin for one link. A plugin answering with hundreds is either
/// broken or hostile; either way the candidate row is not where that should be discovered.
const MAX_FIELDS: usize = 32;

/// Longest value kept, so one field cannot fill the candidate list.
const MAX_VALUE: usize = 512;

/// The installed metadata enrichers.
pub struct MetadataEnrichers {
    plugins: Vec<Enricher>,
}

struct Enricher {
    manifest: PluginManifest,
    /// Domain patterns from `[extension] claims`; empty means every link.
    claims: Vec<String>,
    plugin: MetadataEnricher,
}

impl MetadataEnrichers {
    /// Loads every installed enricher, skipping any that fails to build.
    pub async fn load(
        installer: &PluginInstaller,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        Ok(Self::from_registry(
            &PluginTypeRegistry::load(installer).await?,
            host,
        ))
    }

    /// The same, from a registry the adapters share.
    ///
    /// Loading a registry re-verifies and compiles every installed package, so the one `load`
    /// builds for itself is only worth it for a caller that loads a single adapter. Everything
    /// started together passes one registry through all of them.
    #[must_use]
    pub fn from_registry(
        registry: &PluginTypeRegistry,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Self {
        let mut plugins = registry.instantiate(&PluginType::Enricher, |package| {
            MetadataEnricher::new(package.manifest.clone(), &package.component, host.clone()).map(
                |plugin| Enricher {
                    claims: package
                        .manifest
                        .extension
                        .as_ref()
                        .map(|extension| extension.claims.clone())
                        .unwrap_or_default(),
                    manifest: package.manifest.clone(),
                    plugin,
                },
            )
        });
        keep_newest_version(&mut plugins);
        Self { plugins }
    }

    /// An empty set, for a service running without plugins.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Asks every enricher that claims this link for additional fields.
    ///
    /// Failures are logged and dropped: an enricher that errors must not turn a link the
    /// online check resolved perfectly well into one the person has to think about.
    pub async fn enrich(
        &self,
        url: &url::Url,
        file_name: Option<&str>,
        known: Option<&str>,
    ) -> Vec<EnrichmentField> {
        let mut collected = Vec::new();
        for enricher in &self.plugins {
            if !claims(&enricher.claims, url) {
                continue;
            }
            let plugin_id = enricher.manifest.id.to_string();
            match enricher.plugin.enrich(url.as_str(), file_name, known).await {
                Ok(fields) => {
                    let mut dropped = 0;
                    for (name, value) in fields.into_iter().take(MAX_FIELDS) {
                        let name = name.trim().to_owned();
                        if !may_add(&name) {
                            dropped += 1;
                            continue;
                        }
                        collected.push(EnrichmentField {
                            name,
                            value: value.chars().take(MAX_VALUE).collect(),
                            plugin_id: plugin_id.clone(),
                            fetched_at: Utc::now(),
                        });
                    }
                    if dropped > 0 {
                        // Worth a line: an author whose field is being discarded should be
                        // able to find out why without reading this file.
                        tracing::warn!(
                            plugin = %enricher.manifest.name,
                            dropped,
                            "enricher fields collided with core fields and were dropped"
                        );
                    }
                }
                Err(error) => tracing::warn!(
                    plugin = %enricher.manifest.name,
                    %error,
                    "metadata enricher failed"
                ),
            }
        }
        collected
    }
}

/// Drops every installed version of an enricher but the newest.
///
/// `load_verified` hands out one package per installed *version*, newest first, so a machine
/// that still has 1.2.3 next to 1.2.4 of one enricher asked both: the same link came back with
/// every field twice, each carrying the same `plugin_id`. The first entry for an id wins, which
/// is the highest SemVer, and the copies behind it are left unused on disk.
fn keep_newest_version(plugins: &mut Vec<Enricher>) {
    let mut seen = HashSet::new();
    plugins.retain(|enricher| seen.insert(enricher.manifest.id.to_string()));
}

/// Whether a field a plugin offered may be added.
///
/// The rule an enricher cannot be trusted to keep for itself: a name that collides with
/// something the core resolved is an offer to replace it, and this type exists to add.
fn may_add(name: &str) -> bool {
    !name.is_empty() && !CORE_FIELDS.contains(&name)
}

/// Whether a plugin's claimed domains cover this link. An empty list claims everything, which
/// is the right default for an enricher that decides per URL inside `enrich`.
fn claims(patterns: &[String], url: &url::Url) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    patterns.iter().any(|pattern| {
        let pattern = pattern.to_ascii_lowercase();
        match pattern.strip_prefix("*.") {
            Some(suffix) => host == suffix || host.ends_with(&format!(".{suffix}")),
            None => host == pattern || host.ends_with(&format!(".{pattern}")),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{claims, may_add};

    #[test]
    fn a_field_that_would_replace_a_core_one_is_refused() {
        // The point of the type is that it adds. A plugin returning `file_name` or `size` is
        // offering to rewrite what the online check resolved, and the answer is no.
        for name in ["file_name", "size", "provider", "url", "media", "error"] {
            assert!(!may_add(name), "{name} must not be replaceable");
        }
        assert!(!may_add(""), "an unnamed field is not a field");
        // Namespaced names, which is what an enricher should be producing, go through.
        assert!(may_add("sponsorblock.sponsor"));
        assert!(may_add("sponsorblock.skippable"));
    }

    #[test]
    fn an_empty_claim_list_covers_every_link() {
        let url = url::Url::parse("https://example.com/a").expect("url");
        assert!(claims(&[], &url));
    }

    #[test]
    fn a_claimed_domain_covers_its_subdomains_and_nothing_else() {
        let patterns = vec!["youtube.com".to_owned()];
        for (address, expected) in [
            ("https://youtube.com/watch?v=x", true),
            ("https://www.youtube.com/watch?v=x", true),
            // The suffix has to be a domain boundary, or `notyoutube.com` would match.
            ("https://notyoutube.com/watch?v=x", false),
            ("https://example.com/a", false),
        ] {
            let url = url::Url::parse(address).expect("url");
            assert_eq!(claims(&patterns, &url), expected, "{address}");
        }
    }
}
