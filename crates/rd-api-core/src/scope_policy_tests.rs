use std::collections::BTreeSet;

use super::{Method, ROUTE_POLICY, Requirement, Scope, requirement};

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

/// Only the pre-authentication routes may cost nothing.
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
        // Both halves of a sign-in through the identity provider (RD-190-15): browser
        // navigations, the second one from the provider's site, which a `SameSite=Strict`
        // session cookie does not travel from. The callback's credential is the `state`
        // together with the `rd_oidc` binding of the browser that started it.
        "/api/v1/auth/oidc/callback",
        "/api/v1/auth/oidc/start",
        // Both halves of a passkey sign-in, for the same reason the password login is
        // public: they are how a caller stops being anonymous. The challenge half is the
        // one that allocates server state for an anonymous caller, which is why the
        // ceremony store is bounded rather than merely expiring.
        "/api/v1/auth/passkey/challenge",
        "/api/v1/auth/passkey/login",
        "/api/v1/auth/setup",
        "/api/v1/auth/status",
        "/api/v1/health",
        // The provider's redirect: a cross-site navigation carries no session cookie, so
        // the `state` it echoes is its credential (security audit 2026-09-30, finding 5).
        "/api/v1/oauth/callback",
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
        // The agent's own settings: the clipboard pause it switches and the shortcuts it
        // registers (RD-1180-01, RD-1180-03); nothing else of the configuration.
        "/api/v1/capture/agent-settings",
        "/api/v1/capture/batches",
        // A browser session answers a request a person opened; it cannot start one.
        "/api/v1/capture/browser-sessions",
        "/api/v1/capture/browser-sessions/{id}",
        "/api/v1/capture/browser-sessions/{id}/decline",
        "/api/v1/capture/captchas",
        "/api/v1/capture/captchas/{id}/no-widget",
        "/api/v1/capture/captchas/{id}/skip",
        "/api/v1/capture/captchas/{id}/token",
        "/api/v1/capture/clipboard",
        "/api/v1/capture/cookies",
        "/api/v1/capture/events",
        // A file only the browser could load: its bytes, or its address and cookies.
        "/api/v1/capture/file",
        "/api/v1/capture/nzb",
        "/api/v1/capture/ping",
        "/api/v1/capture/shortcut-report",
        // Figures for the tray: counts and byte totals, nothing that names a file.
        "/api/v1/capture/summary",
    ]
    .into_iter()
    .collect();
    assert_eq!(capture, expected);
}

/// The tray's queue control is three routes on the capture surface, and no capture scope reaches
/// a queue route of the API (RD-1100-06, RD-1240-07).
///
/// `capture:queue` is chosen when an agent is paired; what it buys has to stay exactly "pause
/// everything, resume everything, add everything from the LinkGrabber", so an agent that may
/// pause cannot reorder, delete or read the queue or the LinkGrabber through it.
#[test]
fn the_capture_queue_scope_covers_exactly_the_tray_controls() {
    let controls: BTreeSet<(&str, &str)> = ROUTE_POLICY
        .iter()
        .filter(|entry| entry.requires == Requirement::Scope(Scope::CaptureQueue))
        .map(|entry| (entry.path, entry.method.as_str()))
        .collect();
    let expected: BTreeSet<(&str, &str)> = [
        ("/api/v1/capture/linkgrabber/enqueue", "POST"),
        ("/api/v1/capture/queue/pause", "POST"),
        ("/api/v1/capture/queue/resume", "POST"),
    ]
    .into_iter()
    .collect();
    assert_eq!(controls, expected);
    for entry in ROUTE_POLICY {
        if let Requirement::Scope(required) = entry.requires
            && Scope::CAPTURE.iter().any(|held| held.satisfies(required))
        {
            assert!(
                entry.path.starts_with("/api/v1/capture/"),
                "{} {} is reachable with a capture scope",
                entry.method,
                entry.path
            );
        }
    }
    for path in ["/api/v1/queue/pause", "/api/v1/downloads/bulk"] {
        for method in [Method::GET, Method::POST, Method::PUT, Method::DELETE] {
            if let Some(Requirement::Scope(required)) = requirement(path, &method) {
                assert!(
                    !Scope::CAPTURE.iter().any(|held| held.satisfies(required)),
                    "{method} {path} is reachable with a capture scope"
                );
            }
        }
    }
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
        "/api/v1/object-storage/profiles",
        "/api/v1/usenet/servers",
        "/api/v1/indexers",
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
        ("/api/v1/backups", Method::GET),
        ("/api/v1/backups", Method::PUT),
        ("/api/v1/backups/passphrase", Method::PUT),
        ("/api/v1/backups/restore", Method::POST),
        ("/api/v1/backups/restore", Method::DELETE),
        ("/api/v1/backups/restore/preview", Method::POST),
        ("/api/v1/backups/restore/test", Method::POST),
        ("/api/v1/backups/restore/uploads", Method::POST),
        ("/api/v1/backups/restore/uploads/{id}", Method::PUT),
        ("/api/v1/backups/runs", Method::GET),
        ("/api/v1/backups/runs", Method::POST),
        ("/api/v1/backups/destinations", Method::POST),
        ("/api/v1/backups/destinations/{id}", Method::PUT),
        ("/api/v1/backups/destinations/{id}", Method::DELETE),
        ("/api/v1/backups/destinations/{id}/retention", Method::GET),
        ("/api/v1/backups/archives", Method::GET),
        ("/api/v1/backups/archives/{id}/verify", Method::POST),
        ("/api/v1/backups/verifications", Method::GET),
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
