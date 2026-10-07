//! The table itself: every route and the scope it costs.

use super::*;

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
const CAPTURE_QUEUE: Requirement = Requirement::Scope(Scope::CaptureQueue);
const METRICS: Requirement = Requirement::Scope(Scope::Metrics);

/// Every route, with the scope it costs. Sorted by path then method.
pub const ROUTE_POLICY: &[RoutePolicy] = &[
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
    // The identity provider (RD-190-15): its configuration and the bound identity create a way
    // in, so they cost what a passkey enrolment costs; the handlers demand a session and the
    // password on top (O-CONF). The start and the callback are the sign-in itself.
    entry("/api/v1/auth/oidc", Method::DELETE, SECRETS),
    entry("/api/v1/auth/oidc", Method::GET, SECRETS),
    entry("/api/v1/auth/oidc", Method::PUT, SECRETS),
    entry("/api/v1/auth/oidc/callback", Method::GET, PUBLIC),
    entry("/api/v1/auth/oidc/identity", Method::DELETE, SECRETS),
    entry("/api/v1/auth/oidc/link", Method::POST, SECRETS),
    entry("/api/v1/auth/oidc/start", Method::GET, PUBLIC),
    entry("/api/v1/auth/passkey/challenge", Method::POST, PUBLIC),
    entry("/api/v1/auth/passkey/login", Method::POST, PUBLIC),
    // Not public, unlike the rest of `/auth`: a change of the administrator password is a
    // credential write, so it costs what handing out a credential costs (RD-120-22). The
    // current password is demanded on top of that, inside the handler.
    entry("/api/v1/auth/password", Method::POST, SECRETS),
    entry("/api/v1/auth/password-login/off", Method::POST, SECRETS),
    // Back on only from this machine (D3): the local control token opens it, priced like the
    // other routes it opens, and the handler refuses every other credential.
    entry("/api/v1/auth/password-login/on", Method::POST, ADMIN),
    // A new password without the current one (RD-190-24): only the local control token opens
    // it, priced like the way back in above, and the handler refuses every other credential.
    entry("/api/v1/auth/password/reset", Method::POST, ADMIN),
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
    // The full backup (RD-160-01): an archive of the whole installation, credentials included,
    // written to a folder of the caller's choosing. Every route of it is administration, the
    // price the settings export already has -- reading the schedule included, since it names
    // where the archives lie.
    // Its destinations, archives and verifications (RD-160-02) name where the archives lie
    // and can delete them through retention: the same price.
    entry("/api/v1/backups", Method::GET, ADMIN),
    entry("/api/v1/backups", Method::PUT, ADMIN),
    entry("/api/v1/backups/archives", Method::GET, ADMIN),
    entry("/api/v1/backups/archives/{id}/verify", Method::POST, ADMIN),
    entry("/api/v1/backups/destinations", Method::POST, ADMIN),
    entry("/api/v1/backups/destinations/{id}", Method::DELETE, ADMIN),
    entry("/api/v1/backups/destinations/{id}", Method::PUT, ADMIN),
    entry(
        "/api/v1/backups/destinations/{id}/retention",
        Method::GET,
        ADMIN,
    ),
    entry("/api/v1/backups/passphrase", Method::PUT, ADMIN),
    // Restoring one (RD-160-03) replaces the whole installation at the next start and reads an
    // archive with every credential: administration, like writing one.
    entry("/api/v1/backups/restore", Method::DELETE, ADMIN),
    entry("/api/v1/backups/restore", Method::GET, ADMIN),
    entry("/api/v1/backups/restore", Method::POST, ADMIN),
    entry("/api/v1/backups/restore/preview", Method::POST, ADMIN),
    entry("/api/v1/backups/restore/test", Method::POST, ADMIN),
    entry("/api/v1/backups/restore/uploads", Method::POST, ADMIN),
    entry(
        "/api/v1/backups/restore/uploads/{id}",
        Method::DELETE,
        ADMIN,
    ),
    entry("/api/v1/backups/restore/uploads/{id}", Method::PUT, ADMIN),
    entry("/api/v1/backups/runs", Method::GET, ADMIN),
    entry("/api/v1/backups/runs", Method::POST, ADMIN),
    entry("/api/v1/backups/verifications", Method::GET, ADMIN),
    entry("/api/v1/bandwidth/capabilities", Method::GET, READ),
    entry("/api/v1/bandwidth/manual", Method::DELETE, CONFIG),
    entry("/api/v1/bandwidth/manual", Method::PUT, CONFIG),
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
    // The tray's "pause all" and "resume all" (RD-1100-06): only for an agent paired with them,
    // and the only queue routes a capture token reaches.
    entry("/api/v1/capture/queue/pause", Method::POST, CAPTURE_QUEUE),
    entry("/api/v1/capture/queue/resume", Method::POST, CAPTURE_QUEUE),
    entry("/api/v1/capture/summary", Method::GET, CAPTURE),
    entry("/api/v1/categories", Method::GET, CONFIG),
    entry("/api/v1/categories", Method::POST, CONFIG),
    entry("/api/v1/categories/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/categories/{id}", Method::PUT, CONFIG),
    entry(
        "/api/v1/categories/{id}/collision-policy",
        Method::PUT,
        CONFIG,
    ),
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
    entry("/api/v1/collision-policies", Method::GET, READ),
    entry("/api/v1/collision-prompts", Method::GET, READ),
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
    entry(
        "/api/v1/downloads/{id}/collision-decision",
        Method::POST,
        QUEUE,
    ),
    entry("/api/v1/downloads/{id}/dedupe", Method::POST, QUEUE),
    entry("/api/v1/downloads/{id}/duplicates", Method::GET, READ),
    entry("/api/v1/downloads/{id}/pause", Method::POST, QUEUE),
    entry("/api/v1/downloads/{id}/reset", Method::POST, QUEUE),
    entry("/api/v1/downloads/{id}/resume", Method::POST, QUEUE),
    entry("/api/v1/downloads/{id}/seeding/stop", Method::POST, QUEUE),
    entry("/api/v1/downloads/{id}/sources", Method::GET, READ),
    entry("/api/v1/downloads/{id}/torrent", Method::GET, READ),
    entry("/api/v1/downloads/{id}/torrent/move", Method::POST, QUEUE),
    entry("/api/v1/downloads/{id}/torrent/peers", Method::GET, READ),
    entry("/api/v1/downloads/{id}/torrent/pieces", Method::GET, READ),
    entry("/api/v1/downloads/{id}/torrent/plan", Method::PUT, QUEUE),
    entry(
        "/api/v1/downloads/{id}/torrent/recheck",
        Method::POST,
        QUEUE,
    ),
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
    entry("/api/v1/duplicates/lookup", Method::POST, QUEUE),
    entry("/api/v1/events", Method::GET, READ),
    entry("/api/v1/health", Method::GET, PUBLIC),
    // The download history (RD-1100-04): reading it is a read, adding an entry again is an
    // intake like pasting its links, and emptying it is a clear like the other histories'.
    entry("/api/v1/history", Method::GET, READ),
    entry("/api/v1/history/clear", Method::POST, ADMIN),
    entry("/api/v1/history/{id}/readd", Method::POST, INTAKE),
    entry("/api/v1/hotfolders", Method::GET, CONFIG),
    entry("/api/v1/hotfolders", Method::POST, CONFIG),
    entry("/api/v1/hotfolders/{id}", Method::DELETE, CONFIG),
    entry("/api/v1/hotfolders/{id}", Method::PUT, CONFIG),
    // An indexer is an address and an API key: the credential scope, its `GET` included
    // (RD-180-19). Searching one and taking hits into the LinkGrabber is intake, like an NZB
    // upload -- neither discloses the key.
    entry("/api/v1/indexers", Method::GET, SECRETS),
    entry("/api/v1/indexers", Method::POST, SECRETS),
    entry("/api/v1/indexers/grab", Method::POST, INTAKE),
    entry("/api/v1/indexers/search", Method::POST, INTAKE),
    entry("/api/v1/indexers/{id}", Method::DELETE, SECRETS),
    entry("/api/v1/indexers/{id}", Method::PUT, SECRETS),
    entry("/api/v1/indexers/{id}/caps", Method::POST, SECRETS),
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
    entry(
        "/api/v1/notifications/deliveries/discard-pending",
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
    // An NZB handed to a provider spends that account exactly as a container handed to it
    // through `/accounts/{id}/remote-jobs` does, so it costs the same (RD-191-13).
    entry("/api/v1/nzb/imports/{id}/remote-job", Method::POST, SECRETS),
    // The OAuth redirect lands here, and it cannot cost a scope: it arrives from the provider's
    // site, and the session cookie is `SameSite=Strict`, so a browser does not send it on that
    // navigation. Priced `SECRETS` it refused every sign-in with the login switched on (security
    // audit 2026-09-30, finding 5). The credential is the `state` instead: issued to a caller
    // holding `api:secrets` when the flow began, at least 32 characters, answered once and
    // within fifteen minutes (`auth_flow_guard`).
    entry("/api/v1/oauth/callback", Method::GET, PUBLIC),
    // Object storage profiles hold key pairs, like the remote logins beside them (RD-150-04).
    entry("/api/v1/object-storage/profiles", Method::GET, SECRETS),
    entry("/api/v1/object-storage/profiles", Method::POST, SECRETS),
    entry(
        "/api/v1/object-storage/profiles/{id}",
        Method::DELETE,
        SECRETS,
    ),
    entry("/api/v1/object-storage/profiles/{id}", Method::PUT, SECRETS),
    entry(
        "/api/v1/object-storage/profiles/{id}/test",
        Method::POST,
        SECRETS,
    ),
    entry("/api/v1/openapi.json", Method::GET, PUBLIC),
    entry("/api/v1/packages", Method::GET, READ),
    entry("/api/v1/packages/bulk", Method::POST, QUEUE),
    entry("/api/v1/packages/clear", Method::POST, QUEUE),
    entry("/api/v1/packages/delete", Method::POST, QUEUE),
    entry("/api/v1/packages/extract", Method::POST, QUEUE),
    entry("/api/v1/packages/reorder", Method::POST, QUEUE),
    entry("/api/v1/packages/{id}", Method::DELETE, QUEUE),
    entry("/api/v1/packages/{id}", Method::PATCH, QUEUE),
    entry("/api/v1/packages/{id}/collision-policy", Method::GET, READ),
    entry("/api/v1/packages/{id}/collision-policy", Method::PUT, QUEUE),
    entry("/api/v1/packages/{id}/extract", Method::POST, QUEUE),
    entry("/api/v1/packages/{id}/extract/force", Method::POST, QUEUE),
    entry("/api/v1/packages/{id}/folder", Method::POST, QUEUE),
    entry("/api/v1/packages/{id}/postprocess", Method::GET, READ),
    // The NZB behind a package handed to a provider: the import route's cost (RD-191-13).
    entry("/api/v1/packages/{id}/remote-job", Method::POST, SECRETS),
    // A package's own speed limit (RD-1100-01): read like the package, set like its priority.
    entry("/api/v1/packages/{id}/speed-limit", Method::GET, READ),
    entry("/api/v1/packages/{id}/speed-limit", Method::PUT, QUEUE),
    entry("/api/v1/plugins", Method::GET, ADMIN),
    // Choosing which of the release's own services are installed (RD-160-05).
    entry("/api/v1/plugins/bundled", Method::GET, ADMIN),
    entry("/api/v1/plugins/bundled/install", Method::POST, ADMIN),
    entry("/api/v1/plugins/bundled/remove", Method::POST, ADMIN),
    entry("/api/v1/plugins/i18n/{locale}", Method::GET, READ),
    entry("/api/v1/plugins/install", Method::POST, ADMIN),
    entry("/api/v1/plugins/keys", Method::GET, SECRETS),
    entry("/api/v1/plugins/keys/{key_id}", Method::DELETE, SECRETS),
    // Repositories and the preview are administration, like installing: approving a
    // repository's key is the same kind of decision as confirming a plugin's (RD-140-01).
    entry("/api/v1/plugins/preview", Method::POST, ADMIN),
    entry("/api/v1/plugins/repositories", Method::GET, ADMIN),
    entry("/api/v1/plugins/repositories", Method::POST, ADMIN),
    entry("/api/v1/plugins/repositories/refresh", Method::POST, ADMIN),
    entry("/api/v1/plugins/repositories/settings", Method::PUT, ADMIN),
    entry("/api/v1/plugins/repositories/{id}", Method::DELETE, ADMIN),
    entry("/api/v1/plugins/repositories/{id}", Method::PATCH, ADMIN),
    entry(
        "/api/v1/plugins/repositories/{id}/install",
        Method::POST,
        ADMIN,
    ),
    entry(
        "/api/v1/plugins/repositories/{id}/preview",
        Method::POST,
        ADMIN,
    ),
    // The trust store's second axis, so the same scope as the keys: withdrawing a package is
    // the same kind of decision as withdrawing the key that signed it.
    entry("/api/v1/plugins/revocations", Method::GET, SECRETS),
    entry("/api/v1/plugins/revocations", Method::POST, SECRETS),
    entry(
        "/api/v1/plugins/revocations/{digest}",
        Method::DELETE,
        SECRETS,
    ),
    entry("/api/v1/plugins/superseded", Method::DELETE, ADMIN),
    entry("/api/v1/plugins/updates", Method::GET, ADMIN),
    // Whether every plugin installs its updates itself: the same decision as one plugin's
    // policy, so the same scope (RD-191-10).
    entry("/api/v1/plugins/updates/settings", Method::GET, ADMIN),
    entry("/api/v1/plugins/updates/settings", Method::PUT, ADMIN),
    entry("/api/v1/plugins/{id}", Method::PATCH, ADMIN),
    entry("/api/v1/plugins/{id}/executions", Method::GET, ADMIN),
    // Which build of a plugin runs is the same kind of decision as installing it (RD-140-02).
    entry(
        "/api/v1/plugins/{id}/lifecycle/activate",
        Method::POST,
        ADMIN,
    ),
    entry("/api/v1/plugins/{id}/lifecycle/policy", Method::PUT, ADMIN),
    entry(
        "/api/v1/plugins/{id}/lifecycle/rollback",
        Method::POST,
        ADMIN,
    ),
    entry(
        "/api/v1/plugins/{id}/lifecycle/stage",
        Method::DELETE,
        ADMIN,
    ),
    entry("/api/v1/plugins/{id}/lifecycle/stage", Method::POST, ADMIN),
    entry("/api/v1/plugins/{id}/lifecycle/trial", Method::POST, ADMIN),
    entry("/api/v1/plugins/{id}/superseded", Method::DELETE, ADMIN),
    entry("/api/v1/plugins/{id}/{version}", Method::DELETE, ADMIN),
    entry(
        "/api/v1/postprocess/malware-scanner/test",
        Method::POST,
        CONFIG,
    ),
    entry(
        "/api/v1/postprocess/package-name-preview",
        Method::POST,
        CONFIG,
    ),
    entry("/api/v1/postprocess/plugin-steps", Method::GET, QUEUE),
    entry("/api/v1/postprocess/queue", Method::GET, READ),
    entry("/api/v1/postprocess/scripts", Method::GET, QUEUE),
    entry("/api/v1/postprocess/sort-preview", Method::POST, CONFIG),
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
    entry("/api/v1/queue/pause", Method::DELETE, QUEUE),
    entry("/api/v1/queue/pause", Method::GET, READ),
    entry("/api/v1/queue/pause", Method::PUT, QUEUE),
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
    // Names every configured Usenet server with its quota, which is what listing the servers
    // costs: the figures are a status page's, the list of servers is not (RD-1100-05).
    entry("/api/v1/stats/usenet-servers", Method::GET, SECRETS),
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
    entry("/api/v1/storage/content-index/check", Method::POST, QUEUE),
    // Checking the index is queue work; throwing the index or the history away costs what the
    // other clears cost (RD-180-13).
    entry("/api/v1/storage/content-index/clear", Method::POST, ADMIN),
    entry("/api/v1/storage/link-support", Method::GET, READ),
    entry("/api/v1/storage/operations", Method::GET, READ),
    entry("/api/v1/storage/operations/clear", Method::POST, ADMIN),
    entry("/api/v1/storage/reuse", Method::GET, READ),
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
    entry("/api/v1/subscriptions/{id}/items/page", Method::GET, CONFIG),
    entry(
        "/api/v1/subscriptions/{id}/items/pending",
        Method::PUT,
        CONFIG,
    ),
    entry(
        "/api/v1/subscriptions/{id}/items/requeue",
        Method::POST,
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
    // Stopping the service and the backup before an update (RD-180-02, RD-180-03): the service
    // itself. Refused from anywhere but this machine by the handlers; the local control token
    // opens these two and nothing else (`crate::local_control`).
    entry("/api/v1/system/shutdown", Method::POST, ADMIN),
    entry("/api/v1/system/tools", Method::GET, READ),
    entry(
        "/api/v1/system/tools/manifest/refresh",
        Method::POST,
        CONFIG,
    ),
    entry("/api/v1/system/tools/{name}/activate", Method::POST, CONFIG),
    entry("/api/v1/system/tools/{name}/install", Method::POST, CONFIG),
    entry("/api/v1/system/tools/{name}/rollback", Method::POST, CONFIG),
    // Which version runs and which is offered, like the About page (RD-180-01); checking makes
    // the service reach out on the caller's word, and is the administrator's.
    entry("/api/v1/system/update", Method::GET, READ),
    entry("/api/v1/system/update/check", Method::POST, ADMIN),
    // Installing the offered update stops and replaces the service (RD-180-02); downloading it
    // ahead of the install is the first step of that.
    entry("/api/v1/system/update/download", Method::POST, ADMIN),
    entry("/api/v1/system/update/install", Method::POST, ADMIN),
    entry("/api/v1/system/update/prepare", Method::POST, ADMIN),
    entry("/api/v1/torrents/capabilities", Method::GET, READ),
    entry("/api/v1/torrents/import", Method::POST, INTAKE),
    entry("/api/v1/torrents/network/interfaces", Method::GET, CONFIG),
    entry("/api/v1/torrents/network/status", Method::GET, READ),
    entry("/api/v1/usenet/servers", Method::GET, SECRETS),
    entry("/api/v1/usenet/servers", Method::POST, SECRETS),
    entry("/api/v1/usenet/servers/{id}", Method::DELETE, SECRETS),
    entry("/api/v1/usenet/servers/{id}", Method::PUT, SECRETS),
    entry("/api/v1/usenet/servers/{id}/quota", Method::PUT, SECRETS),
    entry("/api/v1/usenet/servers/{id}/test", Method::POST, SECRETS),
];
