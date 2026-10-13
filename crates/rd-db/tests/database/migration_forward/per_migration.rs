//! Single migrations, each measured on a row written by the schema right before it.
//!
//! Only migrations the older kept release did not ship yet (RD-1280-23): a migration both
//! baselines in [`super::BASELINES`] already carry upgrades nothing any more, and its test goes
//! when the release that makes it so drops the older baseline. `baselines.rs` reads the schema
//! each test here starts from and holds it at or above the older baseline's number, so a test
//! left behind fails there with its number. The tests of `0090` to `0134` (RD-120-37 to
//! RD-1230-03) went with the baselines of 0.6.0 to 1.5.0.
//!
//! A test here starts from an empty database at the schema right before its migration
//! (`super::schema_at`) rather than from the release fixture: each asserts one table, and the
//! fixture's own rows in it would only blur the count.
