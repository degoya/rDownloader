//! Helpers more than one contract module had a copy of (RD-1120-08): the sign-in mocks'
//! records and the remote-job modules' magnet. Only what was identical moved here; a mock
//! whose answers differ per provider stays in its module.

pub mod oauth;
pub mod remote_job;
