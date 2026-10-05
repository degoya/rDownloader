//! Which scope every route requires — one table, checked against the OpenAPI document.
//!
//! ## Why a table and not a check per handler
//!
//! There are a few hundred documented operations — 277 when this was last counted, on
//! 2026-09-19, and the figure is illustrative rather than load-bearing precisely because it
//! said 228 for long enough to be wrong by fifty. Count it with
//! `jq '[.paths[] | keys[] | select(. == ("get","post","put","patch","delete"))] | length'
//! web/openapi.json` rather than trusting this sentence. Annotating each operation by hand is
//! one chance per operation to forget, and a forgotten annotation fails open: the route simply
//! has no check. A table fails the *build* instead, because [`tests`] asserts in both
//! directions that it matches
//! [`crate::openapi_document`] exactly — an operation with no entry is a compile-time-visible
//! test failure, and an entry for an operation that no longer exists is too. Adding a route
//! without deciding what it costs is therefore not possible.
//!
//! The enforcement point is unchanged: [`crate::auth`] already matched the request against
//! `MatchedPath`, the *registered pattern* rather than the raw URI, so a path parameter
//! cannot be spelled in a way that slips past the lookup. This module replaces the hardcoded
//! `READ_ONLY_ROUTES` allowlist that sat there; no handler is touched.
//!
//! ## How the entries were decided
//!
//! Along the lines a person would actually delegate, not along the tags the OpenAPI document
//! happens to carry — `configuration` alone mixes categories with stored passwords.
//!
//! * [`Read`](Scope::Read) stays as narrow as the allowlist it replaces, and for the reason
//!   that allowlist gave: a token pasted into a status page must not become a way to
//!   enumerate the installation. Queue and progress; not the LinkGrabber, not the script and
//!   upload-destination lists, not run histories, not the provider table.
//! * [`Secrets`](Scope::Secrets) covers anything whose purpose is to hold a credential —
//!   accounts, authentication profiles, proxies, remote logins, Usenet servers, API tokens,
//!   the captcha solver's key, plugin signing keys — **including its `GET`**, because listing
//!   them discloses where credentials exist and for what.
//! * [`Admin`](Scope::Admin) is the service itself: plugins, setup, reconnect, power, and the
//!   whole-configuration import/export pair, which is a credential exfiltration route in one
//!   request whatever the individual resources are scoped as.
//!
//! ## Enforcement
//!
//! This table *is* the decision. [`crate::auth::require_session`] asks the credential which
//! scopes it carries and this table what the route costs, and compares them once. It landed
//! in shadow mode first — computing the decision and only logging where it disagreed with the
//! check it replaced — so the classification could be corrected against a running system
//! rather than against a reading of it.

use axum::http::Method;
use rd_core::Scope;

mod table;

pub use table::ROUTE_POLICY;

/// What one route costs.
///
/// Not `Copy`: `http::Method` is not, because it can carry an extension string. The table is
/// only ever read by reference, so it does not matter.
#[derive(Clone, Debug)]
pub struct RoutePolicy {
    /// The registered axum pattern, matched against `MatchedPath` and never the raw URI.
    pub path: &'static str,
    pub method: Method,
    pub requires: Requirement,
}

/// The scope a route needs, or the reason it needs none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Requirement {
    /// Reachable before anyone has authenticated; there is no token to check.
    Public,
    /// Needs this scope.
    Scope(Scope),
}

const fn entry(path: &'static str, method: Method, requires: Requirement) -> RoutePolicy {
    RoutePolicy {
        path,
        method,
        requires,
    }
}

/// The requirement for a matched route, or `None` if the table does not know it.
///
/// `None` is a policy gap and the caller must fail closed on it. It cannot happen for a
/// documented route — the tests below forbid it — but "cannot happen" is not a thing to
/// build an authorisation decision on.
pub fn requirement(path: &str, method: &Method) -> Option<Requirement> {
    ROUTE_POLICY
        .iter()
        .find(|entry| entry.path == path && entry.method == method)
        .map(|entry| entry.requires)
}

/// The scope string a route costs, or `None` for a route that needs no credential.
///
/// Exposed for the conformance test in `crates/rd-api/tests/access/scope_matrix.rs`, which walks the
/// whole API and proves that a token holding every scope *except* the required one is refused.
/// That test has to derive its expectations from this table rather than restate them, or it
/// would only prove that two hand-written lists agree with each other.
///
/// The pair of `&str` arguments rather than typed ones is deliberate: an integration test is
/// an external crate, and this must not require it to depend on `http::Method`.
#[doc(hidden)]
#[must_use]
pub fn required_scope(path: &str, method: &str) -> Option<&'static str> {
    let method = Method::from_bytes(method.as_bytes()).ok()?;
    match requirement(path, &method)? {
        Requirement::Public => None,
        Requirement::Scope(scope) => Some(scope.as_str()),
    }
}

/// How many API operations a token holding exactly this scope can reach.
///
/// Counted from [`ROUTE_POLICY`] rather than written down, so the number a person sees while
/// choosing a scope comes from the same table that will refuse them later. A hand-maintained
/// figure would be wrong the first time somebody added a route, and wrong in the direction
/// that matters: a preview that understates what it grants.
///
/// Public routes are excluded — they are reachable without any token, so counting them would
/// make every scope look alike at the bottom.
#[must_use]
pub fn operations_reachable_by(scope: rd_core::Scope) -> u32 {
    let reachable = ROUTE_POLICY
        .iter()
        .filter(|entry| match entry.requires {
            Requirement::Public => false,
            Requirement::Scope(required) => scope.satisfies(required),
        })
        .count();
    u32::try_from(reachable).unwrap_or(u32::MAX)
}

/// Every `(path, method, scope)` the API enforces, for the conformance test to walk.
///
/// Public routes appear with `None`, so the test can assert they are reachable rather than
/// silently skipping them — a route that quietly became public is exactly the regression
/// worth catching.
#[doc(hidden)]
#[must_use]
pub fn policy_rows() -> Vec<(&'static str, &'static str, Option<&'static str>)> {
    ROUTE_POLICY
        .iter()
        .map(|entry| {
            let scope = match entry.requires {
                Requirement::Public => None,
                Requirement::Scope(scope) => Some(scope.as_str()),
            };
            (entry.path, entry.method.as_str(), scope)
        })
        .collect()
}

#[cfg(test)]
#[path = "scope_policy_tests.rs"]
mod tests;
