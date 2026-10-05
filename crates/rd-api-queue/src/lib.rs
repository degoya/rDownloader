//! The download queue (RD-160-06): downloads and packages, torrents, Usenet, media selection,
//! remote jobs, bandwidth, collisions and duplicates, storage, power and reconnect, the capture
//! summary and the Prometheus metrics.

pub mod auto_remove_service;
pub mod bandwidth_handlers;
pub mod bandwidth_manual_handlers;
pub mod capture_queue;
pub mod capture_summary;
pub mod collision_handlers;
pub mod download_handlers;
pub mod download_sources;
pub mod duplicates;
pub mod history_handlers;
pub mod media_dto;
pub mod media_handlers;
pub mod metrics;
pub mod metrics_format;
pub mod nzb_remote_job_handlers;
pub mod package_clear;
pub mod package_handlers;
pub mod power_handlers;
pub mod queue_pause_handlers;
pub mod reconnect_handlers;
pub mod remote_job_handlers;
pub mod replay_dto;
pub mod replay_handlers;
pub mod storage_handlers;
pub mod torrent_control;
pub mod torrent_handlers;
pub mod torrent_trackers;
pub mod usenet_handlers;

// The modules of the crates below, at this crate's root, so that a module here names them as
// `crate::…` exactly as it did while the HTTP surface was one crate (RD-160-06).
use rd_api_core::{
    ApiError, AppState, audit, container_upload, destination, dto, error, error_codes, hosters,
    postprocess_handlers, reconnect_service, remote_job_service, settings_store, torrent_intake,
};
