//! rd-api integration tests: what comes in: capture, the LinkGrabber, containers, mirrors, replays, torrents and NZBs.
//!
//! One test binary per subject, each suite a module of it (RD-150-10). Every binary links the
//! whole service, and one binary per file meant 57 links of ~550 MB each. A new suite is a
//! module here and a row in `scripts/lib/rd-api-tests.map`, which selects suites by these
//! module names.

#[path = "../common/mod.rs"]
mod common;

mod captcha;
mod capture_boundary;
mod capture_file;
mod capture_intake;
mod container_json;
mod containers;
mod dlc;
mod enqueue_cancellation;
mod event_resume;
mod hotfolder_poll;
mod media_manifest;
mod mirrors;
mod nzb_paused;
mod nzb_remote_job;
mod replay_consent;
mod replay_restart;
mod source_sets;
mod torrent;
