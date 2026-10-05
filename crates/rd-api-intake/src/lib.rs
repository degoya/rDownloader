//! How links get in (RD-160-06): the LinkGrabber and its enqueue path, containers, NZB and
//! capture uploads, captchas, site rules, subscriptions, livestream channels and schedules, and
//! the area bundle that carries them.

pub mod area_backup;
pub mod candidate_handlers;
pub mod captcha_handlers;
pub mod capture_fetch;
pub mod capture_file;
pub mod collector_crawl_verdict;
pub mod collector_enqueue;
pub mod collector_handlers;
pub mod collector_source_sets;
pub mod container_handlers;
pub mod history_readd_handlers;
pub mod indexer_handlers;
pub mod indexer_search;
pub mod nzb_handlers;
pub mod nzb_zip;
pub mod regex_tester;
pub mod remote_listing_handlers;
pub mod site_rules_dto;
pub mod site_rules_handlers;
pub mod site_rules_service;
pub mod stream_handlers;
pub mod stream_schedule_handlers;
pub mod subscription_autoqueue;
pub mod subscription_handlers;

// The modules of the crates below, at this crate's root, so that a module here names them as
// `crate::…` exactly as it did while the HTTP surface was one crate (RD-160-06).
use rd_api_core::{
    ApiError, AppState, audit, auth, automation_input, capture_sanitize, collector_exclusions,
    collector_intake, config_fields, container_upload, destination, dlc_import, dto, error,
    error_codes, hosters, link_check_probe, postprocess_handlers, settings_store, stream_monitor,
    subscription_service, torrent_intake,
};
