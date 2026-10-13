//! The mock authorization server the shared sign-in cases and OneDrive's device cases run
//! against. It answers at the host boundary, so what it records is what the plugin wrote: the
//! markers, and never a value.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolvedHeader, ResolverHost,
};

use super::{Case, DeviceAnswers, SignIn, Tokens};
use crate::common::oauth::{Recorded, Stored, s256};

/// The mock authorization server: a token endpoint, and a device endpoint where the row has one.
pub struct TokenEndpoint {
    tokens: Tokens,
    pkce: bool,
    client_secret: Option<&'static str>,
    /// Whether the account carries the secret of a registered application.
    registered: bool,
    case: Mutex<Case>,
    challenge: Mutex<Option<String>>,
    requests: Mutex<Vec<Recorded>>,
    stored: Mutex<Vec<Stored>>,
}

impl TokenEndpoint {
    fn build(row: &SignIn, case: Case, registered: bool) -> Arc<Self> {
        Arc::new(Self {
            tokens: row.tokens,
            pkce: row.pkce,
            client_secret: row.client_secret,
            registered,
            case: Mutex::new(case),
            challenge: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(Vec::new()),
        })
    }

    pub fn new(row: &SignIn, case: Case) -> Arc<Self> {
        Self::build(row, case, true)
    }

    /// An account whose vault holds no application secret.
    pub fn unregistered(row: &SignIn) -> Arc<Self> {
        Self::build(row, Case::Granted, false)
    }

    pub fn set_case(&self, case: Case) {
        *self.case.lock().expect("case") = case;
    }

    pub fn expect_challenge(&self, challenge: &str) {
        *self.challenge.lock().expect("challenge") = Some(challenge.to_owned());
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("requests").clone()
    }

    pub fn stored(&self) -> Vec<Stored> {
        self.stored.lock().expect("stored").clone()
    }

    fn device(&self) -> DeviceAnswers {
        self.tokens.device.expect("the provider's device answers")
    }
}

#[async_trait]
impl ResolverHost for TokenEndpoint {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let form: Vec<(String, String)> = request
            .query
            .iter()
            .map(|value| (value.name.clone(), value.value_template.clone()))
            .collect();
        let url = request.url.to_string();
        self.requests.lock().expect("requests").push(Recorded {
            method: request.method.clone(),
            url: url.clone(),
            form: form.clone(),
        });
        let field = |name: &str| {
            form.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let body = |status: u16, text: &str, headers: Vec<ResolvedHeader>| {
            Ok(HostHttpResponse {
                status,
                final_url: request.url.clone(),
                headers,
                body: text.as_bytes().to_vec(),
            })
        };
        let case = *self.case.lock().expect("case");
        // The device entrance is two endpoints, not one, told apart by the address.
        if let Some(device) = self.tokens.device
            && url.ends_with("/devicecode")
            && case != Case::Unreachable
        {
            assert_eq!(
                field("client_id").as_deref(),
                Some("{{client_id}}"),
                "a device code is asked for with the application marker, never a value"
            );
            return body(200, device.code, Vec::new());
        }
        match case {
            // The check a real authorization server makes: the verifier the client kept back
            // has to hash to the challenge it published. Without it the "PKCE" here would be
            // decoration.
            Case::Granted if self.pkce => {
                let expected = self.challenge.lock().expect("challenge").clone();
                match (expected, field("code_verifier")) {
                    (Some(expected), Some(verifier)) if s256(&verifier) == expected => {
                        body(200, self.tokens.granted, Vec::new())
                    }
                    _ => body(400, self.tokens.invalid_grant, Vec::new()),
                }
            }
            Case::Granted => body(200, self.tokens.granted, Vec::new()),
            Case::RenewalGranted => {
                assert!(
                    field("code_verifier").is_none(),
                    "a refresh grant must not carry a PKCE verifier"
                );
                body(200, self.tokens.renewed, Vec::new())
            }
            Case::Denied => body(400, self.tokens.denied, Vec::new()),
            Case::InvalidGrant => body(400, self.tokens.invalid_grant, Vec::new()),
            Case::InvalidClient => body(
                401,
                self.tokens
                    .invalid_client
                    .expect("an invalid_client answer"),
                Vec::new(),
            ),
            Case::RateLimited => body(
                429,
                self.tokens.rate_limited,
                vec![ResolvedHeader {
                    name: "Retry-After".to_owned(),
                    value: "90".to_owned(),
                }],
            ),
            Case::Unreachable => Err(Failure::coded(
                FailureKind::Offline,
                "plugin.offline",
                "the provider could not be reached",
            )),
            Case::DevicePending => body(400, self.device().pending, Vec::new()),
            Case::DeviceGranted => {
                assert!(
                    field("code_verifier").is_none(),
                    "a device grant carries no PKCE verifier"
                );
                body(200, self.tokens.granted, Vec::new())
            }
            Case::DeviceDeclined => body(400, self.device().declined, Vec::new()),
        }
    }

    async fn secret_available(&self, _account_id: AccountId, reference: &str) -> bool {
        match self.client_secret {
            Some(secret) => self.registered && reference == secret,
            None => true,
        }
    }

    async fn store_oauth_token(
        &self,
        account_id: AccountId,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_in_seconds: Option<u64>,
    ) -> Result<(), Failure> {
        crate::common::oauth::store(
            &self.stored,
            account_id,
            access_token,
            refresh_token,
            expires_in_seconds,
        )
    }
}
