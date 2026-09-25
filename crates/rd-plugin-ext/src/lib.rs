//! Adapters that connect the extension plugin types to the parts of the application they
//! plug into (RD-090-12 … RD-090-17, RD-107-06, RD-110-33).
//!
//! One crate above `rd-plugin-host` and the domain crates, as `rd-plugin-transfer` is for
//! transfer backends. The host owns sandboxing and the contract; these adapters own the
//! translation between a plugin's answer and the domain model — and, more importantly, the
//! rule that a plugin proposes while the core decides.

mod auth;
mod crawler;
mod enricher;
mod intake;
mod notifier;
mod oauth;
mod postprocess;
mod provider;
mod remote_job;
mod siterules;
mod storage;
mod stream_transform;

pub use auth::AuthProviders;
pub use crawler::{
    CrawlOutcome, CrawledLink, FolderCrawlers, ShareLogin, share_login, split_crawled_address,
};
pub use enricher::MetadataEnrichers;
pub use intake::{IntakeCandidate, IntakeParsers};
pub use notifier::{DestinationInfo, NotifierPlugins};
pub use oauth::OAuthProviders;
pub use postprocess::{PluginSteps, StepInfo};
pub use provider::{ProviderError, ProviderResult};
pub use rd_plugin_host::extension::{CacheAnswer, CacheKind, CacheQuery, CacheState};
pub use remote_job::{
    JobRefusal, PollOutcome, ReadyArtifact, RemoteJobDriver, RemoteJobRunners, RunnerInfo,
    StartOutcome,
};
pub use siterules::{HostRuleRunner, RuleOutcome, RuleRunner, SiteRules};
pub use storage::{StorageDestinations, UploadDestinationInfo};
pub use stream_transform::{StreamTransformInfo, StreamTransformProviders};
