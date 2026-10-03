//! Keys and tokens of a stand-in identity provider, for tests in this crate and in `rd-api`.
//!
//! Public but hidden, like `LocalControl::for_token`: the integration tests run a fake provider
//! (discovery, key set, token endpoint) and need to sign what it hands out exactly as a real one
//! would. Nothing in the service calls this.

use aws_lc_rs::{
    rand::SystemRandom,
    rsa::KeySize,
    signature::{self, EcdsaKeyPair, Ed25519KeyPair, KeyPair as _, RsaKeyPair},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};

enum Material {
    Ec(EcdsaKeyPair),
    Ed(Ed25519KeyPair),
    Rsa(RsaKeyPair),
}

/// One signing key of the stand-in provider.
pub struct TestKey {
    pub kid: String,
    material: Material,
}

impl TestKey {
    /// A P-256 key, for `ES256`.
    ///
    /// # Panics
    ///
    /// When the key cannot be generated, which is a broken test environment.
    #[must_use]
    pub fn es256(kid: &str) -> Self {
        let pair = EcdsaKeyPair::generate(&signature::ECDSA_P256_SHA256_FIXED_SIGNING)
            .expect("a P-256 key");
        Self {
            kid: kid.to_owned(),
            material: Material::Ec(pair),
        }
    }

    /// An Ed25519 key, for `EdDSA`.
    ///
    /// # Panics
    ///
    /// When the key cannot be generated.
    #[must_use]
    pub fn ed25519(kid: &str) -> Self {
        Self {
            kid: kid.to_owned(),
            material: Material::Ed(Ed25519KeyPair::generate().expect("an Ed25519 key")),
        }
    }

    /// A 2048-bit RSA key, for `RS256` and `PS256`.
    ///
    /// # Panics
    ///
    /// When the key cannot be generated.
    #[must_use]
    pub fn rsa(kid: &str) -> Self {
        Self {
            kid: kid.to_owned(),
            material: Material::Rsa(RsaKeyPair::generate(KeySize::Rsa2048).expect("an RSA key")),
        }
    }

    /// The algorithm this key signs with by default.
    #[must_use]
    pub fn algorithm(&self) -> &'static str {
        match self.material {
            Material::Ec(_) => "ES256",
            Material::Ed(_) => "EdDSA",
            Material::Rsa(_) => "RS256",
        }
    }

    /// The public half as a JWK, as a provider's key set publishes it.
    #[must_use]
    pub fn jwk(&self) -> Value {
        match &self.material {
            Material::Ec(pair) => {
                let point = pair.public_key().as_ref();
                json!({
                    "kty": "EC",
                    "crv": "P-256",
                    "kid": self.kid,
                    "use": "sig",
                    "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
                    "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
                })
            }
            Material::Ed(pair) => json!({
                "kty": "OKP",
                "crv": "Ed25519",
                "kid": self.kid,
                "use": "sig",
                "x": URL_SAFE_NO_PAD.encode(pair.public_key().as_ref()),
            }),
            Material::Rsa(pair) => {
                let public = pair.public_key();
                json!({
                    "kty": "RSA",
                    "kid": self.kid,
                    "use": "sig",
                    "n": URL_SAFE_NO_PAD.encode(public.modulus().big_endian_without_leading_zero()),
                    "e": URL_SAFE_NO_PAD.encode(public.exponent().big_endian_without_leading_zero()),
                })
            }
        }
    }

    /// The JOSE header this key signs under: its default algorithm and its key id.
    #[must_use]
    pub fn header(&self) -> Value {
        json!({ "alg": self.algorithm(), "kid": self.kid, "typ": "JWT" })
    }

    /// A compact JWS of `claims` under `header`, signed by this key.
    ///
    /// The header is not checked against the key, so a test can claim one algorithm and sign
    /// with another. An RSA key signs with PSS when the header says `PS256`, else PKCS#1.
    ///
    /// # Panics
    ///
    /// When signing fails.
    #[must_use]
    pub fn sign(&self, header: &Value, claims: &Value) -> String {
        let input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let signature = match &self.material {
            Material::Ec(pair) => pair
                .sign(&SystemRandom::new(), input.as_bytes())
                .expect("an ECDSA signature")
                .as_ref()
                .to_vec(),
            Material::Ed(pair) => pair.sign(input.as_bytes()).as_ref().to_vec(),
            Material::Rsa(pair) => {
                let mut bytes = vec![0; pair.public_modulus_len()];
                let padding: &'static dyn signature::RsaEncoding = if header["alg"] == "PS256" {
                    &signature::RSA_PSS_SHA256
                } else {
                    &signature::RSA_PKCS1_SHA256
                };
                pair.sign(padding, &SystemRandom::new(), input.as_bytes(), &mut bytes)
                    .expect("an RSA signature");
                bytes
            }
        };
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature))
    }
}

/// A key set holding `keys`.
#[must_use]
pub fn key_set(keys: &[&TestKey]) -> Value {
    json!({ "keys": keys.iter().map(|key| key.jwk()).collect::<Vec<_>>() })
}

/// A token with `header` and `claims` and a signature of arbitrary bytes: what `alg: none` and
/// an HMAC-signed forgery look like to a verifier that must refuse them before reading further.
#[must_use]
pub fn unsigned(header: &Value, claims: &Value, signature: &[u8]) -> String {
    format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string()),
        URL_SAFE_NO_PAD.encode(signature)
    )
}
