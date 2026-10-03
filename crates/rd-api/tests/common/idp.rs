//! A stand-in identity provider (RD-190-15): the discovery document, the key set and the token
//! endpoint on a loopback port, signing ID tokens with real keys (`rd_authn::oidc_testing`).
//!
//! There is no authorization endpoint. The test plays the browser and the person at the
//! provider: it reads the authorization request the service built, tells the provider which code
//! it will be asked to redeem and what that code stands for ([`Grant`]), and calls the callback
//! itself. The token endpoint then checks what a real one checks — the client's Basic
//! credentials, a code used once, the PKCE verifier against the challenge, the redirect URI — so a
//! service that skipped any of them would fail here rather than pass against a stub.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rd_authn::oidc_testing::{TestKey, key_set};
use serde_json::{Value, json};

/// The client ID every test registers.
pub const CLIENT_ID: &str = "rdownloader";

/// What one authorization code stands for.
#[derive(Clone, Debug)]
pub struct Grant {
    challenge: String,
    redirect_uri: String,
    nonce: String,
    /// Laid over the default claims; a `null` removes the claim.
    claims: Value,
    /// Which key signs, by the index [`FakeIdp::add_key`] returned.
    key: usize,
    /// The JOSE header, when not the signing key's own.
    header: Option<Value>,
}

impl Grant {
    /// The grant an authorization request asks for, signed by the first key, for `subject`.
    ///
    /// # Panics
    ///
    /// When the request lacks a parameter the service always sends.
    pub fn for_request(authorization_url: &str, subject: &str) -> Self {
        let url = url::Url::parse(authorization_url).expect("an authorization URL");
        let parameter = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
                .unwrap_or_else(|| panic!("the authorization request has no {name}"))
        };
        Self {
            challenge: parameter("code_challenge"),
            redirect_uri: parameter("redirect_uri"),
            nonce: parameter("nonce"),
            claims: json!({ "sub": subject }),
            key: 0,
            header: None,
        }
    }

    /// Lays `claims` over what the token would otherwise say.
    #[must_use]
    pub fn with_claims(mut self, claims: Value) -> Self {
        if let (Some(target), Some(source)) = (self.claims.as_object_mut(), claims.as_object()) {
            for (name, value) in source {
                target.insert(name.clone(), value.clone());
            }
        }
        self
    }

    /// Signs with the key at `key`.
    #[must_use]
    pub fn signed_by(mut self, key: usize) -> Self {
        self.key = key;
        self
    }

    /// Signs under `header` instead of the key's own.
    #[must_use]
    pub fn with_header(mut self, header: Value) -> Self {
        self.header = Some(header);
        self
    }
}

struct Inner {
    keys: Vec<Arc<TestKey>>,
    /// Indexes of the keys the key set publishes.
    published: Vec<usize>,
    codes: HashMap<String, Grant>,
    discovery: Vec<(String, Value)>,
    jwks_fetches: u32,
    token_requests: u32,
    minted: Vec<String>,
}

/// The provider, listening on `127.0.0.1`.
#[derive(Clone)]
pub struct FakeIdp {
    /// `http://127.0.0.1:<port>`, as its discovery document states it.
    pub issuer: String,
    secret: String,
    access_token: String,
    refresh_token: String,
    inner: Arc<Mutex<Inner>>,
}

impl FakeIdp {
    /// A provider whose client secret is `secret`, with one ES256 key published.
    pub async fn start(secret: &str) -> Self {
        Self::start_with(secret, "an-access-token", "a-refresh-token").await
    }

    /// The same, answering the token request with these access and refresh tokens — what the
    /// service must drop unread.
    pub async fn start_with(secret: &str, access_token: &str, refresh_token: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the provider");
        let address = listener.local_addr().expect("the provider's address");
        let idp = Self {
            issuer: format!("http://{address}"),
            secret: secret.to_owned(),
            access_token: access_token.to_owned(),
            refresh_token: refresh_token.to_owned(),
            inner: Arc::new(Mutex::new(Inner {
                keys: vec![Arc::new(TestKey::es256("key-1"))],
                published: vec![0],
                codes: HashMap::new(),
                discovery: Vec::new(),
                jwks_fetches: 0,
                token_requests: 0,
                minted: Vec::new(),
            })),
        };
        let router = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/token", post(token))
            .with_state(idp.clone());
        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve the provider");
        });
        idp
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("the provider's state")
    }

    /// Adds a signing key; `published` decides whether the key set carries it.
    pub fn add_key(&self, key: TestKey, published: bool) -> usize {
        let mut inner = self.lock();
        inner.keys.push(Arc::new(key));
        let index = inner.keys.len() - 1;
        if published {
            inner.published.push(index);
        }
        index
    }

    /// Publishes exactly the keys at `indexes`: a rotation.
    pub fn publish(&self, indexes: &[usize]) {
        self.lock().published = indexes.to_vec();
    }

    /// Sets a member of the discovery document.
    pub fn override_discovery(&self, member: &str, value: Value) {
        self.lock().discovery.push((member.to_owned(), value));
    }

    /// The code the browser will bring back, and what it stands for.
    pub fn expect_code(&self, code: &str, grant: Grant) {
        self.lock().codes.insert(code.to_owned(), grant);
    }

    pub fn jwks_fetches(&self) -> u32 {
        self.lock().jwks_fetches
    }

    pub fn token_requests(&self) -> u32 {
        self.lock().token_requests
    }

    /// Every ID token handed out, for the search that it was not handed on.
    pub fn minted(&self) -> Vec<String> {
        self.lock().minted.clone()
    }
}

async fn discovery(State(idp): State<FakeIdp>) -> Json<Value> {
    let issuer = &idp.issuer;
    let mut document = json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "end_session_endpoint": format!("{issuer}/logout"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256", "ES256", "EdDSA"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic"],
    });
    for (member, value) in &idp.lock().discovery {
        document[member.as_str()] = value.clone();
    }
    Json(document)
}

async fn jwks(State(idp): State<FakeIdp>) -> Json<Value> {
    let mut inner = idp.lock();
    inner.jwks_fetches += 1;
    let keys: Vec<Arc<TestKey>> = inner
        .published
        .iter()
        .map(|index| Arc::clone(&inner.keys[*index]))
        .collect();
    let keys: Vec<&TestKey> = keys.iter().map(|key| &**key).collect();
    Json(key_set(&keys))
}

async fn token(
    State(idp): State<FakeIdp>,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, Json<Value>) {
    let refuse = |error: &str| (StatusCode::BAD_REQUEST, Json(json!({ "error": error })));
    let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let mut inner = idp.lock();
    inner.token_requests += 1;
    let expected = format!(
        "Basic {}",
        STANDARD.encode(format!("{CLIENT_ID}:{}", idp.secret))
    );
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid_client" })),
        );
    }
    if form.get("grant_type").map(String::as_str) != Some("authorization_code") {
        return refuse("unsupported_grant_type");
    }
    // Removed on use: a provider redeems a code once.
    let Some(grant) = form.get("code").and_then(|code| inner.codes.remove(code)) else {
        return refuse("invalid_grant");
    };
    let verifier = form.get("code_verifier").map(String::as_str).unwrap_or("");
    if rd_authn::oidc::code_challenge(verifier) != grant.challenge
        || form.get("redirect_uri") != Some(&grant.redirect_uri)
    {
        return refuse("invalid_grant");
    }
    let now = chrono::Utc::now().timestamp();
    let mut claims = json!({
        "iss": idp.issuer,
        "aud": CLIENT_ID,
        "exp": now + 300,
        "iat": now,
        "nonce": grant.nonce,
        "preferred_username": "owner",
        "email": "owner@example.com",
    });
    if let (Some(target), Some(source)) = (claims.as_object_mut(), grant.claims.as_object()) {
        for (name, value) in source {
            if value.is_null() {
                target.remove(name);
            } else {
                target.insert(name.clone(), value.clone());
            }
        }
    }
    let key = Arc::clone(&inner.keys[grant.key]);
    let header = grant.header.clone().unwrap_or_else(|| key.header());
    let id_token = key.sign(&header, &claims);
    inner.minted.push(id_token.clone());
    (
        StatusCode::OK,
        Json(json!({
            "access_token": idp.access_token,
            "token_type": "Bearer",
            "expires_in": 300,
            "refresh_token": idp.refresh_token,
            "id_token": id_token,
        })),
    )
}
