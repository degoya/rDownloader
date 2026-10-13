//! What the contract modules share beyond `tests/common/` (RD-1280-05): the storage harness the
//! five cloud drives run their common cases through, and the one table every fixture
//! directory's credential check reads.
//!
//! `common/` holds records and constants; this holds cases. A case here runs a real component
//! from a row of a table, and the module that owns the provider names the test, so a filter or
//! a failure still points at the provider: `dropbox_contract::two_flows_in_the_same_moment_*`.

pub mod fixtures;
pub mod storage;
