//! rd-plugin-ext integration tests: every plugin world driven through its real component.
//!
//! One test binary, each former test file a module of it (RD-1120-08, the RD-150-10 pattern of
//! `rd-api`). Every binary linked Wasmtime and the whole plugin host, and 34 of them took 71 s
//! on Linux and 151 s on Windows to build for 47 library tests. Helpers more than one module
//! needs live in `tests/common/`. A new contract file is a module here; the `no-components`
//! nextest profile leaves this binary out by its name, `contract`.

#[path = "../common/mod.rs"]
mod common;

mod auth_contract;
mod box_contract;
mod crawler_contract;
mod directory_index_crawler_contract;
mod dropbox_contract;
mod enricher_contract;
mod google_drive_contract;
mod intake_contract;
mod mediafire_crawler_contract;
mod mega_account_contract;
mod mega_auth_contract;
mod mega_contract;
mod nextcloud_crawler_contract;
mod notifier_contract;
mod oauth_contract;
mod offcloud_cloud_contract;
mod onedrive_contract;
mod onefichier_contract;
mod pcloud_contract;
mod peeplink_crawler_contract;
mod pixeldrain_crawler_contract;
mod postprocess_contract;
mod premiumize_transfers_contract;
mod putio_contract;
mod putio_remote_job_contract;
mod realdebrid_contract;
mod remote_job_contract;
mod remote_job_providers_contract;
mod rename_postprocess_contract;
mod seedr_remote_job_contract;
mod storage_contract;
mod stream_transform_contract;
mod torbox_auth_contract;
mod torbox_remote_job_contract;
