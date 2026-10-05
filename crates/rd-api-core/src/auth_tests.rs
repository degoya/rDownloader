use axum::http::{HeaderMap, Method};
use rd_core::{EventKind, Scope};

use super::{Granted, malformed_credential};

fn read_only() -> Granted {
    Granted(vec![Scope::Read])
}

fn session() -> Granted {
    Granted(Scope::API.to_vec())
}

/// The four ways a credential arrives unusable, each with its own sentence.
///
/// All four reach the service as the same `401`, so the log line is the only thing that
/// separates them. The MCP endpoint is where this matters: a connector dialog can be
/// filled in wrongly in every one of these ways and reports nothing but "failed".
#[test]
fn each_shape_of_unusable_credential_is_named() {
    let mut headers = HeaderMap::new();
    assert_eq!(
        malformed_credential(&headers).expect_err("no header is unusable"),
        "no Authorization header was sent"
    );

    headers.insert(
        axum::http::header::AUTHORIZATION,
        "Basic abc".parse().expect("value"),
    );
    assert_eq!(
        malformed_credential(&headers).expect_err("Basic is unusable"),
        "the Authorization header is not a Bearer credential"
    );

    headers.insert(
        axum::http::header::AUTHORIZATION,
        "Bearer ".parse().expect("value"),
    );
    assert_eq!(
        malformed_credential(&headers).expect_err("an empty Bearer is unusable"),
        "the Bearer credential is empty"
    );

    // The trap that costs the most time: two sources configured for one header. The first
    // wins, so a correct token in the second is never read and the refusal looks like a
    // wrong token rather than a duplicate.
    headers.append(
        axum::http::header::AUTHORIZATION,
        "Bearer real".parse().expect("value"),
    );
    assert_eq!(
        malformed_credential(&headers).expect_err("a duplicate header is unusable"),
        "2 Authorization headers were sent, and only the first is read"
    );
}

#[test]
fn a_single_well_formed_bearer_is_handed_on_for_the_store_to_judge() {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        "Bearer s3cret".parse().expect("value"),
    );
    assert_eq!(malformed_credential(&headers).expect("a token"), "s3cret");
}

/// Both ends of the policy, and nothing between them refused (audit 2026-09-30, finding 8).
#[test]
fn a_password_is_bounded_at_both_ends() {
    assert_eq!(
        super::validate_password("short")
            .expect_err("too short")
            .code(),
        "auth.password_too_short"
    );
    assert!(super::validate_password(&"x".repeat(10)).is_ok());
    assert!(super::validate_password(&"x".repeat(super::MAX_PASSWORD_CHARS)).is_ok());
    assert_eq!(
        super::validate_password(&"x".repeat(super::MAX_PASSWORD_CHARS + 1))
            .expect_err("too long")
            .code(),
        "auth.password_too_long"
    );
}

#[test]
fn a_read_only_subscriber_sees_queue_events_only() {
    for kind in [
        EventKind::DownloadProgress,
        EventKind::DownloadState,
        EventKind::PackageState,
        EventKind::PostprocessProgress,
        EventKind::StorageCapacity,
        EventKind::TorrentStats,
    ] {
        assert!(
            read_only().may_observe(&kind),
            "{kind:?} is what a dashboard exists to show"
        );
    }
}

#[test]
fn a_read_only_subscriber_never_sees_configuration_events() {
    // These carry the ids of accounts, proxies, credentials, plugins and settings
    // changes. Streaming them would hand a monitoring token a map of the installation.
    for kind in [
        EventKind::AccountChanged,
        EventKind::AuthProfileChanged,
        EventKind::AutomationChanged,
        EventKind::CaptureChanged,
        EventKind::CategoryChanged,
        EventKind::CollectorChanged,
        EventKind::CollectorIntake,
        EventKind::HotFolderChanged,
        EventKind::ManagedToolChanged,
        EventKind::PluginCatalogChanged,
        EventKind::PluginChanged,
        EventKind::PostprocessCatalogChanged,
        EventKind::PluginTrustChanged,
        EventKind::ProxyChanged,
        EventKind::RemoteCredentialChanged,
        EventKind::System,
        EventKind::UsenetChanged,
    ] {
        assert!(
            !read_only().may_observe(&kind),
            "{kind:?} reached a read-only subscriber"
        );
    }
}

#[test]
fn a_session_sees_the_whole_bus() {
    for kind in [
        EventKind::AccountChanged,
        EventKind::DownloadState,
        EventKind::PluginChanged,
        EventKind::CollectorIntake,
    ] {
        assert!(session().may_observe(&kind), "{kind:?}");
    }
}

/// A subscriber with no scopes at all sees nothing, rather than everything.
///
/// This is the shape of the mistake worth guarding: the previous default, applied when
/// the extension was absent, was "full access". Mounting the stream outside the session
/// layer would then have published the whole bus to anyone who asked.
#[test]
fn a_subscriber_with_no_scopes_sees_nothing() {
    let none = Granted::default();
    for kind in [
        EventKind::DownloadProgress,
        EventKind::AccountChanged,
        EventKind::System,
    ] {
        assert!(!none.may_observe(&kind), "{kind:?} reached an empty grant");
    }
}

/// A credential scope must not double as a way to watch the queue.
#[test]
fn a_secrets_only_subscriber_sees_only_credential_events() {
    let secrets = Granted(vec![Scope::Secrets]);
    assert!(secrets.may_observe(&EventKind::AccountChanged));
    assert!(!secrets.may_observe(&EventKind::DownloadProgress));
}

/// An event's scope follows the scope of the write that produces it.
///
/// The trust store's four writes -- trusting and revoking a signing key, withdrawing and
/// reinstating a package digest -- sit behind `Secrets`-scoped routes, while installing,
/// enabling, disabling and removing a plugin is `Admin`. Both halves used to announce
/// themselves as `PluginChanged`, which was wrong in both directions at once: the token
/// allowed to make the trust write never saw the event it caused, and the key ids and
/// digests those payloads carry were delivered to `Admin` subscribers, who may not read
/// the tables they name.
///
/// Both directions are asserted because [`Granted::may_observe`] checks exact possession
/// and not [`Scope::satisfies`]: `Admin` and `Secrets` are siblings and neither confers
/// the other, so a kind filed under the wrong one is invisible to exactly the scope that
/// should see it. Nothing else in the suite would notice.
#[test]
fn plugin_trust_is_secrets_and_plugin_administration_is_admin() {
    let secrets = Granted(vec![Scope::Secrets]);
    let admin = Granted(vec![Scope::Admin]);

    assert!(
        secrets.may_observe(&EventKind::PluginTrustChanged),
        "the scope that may write a trust decision must see the event it caused"
    );
    assert!(
        !admin.may_observe(&EventKind::PluginTrustChanged),
        "a key id or a digest was streamed to a scope that may not read those tables"
    );

    assert!(
        admin.may_observe(&EventKind::PluginChanged),
        "installing or removing a plugin is administration and must reach it"
    );
    assert!(
        !secrets.may_observe(&EventKind::PluginChanged),
        "the credential scope must not double as a view of the plugin inventory"
    );
}

fn facts(path: &str, method: Method) -> super::RequestFacts {
    super::RequestFacts {
        path: path.to_owned(),
        method,
        headers: HeaderMap::new(),
        host: None,
        from_this_machine: true,
    }
}

/// The positive direction of the policy, decided rather than executed.
///
/// `tests/access/scope_matrix.rs` drives the *negative* direction through the real router across
/// every route. It deliberately does not drive the positive one: letting 233 requests
/// through reaches real handlers, some of which open network connections or spawn external
/// tools. So the "a token that holds the scope gets through" half is checked here, against
/// the same table, where it costs nothing.
#[test]
fn a_scope_the_route_requires_is_not_refused() {
    for (path, method, required) in crate::policy_rows() {
        let Some(required) = required else { continue };
        let Some(scope) = Scope::parse(required) else {
            panic!("{required} is not a scope");
        };
        let method = Method::from_bytes(method.as_bytes()).expect("method");
        assert!(
            super::scope_refusal(Some(&facts(path, method)), &[scope]).is_none(),
            "{path} requires {required} and refused a token holding exactly it"
        );
    }
}

/// A public route is reachable with no scope at all.
#[test]
fn a_public_route_needs_nothing() {
    assert!(super::scope_refusal(Some(&facts("/api/v1/health", Method::GET)), &[]).is_none());
}

/// A route the table does not know is refused, not waved through.
///
/// The exhaustiveness test in `scope_policy` means this cannot happen. It is still the
/// branch worth pinning: "cannot happen" and "fails open when it does" is how
/// authorisation holes are made.
#[test]
fn an_unclassified_route_fails_closed() {
    let refusal = super::scope_refusal(Some(&facts("/api/v1/invented", Method::GET)), Scope::API);
    assert_eq!(
        refusal.map(|error| error.code().to_owned()),
        Some("scope.route_unclassified".to_owned())
    );
}

/// A request that matched no route is not a policy question, and must not be refused as one.
#[test]
fn an_unmatched_request_is_left_to_the_fallback() {
    assert!(super::scope_refusal(None, &[]).is_none());
}

/// The refusal names the scope that was missing, so the message can say what to do.
#[test]
fn a_refusal_names_the_scope_it_wanted() {
    let refusal = super::scope_refusal(
        Some(&facts("/api/v1/accounts", Method::GET)),
        &[Scope::Read],
    )
    .expect("a read-only token must not reach accounts");
    assert_eq!(refusal.code(), "auth.scope_insufficient");
}
