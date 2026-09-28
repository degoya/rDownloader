//! rd-api integration tests: the MCP toolbox, every tool through the handlers.
//!
//! One test binary per subject, each suite a module of it (RD-150-10). Every binary links the
//! whole service, and one binary per file meant 57 links of ~550 MB each. A new suite is a
//! module here and a row in `scripts/lib/rd-api-tests.map`, which selects suites by these
//! module names.

#[path = "../common/mod.rs"]
mod common;

mod mcp;
