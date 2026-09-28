//! Intake parser and URL normalizer plugins (RD-090-12).
//!
//! A parser proposes candidates; it never queues anything. What it returns goes through the
//! same LinkGrabber review, the same blocklist and the same routing rules a pasted link
//! does, so the worst a bad parser can do is suggest links a person then declines.

use std::{collections::HashSet, sync::Arc};

use anyhow::Result;
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{PluginInstaller, PluginManifest, PluginType, PluginTypeRegistry};

/// A candidate a parser proposed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntakeCandidate {
    pub url: url::Url,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    pub package_hint: Option<String>,
}

/// Most candidates accepted from one plugin for one input.
///
/// A parser that answers with thousands of links is either broken or hostile; either way the
/// review list is not the place to find out.
const MAX_CANDIDATES: usize = 500;

/// The installed intake parsers, newest version of each.
pub struct IntakeParsers {
    plugins: Vec<Parser>,
}

struct Parser {
    manifest: PluginManifest,
    plugin: rd_plugin_host::extension::IntakeParser,
}

impl IntakeParsers {
    /// Loads every installed intake parser, skipping any that fails to build.
    ///
    /// A broken plugin costs its own feature and nothing else: intake still works, and the
    /// failure is logged rather than taking the LinkGrabber down with it.
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
        let mut plugins = registry.instantiate(&PluginType::Intake, |package| {
            rd_plugin_host::extension::IntakeParser::new(
                package.manifest.clone(),
                &package.component,
                host.clone(),
            )
            .map(|plugin| Parser {
                manifest: package.manifest.clone(),
                plugin,
            })
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

    /// Whether any parser is installed at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Asks every parser that claims the input for candidates.
    ///
    /// Failures are logged and dropped: a parser that errors must not turn a paste that the
    /// native scanner would have handled into an error the person cannot act on.
    pub async fn parse(&self, input: &str) -> Vec<IntakeCandidate> {
        let mut collected = Vec::new();
        for parser in &self.plugins {
            match parser.plugin.parse(input).await {
                Ok(candidates) => {
                    for proposal in candidates.into_iter().take(MAX_CANDIDATES) {
                        // A proposal that is not a URL is dropped rather than reported: the
                        // person pasting the text cannot fix someone else's parser.
                        let Ok(url) = url::Url::parse(&proposal.url) else {
                            tracing::warn!(
                                plugin = %parser.manifest.name,
                                "intake parser proposed something that is not a URL"
                            );
                            continue;
                        };
                        collected.push(IntakeCandidate {
                            url,
                            file_name: proposal.file_name,
                            size: proposal.size,
                            package_hint: proposal.package_hint,
                        });
                    }
                }
                Err(error) => tracing::warn!(
                    plugin = %parser.manifest.name,
                    %error,
                    "intake parser failed"
                ),
            }
        }
        collected
    }

    /// Every source set the installed parsers state for `input` (RD-150-03), checked.
    ///
    /// Each set comes back with the address it belongs to — the one the same parser proposed
    /// through [`Self::parse`] — so the intake can put the two back together. A set whose
    /// address is not a URL, is not one of its own sources, or keeps no source after
    /// [`rd_core::SourceSet::checked`] is dropped with a warning, like a bad proposal is.
    pub async fn source_sets(&self, input: &str) -> Vec<(url::Url, rd_core::SourceSet)> {
        let mut collected = Vec::new();
        for parser in self
            .plugins
            .iter()
            .filter(|parser| parser.plugin.states_sources())
        {
            let proposals = match parser.plugin.source_sets(input).await {
                Ok(proposals) => proposals,
                Err(error) => {
                    tracing::warn!(
                        plugin = %parser.manifest.name,
                        %error,
                        "intake parser could not state its sources"
                    );
                    continue;
                }
            };
            for proposal in proposals.into_iter().take(MAX_CANDIDATES) {
                let Some(checked) = checked_set(proposal) else {
                    tracing::warn!(
                        plugin = %parser.manifest.name,
                        "intake parser stated a source set the host cannot use"
                    );
                    continue;
                };
                if !collected
                    .iter()
                    .any(|(primary, _): &(url::Url, rd_core::SourceSet)| *primary == checked.0)
                {
                    collected.push(checked);
                }
            }
        }
        collected
    }

    /// Offers a URL to each parser for normalisation, keeping the first rewrite.
    ///
    /// A rewrite that is not a valid URL, or that changes the host, is discarded: a
    /// normalizer exists to canonicalise an address, not to redirect it somewhere else.
    pub async fn normalize(&self, url: &url::Url) -> Option<url::Url> {
        for parser in &self.plugins {
            let Ok(Some(rewritten)) = parser.plugin.normalize(url.as_str()).await else {
                continue;
            };
            let Ok(parsed) = url::Url::parse(&rewritten) else {
                tracing::warn!(plugin = %parser.manifest.name, "normalizer returned a non-URL");
                continue;
            };
            if parsed.host_str() != url.host_str() || parsed.scheme() != url.scheme() {
                tracing::warn!(
                    plugin = %parser.manifest.name,
                    "normalizer tried to change the host or scheme; ignored"
                );
                continue;
            }
            return Some(parsed);
        }
        None
    }
}

/// A parser's source set, checked, keyed by the address it was proposed under.
fn checked_set(
    proposal: rd_plugin_host::extension::SourceSetProposal,
) -> Option<(url::Url, rd_core::SourceSet)> {
    let primary = url::Url::parse(proposal.primary_url.trim()).ok()?;
    let hashes: Vec<rd_core::StatedHash> = proposal
        .hashes
        .into_iter()
        .map(|(algorithm, value)| rd_core::StatedHash { algorithm, value })
        .collect();
    let set =
        rd_core::SourceSet::checked(proposal.sources, proposal.size, &hashes, proposal.pieces)?;
    // The proposal is the download's own address; a set that does not contain it describes
    // some other file, and mixing the two is exactly what the hashes are there to prevent.
    set.sources
        .iter()
        .any(|source| source.url == primary)
        .then_some((primary, set))
}

/// Drops every installed version of a parser but the newest.
///
/// `load_verified` hands out one package per installed *version*, newest first, so a machine
/// that still has 1.2.3 next to 1.2.4 of one parser ran both: every paste was offered to the
/// same plugin twice and produced each candidate twice. The first entry for an id wins, which
/// is the highest SemVer, and the copies behind it are left unused on disk.
fn keep_newest_version(plugins: &mut Vec<Parser>) {
    let mut seen = HashSet::new();
    plugins.retain(|parser| seen.insert(parser.manifest.id.to_string()));
}

#[cfg(test)]
mod tests {
    use rd_plugin_host::extension::SourceSetProposal;

    use super::checked_set;

    fn proposal(primary: &str, sources: &[&str]) -> SourceSetProposal {
        SourceSetProposal {
            primary_url: primary.to_owned(),
            file_name: Some("f.iso".to_owned()),
            size: Some(10),
            sources: sources
                .iter()
                .map(|url| ((*url).to_owned(), None, None))
                .collect(),
            hashes: vec![("sha-1".to_owned(), "0".repeat(40))],
            pieces: None,
        }
    }

    #[test]
    fn a_set_is_kept_under_the_address_it_was_proposed_with() {
        let (primary, set) = checked_set(proposal(
            "https://a.example/f.iso",
            &["https://a.example/f.iso", "https://b.example/f.iso"],
        ))
        .expect("set");
        assert_eq!(primary.as_str(), "https://a.example/f.iso");
        assert_eq!(set.sources.len(), 2);
        assert!(set.checksum.is_some());
    }

    #[test]
    fn a_set_that_does_not_contain_its_own_address_is_dropped() {
        assert!(
            checked_set(proposal(
                "https://a.example/f.iso",
                &["https://b.example/f.iso"]
            ))
            .is_none()
        );
        assert!(checked_set(proposal("not a url", &["https://b.example/f.iso"])).is_none());
    }
}
