//! rd-api integration tests: the queue and its limits: order, removal, categories, storage, bandwidth, power and reconnects.
//!
//! One test binary per subject, each suite a module of it (RD-150-10). Every binary links the
//! whole service, and one binary per file meant 57 links of ~550 MB each. A new suite is a
//! module here and a row in `scripts/lib/rd-api-tests.map`, which selects suites by these
//! module names.

#[path = "../common/mod.rs"]
mod common;

mod auto_remove;
mod bandwidth;
mod bandwidth_manual;
mod category_move_and_reset;
mod clear_list;
mod collisions;
mod power;
mod queue_pause;
mod reconnect;
mod reorder;
mod service_switches;
mod storage_capacity;
mod storage_clear;
mod storage_roots;
