//! The ID token checks, one threat of ADR 0021 per test where the threat lives in the token.
//!
//! Every token here is signed by a real key (`crate::oidc_testing`), so a refusal is the rule
//! under test refusing and not a signature that never verified. The verification is
//! `openidconnect`'s; these tests hold it to ADR 0021's rules as configured in `oidc_token`.

use serde_json::{Value, json};

use super::*;
use crate::oidc_testing::{TestKey, key_set, unsigned};

const ISSUER: &str = "https://id.example.com/application/o/rdownloader/";
const CLIENT: &str = "rdownloader";
const NONCE: &str = "the-nonce-of-this-flow";
const NOW: i64 = 1_800_000_000;

fn claims() -> Value {
    json!({
        "iss": ISSUER,
        "sub": "administrator-subject",
        "aud": CLIENT,
        "exp": NOW + 300,
        "iat": NOW - 5,
        "nonce": NONCE,
        "preferred_username": "owner",
        "groups": ["rdownloader-admins", "family"],
    })
}

fn with(mut claims: Value, name: &str, value: Value) -> Value {
    claims[name] = value;
    claims
}

fn without(mut claims: Value, name: &str) -> Value {
    if let Some(object) = claims.as_object_mut() {
        object.remove(name);
    }
    claims
}

fn expectations(algorithms: &[Algorithm]) -> Expectations<'_> {
    Expectations {
        issuer: ISSUER,
        client_id: CLIENT,
        nonce: NONCE,
        flow_started_at: NOW - 30,
        now: NOW,
        algorithms,
    }
}

fn keys(keys: &[&TestKey]) -> JsonWebKeySet {
    serde_json::from_value(key_set(keys)).expect("a key set")
}

fn verify(token: &str, set: &JsonWebKeySet) -> Result<IdentityClaims, TokenError> {
    verify_id_token(token, set, &expectations(&Algorithm::ALLOWED))
}

#[test]
fn each_allowed_algorithm_verifies_with_a_key_of_its_family() {
    let ec = TestKey::es256("ec");
    let ed = TestKey::ed25519("ed");
    let rsa = TestKey::rsa("rsa");
    let set = keys(&[&ec, &ed, &rsa]);
    for (key, alg) in [
        (&ec, "ES256"),
        (&ed, "EdDSA"),
        (&rsa, "RS256"),
        (&rsa, "PS256"),
    ] {
        let token = key.sign(&json!({ "alg": alg, "kid": key.kid }), &claims());
        let identity = verify(&token, &set).unwrap_or_else(|error| panic!("{alg}: {error}"));
        assert_eq!(identity.subject, "administrator-subject");
        assert_eq!(identity.issuer, ISSUER);
        assert_eq!(identity.label().as_deref(), Some("owner"));
    }
}

/// Without a key id the one key of the token's family is used; with several of them the token
/// has to say which (OIDC Core §10.1), and a guess is not made.
#[test]
fn a_token_without_a_key_id_needs_exactly_one_key_of_its_family() {
    let first = TestKey::es256("first");
    let second = TestKey::es256("second");
    let rsa = TestKey::rsa("rsa");
    let token = second.sign(&json!({ "alg": "ES256" }), &claims());
    assert!(verify(&token, &keys(&[&second, &rsa])).is_ok());
    assert_eq!(
        verify(&token, &keys(&[&first, &second])).map(|_| ()),
        Err(TokenError::AmbiguousKey)
    );
}

#[test]
fn a_changed_claim_breaks_the_signature() {
    let key = TestKey::es256("ec");
    let token = key.sign(&key.header(), &claims());
    let forged = key.sign(&key.header(), &with(claims(), "sub", json!("intruder")));
    let mut parts: Vec<&str> = token.split('.').collect();
    parts[1] = forged.split('.').nth(1).expect("claims");
    assert_eq!(
        verify(&parts.join("."), &keys(&[&key])).map(|_| ()),
        Err(TokenError::Signature)
    );
}

/// O-ALG: `alg: none` is refused before any key is looked at.
#[test]
fn o_alg_an_unsigned_token_is_refused() {
    let key = TestKey::es256("ec");
    let token = unsigned(&json!({ "alg": "none" }), &claims(), b"");
    assert_eq!(
        verify(&token, &keys(&[&key])).map(|_| ()),
        Err(TokenError::Algorithm)
    );
}

/// O-ALG: an HMAC keyed by a value an attacker knows — the client secret, a public key's bytes —
/// is never an algorithm this service checks, whatever the signature says.
#[test]
fn o_alg_an_hmac_token_is_refused() {
    let key = TestKey::rsa("rsa");
    for alg in ["HS256", "HS384", "HS512"] {
        let token = unsigned(
            &json!({ "alg": alg, "kid": "rsa" }),
            &claims(),
            b"mac-keyed-by-the-client-secret",
        );
        assert_eq!(
            verify(&token, &keys(&[&key])).map(|_| ()),
            Err(TokenError::Algorithm),
            "{alg}"
        );
    }
}

/// O-ALG: key confusion — an RSA key named by a token that claims `ES256`. No key of the
/// algorithm's family carries that id, so none is used.
#[test]
fn o_alg_a_key_of_another_family_is_refused() {
    let rsa = TestKey::rsa("rsa");
    let token = rsa.sign(&json!({ "alg": "ES256", "kid": "rsa" }), &claims());
    assert_eq!(
        verify(&token, &keys(&[&rsa])).map(|_| ()),
        Err(TokenError::UnknownKey)
    );
}

/// An allowed algorithm the provider did not list is refused as well: the allowlist is
/// intersected with the discovery document.
#[test]
fn an_algorithm_the_provider_does_not_use_is_refused() {
    let key = TestKey::es256("ec");
    let token = key.sign(&key.header(), &claims());
    assert_eq!(
        verify_id_token(&token, &keys(&[&key]), &expectations(&[Algorithm::Rs256])).map(|_| ()),
        Err(TokenError::Algorithm)
    );
}

#[test]
fn a_key_published_for_encryption_or_another_algorithm_never_verifies() {
    let key = TestKey::es256("ec");
    let token = key.sign(&key.header(), &claims());
    for (member, value) in [("use", "enc"), ("alg", "ES384")] {
        let mut set = key_set(&[&key]);
        set["keys"][0][member] = json!(value);
        let set: JsonWebKeySet = serde_json::from_value(set).expect("a key set");
        assert_eq!(
            verify(&token, &set).map(|_| ()),
            Err(TokenError::UnknownKey),
            "{member}"
        );
    }
}

/// O-JWKS: an unknown key id is reported as such, so the caller can refetch — gated.
#[test]
fn an_unknown_key_id_is_its_own_refusal() {
    let known = TestKey::es256("known");
    let rotated = TestKey::es256("rotated-in");
    let token = rotated.sign(&rotated.header(), &claims());
    let set = keys(&[&known]);
    assert_eq!(
        verify(&token, &set).map(|_| ()),
        Err(TokenError::UnknownKey)
    );
    assert!(verify(&token, &keys(&[&known, &rotated])).is_ok());
}

#[test]
fn a_critical_extension_is_refused() {
    let key = TestKey::es256("ec");
    let token = key.sign(
        &json!({ "alg": "ES256", "kid": "ec", "crit": ["exp"] }),
        &claims(),
    );
    assert_eq!(
        verify(&token, &keys(&[&key])).map(|_| ()),
        Err(TokenError::Malformed)
    );
}

#[test]
fn malformed_tokens_are_refused() {
    let key = TestKey::es256("ec");
    let set = keys(&[&key]);
    let good = key.sign(&key.header(), &claims());
    let extended = format!("{good}.extra");
    let undotted = good.replace('.', "!");
    let candidates: [&str; 5] = ["", "a.b", "a.b.c.d", &extended, &undotted];
    for token in candidates {
        assert!(verify(token, &set).is_err(), "{token}");
    }
}

/// O-ISS: a token from another issuer, even signed by a key this provider publishes.
#[test]
fn o_iss_a_token_from_another_issuer_is_refused() {
    let key = TestKey::es256("ec");
    for issuer in [
        "https://id.example.com/application/o/other/",
        // Byte for byte: the trailing slash is part of the issuer.
        "https://id.example.com/application/o/rdownloader",
    ] {
        let token = key.sign(&key.header(), &with(claims(), "iss", json!(issuer)));
        assert_eq!(
            verify(&token, &keys(&[&key])).map(|_| ()),
            Err(TokenError::Issuer),
            "{issuer}"
        );
    }
}

/// O-AUD: a token minted for another client of the same provider.
#[test]
fn o_aud_a_token_for_another_client_is_refused() {
    let key = TestKey::es256("ec");
    let set = keys(&[&key]);
    let foreign = key.sign(&key.header(), &with(claims(), "aud", json!("jellyfin")));
    assert_eq!(
        verify(&foreign, &set).map(|_| ()),
        Err(TokenError::Audience)
    );
    // Any audience besides this client is untrusted, `azp` or not — stricter than the ADR's
    // "several audiences need `azp`", and the library's default.
    let several = with(claims(), "aud", json!(["jellyfin", CLIENT]));
    for token in [
        key.sign(&key.header(), &several),
        key.sign(&key.header(), &with(several.clone(), "azp", json!(CLIENT))),
    ] {
        assert_eq!(verify(&token, &set).map(|_| ()), Err(TokenError::Audience));
    }
    let token = key.sign(&key.header(), &with(claims(), "aud", json!([CLIENT])));
    assert!(verify(&token, &set).is_ok());
}

/// O-TIME: the leeway is sixty seconds and not one more.
#[test]
fn o_time_expiry_has_a_fixed_minute_of_leeway() {
    let key = TestKey::es256("ec");
    let set = keys(&[&key]);
    let expired_by = |seconds: i64| {
        let token = key.sign(&key.header(), &with(claims(), "exp", json!(NOW - seconds)));
        verify(&token, &set).map(|_| ())
    };
    assert_eq!(expired_by(61), Err(TokenError::Expired));
    assert_eq!(expired_by(59), Ok(()));
}

#[test]
fn o_time_a_token_from_the_future_is_refused() {
    let key = TestKey::es256("ec");
    let set = keys(&[&key]);
    let token = key.sign(&key.header(), &with(claims(), "iat", json!(NOW + 61)));
    assert_eq!(
        verify(&token, &set).map(|_| ()),
        Err(TokenError::IssuedInFuture)
    );
    let token = key.sign(&key.header(), &with(claims(), "iat", json!(NOW + 59)));
    assert!(verify(&token, &set).is_ok());
}

#[test]
fn the_time_claims_are_required() {
    let key = TestKey::es256("ec");
    for claim in ["exp", "iat"] {
        let token = key.sign(&key.header(), &without(claims(), claim));
        assert_eq!(
            verify(&token, &keys(&[&key])).map(|_| ()),
            Err(TokenError::Malformed),
            "{claim}"
        );
    }
}

/// O-REPLAY: a token minted for another sign-in — another nonce, or issued before this one
/// started.
#[test]
fn o_replay_another_flows_token_is_refused() {
    let key = TestKey::es256("ec");
    let set = keys(&[&key]);
    let token = key.sign(
        &key.header(),
        &with(claims(), "nonce", json!("an-older-nonce")),
    );
    assert_eq!(verify(&token, &set).map(|_| ()), Err(TokenError::Nonce));
    let token = key.sign(&key.header(), &without(claims(), "nonce"));
    assert_eq!(verify(&token, &set).map(|_| ()), Err(TokenError::Nonce));
    // The flow started thirty seconds ago; a token from before that, beyond the leeway.
    let token = key.sign(&key.header(), &with(claims(), "iat", json!(NOW - 30 - 61)));
    assert_eq!(
        verify(&token, &set).map(|_| ()),
        Err(TokenError::IssuedBeforeFlow)
    );
}

#[test]
fn a_token_without_a_subject_is_refused() {
    let key = TestKey::es256("ec");
    let set = keys(&[&key]);
    // Without `sub` the token is no ID token at all; an empty one is refused by name.
    let token = key.sign(&key.header(), &without(claims(), "sub"));
    assert_eq!(verify(&token, &set).map(|_| ()), Err(TokenError::Malformed));
    let token = key.sign(&key.header(), &with(claims(), "sub", json!("")));
    assert_eq!(verify(&token, &set).map(|_| ()), Err(TokenError::Subject));
}

/// O-WHO's half that lives in the token: the group condition reads a string or a list.
#[test]
fn the_group_claim_is_read_as_a_string_or_a_list() {
    let key = TestKey::es256("ec");
    let set = keys(&[&key]);
    let token = key.sign(&key.header(), &claims());
    let identity = verify(&token, &set).expect("verified");
    assert!(identity.has_group("groups", "rdownloader-admins"));
    assert!(!identity.has_group("groups", "rdownloader"));
    assert!(!identity.has_group("roles", "rdownloader-admins"));
    let token = key.sign(&key.header(), &with(claims(), "groups", json!("family")));
    assert!(
        verify(&token, &set)
            .expect("verified")
            .has_group("groups", "family")
    );
}

#[test]
fn every_refusal_has_a_distinct_reason() {
    let all = [
        TokenError::Malformed,
        TokenError::Algorithm,
        TokenError::UnknownKey,
        TokenError::AmbiguousKey,
        TokenError::Signature,
        TokenError::Issuer,
        TokenError::Audience,
        TokenError::Expired,
        TokenError::IssuedInFuture,
        TokenError::IssuedBeforeFlow,
        TokenError::Nonce,
        TokenError::Subject,
    ];
    let reasons: std::collections::BTreeSet<&str> =
        all.iter().map(|error| error.reason()).collect();
    assert_eq!(reasons.len(), all.len());
}

/// The person's details stay out of a `{:?}`: issuer and subject only.
#[test]
fn the_debug_form_names_issuer_and_subject_only() {
    let key = TestKey::es256("ec");
    let token = key.sign(&key.header(), &claims());
    let identity = verify(&token, &keys(&[&key])).expect("verified");
    let printed = format!("{identity:?}");
    assert!(printed.contains("administrator-subject"), "{printed}");
    assert!(!printed.contains("owner"), "{printed}");
    assert!(!printed.contains("rdownloader-admins"), "{printed}");
}
