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

/// What one route costs.
///
/// Not `Copy`: `http::Method` is not, because it can carry an extension string. The table is
/// only ever read by reference, so it does not matter.
#[derive(Clone, Debug)]
pub(crate) struct RoutePolicy {
    /// The registered axum pattern, matched against `MatchedPath` and never the raw URI.
    pub path: &'static str,
    pub method: Method,
    pub requires: Requirement,
}

/// The scope a route needs, or the reason it needs none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Requirement {
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

// Short aliases, so the table below reads as a table rather than as 232 lines of
// `Requirement::Scope(Scope::…)`.
const PUBLIC: Requirement = Requirement::Public;
const READ: Requirement = Requirement::Scope(Scope::Read);
const INTAKE: Requirement = Requirement::Scope(Scope::Intake);
const QUEUE: Requirement = Requirement::Scope(Scope::Queue);
const CONFIG: Requirement = Requirement::Scope(Scope::Config);
const SECRETS: Requirement = Requirement::Scope(Scope::Secrets);
const ADMIN: Requirement = Requirement::Scope(Scope::Admin);
const CAPTURE: Requirement = Requirement::Scope(Scope::Capture);
const METRICS: Requirement = Requirement::Scope(Scope::Metrics);

/// Every route, with the scope it costs. Sorted by path then method.
pub(crate) const ROUTE_POLICY: &[RoutePolicy] = &[
    entry("/api/v1/accounts", Method::GET, SECRETS),
    entry("/api/v1/accounts", Method::POST, SECRETS),
    entry("/api/v1/accounts/{id}", Method::DELETE, SECRETS),
    entry("/api/v1/accounts/{id}", Method::PUT, SECRETS),
    entry("/api/v1/accounts/{id}/auth", Method::DELETE, SECRETS),
    entry("/api/v1/accounts/{id}/auth", Method::GET, SECRETS),
    entry("/api/v1/accounts/{id}/auth/begin", Method::POST, SECRETS),
    // Asking the extension for a browser session ends in cookies on the account, so it costs
    // what typing them into the account form costs (RD-120-45).
    entry(
        "/api/v1/accounts/{id}/browser-session",
        Method::DELETE,
        SECRETS,
    ),
    entry(
        "/api/v1/accounts/{id}/browser-session",
        Method::GET,
        SECRETS,
    ),
    entry(
        "/api/v1/accounts/{id}/browser-session",
        Method::POST,
        SECRETS,
    ),
    entry("/api/v1/accounts/{id}/hosters", Method::GET, SECRETS),
    // Handing a magnet to somebody's provider account spends that account and leaves
    // something behind in it, so it costs what the account itself costs (RD-108-04).
    entry("/api/v1/accounts/{id}/remote-jobs", Method::POST, SECRETS),
    entry("/api/v1/accounts/{id}/test", Method::POST, SECRETS),
    entry("/api/v1/api-tokens", Method::GET, SECRETS),
    entry("/api/v1/api-tokens", Method::POST, SECRETS),
    entry("/api/v1/api-tokens/scopes", Method::GET, SECRETS),
    entry("/api/v1/api-tokens/{id}", Method::DELETE, SECRETS),
    // Re-scoping is the one route that can *widen* a credential, so it costs the area that
    // already covers handing one out in the first place.
    entry("/api/v1/api-tokens/{id}", Method::PATCH, SECRETS),
    // The audit log names who acted, from where, on what. It is the most concentrated
    // description of an installation this service holds, so it costs what diagnostics
    // costs and one step sharper: an `api:read` token on a status page must not read it
    // (RD-110-03).
    entry("/api/v1/audit/export", Method::GET, ADMIN),
    entry("/api/v1/audit/records", Method::GET, ADMIN),
    // Emptying the audit log is the sharpest thing anybody can do to it, so it costs what
    // reading it costs and no less (RD-120-34).
    entry("/api/v1/audit/records/clear", Method::POST, ADMIN),
    entry("/api/v1/auth-profiles", Method::GET, SECRETS),
    entry("/api/v1/auth-profiles", Method::POST, SECRETS),
    entry("/api/v1/auth-profiles/{id}", Method::DELETE, SECRETS),
    entry("/api/v1/auth-profiles/{id}", Method::PUT, SECRETS),
    entry("/api/v1/auth-profiles/{id}/disable", Method::POST, SECRETS),
    entry("/api/v1/auth-profiles/{id}/enable", Method::POST, SECRETS),
    entry("/api/v1/auth-profiles/{id}/test", Method::POST, SECRETS),
    entry("/api/v1/auth/login", Method::POST, PUBLIC),
    entry("/api/v1/auth/logout", Method::POST, PUBLIC),
    entry("/api/v1/auth/passkey/challenge", Method::POST, PUBLIC),
    entry("/api/v1/auth/passkey/login", Method::POST, PUBLIC),
    // Not public, unlike the rest of `/auth`: a change of the administrator password is a
    // credential write, so it costs what handing out a credential costs (RD-120-22). The
    // current password is demanded on top of that, inside the handler.
    entry("/api/v1/auth/password", Method::POST, SECRETS),
    entry("/api/v1/auth/setup", Method::POST, PUBLIC),
    entry("/api/v1/auth/status", Method::GET, PUBLIC),
    entry("/api/v1/automations", Method::GET, CONFIG),
    entry("/api/v1/automations", Method::POST, CONFIG),
    entry("/api/v1/automations/dry-run", Method::POST, QUEUE),
    entry("/api/v1/automations/export", Method::GET, ADMIN),
    entry("/api/v1/automations/import", Method::POST, ADMIN),
    entry("/api/v1/automations/runs", Method::GET, CONFIG),
    entry("/api/v1/automations/vocabulary", Method::GET, CONFIG),
    entry("/api/v1/automations/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/automations/{id}", Method::PUT, CONFIG),
    entry("/api/v1/automations/{id}/enable", Method::POST, CONFIG),
    entry("/api/v1/automations/{id}/versions", Method::GET, CONFIG),
    entry("/api/v1/bandwidth/capabilities", Method::GET, READ),
    entry("/api/v1/bandwidth/profiles", Method::GET, CONFIG),
    entry("/api/v1/bandwidth/profiles", Method::POST, CONFIG),
    entry("/api/v1/bandwidth/profiles/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/bandwidth/profiles/{id}", Method::PUT, CONFIG),
    entry("/api/v1/bandwidth/schedule", Method::GET, CONFIG),
    entry("/api/v1/bandwidth/schedule", Method::PUT, CONFIG),
    entry("/api/v1/bandwidth/status", Method::GET, READ),
    entry("/api/v1/captcha-answerers", Method::GET, QUEUE),
    entry("/api/v1/captcha-config", Method::GET, SECRETS),
    entry("/api/v1/captcha-config", Method::PUT, SECRETS),
    entry("/api/v1/captcha-config/test", Method::POST, SECRETS),
    entry("/api/v1/captchas", Method::GET, QUEUE),
    entry("/api/v1/captchas/{id}/click", Method::POST, QUEUE),
    entry("/api/v1/captchas/{id}/skip", Method::POST, QUEUE),
    entry("/api/v1/captchas/{id}/solution", Method::POST, QUEUE),
    entry("/api/v1/capture/agents", Method::GET, SECRETS),
    entry("/api/v1/capture/agents/{id}", Method::DELETE, SECRETS),
    entry("/api/v1/capture/batches", Method::POST, CAPTURE),
    entry("/api/v1/capture/browser-sessions", Method::GET, CAPTURE),
    entry(
        "/api/v1/capture/browser-sessions/{id}",
        Method::POST,
        CAPTURE,
    ),
    entry(
        "/api/v1/capture/browser-sessions/{id}/decline",
        Method::POST,
        CAPTURE,
    ),
    entry("/api/v1/capture/captchas", Method::GET, CAPTURE),
    entry(
        "/api/v1/capture/captchas/{id}/no-widget",
        Method::POST,
        CAPTURE,
    ),
    entry("/api/v1/capture/captchas/{id}/skip", Method::POST, CAPTURE),
    entry("/api/v1/capture/captchas/{id}/token", Method::POST, CAPTURE),
    entry("/api/v1/capture/cookies", Method::POST, CAPTURE),
    entry("/api/v1/capture/events", Method::GET, CAPTURE),
    entry("/api/v1/capture/file", Method::POST, CAPTURE),
    entry("/api/v1/capture/nzb", Method::POST, CAPTURE),
    entry("/api/v1/capture/pair", Method::POST, SECRETS),
    entry("/api/v1/capture/ping", Method::GET, CAPTURE),
    entry("/api/v1/capture/summary", Method::GET, CAPTURE),
    entry("/api/v1/categories", Method::GET, CONFIG),
    entry("/api/v1/categories", Method::POST, CONFIG),
    entry("/api/v1/categories/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/categories/{id}", Method::PUT, CONFIG),
    entry("/api/v1/categories/{id}/postprocess", Method::PATCH, CONFIG),
    entry("/api/v1/categories/{id}/seeding", Method::DELETE, CONFIG),
    entry("/api/v1/categories/{id}/seeding", Method::PUT, CONFIG),
    entry("/api/v1/category-rules", Method::GET, CONFIG),
    entry("/api/v1/category-rules", Method::POST, CONFIG),
    entry("/api/v1/category-rules/test-regex", Method::POST, CONFIG),
    entry("/api/v1/category-rules/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/category-rules/{id}", Method::PUT, CONFIG),
    entry("/api/v1/collector/batches", Method::GET, QUEUE),
    entry("/api/v1/collector/batches", Method::POST, INTAKE),
    entry("/api/v1/collector/candidates", Method::DELETE, QUEUE),
    entry("/api/v1/collector/candidates", Method::GET, QUEUE),
    entry("/api/v1/collector/candidates/check", Method::POST, QUEUE),
    entry("/api/v1/collector/candidates/move", Method::POST, QUEUE),
    entry("/api/v1/collector/candidates/reorder", Method::POST, QUEUE),
    entry("/api/v1/collector/candidates/{id}", Method::DELETE, QUEUE),
    entry("/api/v1/collector/candidates/{id}", Method::PATCH, QUEUE),
    entry(
        "/api/v1/collector/candidates/{id}/auth-profile",
        Method::PUT,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/enqueue",
        Method::POST,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/listing",
        Method::GET,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/listing/plan",
        Method::PUT,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/media",
        Method::GET,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/media/output-preview",
        Method::POST,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/media/preview",
        Method::POST,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/media/selection",
        Method::PUT,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/mirror",
        Method::DELETE,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/mirror",
        Method::POST,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/mirror/dissolve",
        Method::POST,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/replay-consent",
        Method::DELETE,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/replay-consent",
        Method::POST,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/replay-preview",
        Method::GET,
        SECRETS,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/torrent",
        Method::GET,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/torrent/plan",
        Method::PUT,
        QUEUE,
    ),
    entry(
        "/api/v1/collector/candidates/{id}/torrent/resolve",
        Method::POST,
        QUEUE,
    ),
    entry("/api/v1/collector/entries/reorder", Method::POST, QUEUE),
    entry("/api/v1/collector/mirror-preference", Method::GET, QUEUE),
    entry("/api/v1/collector/mirror-preference", Method::PUT, QUEUE),
    entry("/api/v1/collector/packages", Method::GET, QUEUE),
    entry("/api/v1/collector/packages/bulk", Method::POST, QUEUE),
    entry("/api/v1/collector/packages/enqueue", Method::POST, QUEUE),
    entry("/api/v1/collector/packages/regroup", Method::POST, QUEUE),
    entry("/api/v1/collector/packages/reorder", Method::POST, QUEUE),
    entry("/api/v1/collector/packages/{id}", Method::DELETE, QUEUE),
    entry("/api/v1/collector/packages/{id}", Method::PATCH, QUEUE),
    entry(
        "/api/v1/collector/packages/{id}/enqueue",
        Method::POST,
        QUEUE,
    ),
    entry("/api/v1/containers/import", Method::POST, INTAKE),
    // Diagnostics are the service itself (RD-110-02): the log store names hosts, files and
    // failures across the whole installation, and the bundle carries the configuration.
    entry("/api/v1/diagnostics/bundle", Method::POST, ADMIN),
    entry("/api/v1/diagnostics/bundle/preview", Method::GET, ADMIN),
    entry("/api/v1/diagnostics/bundles/{name}", Method::GET, ADMIN),
    entry("/api/v1/diagnostics/logs", Method::GET, ADMIN),
    entry("/api/v1/diagnostics/logs/clear", Method::POST, ADMIN),
    entry("/api/v1/dlc/import", Method::POST, INTAKE),
    entry("/api/v1/downloads", Method::GET, READ),
    entry("/api/v1/downloads", Method::POST, INTAKE),
    entry("/api/v1/downloads/bulk", Method::POST, QUEUE),
    entry("/api/v1/downloads/extract", Method::POST, QUEUE),
    entry("/api/v1/downloads/rates", Method::GET, READ),
    entry("/api/v1/downloads/reorder", Method::POST, QUEUE),
    entry("/api/v1/downloads/summary", Method::GET, READ),
    entry("/api/v1/downloads/{id}", Method::DELETE, QUEUE),
    entry("/api/v1/downloads/{id}", Method::PATCH, QUEUE),
    entry("/api/v1/downloads/{id}/auth-profile", Method::PUT, QUEUE),
    entry("/api/v1/downloads/{id}/cancel", Method::POST, QUEUE),
    entry("/api/v1/downloads/{id}/pause", Method::POST, QUEUE),
    entry("/api/v1/downloads/{id}/reset", Method::POST, QUEUE),
    entry("/api/v1/downloads/{id}/resume", Method::POST, QUEUE),
    entry("/api/v1/downloads/{id}/seeding/stop", Method::POST, QUEUE),
    entry("/api/v1/downloads/{id}/torrent", Method::GET, READ),
    entry("/api/v1/downloads/{id}/torrent/peers", Method::GET, READ),
    entry("/api/v1/downloads/{id}/torrent/pieces", Method::GET, READ),
    entry("/api/v1/downloads/{id}/torrent/plan", Method::PUT, QUEUE),
    entry(
        "/api/v1/downloads/{id}/torrent/seeding",
        Method::DELETE,
        QUEUE,
    ),
    entry("/api/v1/downloads/{id}/torrent/seeding", Method::GET, READ),
    entry("/api/v1/downloads/{id}/torrent/seeding", Method::PUT, QUEUE),
    entry("/api/v1/downloads/{id}/torrent/stats", Method::GET, READ),
    entry("/api/v1/downloads/{id}/torrent/trackers", Method::GET, READ),
    entry(
        "/api/v1/downloads/{id}/torrent/trackers",
        Method::PUT,
        QUEUE,
    ),
    entry(
        "/api/v1/downloads/{id}/torrent/trackers/reannounce",
        Method::POST,
        QUEUE,
    ),
    entry(
        "/api/v1/downloads/{id}/torrent/trackers/scrape",
        Method::POST,
        QUEUE,
    ),
    entry("/api/v1/events", Method::GET, READ),
    entry("/api/v1/health", Method::GET, PUBLIC),
    entry("/api/v1/hotfolders", Method::GET, CONFIG),
    entry("/api/v1/hotfolders", Method::POST, CONFIG),
    entry("/api/v1/hotfolders/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/hotfolders/{id}", Method::PUT, CONFIG),
    // The one route the scrape scope reaches, and the one route `api:read` does not: a
    // Prometheus target is a credential that lives in a configuration file for years, so it
    // gets an island of its own (RD-110-01).
    entry("/api/v1/metrics", Method::GET, METRICS),
    entry("/api/v1/mfa", Method::GET, SECRETS),
    entry("/api/v1/mfa/credentials/{id}", Method::DELETE, SECRETS),
    entry("/api/v1/mfa/disable", Method::POST, SECRETS),
    entry("/api/v1/mfa/passkey", Method::POST, SECRETS),
    entry("/api/v1/mfa/passkey/confirm", Method::POST, SECRETS),
    entry("/api/v1/mfa/recovery-codes", Method::POST, SECRETS),
    entry("/api/v1/mfa/totp", Method::POST, SECRETS),
    entry("/api/v1/mfa/totp/{id}/confirm", Method::POST, SECRETS),
    entry("/api/v1/notifications/deliveries", Method::GET, CONFIG),
    // Reading the history is configuration work; throwing it away costs what the other clears
    // cost (RD-130-08).
    entry(
        "/api/v1/notifications/deliveries/clear",
        Method::POST,
        ADMIN,
    ),
    entry("/api/v1/notifications/destinations", Method::GET, CONFIG),
    entry("/api/v1/notifications/rules", Method::GET, CONFIG),
    entry("/api/v1/notifications/rules", Method::POST, CONFIG),
    entry("/api/v1/notifications/rules/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/notifications/rules/{id}", Method::PUT, CONFIG),
    entry("/api/v1/notifications/targets", Method::GET, CONFIG),
    entry("/api/v1/notifications/targets", Method::POST, CONFIG),
    entry("/api/v1/notifications/targets/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/notifications/targets/{id}", Method::PUT, CONFIG),
    entry(
        "/api/v1/notifications/targets/{id}/test",
        Method::POST,
        CONFIG,
    ),
    entry("/api/v1/nzb/imports", Method::GET, QUEUE),
    entry("/api/v1/nzb/imports", Method::POST, INTAKE),
    entry("/api/v1/nzb/imports/{id}", Method::DELETE, QUEUE),
    entry("/api/v1/nzb/imports/{id}", Method::PATCH, QUEUE),
    entry("/api/v1/nzb/imports/{id}/enqueue", Method::POST, QUEUE),
    entry("/api/v1/nzb/imports/{id}/files", Method::GET, QUEUE),
    entry("/api/v1/nzb/imports/{id}/postprocess", Method::GET, QUEUE),
    // The OAuth redirect lands here, and it costs the same scope as every other route that
    // touches an account's credentials. It could have been argued the other way -- a provider
    // sends the browser back with no idea what a session is, and a session that lapsed while
    // the person was away loses the code -- but the request arrives in the browser that
    // started the flow seconds earlier, so the cookie is there in every ordinary case. Making
    // it public would put an unauthenticated token exchange in the surface for a convenience
    // nobody usually needs, and a lost code costs one more press of the sign-in button.
    entry("/api/v1/oauth/callback", Method::GET, SECRETS),
    entry("/api/v1/openapi.json", Method::GET, PUBLIC),
    entry("/api/v1/packages", Method::GET, READ),
    entry("/api/v1/packages/bulk", Method::POST, QUEUE),
    entry("/api/v1/packages/clear", Method::POST, QUEUE),
    entry("/api/v1/packages/delete", Method::POST, QUEUE),
    entry("/api/v1/packages/extract", Method::POST, QUEUE),
    entry("/api/v1/packages/reorder", Method::POST, QUEUE),
    entry("/api/v1/packages/{id}", Method::DELETE, QUEUE),
    entry("/api/v1/packages/{id}", Method::PATCH, QUEUE),
    entry("/api/v1/packages/{id}/extract", Method::POST, QUEUE),
    entry("/api/v1/packages/{id}/extract/force", Method::POST, QUEUE),
    entry("/api/v1/packages/{id}/folder", Method::POST, QUEUE),
    entry("/api/v1/packages/{id}/postprocess", Method::GET, READ),
    entry("/api/v1/plugins", Method::GET, ADMIN),
    entry("/api/v1/plugins/i18n/{locale}", Method::GET, READ),
    entry("/api/v1/plugins/install", Method::POST, ADMIN),
    entry("/api/v1/plugins/keys", Method::GET, SECRETS),
    entry("/api/v1/plugins/keys/{key_id}", Method::DELETE, SECRETS),
    // The trust store's second axis, so the same scope as the keys: withdrawing a package is
    // the same kind of decision as withdrawing the key that signed it.
    entry("/api/v1/plugins/revocations", Method::GET, SECRETS),
    entry("/api/v1/plugins/revocations", Method::POST, SECRETS),
    entry(
        "/api/v1/plugins/revocations/{digest}",
        Method::DELETE,
        SECRETS,
    ),
    entry("/api/v1/plugins/{id}", Method::PATCH, ADMIN),
    entry("/api/v1/plugins/{id}/executions", Method::GET, ADMIN),
    entry("/api/v1/plugins/{id}/{version}", Method::DELETE, ADMIN),
    entry("/api/v1/postprocess/plugin-steps", Method::GET, QUEUE),
    entry("/api/v1/postprocess/queue", Method::GET, READ),
    entry("/api/v1/postprocess/scripts", Method::GET, QUEUE),
    entry(
        "/api/v1/postprocess/upload-destinations",
        Method::GET,
        QUEUE,
    ),
    entry("/api/v1/power/cancel", Method::POST, ADMIN),
    entry("/api/v1/power/status", Method::GET, ADMIN),
    entry("/api/v1/providers", Method::GET, CONFIG),
    entry("/api/v1/proxy-profiles", Method::GET, SECRETS),
    entry("/api/v1/proxy-profiles", Method::POST, SECRETS),
    entry("/api/v1/proxy-profiles/{id}", Method::DELETE, SECRETS),
    entry("/api/v1/proxy-profiles/{id}", Method::PUT, SECRETS),
    entry("/api/v1/reconnect", Method::GET, ADMIN),
    entry("/api/v1/reconnect", Method::POST, ADMIN),
    entry("/api/v1/remote-credentials", Method::GET, SECRETS),
    entry("/api/v1/remote-credentials", Method::POST, SECRETS),
    entry("/api/v1/remote-credentials/ssh-hosts", Method::GET, SECRETS),
    entry(
        "/api/v1/remote-credentials/ssh-hosts",
        Method::POST,
        SECRETS,
    ),
    entry(
        "/api/v1/remote-credentials/ssh-hosts/{host}/{port}/{algorithm}",
        Method::DELETE,
        SECRETS,
    ),
    entry("/api/v1/remote-credentials/{id}", Method::DELETE, SECRETS),
    entry("/api/v1/remote-credentials/{id}", Method::PUT, SECRETS),
    entry(
        "/api/v1/remote-credentials/{id}/test",
        Method::POST,
        SECRETS,
    ),
    // The same area as the accounts they run on: the list says which accounts hold work at
    // which provider, and `discard` deletes at that provider on a confirmed request.
    entry("/api/v1/remote-jobs", Method::GET, SECRETS),
    // Which providers can take a job at all (RD-120-23). `CONFIG`, not `SECRETS` like the four
    // routes around it, and the difference is what the answer is *about*. Those act on one
    // account's jobs; this one names no account, reads no credential and touches no row -- it
    // is the key set of the runner table, which fills itself from the `claims` of installed
    // manifests. That is the same kind of fact, from the same source, as
    // `GET /api/v1/providers` above, which is already `CONFIG`.
    //
    // The deciding argument is that `/api/v1/providers` lists *every* installed provider under
    // `CONFIG`. Demanding `SECRETS` for a strict subset of that would not close anything: a
    // caller who may not read this could read the superset next door. It would be an
    // inconsistency wearing the shape of a restriction.
    entry("/api/v1/remote-jobs/providers", Method::GET, CONFIG),
    entry("/api/v1/remote-jobs/{id}", Method::DELETE, SECRETS),
    entry("/api/v1/remote-jobs/{id}/choice", Method::POST, SECRETS),
    entry("/api/v1/remote-jobs/{id}/discard", Method::POST, SECRETS),
    entry("/api/v1/routing/export", Method::GET, ADMIN),
    entry("/api/v1/routing/import", Method::POST, ADMIN),
    entry("/api/v1/sessions", Method::GET, SECRETS),
    entry("/api/v1/sessions/revoke-others", Method::POST, SECRETS),
    entry("/api/v1/sessions/{id}", Method::DELETE, SECRETS),
    entry("/api/v1/settings", Method::GET, CONFIG),
    entry("/api/v1/settings", Method::PUT, CONFIG),
    entry("/api/v1/settings/export", Method::POST, ADMIN),
    entry("/api/v1/settings/import", Method::POST, ADMIN),
    entry("/api/v1/settings/reset", Method::POST, ADMIN),
    entry("/api/v1/setup/complete", Method::POST, ADMIN),
    entry("/api/v1/setup/status", Method::GET, CONFIG),
    entry(
        "/api/v1/site-rule-groups/{group}/enabled",
        Method::PUT,
        CONFIG,
    ),
    // Which pages this installation recognises, and the rules that do it: configuration, the
    // same as the routing rules and the hot folders it sits beside. The export is the one
    // step sharper case it is not — a rule holds no credential, only the person's own
    // patterns, so it stays where the rest of the area is.
    entry("/api/v1/site-rules", Method::GET, CONFIG),
    entry("/api/v1/site-rules", Method::POST, CONFIG),
    entry("/api/v1/site-rules/export", Method::GET, CONFIG),
    entry("/api/v1/site-rules/import", Method::POST, CONFIG),
    // The trial run fetches a page the person named, through the rule they wrote. It reaches
    // the network, so it costs what changing the rule costs and not what reading it does.
    entry("/api/v1/site-rules/test", Method::POST, CONFIG),
    entry("/api/v1/site-rules/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/site-rules/{id}", Method::PUT, CONFIG),
    entry("/api/v1/site-rules/{id}/enabled", Method::PUT, CONFIG),
    entry("/api/v1/stats/transfers", Method::GET, READ),
    // Reading the figures is a status page's business; throwing them away is not
    // (RD-120-34).
    entry("/api/v1/stats/transfers/clear", Method::POST, ADMIN),
    entry("/api/v1/storage-roots", Method::GET, CONFIG),
    entry("/api/v1/storage-roots", Method::POST, CONFIG),
    entry("/api/v1/storage-roots/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/storage-roots/{id}", Method::PUT, CONFIG),
    entry("/api/v1/storage/capacity", Method::GET, READ),
    entry(
        "/api/v1/storage/capacity/{target}/resume",
        Method::POST,
        QUEUE,
    ),
    entry("/api/v1/streams/channels", Method::GET, CONFIG),
    entry("/api/v1/streams/channels", Method::POST, CONFIG),
    entry("/api/v1/streams/channels/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/streams/channels/{id}", Method::PUT, CONFIG),
    entry("/api/v1/streams/export", Method::GET, ADMIN),
    entry("/api/v1/streams/import", Method::POST, ADMIN),
    entry("/api/v1/streams/record", Method::POST, INTAKE),
    entry("/api/v1/streams/runs", Method::GET, CONFIG),
    entry("/api/v1/streams/schedules", Method::GET, CONFIG),
    entry("/api/v1/streams/schedules", Method::POST, CONFIG),
    entry("/api/v1/streams/schedules/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/streams/schedules/{id}", Method::PUT, CONFIG),
    entry("/api/v1/subscriptions", Method::GET, CONFIG),
    entry("/api/v1/subscriptions", Method::POST, CONFIG),
    entry("/api/v1/subscriptions/caps", Method::POST, CONFIG),
    entry("/api/v1/subscriptions/export", Method::GET, ADMIN),
    entry("/api/v1/subscriptions/import", Method::POST, ADMIN),
    entry("/api/v1/subscriptions/items/{id}", Method::PUT, CONFIG),
    entry("/api/v1/subscriptions/review-summary", Method::GET, CONFIG),
    entry("/api/v1/subscriptions/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/subscriptions/{id}", Method::PUT, CONFIG),
    entry("/api/v1/subscriptions/{id}/caps", Method::POST, CONFIG),
    entry("/api/v1/subscriptions/{id}/disable", Method::POST, CONFIG),
    entry("/api/v1/subscriptions/{id}/enable", Method::POST, CONFIG),
    entry("/api/v1/subscriptions/{id}/history", Method::DELETE, CONFIG),
    entry("/api/v1/subscriptions/{id}/items", Method::GET, CONFIG),
    entry("/api/v1/subscriptions/{id}/items/page", Method::GET, CONFIG),
    entry(
        "/api/v1/subscriptions/{id}/items/pending",
        Method::PUT,
        CONFIG,
    ),
    entry("/api/v1/subscriptions/{id}/poll", Method::POST, QUEUE),
    entry("/api/v1/subscriptions/{id}/runs", Method::GET, CONFIG),
    // Which build runs and what it ships (RD-130-12). Not public like the health check: the
    // commit and the build time name the exact tree. Not configuration either, so a status
    // page's token may show them.
    entry("/api/v1/system/about", Method::GET, READ),
    entry("/api/v1/system/about/licenses", Method::GET, READ),
    // Counting what a clear would remove discloses how much the installation has done and
    // how long it has run, which is the same disclosure the audit log is priced for.
    entry("/api/v1/system/data-reset", Method::GET, ADMIN),
    entry("/api/v1/system/media", Method::GET, READ),
    entry("/api/v1/system/tools", Method::GET, READ),
    entry(
        "/api/v1/system/tools/manifest/refresh",
        Method::POST,
        CONFIG,
    ),
    entry("/api/v1/system/tools/{name}/activate", Method::POST, CONFIG),
    entry("/api/v1/system/tools/{name}/install", Method::POST, CONFIG),
    entry("/api/v1/system/tools/{name}/rollback", Method::POST, CONFIG),
    entry("/api/v1/torrents/capabilities", Method::GET, READ),
    entry("/api/v1/torrents/import", Method::POST, INTAKE),
    entry("/api/v1/torrents/network/interfaces", Method::GET, CONFIG),
    entry("/api/v1/torrents/network/status", Method::GET, READ),
    entry("/api/v1/usenet/servers", Method::GET, SECRETS),
    entry("/api/v1/usenet/servers", Method::POST, SECRETS),
    entry("/api/v1/usenet/servers/{id}", Method::DELETE, SECRETS),
    entry("/api/v1/usenet/servers/{id}", Method::PUT, SECRETS),
    entry("/api/v1/usenet/servers/{id}/test", Method::POST, SECRETS),
];

/// The requirement for a matched route, or `None` if the table does not know it.
///
/// `None` is a policy gap and the caller must fail closed on it. It cannot happen for a
/// documented route — the tests below forbid it — but "cannot happen" is not a thing to
/// build an authorisation decision on.
pub(crate) fn requirement(path: &str, method: &Method) -> Option<Requirement> {
    ROUTE_POLICY
        .iter()
        .find(|entry| entry.path == path && entry.method == method)
        .map(|entry| entry.requires)
}

/// The scope string a route costs, or `None` for a route that needs no credential.
///
/// Exposed for the conformance test in `crates/rd-api/tests/scope_matrix.rs`, which walks the
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
pub(crate) fn operations_reachable_by(scope: rd_core::Scope) -> u32 {
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
mod tests {
    use std::collections::BTreeSet;

    use super::{Method, ROUTE_POLICY, Requirement, Scope, requirement};

    /// Routes the router registers that utoipa does not document, with the reason.
    ///
    /// The exhaustiveness test allows exactly these four to be absent from the OpenAPI document
    /// and nothing else, so an endpoint that quietly stops being documented is still caught.
    const UNDOCUMENTED: &[(&str, &str)] = &[
        (
            "/api/v1/events",
            "server-sent events; utoipa has no stream response type",
        ),
        (
            "/api/v1/capture/events",
            "server-sent events, capture surface",
        ),
        (
            "/api/v1/capture/nzb",
            "multipart upload; documented in the capture contract instead",
        ),
        (
            "/api/v1/openapi.json",
            "serves the document, so it cannot appear inside it",
        ),
    ];

    /// Every `(path, method)` in the document, as the router registered them.
    ///
    /// Read out of the serialised JSON rather than utoipa's own types on purpose: the JSON is
    /// the contract — it is what `scripts/api-contract.sh` writes and what the frontend types
    /// are generated from — and it does not move under us when utoipa reshapes `PathItem`.
    fn documented() -> BTreeSet<(String, Method)> {
        let document = serde_json::to_value(crate::openapi_document()).expect("serialise");
        let paths = document
            .get("paths")
            .and_then(serde_json::Value::as_object)
            .expect("the document has paths");
        let mut operations = BTreeSet::new();
        for (path, item) in paths {
            let item = item.as_object().expect("path item");
            for method in item.keys() {
                let Ok(method) = Method::from_bytes(method.to_uppercase().as_bytes()) else {
                    continue;
                };
                operations.insert((path.clone(), method));
            }
        }
        assert!(
            operations.len() > 200,
            "only {} operations were found; the document did not serialise as expected",
            operations.len()
        );
        operations
    }

    fn policy_keys() -> BTreeSet<(String, Method)> {
        ROUTE_POLICY
            .iter()
            .map(|entry| (entry.path.to_owned(), entry.method.clone()))
            .collect()
    }

    /// Adding a route without deciding what it costs must not be possible.
    ///
    /// This is the whole reason the policy is a table. A per-handler check that someone
    /// forgets leaves a route with no check at all, and nothing says so; a missing table row
    /// fails here, before the route can ship.
    #[test]
    fn every_documented_operation_has_a_policy_entry() {
        let missing: Vec<_> = documented()
            .difference(&policy_keys())
            .map(|(path, method)| format!("{method} {path}"))
            .collect();
        assert!(
            missing.is_empty(),
            "these operations have no scope decision in ROUTE_POLICY:\n  {}",
            missing.join("\n  ")
        );
    }

    /// And the table must not describe routes that no longer exist.
    ///
    /// A stale entry is not merely untidy: it is a scope decision that reads as coverage of
    /// something, and the something is gone. The four exceptions are routes the router really
    /// does serve and utoipa cannot document; each carries its reason in [`UNDOCUMENTED`].
    #[test]
    fn every_policy_entry_is_a_documented_operation_or_a_named_exception() {
        let documented = documented();
        let allowed: BTreeSet<&str> = UNDOCUMENTED.iter().map(|(path, _)| *path).collect();
        let stale: Vec<_> = policy_keys()
            .into_iter()
            .filter(|key| !documented.contains(key))
            .filter(|(path, _)| !allowed.contains(path.as_str()))
            .map(|(path, method)| format!("{method} {path}"))
            .collect();
        assert!(
            stale.is_empty(),
            "ROUTE_POLICY describes operations the API does not have:\n  {}",
            stale.join("\n  ")
        );
    }

    /// Every documented exception has to actually be in the table, or it is just a comment.
    #[test]
    fn every_undocumented_route_is_still_in_the_table() {
        let paths: BTreeSet<&str> = ROUTE_POLICY.iter().map(|entry| entry.path).collect();
        for (path, reason) in UNDOCUMENTED {
            assert!(paths.contains(path), "{path} is exempted but has no entry");
            assert!(!reason.is_empty(), "{path} is exempted without a reason");
        }
    }

    /// The two endpoints RD-108-19 withdrew are gone from both the document and the table.
    ///
    /// Neither had a client. `plan/preview` duplicated the `PUT` that already reports which
    /// files a pattern drops, and `replay-restart` duplicated `downloads/{id}/reset` — which
    /// does strictly more and, unlike it, clears the scheduler's stop reason. Named here so
    /// re-adding either is a decision somebody makes on purpose.
    #[test]
    fn the_endpoints_withdrawn_for_having_no_caller_stay_withdrawn() {
        let withdrawn = [
            (
                "/api/v1/collector/candidates/{id}/torrent/plan/preview",
                Method::POST,
            ),
            ("/api/v1/downloads/{id}/replay-restart", Method::POST),
        ];
        let documented = documented();
        let policy = policy_keys();
        for (path, method) in withdrawn {
            let key = (path.to_owned(), method);
            assert!(!documented.contains(&key), "{path} is documented again");
            assert!(!policy.contains(&key), "{path} is in ROUTE_POLICY again");
        }
    }

    /// One decision per route, or the lookup silently picks whichever came first.
    #[test]
    fn no_route_is_listed_twice() {
        assert_eq!(
            policy_keys().len(),
            ROUTE_POLICY.len(),
            "ROUTE_POLICY contains a duplicate (path, method)"
        );
    }

    /// A sorted table makes a diff show the addition rather than a reshuffle.
    #[test]
    fn the_table_is_sorted() {
        let actual: Vec<_> = ROUTE_POLICY
            .iter()
            .map(|entry| (entry.path, entry.method.as_str()))
            .collect();
        let mut sorted = actual.clone();
        sorted.sort_unstable();
        assert_eq!(
            actual, sorted,
            "ROUTE_POLICY is not sorted by path then method"
        );
    }

    /// Only the five pre-authentication routes may cost nothing.
    ///
    /// `Public` is the one value that turns the check off, so it is worth naming exactly
    /// which routes carry it rather than trusting that nobody adds a sixth.
    #[test]
    fn exactly_the_pre_authentication_routes_are_public() {
        let public: BTreeSet<&str> = ROUTE_POLICY
            .iter()
            .filter(|entry| entry.requires == Requirement::Public)
            .map(|entry| entry.path)
            .collect();
        let expected: BTreeSet<&str> = [
            "/api/v1/auth/login",
            // Ending a session you do not have is a no-op, and demanding a credential would
            // leave someone whose session already lapsed unable to clear the stale cookie
            // their browser keeps sending.
            "/api/v1/auth/logout",
            // Both halves of a passkey sign-in, for the same reason the password login is
            // public: they are how a caller stops being anonymous. The challenge half is the
            // one that allocates server state for an anonymous caller, which is why the
            // ceremony store is bounded rather than merely expiring.
            "/api/v1/auth/passkey/challenge",
            "/api/v1/auth/passkey/login",
            "/api/v1/auth/setup",
            "/api/v1/auth/status",
            "/api/v1/health",
            "/api/v1/openapi.json",
        ]
        .into_iter()
        .collect();
        assert_eq!(public, expected);
    }

    /// The capture scope belongs to the capture surface and to nothing else.
    ///
    /// Both directions matter: a capture token must not reach the API, and an API scope must
    /// not reach the extension endpoints. The router enforces this with a separate layer; if
    /// the table ever disagreed with the router, the table would be the lie.
    #[test]
    fn the_capture_scope_covers_exactly_the_capture_router() {
        let capture: BTreeSet<&str> = ROUTE_POLICY
            .iter()
            .filter(|entry| entry.requires == Requirement::Scope(Scope::Capture))
            .map(|entry| entry.path)
            .collect();
        let expected: BTreeSet<&str> = [
            "/api/v1/capture/batches",
            // A browser session answers a request a person opened; it cannot start one.
            "/api/v1/capture/browser-sessions",
            "/api/v1/capture/browser-sessions/{id}",
            "/api/v1/capture/browser-sessions/{id}/decline",
            "/api/v1/capture/captchas",
            "/api/v1/capture/captchas/{id}/no-widget",
            "/api/v1/capture/captchas/{id}/skip",
            "/api/v1/capture/captchas/{id}/token",
            "/api/v1/capture/cookies",
            "/api/v1/capture/events",
            // A file only the browser could load: its bytes, or its address and cookies.
            "/api/v1/capture/file",
            "/api/v1/capture/nzb",
            "/api/v1/capture/ping",
            // Figures for the tray: counts and byte totals, nothing that names a file.
            "/api/v1/capture/summary",
        ]
        .into_iter()
        .collect();
        assert_eq!(capture, expected);
    }

    /// The read-only surface must not have quietly grown past what it replaced.
    ///
    /// The allowlist this table supersedes existed so a token pasted into a status page could
    /// not enumerate the installation. Every one of its routes still has to be readable, and
    /// nothing that describes *configuration* may have joined them.
    #[test]
    fn the_read_scope_still_covers_the_old_allowlist_and_no_configuration() {
        for path in [
            "/api/v1/downloads",
            "/api/v1/downloads/summary",
            "/api/v1/events",
            "/api/v1/packages",
            "/api/v1/packages/{id}/postprocess",
            "/api/v1/postprocess/queue",
            "/api/v1/storage/capacity",
            "/api/v1/system/media",
        ] {
            assert_eq!(
                requirement(path, &Method::GET),
                Some(Requirement::Scope(Scope::Read)),
                "{path} was readable before this table and must stay so"
            );
        }
        // Spot-checks on the boundary the allowlist's own comment drew.
        for path in [
            "/api/v1/accounts",
            "/api/v1/collector/candidates",
            "/api/v1/settings",
            "/api/v1/auth-profiles",
            "/api/v1/proxy-profiles",
        ] {
            assert_ne!(
                requirement(path, &Method::GET),
                Some(Requirement::Scope(Scope::Read)),
                "{path} must not be reachable with a read-only token"
            );
        }
    }

    /// Listing a credential resource discloses where credentials are; that is not free.
    #[test]
    fn reading_a_credential_resource_costs_the_secrets_scope() {
        for path in [
            "/api/v1/accounts",
            "/api/v1/auth-profiles",
            "/api/v1/proxy-profiles",
            "/api/v1/remote-credentials",
            "/api/v1/usenet/servers",
            "/api/v1/api-tokens",
            "/api/v1/captcha-config",
            "/api/v1/plugins/keys",
        ] {
            assert_eq!(
                requirement(path, &Method::GET),
                Some(Requirement::Scope(Scope::Secrets)),
                "{path}"
            );
        }
    }

    /// A whole-configuration export is a credential exfiltration route in one request.
    #[test]
    fn whole_configuration_transfer_costs_administration() {
        for (path, method) in [
            ("/api/v1/settings/export", Method::POST),
            ("/api/v1/settings/import", Method::POST),
            ("/api/v1/settings/reset", Method::POST),
            ("/api/v1/routing/export", Method::GET),
            ("/api/v1/routing/import", Method::POST),
        ] {
            assert_eq!(
                requirement(path, &method),
                Some(Requirement::Scope(Scope::Admin)),
                "{method} {path}"
            );
        }
    }

    /// An unknown route yields no decision, so the caller can fail closed on it.
    #[test]
    fn an_unknown_route_has_no_requirement() {
        assert_eq!(requirement("/api/v1/nothing", &Method::GET), None);
        // Right path, wrong method: still no decision, rather than the other method's.
        assert_eq!(requirement("/api/v1/health", &Method::DELETE), None);
    }
}
