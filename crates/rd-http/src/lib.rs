//! Resumable HTTP download engine.

mod client_pool;
mod cookies;
mod engine;
mod hostlimit;
mod plan;
mod probe;
mod redirect;
mod sniff;
mod tls;
mod transform;

pub use client_pool::{
    AuthMaterial, ClientContext, ClientKey, ClientPool, NetworkDefaults, ProxyCredentials,
    SharedNetworkDefaults,
};
pub use cookies::{
    CookieDomainRefused, CookieScope, earliest_expiry, import_cookie_jar, import_into,
};
pub use engine::{
    CheckpointSink, DownloadEngine, DownloadOutcome, DownloadRequest, HttpDownloadError,
    LOCAL_IO_CODE, ReplayPayload, TransformPlan,
};
pub use hostlimit::{DEFAULT_CONNECTIONS_PER_HOST, HostLimits, MAX_CONNECTIONS_PER_HOST};
pub use plan::{ChunkSpec, plan_chunks};
pub use probe::{
    ConditionalBody, FetchedDocument, ProbeResult, VerbatimBody, contradicts_announced_size,
    fetch_bytes, fetch_conditional, fetch_document, fetch_text_verbatim, peek_body_text, probe,
    probe_with_headers,
};
pub use redirect::{RedirectGate, ReplayScope, is_approved, origin_of, with_redirect_gate};
pub use sniff::{SniffedBody, fetch_sniffed};
pub use tls::client_config as tls_client_config;
pub use transform::{MacWalker, ResumePlan, StreamTransform, TransformCheckpoint, plan_resume};
