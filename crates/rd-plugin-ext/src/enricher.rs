//! Metadata enricher plugins (RD-090-14).
//!
//! An enricher adds fields; it never replaces one. That is the whole rule, and it is enforced
//! here rather than trusted to the plugin: a name that collides with something the core
//! resolved itself is dropped and counted, so a plugin cannot rewrite a file name, a size or
//! a provider by returning a field that happens to be called one of those.

use chrono::Utc;
use rd_core::EnrichmentField;
use rd_plugin_host::{PluginManifest, extension::MetadataEnricher};

use crate::PluginSet;

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

/// The installed metadata enrichers, newest version of each.
pub type MetadataEnrichers = PluginSet<MetadataEnricher>;

impl MetadataEnrichers {
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
        for enricher in self.iter() {
            if !claims(claimed_domains(&enricher.manifest), url) {
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

/// Domain patterns from `[extension] claims`; empty means every link.
fn claimed_domains(manifest: &PluginManifest) -> &[String] {
    manifest
        .extension
        .as_ref()
        .map(|extension| extension.claims.as_slice())
        .unwrap_or_default()
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
    // A claim is a site: `youtube.com` and `*.youtube.com` both stand for the domain and every
    // name below it (RD-191-06, PLUG-17: the one matcher, with the apex included).
    patterns.iter().any(|pattern| {
        let pattern = pattern.to_ascii_lowercase();
        let site = if pattern.starts_with("*.") {
            pattern
        } else {
            format!("*.{pattern}")
        };
        rd_core::host_pattern_matches(&site, &host, rd_core::WildcardApex::Included)
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
