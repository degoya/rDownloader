//! The minting rules of `api_tokens.rs`: which scope strings a request may name, and the
//! expiry a token may be given.

use super::{MAX_EXPIRY_DAYS, expiry, requested_scopes};
use crate::dto::ApiTokenRequest;

fn request(scopes: &[&str]) -> ApiTokenRequest {
    ApiTokenRequest {
        label: "test".to_owned(),
        scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        expires_in_days: None,
    }
}

#[test]
fn a_named_set_is_granted_exactly() {
    let scopes =
        requested_scopes(&request(&["api:queue", "api:intake"])).expect("both are real areas");
    assert_eq!(
        scopes,
        vec!["api:intake".to_owned(), "api:queue".to_owned()]
    );
}

/// The property the whole model rests on: minting never hands out more than was asked
/// for. `api:queue` implies reading at *check* time, which is not the same as storing a
/// second scope — storing it would make the token look broader than it is in the list.
#[test]
fn minting_never_widens_a_request() {
    for area in [
        "api:read",
        "api:intake",
        "api:queue",
        "api:config",
        "api:secrets",
        "api:admin",
    ] {
        let scopes = requested_scopes(&request(&[area])).expect("a real area");
        assert_eq!(scopes, vec![area.to_owned()], "{area} was widened");
    }
}

/// The capture surface and the API are isolated in both directions, and a minting call is
/// exactly where somebody would try to bridge them.
#[test]
fn the_capture_scope_cannot_be_minted_as_an_api_token() {
    // Nor the tray's queue control (RD-1100-06): it is chosen when an agent is paired.
    for scope in ["capture:*", "capture:queue"] {
        let error = requested_scopes(&request(&[scope])).expect_err("capture is not an API area");
        assert_eq!(error.code(), "api.scope_unknown", "{scope}");
    }
}

/// Dropping it would produce a token weaker than the caller believes, which then fails
/// somewhere else entirely, long after the cause.
#[test]
fn an_unknown_scope_is_refused_rather_than_ignored() {
    for name in ["api:everything", "", "read", "api:Read"] {
        assert!(
            requested_scopes(&request(&[name])).is_err(),
            "`{name}` was accepted"
        );
    }
}

/// Re-scoping an existing token goes through the same resolver as minting a new one, so
/// the refusals cannot be softer on the path that *widens* a credential than on the path
/// that creates one.
#[test]
fn re_scoping_resolves_by_exactly_the_same_rules_as_minting() {
    for named in [
        vec!["api:queue".to_owned(), "api:intake".to_owned()],
        vec!["api:*".to_owned()],
        vec!["api:read".to_owned(), "api:read".to_owned()],
    ] {
        assert_eq!(
            super::resolve_scopes(&named).expect("real areas"),
            requested_scopes(&ApiTokenRequest {
                label: "test".to_owned(),
                scopes: named.clone(),
                expires_in_days: None,
            })
            .expect("real areas"),
            "{named:?}"
        );
    }
    for refused in [vec!["capture:*".to_owned()], vec!["api:nope".to_owned()]] {
        assert_eq!(
            super::resolve_scopes(&refused)
                .expect_err("not an API area")
                .code(),
            "api.scope_unknown",
            "{refused:?}"
        );
    }
}

#[test]
fn duplicates_collapse_rather_than_being_stored_twice() {
    let scopes =
        requested_scopes(&request(&["api:read", "api:read", " api:read "])).expect("a real area");
    assert_eq!(scopes, vec!["api:read".to_owned()]);
}

/// Naming no area is least privilege, never everything.
#[test]
fn a_request_naming_no_area_gets_read_access() {
    assert_eq!(
        requested_scopes(&request(&[])).expect("read default"),
        vec![rd_core::API_READ_SCOPE.to_owned()]
    );
}

/// The preview a person chooses against must come from the table that will refuse them,
/// and must order the areas the way the model does.
#[test]
fn the_capability_preview_is_ordered_and_non_trivial() {
    let mut previous = 0;
    for scope in rd_core::Scope::API {
        let reachable = crate::scope_policy::operations_reachable_by(*scope);
        assert!(reachable > 0, "{scope:?} reaches nothing at all");
        if *scope == rd_core::Scope::Metrics {
            // The island (RD-110-01): one route, and nothing of the ladder.
            assert_eq!(
                reachable, 1,
                "api:metrics must reach exactly the exposition"
            );
            continue;
        }
        if *scope != rd_core::Scope::Secrets {
            // Everything that acts also reads, so each area reaches strictly more than
            // reading alone. Secrets is the deliberate exception: it confers nothing.
            assert!(
                reachable > previous || *scope == rd_core::Scope::Read,
                "{scope:?} reaches no more than the area before it"
            );
        }
        if *scope == rd_core::Scope::Read {
            previous = reachable;
        }
    }
}

/// No expiry is the default and stays one (RD-1110-07); a chosen one lands that many days
/// ahead, and a value outside 1 to 3650 days is refused rather than clamped.
#[test]
fn an_expiry_is_optional_and_bounded() {
    assert_eq!(expiry(None).expect("never expires"), None);
    let before = chrono::Utc::now();
    let at = expiry(Some(30)).expect("thirty days").expect("an expiry");
    let ahead = at - before;
    assert!(ahead >= chrono::Duration::days(30), "{ahead}");
    assert!(
        ahead < chrono::Duration::days(30) + chrono::Duration::minutes(1),
        "{ahead}"
    );
    expiry(Some(MAX_EXPIRY_DAYS)).expect("the longest expiry");
    for refused in [0, MAX_EXPIRY_DAYS + 1, u32::MAX] {
        let error = expiry(Some(refused)).expect_err("out of range");
        assert_eq!(error.code(), "api.token_expiry_range", "{refused}");
    }
}
