//! The flow and the provider rules, one threat of ADR 0021 per test where the threat lives here.

use std::net::{IpAddr, Ipv4Addr};

use super::*;
use crate::CeremonyStore;

const ISSUER: &str = "https://id.example.com/application/o/rdownloader/";

/// A discovery document as a provider serves it, with `changes` laid over it.
fn document(changes: serde_json::Value) -> serde_json::Value {
    let mut document = serde_json::json!({
        "issuer": ISSUER,
        "authorization_endpoint": "https://id.example.com/application/o/authorize/?prompt=login",
        "token_endpoint": "https://id.example.com/application/o/token/",
        "jwks_uri": "https://id.example.com/application/o/rdownloader/jwks/",
        "end_session_endpoint": "https://id.example.com/application/o/rdownloader/end-session/",
        "response_types_supported": ["code", "id_token"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256", "HS256"],
        "code_challenge_methods_supported": ["plain", "S256"],
    });
    if let (Some(target), Some(source)) = (document.as_object_mut(), changes.as_object()) {
        for (name, value) in source {
            target.insert(name.clone(), value.clone());
        }
    }
    document
}

fn metadata_with(changes: serde_json::Value) -> ProviderMetadata {
    serde_json::from_value(document(changes)).expect("a discovery document")
}

fn metadata() -> ProviderMetadata {
    metadata_with(serde_json::json!({}))
}

/// RFC 7636, appendix B — the library's S256.
#[test]
fn the_challenge_is_the_rfc_7636_s256_of_the_verifier() {
    assert_eq!(
        code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn a_usable_provider_offers_its_allowed_algorithms_only() {
    // HS256 is on the provider's list and not on ours.
    assert_eq!(
        check_provider(&metadata(), ISSUER),
        Ok(vec![Algorithm::Rs256])
    );
}

/// O-ISS: a discovery document that speaks for another issuer, byte for byte.
#[test]
fn o_iss_a_discovery_document_for_another_issuer_is_refused() {
    for configured in [
        "https://id.example.com/application/o/rdownloader",
        "https://id.example.com/application/o/other/",
        "https://ID.example.com/application/o/rdownloader/",
    ] {
        assert_eq!(
            check_provider(&metadata(), configured),
            Err(DiscoveryError::IssuerMismatch),
            "{configured}"
        );
    }
}

#[test]
fn a_provider_without_s256_or_an_allowed_algorithm_is_refused() {
    let no_pkce =
        metadata_with(serde_json::json!({ "code_challenge_methods_supported": ["plain"] }));
    assert_eq!(
        check_provider(&no_pkce, ISSUER),
        Err(DiscoveryError::NoPkce)
    );
    let silent = metadata_with(serde_json::json!({ "code_challenge_methods_supported": [] }));
    assert_eq!(check_provider(&silent, ISSUER), Err(DiscoveryError::NoPkce));

    let mac_only = metadata_with(
        serde_json::json!({ "id_token_signing_alg_values_supported": ["HS256", "none"] }),
    );
    assert_eq!(
        check_provider(&mac_only, ISSUER),
        Err(DiscoveryError::NoAlgorithm)
    );
}

#[test]
fn every_endpoint_has_to_be_https_or_on_this_machine() {
    let plain =
        metadata_with(serde_json::json!({ "token_endpoint": "http://id.example.com/token" }));
    assert_eq!(
        check_provider(&plain, ISSUER),
        Err(DiscoveryError::InsecureEndpoint)
    );
    let logout = metadata_with(
        serde_json::json!({ "end_session_endpoint": "http://id.example.com/logout" }),
    );
    assert_eq!(
        check_provider(&logout, ISSUER),
        Err(DiscoveryError::InsecureEndpoint)
    );
    let no_token = metadata_with(serde_json::json!({ "token_endpoint": null }));
    assert_eq!(
        check_provider(&no_token, ISSUER),
        Err(DiscoveryError::InsecureEndpoint)
    );

    assert!(endpoint_allowed("https://id.example.com/token"));
    assert!(endpoint_allowed("http://127.0.0.1:9000/token"));
    assert!(endpoint_allowed("http://[::1]:9000/token"));
    assert!(endpoint_allowed("http://localhost:9000/token"));
    for refused in [
        "http://192.168.1.10/token",
        "http://localhost.evil.example/token",
        "https://user:secret@id.example.com/token",
        "https://id.example.com/token#fragment",
        "ftp://id.example.com/token",
        "not a url",
    ] {
        assert!(!endpoint_allowed(refused), "{refused}");
    }
    assert!(!issuer_allowed("https://id.example.com/?tenant=1"));
    assert!(issuer_allowed(ISSUER));
}

#[test]
fn the_secret_goes_into_the_basic_header_unless_the_provider_takes_only_the_form() {
    assert_eq!(
        client_authentication(&metadata()),
        ClientAuthentication::Basic
    );
    let post = metadata_with(
        serde_json::json!({ "token_endpoint_auth_methods_supported": ["client_secret_post"] }),
    );
    assert_eq!(client_authentication(&post), ClientAuthentication::Post);
    let both = metadata_with(serde_json::json!({
        "token_endpoint_auth_methods_supported": ["client_secret_post", "client_secret_basic"]
    }));
    assert_eq!(client_authentication(&both), ClientAuthentication::Basic);
}

#[test]
fn the_authorization_request_carries_every_parameter_of_the_flow() {
    let (flow, _) = Flow::start(FlowPurpose::SignIn, ISSUER, "rdownloader", None, 1);
    let address = authorization_url(
        &metadata(),
        "rdownloader",
        "https://dl.example.com/api/v1/auth/oidc/callback",
        &flow,
        "the-state",
        Some("groups"),
    )
    .expect("an authorization URL");
    let url = url::Url::parse(&address).expect("a URL");
    let query: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(query["prompt"], "login", "the endpoint's own query stays");
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["scope"], "openid profile groups");
    assert_eq!(query["client_id"], "rdownloader");
    assert_eq!(
        query["redirect_uri"],
        "https://dl.example.com/api/v1/auth/oidc/callback"
    );
    assert_eq!(query["state"], "the-state");
    assert_eq!(query["nonce"], flow.nonce);
    assert_eq!(query["code_challenge"], code_challenge(&flow.verifier));
    assert_eq!(query["code_challenge_method"], "S256");
    assert!(
        !address.contains(&flow.verifier),
        "the verifier never leaves the service"
    );
}

#[test]
fn every_secret_of_a_flow_is_fresh() {
    let (first, first_binding) = Flow::start(FlowPurpose::SignIn, ISSUER, "c", None, 1);
    let (second, second_binding) = Flow::start(FlowPurpose::SignIn, ISSUER, "c", None, 1);
    assert_ne!(first.nonce, second.nonce);
    assert_ne!(first.verifier, second.verifier);
    assert_ne!(first_binding, second_binding);
    // 32 bytes of base64url without padding.
    assert_eq!(first.verifier.len(), 43);
    assert_eq!(first_binding.len(), 43);
    assert!(!first.binding_digest.contains(&first_binding));
}

/// O-CSRF: only the browser that started a flow holds its binding.
#[test]
fn o_csrf_a_flow_answers_only_the_browser_that_started_it() {
    let (flow, binding) = Flow::start(FlowPurpose::SignIn, ISSUER, "c", None, 1);
    let (_, other_browser) = Flow::start(FlowPurpose::SignIn, ISSUER, "c", None, 1);
    assert!(flow.bound_to(Some(&binding)));
    assert!(!flow.bound_to(None));
    assert!(!flow.bound_to(Some("")));
    assert!(!flow.bound_to(Some(&other_browser)));
    assert!(!flow.bound_to(Some(&flow.binding_digest)));
}

#[test]
fn the_binding_cookie_is_lax_scoped_and_short_lived() {
    let cookie = binding_cookie("value", true, "/downloads");
    assert!(cookie.starts_with("rd_oidc=value;"));
    for part in [
        "HttpOnly",
        "SameSite=Lax",
        "Path=/downloads/api/v1/auth/oidc/",
        "Secure",
        "Max-Age=600",
    ] {
        assert!(cookie.contains(part), "{part} in {cookie}");
    }
    assert!(!binding_cookie("value", false, "").contains("Secure"));
    assert!(binding_cookie("value", false, "").contains("Path=/api/v1/auth/oidc/;"));
    assert!(expired_binding_cookie("").contains("Max-Age=0"));
    assert_eq!(
        binding_from_cookies("rd_session=abc; rd_oidc=xyz; other=1"),
        Some("xyz")
    );
    assert_eq!(binding_from_cookies("rd_oidc="), None);
    assert_eq!(binding_from_cookies("rd_session=abc"), None);
}

/// O-REDIR: the return path stays inside the application.
#[test]
fn o_redir_the_return_path_cannot_leave_the_application() {
    for refused in [
        "//evil.example",
        "//evil.example/settings",
        "/\\evil.example",
        "https://evil.example/",
        "/x?next=https://evil.example",
        "settings",
        "/a\nb",
    ] {
        assert_eq!(safe_return_path(Some(refused)), "/", "{refused:?}");
    }
    assert_eq!(safe_return_path(None), "/");
    assert_eq!(
        safe_return_path(Some("/settings/security")),
        "/settings/security"
    );
    assert_eq!(
        safe_return_path(Some("/downloads?view=1")),
        "/downloads?view=1"
    );
}

/// O-JWKS: however many forged tokens name unknown keys, one refetch per interval.
#[test]
fn o_jwks_a_burst_of_unknown_keys_refetches_once() {
    let gate = RefetchGate::default();
    let start = Instant::now();
    assert!(gate.allow(start));
    for second in 1..100 {
        assert!(!gate.allow(start + Duration::from_secs(second)));
    }
    assert!(gate.allow(start + JWKS_REFETCH_INTERVAL));
}

fn address(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(203, 0, 113, last))
}

/// O-FLOOD: the public start allocates server state, so the flows are bounded — and bounded the
/// way the passkey ceremonies are, so one address flooding the start evicts only its own flows.
#[test]
fn o_flood_starts_from_one_address_never_evict_another_address_flow() {
    let flows = CeremonyStore::with_ttl(FLOW_TTL);
    let (mine, _) = Flow::start(FlowPurpose::SignIn, ISSUER, "c", None, 1);
    let state = flows.insert(address(1), mine);
    for _ in 0..200 {
        let (flood, _) = Flow::start(FlowPurpose::SignIn, ISSUER, "c", None, 1);
        flows.insert(address(2), flood);
    }
    assert!(
        flows.take(&state).is_some(),
        "a flood from one address evicted another address's sign-in"
    );
}

/// O-STEAL: a state is spent on first use, whatever happens after.
#[test]
fn o_steal_a_state_answers_once() {
    let flows = CeremonyStore::with_ttl(FLOW_TTL);
    let (flow, _) = Flow::start(FlowPurpose::SignIn, ISSUER, "c", None, 1);
    let state = flows.insert(address(1), flow);
    assert!(state.len() >= 32, "a state of at least 32 characters");
    assert!(flows.take(&state).is_some());
    assert!(flows.take(&state).is_none());
}
