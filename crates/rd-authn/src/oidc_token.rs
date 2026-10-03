//! The ID token an identity provider hands back, verified (RD-190-15, ADR 0021).
//!
//! The verification is `openidconnect`'s (owner's decision, 2026-10-02): the JOSE parsing, the key
//! selection, the signature over the provider's key set, issuer, audience, expiry and nonce. This
//! module only configures it to the ADR's rules and translates its refusals:
//!
//! * the algorithms are [`Algorithm::ALLOWED`] — RS256, PS256, ES256, EdDSA — intersected with what
//!   the provider says it signs with; `none` and every `HS*` are never on the list, and the verifier
//!   is a public client's, so no client secret can ever become a signing key;
//! * the clock gets [`CLOCK_LEEWAY_SECONDS`] for `exp`, and `iat` may be neither more than that in
//!   the future nor earlier than the sign-in started (less the same leeway);
//! * any audience besides the client is refused, which is stricter than demanding `azp`;
//! * the subject must not be empty, and the claims beyond the standard ones are kept for the
//!   optional group condition.

use openidconnect::{
    AdditionalClaims, ClaimsVerificationError, ClientId, IdToken, IdTokenVerifier, IssuerUrl,
    Nonce, SignatureVerificationError,
    core::{
        CoreGenderClaim, CoreJsonWebKey, CoreJsonWebKeySet, CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// How far a provider's clock may be from this one, either way.
///
/// Fixed rather than a setting: a leeway somebody can raise is a token lifetime somebody can
/// raise, and a minute covers every clock that is synchronised at all.
pub const CLOCK_LEEWAY_SECONDS: i64 = 60;

/// A provider's published keys (`jwks_uri`), as the library reads them.
pub type JsonWebKeySet = CoreJsonWebKeySet;

/// The claims beyond the standard ones, for the group condition (D2).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ExtraClaims(Map<String, Value>);

impl AdditionalClaims for ExtraClaims {}

type ProviderIdToken = IdToken<
    ExtraClaims,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;

/// The signature algorithms an ID token may use. `none` and every `HS*` are absent on purpose:
/// an HMAC is keyed by the client secret, which would make a value this service stores and the
/// provider's administrators can read a key that signs in as the administrator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Algorithm {
    Rs256,
    Ps256,
    Es256,
    EdDsa,
}

impl Algorithm {
    /// The allowlist, in the order a provider's list is intersected with.
    pub const ALLOWED: [Self; 4] = [Self::Rs256, Self::Ps256, Self::Es256, Self::EdDsa];

    /// The library's name for it.
    #[must_use]
    pub const fn core(self) -> CoreJwsSigningAlgorithm {
        match self {
            Self::Rs256 => CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            Self::Ps256 => CoreJwsSigningAlgorithm::RsaSsaPssSha256,
            Self::Es256 => CoreJwsSigningAlgorithm::EcdsaP256Sha256,
            Self::EdDsa => CoreJwsSigningAlgorithm::EdDsa,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rs256 => "RS256",
            Self::Ps256 => "PS256",
            Self::Es256 => "ES256",
            Self::EdDsa => "EdDSA",
        }
    }
}

/// What a token has to match.
pub struct Expectations<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub nonce: &'a str,
    /// Unix seconds the sign-in started at: a token issued before it was not minted for it.
    pub flow_started_at: i64,
    /// Unix seconds now.
    pub now: i64,
    /// [`Algorithm::ALLOWED`] intersected with what the provider says it signs with.
    pub algorithms: &'a [Algorithm],
}

/// The identity a verified token names.
#[derive(Clone)]
pub struct IdentityClaims {
    pub issuer: String,
    pub subject: String,
    label: Option<String>,
    extra: Map<String, Value>,
}

/// Issuer and subject only: the other claims are the person's details, not the service's log.
impl std::fmt::Debug for IdentityClaims {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityClaims")
            .field("issuer", &self.issuer)
            .field("subject", &self.subject)
            .finish_non_exhaustive()
    }
}

impl IdentityClaims {
    /// What to call this identity on screen: the first of `preferred_username`, `name` and
    /// `email` the token carries. A label and never a decision — each of them is editable at the
    /// provider and two providers can hand out the same one.
    #[must_use]
    pub fn label(&self) -> Option<String> {
        self.label.clone()
    }

    /// Whether the claim `claim` is `value` or a list holding it. The optional group condition
    /// (D2): it can only narrow who signs in, never name the administrator on its own.
    #[must_use]
    pub fn has_group(&self, claim: &str, value: &str) -> bool {
        match self.extra.get(claim) {
            Some(Value::String(single)) => single == value,
            Some(Value::Array(values)) => values.iter().any(|entry| entry.as_str() == Some(value)),
            _ => false,
        }
    }
}

/// Why a token was refused. The [`reason`](Self::reason) is what the log carries: the check that
/// failed, never a value from the token.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TokenError {
    #[error("the token is not a compact JWS with the claims an ID token needs")]
    Malformed,
    #[error("the token is signed with an algorithm outside the allowlist or not offered")]
    Algorithm,
    #[error("no key of the provider's key set fits the token's key id and algorithm")]
    UnknownKey,
    #[error("the token names no key id and the key set holds several that fit")]
    AmbiguousKey,
    #[error("the signature does not verify")]
    Signature,
    #[error("the token was issued by another issuer")]
    Issuer,
    #[error("the token was minted for another client, or for others besides this one")]
    Audience,
    #[error("the token has expired")]
    Expired,
    #[error("the token says it was issued in the future")]
    IssuedInFuture,
    #[error("the token was issued before this sign-in started")]
    IssuedBeforeFlow,
    #[error("the token does not carry this sign-in's nonce")]
    Nonce,
    #[error("the token names no subject")]
    Subject,
}

impl TokenError {
    /// A stable word for the check that failed.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::Algorithm => "algorithm",
            Self::UnknownKey => "unknown_key",
            Self::AmbiguousKey => "ambiguous_key",
            Self::Signature => "signature",
            Self::Issuer => "issuer",
            Self::Audience => "audience",
            Self::Expired => "expired",
            Self::IssuedInFuture => "issued_in_future",
            Self::IssuedBeforeFlow => "issued_before_flow",
            Self::Nonce => "nonce",
            Self::Subject => "subject",
        }
    }
}

/// The `iat` refusals, as the issue-time check reports them to the library.
const ISSUED_IN_FUTURE: &str = "issued_in_future";
const ISSUED_BEFORE_FLOW: &str = "issued_before_flow";

/// Verifies `token` against `keys` and `expect`.
///
/// # Errors
///
/// The check that failed. [`TokenError::UnknownKey`] is the one a caller may answer by fetching
/// the key set again — once, and not more often than the refetch gate allows.
pub fn verify_id_token(
    token: &str,
    keys: &JsonWebKeySet,
    expect: &Expectations<'_>,
) -> Result<IdentityClaims, TokenError> {
    let token: ProviderIdToken = token.parse().map_err(|_| TokenError::Malformed)?;
    let issuer = IssuerUrl::new(expect.issuer.to_owned()).map_err(|_| TokenError::Issuer)?;
    let now = expect.now;
    let started = expect.flow_started_at;
    // The library refuses once the current time reaches `exp`; a clock read a minute early is
    // the leeway, and a fixed one.
    let clock = chrono::DateTime::from_timestamp(now.saturating_sub(CLOCK_LEEWAY_SECONDS), 0)
        .unwrap_or_default();
    let verifier: IdTokenVerifier<'_, CoreJsonWebKey> = IdTokenVerifier::new_public_client(
        ClientId::new(expect.client_id.to_owned()),
        issuer,
        keys.clone(),
    )
    .set_allowed_algs(expect.algorithms.iter().map(|algorithm| algorithm.core()))
    .set_time_fn(move || clock)
    .set_issue_time_verifier_fn(move |issued: chrono::DateTime<chrono::Utc>| {
        let issued = issued.timestamp();
        if issued > now.saturating_add(CLOCK_LEEWAY_SECONDS) {
            Err(ISSUED_IN_FUTURE.to_owned())
        } else if issued < started.saturating_sub(CLOCK_LEEWAY_SECONDS) {
            Err(ISSUED_BEFORE_FLOW.to_owned())
        } else {
            Ok(())
        }
    });
    let nonce = Nonce::new(expect.nonce.to_owned());
    let claims = token.into_claims(&verifier, &nonce).map_err(refusal)?;

    let subject = claims.subject().as_str().to_owned();
    if subject.is_empty() || subject.len() > 255 {
        return Err(TokenError::Subject);
    }
    let label = claims
        .preferred_username()
        .map(|name| name.as_str().to_owned())
        .or_else(|| {
            claims
                .name()
                .and_then(|name| name.get(None))
                .map(|name| name.as_str().to_owned())
        })
        .or_else(|| claims.email().map(|email| email.as_str().to_owned()))
        .map(|value| value.trim().chars().take(100).collect::<String>())
        .filter(|value| !value.is_empty());
    Ok(IdentityClaims {
        issuer: expect.issuer.to_owned(),
        subject,
        label,
        extra: claims.additional_claims().0.clone(),
    })
}

/// The library's refusal, as the check that failed.
fn refusal(error: ClaimsVerificationError) -> TokenError {
    match error {
        ClaimsVerificationError::InvalidIssuer(_) => TokenError::Issuer,
        ClaimsVerificationError::InvalidAudience(_) => TokenError::Audience,
        ClaimsVerificationError::InvalidNonce(_) => TokenError::Nonce,
        ClaimsVerificationError::InvalidSubject(_) => TokenError::Subject,
        ClaimsVerificationError::Expired(reason) => match reason.as_str() {
            ISSUED_IN_FUTURE => TokenError::IssuedInFuture,
            ISSUED_BEFORE_FLOW => TokenError::IssuedBeforeFlow,
            _ => TokenError::Expired,
        },
        ClaimsVerificationError::SignatureVerification(signature) => match signature {
            SignatureVerificationError::DisallowedAlg(_)
            | SignatureVerificationError::UnsupportedAlg(_)
            | SignatureVerificationError::NoSignature => TokenError::Algorithm,
            SignatureVerificationError::NoMatchingKey => TokenError::UnknownKey,
            SignatureVerificationError::AmbiguousKeyId(_) => TokenError::AmbiguousKey,
            _ => TokenError::Signature,
        },
        _ => TokenError::Malformed,
    }
}

#[cfg(test)]
#[path = "oidc_token_tests.rs"]
mod tests;
