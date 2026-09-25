//! NZB segment transport, yEnc decoding and server fallback.

mod assembly_resume;
mod config;
mod error;
mod nntp;
mod pool;
mod runner;
mod segments;
mod worker;
mod yenc;

#[cfg(test)]
mod assembly_resume_tests;
#[cfg(all(test, feature = "failpoints"))]
mod crash_restart_tests;
#[cfg(test)]
mod out_of_order_tests;
#[cfg(test)]
mod pipelining_tests;
#[cfg(test)]
mod pool_tests;
#[cfg(test)]
mod resume_after_crash_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod throughput_bench;
#[cfg(test)]
mod transient_tests;
#[cfg(test)]
mod worker_tests;

pub use config::{
    connection_fingerprint, enabled_server_configs, server_config, server_config_with_ca,
};
pub use error::{NntpStatusError, nntp_status};
pub use nntp::{NntpClient, NntpServerConfig, Socks5Proxy};
pub use pool::{FetchError, NntpPool, PooledArticle};
pub use runner::{UsenetRunner, UsenetRunnerConfig};
pub use segments::{ArticleSource, AssembledFile, SegmentRequest, download_file};
pub use yenc::{DecodedArticle, YencMetadata, decode_yenc};
