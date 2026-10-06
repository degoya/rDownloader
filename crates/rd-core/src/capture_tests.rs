use super::{
    API_ADMIN_SCOPE, API_CONFIG_SCOPE, API_INTAKE_SCOPE, API_METRICS_SCOPE, API_QUEUE_SCOPE,
    API_READ_SCOPE, API_SCOPE, API_SECRETS_SCOPE, CAPTURE_QUEUE_SCOPE, CAPTURE_SCOPE,
    CAPTURED_HEADER_ALLOWLIST, CapturedRequest, Scope, granted_scopes, is_allowed_captured_header,
    is_credential_header, scope_satisfies, scopes_grant, scopes_satisfy,
};

/// Nothing confers the two scopes that matter most, not even administration.
///
/// The whole point of splitting the vocabulary: if any scope quietly implied `Secrets`,
/// a token minted to reorder a queue would be able to read every stored password.
#[test]
fn no_scope_implies_secrets_or_administration() {
    for scope in Scope::API.iter().chain(Scope::CAPTURE) {
        for forbidden in [Scope::Secrets, Scope::Admin] {
            if *scope == forbidden {
                continue;
            }
            assert!(
                !scope.satisfies(forbidden),
                "{} confers {}",
                scope.as_str(),
                forbidden.as_str()
            );
        }
    }
}

/// The two event kinds RD-140 added are configuration, and nothing wider.
///
/// An automation carries the rules an operator wrote and a managed tool names a helper
/// binary this installation verified; both are written through `Config`-scoped endpoints.
/// `Read` would put them on a monitoring token's stream, and `Admin` would hide them from
/// the tokens that are allowed to make the change in the first place.
#[test]
fn an_automation_and_a_managed_tool_are_configuration_events() {
    for kind in [
        crate::EventKind::AutomationChanged,
        crate::EventKind::ManagedToolChanged,
    ] {
        assert_eq!(
            Scope::of_event(&kind),
            Scope::Config,
            "{kind:?} left the configuration scope"
        );
        assert!(
            !Scope::Read.satisfies(Scope::of_event(&kind)),
            "{kind:?} reached a read-only subscriber"
        );
    }
}

/// Anything that acts can also look, because controlling what you cannot see is useless
/// and demanding a second scope for it only teaches people to grant `api:*`.
#[test]
fn every_acting_scope_confers_reading() {
    for scope in [Scope::Intake, Scope::Queue, Scope::Config, Scope::Admin] {
        assert!(scope.satisfies(Scope::Read), "{}", scope.as_str());
    }
}

/// A credential token is not a queue reader.
#[test]
fn the_secrets_scope_confers_nothing_at_all() {
    assert!(Scope::Secrets.implies().is_empty());
    assert!(!Scope::Secrets.satisfies(Scope::Read));
}

/// A legacy token keeps every capability it had; narrowing it silently would leave it
/// authenticating fine and failing at the one job it exists for.
#[test]
fn a_legacy_full_access_token_expands_to_every_api_scope() {
    let granted = granted_scopes([API_SCOPE]);
    for scope in Scope::API {
        assert!(granted.contains(scope), "{} was lost", scope.as_str());
    }
    // …and gains nothing it never had.
    assert!(!granted.contains(&Scope::Capture));
}

/// The capture surface stays separate in both directions, whatever the vocabulary grows to.
#[test]
fn capture_is_isolated_from_every_api_scope() {
    for scope in Scope::API {
        assert!(!scope.satisfies(Scope::Capture), "{}", scope.as_str());
        assert!(!Scope::Capture.satisfies(*scope), "{}", scope.as_str());
    }
    assert!(!granted_scopes([API_SCOPE]).contains(&Scope::Capture));
    assert!(granted_scopes([CAPTURE_SCOPE]) == vec![Scope::Capture]);
}

/// The tray's queue control is an island of its own (RD-1100-06): a token paired with it
/// gains the two capture queue routes and nothing else, and no other scope -- not `api:*`,
/// not `api:queue`, not `capture:*` -- reaches it.
#[test]
fn the_capture_queue_scope_is_isolated_in_both_directions() {
    assert_eq!(
        granted_scopes([CAPTURE_SCOPE, CAPTURE_QUEUE_SCOPE]),
        vec![Scope::Capture, Scope::CaptureQueue]
    );
    assert!(Scope::CaptureQueue.implies().is_empty());
    for other in Scope::API.iter().chain(std::iter::once(&Scope::Capture)) {
        assert!(!other.satisfies(Scope::CaptureQueue), "{}", other.as_str());
        assert!(!Scope::CaptureQueue.satisfies(*other), "{}", other.as_str());
    }
    assert!(!granted_scopes([API_SCOPE]).contains(&Scope::CaptureQueue));
    assert!(!scope_satisfies(API_SCOPE, CAPTURE_QUEUE_SCOPE));
    assert!(!scope_satisfies(CAPTURE_SCOPE, CAPTURE_QUEUE_SCOPE));
    assert!(!scope_satisfies(CAPTURE_QUEUE_SCOPE, API_QUEUE_SCOPE));
}

/// Round-tripping is what makes the persisted strings and the enum one vocabulary.
#[test]
fn every_scope_parses_back_from_its_string() {
    for scope in Scope::API.iter().chain(Scope::CAPTURE) {
        assert_eq!(Scope::parse(scope.as_str()), Some(*scope));
    }
    assert_eq!(Scope::parse("api:*"), None, "api:* is not a single scope");
    assert_eq!(Scope::parse("nonsense"), None);
}

/// Scope strings must not be prefixes of one another in a way a sloppy check could match.
#[test]
fn the_scope_strings_are_distinct() {
    let strings = [
        API_READ_SCOPE,
        API_INTAKE_SCOPE,
        API_QUEUE_SCOPE,
        API_CONFIG_SCOPE,
        API_SECRETS_SCOPE,
        API_ADMIN_SCOPE,
        API_METRICS_SCOPE,
        CAPTURE_SCOPE,
        CAPTURE_QUEUE_SCOPE,
        API_SCOPE,
    ];
    for (index, left) in strings.iter().enumerate() {
        for right in &strings[index + 1..] {
            assert_ne!(left, right);
        }
    }
}

/// The token store compares strings; it has to agree with the enum.
#[test]
fn the_string_and_enum_comparisons_agree() {
    assert!(scope_satisfies(API_ADMIN_SCOPE, API_READ_SCOPE));
    assert!(!scope_satisfies(API_ADMIN_SCOPE, API_SECRETS_SCOPE));
    assert!(!scope_satisfies(API_SECRETS_SCOPE, API_READ_SCOPE));
    assert!(scope_satisfies(API_SCOPE, API_SECRETS_SCOPE));
    assert!(!scope_satisfies(API_SCOPE, CAPTURE_SCOPE));
    assert!(scopes_grant([API_QUEUE_SCOPE], Scope::Read));
    assert!(!scopes_grant([API_QUEUE_SCOPE], Scope::Config));
}

/// The scrape scope is an island (RD-110-01): a token that may scrape can do nothing
/// else, and no scope short of `api:*` may scrape.
#[test]
fn the_metrics_scope_is_isolated_in_both_directions() {
    assert!(scopes_grant([API_METRICS_SCOPE], Scope::Metrics));
    assert_eq!(granted_scopes([API_METRICS_SCOPE]), vec![Scope::Metrics]);
    for other in Scope::API.iter().filter(|scope| **scope != Scope::Metrics) {
        assert!(
            !scopes_grant([other.as_str()], Scope::Metrics),
            "{} must not reach the metrics",
            other.as_str()
        );
        assert!(
            !scopes_grant([API_METRICS_SCOPE], *other),
            "api:metrics must not reach {}",
            other.as_str()
        );
    }
    assert!(scopes_grant([API_SCOPE], Scope::Metrics));
    assert!(!scope_satisfies(API_METRICS_SCOPE, API_READ_SCOPE));
    assert!(!scope_satisfies(API_ADMIN_SCOPE, API_METRICS_SCOPE));
}

#[test]
fn full_api_access_covers_the_read_only_surface() {
    assert!(scope_satisfies(API_SCOPE, API_READ_SCOPE));
    assert!(scope_satisfies(API_SCOPE, API_SCOPE));
    assert!(scope_satisfies(API_READ_SCOPE, API_READ_SCOPE));
}

#[test]
fn a_read_only_token_never_grows_into_full_access() {
    assert!(!scope_satisfies(API_READ_SCOPE, API_SCOPE));
}

#[test]
fn capture_tokens_stay_isolated_from_the_api_scopes() {
    // The strings share no implication in either direction; a browser extension token
    // must not become a queue reader and an API token must not reach capture intake.
    for required in [API_SCOPE, API_READ_SCOPE] {
        assert!(!scope_satisfies(CAPTURE_SCOPE, required));
    }
    for held in [API_SCOPE, API_READ_SCOPE] {
        assert!(!scope_satisfies(held, CAPTURE_SCOPE));
    }
}

#[test]
fn any_held_scope_can_satisfy_the_requirement() {
    assert!(scopes_satisfy([CAPTURE_SCOPE, API_SCOPE], API_READ_SCOPE));
    assert!(!scopes_satisfy([CAPTURE_SCOPE], API_READ_SCOPE));
    assert!(!scopes_satisfy([], API_READ_SCOPE));
}

#[test]
fn every_allowlisted_header_survives_the_credential_backstop() {
    // `is_credential_header` matches substrings (`auth`, `key`, `token`, `secret`,
    // `session`). Without this test a future allowlist entry such as `x-goog-api-key`
    // would be dropped at runtime with nothing but a log line to show for it.
    for header in CAPTURED_HEADER_ALLOWLIST {
        assert!(
            is_allowed_captured_header(header),
            "allowlisted header `{header}` is refused by the credential backstop"
        );
    }
}

#[test]
fn credential_headers_are_still_refused() {
    for header in [
        "cookie",
        "set-cookie",
        "authorization",
        "proxy-authorization",
        "x-api-key",
        "x-session-id",
    ] {
        assert!(is_credential_header(header), "{header} must be refused");
        assert!(!is_allowed_captured_header(header), "{header} allowlisted");
    }
}

#[test]
fn a_contract_v1_payload_still_deserializes() {
    // The v1 extension sends no body, no origins and no replay markers; every v2 field
    // has to default rather than fail the hand-off.
    let payload = serde_json::json!({
        "method": "GET",
        "referrer": "https://example.com/page",
        "headers": [{ "name": "accept", "value": "*/*" }]
    });
    let request: CapturedRequest = serde_json::from_value(payload).expect("v1 payload");
    assert_eq!(request.method, "GET");
    assert!(request.body.is_none());
    assert!(request.approved_origins.is_empty());
    // A v1 capture is reproducible unless something later says otherwise.
    assert!(request.replayable);
    assert!(request.blocked_reason.is_none());
}

#[test]
fn the_raw_body_is_never_serialized_back() {
    let request = CapturedRequest {
        method: "POST".to_owned(),
        body_b64: Some("c2VjcmV0".to_owned()),
        ..CapturedRequest::default()
    };
    let serialized = serde_json::to_string(&request).expect("serialize");
    assert!(
        !serialized.contains("c2VjcmV0"),
        "raw body leaked into the response: {serialized}"
    );
}
