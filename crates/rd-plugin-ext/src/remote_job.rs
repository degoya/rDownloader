//! Remote-job plugins (RD-108-03): the adapter between `world remote-job-plugin` and the
//! sweep that drives a row through it.
//!
//! The host wrapper in `rd_plugin_host::extension::remote_job` already bounds every list and
//! strips every path; what this file adds is the selection and the translation. Which plugin
//! a job runs on is decided twice and by two different keys: a *new* job by the provider slug
//! the account carries, exactly as an authentication flow is; an *existing* job by the plugin
//! id its row names, because the remote identifier and the plugin's own bookkeeping are that
//! plugin's vocabulary and no other plugin's. What a finished job hands back is reduced to
//! addresses the LinkGrabber may take, the same way a crawler's answer is.
//!
//! The wrapper sits behind [`RemoteJobDriver`] so the sweep in `rd-api` and the tests here
//! can be driven without a compiled WebAssembly component. The trait is public because a
//! mock provider in another crate has to implement it; it is hidden because nothing outside a
//! test should.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use anyhow::Result;
use rd_core::RemoteJobFile;
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{
    PluginInstaller, PluginManifest, PluginType, PluginTypeRegistry,
    extension::{CacheKind, RemoteJobPlugin, RemoteJobProgress, RemoteJobRefusal},
};

mod cache;
mod calls;
mod driver;
mod outcome;

pub use driver::RemoteJobDriver;
pub use outcome::{JobRefusal, PollOutcome, ReadyArtifact, StartOutcome};

/// Most addresses taken from one finished job.
///
/// The same figure the crawlers use, and lower than the host wrapper's own ceiling for the
/// same reason: this is the number that lands in somebody's review list.
const MAX_ARTIFACTS: usize = 500;

/// The stable code for a plugin that trapped, ran out of fuel or timed out instead of
/// answering. Not retried: a guest that cannot finish a call says nothing about the job, and
/// a poll loop that waited for it to start finishing would wait for ever.
const UNSPECIFIED: &str = "plugin.remote_job_failed";
/// The stable code for a job that finished with nothing the LinkGrabber could take.
const EMPTY: &str = "plugin.remote_job_empty";
/// The stable code for a row whose plugin is not installed any more.
const NO_PLUGIN: &str = "remote_job.no_plugin";
/// The stable code for a job that asked for a choice again after one was made (RD-120-35).
const CHOICE_NOT_KEPT: &str = "remote_job.choice_not_kept";

/// What one installed plugin is known by, apart from its code.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct RunnerInfo {
    /// The manifest id, stable across versions. What a row names.
    pub plugin_id: String,
    /// The manifest name, for logs and the batch a finished job becomes.
    pub name: String,
    /// The provider slugs whose accounts this plugin runs jobs on, lower-case.
    pub claims: Vec<String>,
}

/// The provider slugs a manifest claims, lower-case: what a runner is routed by and what the
/// form is offered, from one function so the two cannot drift apart.
fn claims_of(manifest: &PluginManifest) -> Vec<String> {
    manifest
        .extension
        .as_ref()
        .map(|extension| {
            extension
                .claims
                .iter()
                .map(|claim| claim.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

struct Runner {
    info: RunnerInfo,
    plugin: Box<dyn RemoteJobDriver>,
    /// What `cache-kinds` answered, asked once per load (RD-130-11). A plugin that traps on
    /// the question is taken at "none" and never asked again until the next load.
    kinds: tokio::sync::OnceCell<Vec<CacheKind>>,
}

/// The installed remote-job plugins, newest version of each.
pub struct RemoteJobRunners {
    plugins: Vec<Runner>,
    /// Provider slug to the plugin that runs jobs for it. What a *new* job is routed by.
    by_slug: HashMap<String, usize>,
    /// Manifest id to the plugin. What an *existing* row is routed by.
    by_id: HashMap<String, usize>,
}

impl RemoteJobRunners {
    /// Loads every installed remote-job plugin, skipping any that fails to build.
    ///
    /// A broken plugin costs its own feature and nothing else: the service starts, the rows
    /// that name it fail with a code that says so, and the failure is logged.
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
        let loaded = registry.instantiate(&PluginType::RemoteJob, |package| {
            RemoteJobPlugin::new(package.manifest.clone(), &package.component, host.clone()).map(
                |plugin| {
                    (
                        RunnerInfo {
                            plugin_id: package.manifest.id.to_string(),
                            name: package.manifest.name.clone(),
                            claims: claims_of(&package.manifest),
                        },
                        Box::new(plugin) as Box<dyn RemoteJobDriver>,
                    )
                },
            )
        });
        Self::from_drivers(loaded)
    }

    /// Runners over already compiled plugins, for a contract test that built the component
    /// itself.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn from_plugins(plugins: Vec<RemoteJobPlugin>) -> Self {
        Self::from_drivers(
            plugins
                .into_iter()
                .map(|plugin| {
                    let manifest = plugin.manifest();
                    (
                        RunnerInfo {
                            plugin_id: manifest.id.to_string(),
                            name: manifest.name.clone(),
                            claims: claims_of(manifest),
                        },
                        Box::new(plugin) as Box<dyn RemoteJobDriver>,
                    )
                })
                .collect(),
        )
    }

    /// Runners over anything that implements the driver, for a mock provider.
    ///
    /// The input order is the installer's: newest version of each plugin first, so the first
    /// claim of a slug and the first occurrence of an id win.
    #[doc(hidden)]
    #[must_use]
    pub fn from_drivers(loaded: Vec<(RunnerInfo, Box<dyn RemoteJobDriver>)>) -> Self {
        let mut plugins = Vec::new();
        let mut by_slug = HashMap::new();
        let mut by_id = HashMap::new();
        for (info, plugin) in loaded {
            if info.claims.is_empty() {
                // A job runs on an account, and an account belongs to a provider. A plugin
                // that claims none names no account it could run for.
                tracing::warn!(
                    plugin = %info.name,
                    "remote-job plugin claims no provider and cannot be used"
                );
                continue;
            }
            let index = plugins.len();
            for slug in &info.claims {
                // Two plugins claiming one provider is a conflict, not a chain: the first
                // -- the newest -- wins.
                by_slug.entry(slug.clone()).or_insert(index);
            }
            by_id.entry(info.plugin_id.clone()).or_insert(index);
            plugins.push(Runner {
                info,
                plugin,
                kinds: tokio::sync::OnceCell::new(),
            });
        }
        Self {
            plugins,
            by_slug,
            by_id,
        }
    }

    /// An empty set, for a service running without plugins.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: Vec::new(),
            by_slug: HashMap::new(),
            by_id: HashMap::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// The provider slugs jobs can be started on.
    #[must_use]
    pub fn providers(&self) -> BTreeSet<String> {
        self.by_slug.keys().cloned().collect()
    }

    /// The provider slugs the installed `remote-job` manifests claim, without compiling any
    /// of them (RD-120-51).
    ///
    /// The same set [`providers`] answers once the runners are built, read from the manifests
    /// alone: `by_slug` holds every claim of every loaded plugin, lower-cased by the same
    /// [`claims_of`], and a plugin that claims nothing contributes nothing either way. Which
    /// plugin wins a slug two of them claim is the runners' business and does not change the
    /// set. The one difference is a package whose component fails to compile: it is listed
    /// here and skipped there, and a job started on it is refused as `remote_job.no_plugin`.
    ///
    /// [`providers`]: RemoteJobRunners::providers
    #[must_use]
    pub fn claimed_providers<'a>(
        manifests: impl IntoIterator<Item = &'a PluginManifest>,
    ) -> BTreeSet<String> {
        manifests
            .into_iter()
            .filter(|manifest| manifest.plugin_type == PluginType::RemoteJob)
            .flat_map(claims_of)
            .collect()
    }

    /// The provider slugs whose remote-job plugin declares it takes `format` as a container
    /// (`[extension] containers`, RD-191-13), read from the manifests alone like
    /// [`claimed_providers`]. A subset of that set: a plugin that declares nothing offers
    /// nothing here, and `identify` still decides when a source actually arrives.
    ///
    /// [`claimed_providers`]: RemoteJobRunners::claimed_providers
    #[must_use]
    pub fn providers_accepting<'a>(
        manifests: impl IntoIterator<Item = &'a PluginManifest>,
        format: &str,
    ) -> BTreeSet<String> {
        manifests
            .into_iter()
            .filter(|manifest| manifest.plugin_type == PluginType::RemoteJob)
            .filter(|manifest| {
                manifest
                    .extension
                    .as_ref()
                    .is_some_and(|extension| extension.containers.iter().any(|kind| kind == format))
            })
            .flat_map(claims_of)
            .collect()
    }

    /// The name of the plugin a row names, for a log line or a batch label.
    #[must_use]
    pub fn plugin_name(&self, plugin_id: &str) -> Option<&str> {
        self.runner(plugin_id)
            .map(|runner| runner.info.name.as_str())
    }

    fn runner(&self, plugin_id: &str) -> Option<&Runner> {
        self.by_id
            .get(plugin_id)
            .and_then(|index| self.plugins.get(*index))
    }

    /// Flattens a call's two layers of failure into the one the sweep reads.
    fn settle<T>(
        runner: &Runner,
        answer: Result<Result<T, RemoteJobRefusal>>,
    ) -> Result<T, JobRefusal> {
        match answer {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(refusal)) => Err(refusal.into()),
            Err(error) => Err(Self::trapped(runner, &error)),
        }
    }

    /// A guest that trapped, ran out of fuel or timed out. Logged with the plugin's name and
    /// reported under one code; the message names no provider text because there was none.
    fn trapped(runner: &Runner, error: &anyhow::Error) -> JobRefusal {
        tracing::warn!(plugin = %runner.info.name, %error, "remote-job plugin failed");
        JobRefusal::permanent(
            UNSPECIFIED,
            format!("{} could not complete the call", runner.info.name),
        )
    }

    /// Turns what a plugin said about a job into what the sweep may act on.
    fn accept(runner: &Runner, progress: RemoteJobProgress) -> PollOutcome {
        match progress {
            RemoteJobProgress::Preparing {
                retry_after_seconds,
            } => PollOutcome::Preparing {
                retry_after_seconds,
            },
            RemoteJobProgress::AwaitingChoice { entries } => {
                let entries: Vec<RemoteJobFile> = entries
                    .into_iter()
                    .map(|entry| RemoteJobFile {
                        id: entry.id,
                        path: entry.path,
                        size: entry.size,
                        selected: entry.selected,
                    })
                    .collect();
                if entries.is_empty() {
                    // A question with no options is not a question a person can answer, and
                    // a row parked in `awaiting_choice` with nothing to show would wait for
                    // an answer that cannot be given.
                    return PollOutcome::Refused(JobRefusal::permanent(
                        EMPTY,
                        format!("{} offered nothing to choose from", runner.info.name),
                    ));
                }
                PollOutcome::AwaitingChoice(entries)
            }
            RemoteJobProgress::Working(work) => PollOutcome::Working(work),
            RemoteJobProgress::Ready { artifacts } => {
                let mut accepted = Vec::new();
                for artifact in artifacts.into_iter().take(MAX_ARTIFACTS) {
                    // A proposal that is not an address is dropped rather than reported: the
                    // person who pasted the magnet cannot fix somebody else's plugin.
                    let Ok(url) = url::Url::parse(&artifact.url) else {
                        tracing::warn!(
                            plugin = %runner.info.name,
                            "remote-job plugin proposed something that is not a URL"
                        );
                        continue;
                    };
                    if !matches!(url.scheme(), "http" | "https") {
                        continue;
                    }
                    accepted.push(ReadyArtifact {
                        url,
                        file_name: crate::crawler::sanitize(artifact.file_name.as_deref()),
                        size: artifact.size,
                        package_hint: crate::crawler::sanitize(artifact.package_hint.as_deref()),
                    });
                }
                if accepted.is_empty() {
                    return PollOutcome::Refused(JobRefusal::permanent(
                        EMPTY,
                        format!("{} finished with no address to download", runner.info.name),
                    ));
                }
                PollOutcome::Ready(accepted)
            }
            RemoteJobProgress::Failed(refusal) => PollOutcome::Refused(refusal.into()),
        }
    }
}

#[cfg(test)]
#[path = "remote_job_tests.rs"]
mod tests;
