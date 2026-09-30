//! The HTTP client the CLI talks to a server with.
//!
//! One path for local and remote alike. "Local" is only the default address: a running
//! service already holds the SQLite file, and a second code path that opened the database
//! directly would be a different implementation of every command, with its own bugs and its
//! own answer to "what does the server think right now".

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

/// Default address, matching the service's own default listen address.
pub const DEFAULT_SERVER: &str = "http://127.0.0.1:8710";

/// Why a command failed, mapped to a distinct process exit code.
///
/// A script that gets `1` for everything cannot tell "wrong token" from "host is down" from
/// "that id does not exist", and those call for three different reactions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    /// The arguments could not be used at all (2).
    Usage = 2,
    /// The server could not be reached (3).
    Unreachable = 3,
    /// The credential was missing, wrong or insufficient (4).
    Unauthorized = 4,
    /// The server answered, but the thing asked for does not exist (5).
    NotFound = 5,
    /// Anything else the server refused (1).
    Other = 1,
}

impl Failure {
    /// The exit code this failure ends the process with.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// Classifies an HTTP status.
    #[must_use]
    pub const fn of_status(status: u16) -> Self {
        match status {
            401 | 403 => Self::Unauthorized,
            404 => Self::NotFound,
            400 | 422 => Self::Usage,
            _ => Self::Other,
        }
    }
}

/// An error carrying the exit code the CLI should end with.
#[derive(Debug)]
pub struct CommandError {
    pub failure: Failure,
    pub message: String,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

impl CommandError {
    #[must_use]
    pub fn new(failure: Failure, message: impl Into<String>) -> Self {
        Self {
            failure,
            message: message.into(),
        }
    }
}

/// A connection to one rDownloader server.
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Client {
    /// Builds a client for `server`, authenticating with `token` when one is given.
    pub fn new(server: &str, token: Option<String>, timeout_seconds: u64) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_seconds))
            .build()
            .context("build HTTP client")?;
        Self::with(http, server, token)
    }

    /// A client for the service on this machine: [`local_http`]'s, so the local control token
    /// never travels through a proxy the environment names.
    pub fn local(server: &str, token: Option<String>, timeout_seconds: u64) -> Result<Self> {
        let http = local_http(std::time::Duration::from_secs(timeout_seconds))?;
        Self::with(http, server, token)
    }

    fn with(http: reqwest::Client, server: &str, token: Option<String>) -> Result<Self> {
        let base = server.trim_end_matches('/').to_owned();
        if !base.starts_with("http://") && !base.starts_with("https://") {
            return Err(CommandError::new(
                Failure::Usage,
                "server address must start with http:// or https://",
            )
            .into());
        }
        Ok(Self { http, base, token })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send::<T, ()>(reqwest::Method::GET, path, None).await
    }

    pub async fn post<T: DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send::<T, ()>(reqwest::Method::DELETE, path, None)
            .await
    }

    async fn send<T: DeserializeOwned, B: serde::Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T> {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|error| {
            CommandError::new(
                Failure::Unreachable,
                format!("request to {} failed: {error}", self.base),
            )
        })?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status.is_success() {
            // A 200 with an empty body is a valid answer for a delete; `null` deserializes
            // into the unit type, which is what those calls ask for.
            let text = if text.trim().is_empty() {
                "null"
            } else {
                &text
            };
            return serde_json::from_str(text)
                .with_context(|| format!("unexpected response from {path}"));
        }
        Err(CommandError::new(Failure::of_status(status.as_u16()), describe(status, &text)).into())
    }
}

/// The HTTP client for everything that talks to the service on this machine: `stop`, the
/// updater's stop and its health checks (security review 2026-09-30, finding 1).
///
/// Without a proxy, whatever `HTTP_PROXY`/`ALL_PROXY` or the system settings say: reqwest's
/// proxy matcher exempts nothing but `NO_PROXY`, not even loopback, so a corporate proxy
/// received the Bearer token of the local control file and every update failed with
/// `update.service_did_not_stop`.
pub fn local_http(timeout: std::time::Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .build()
        .context("build HTTP client")
}

/// Turns an error response into one line, preferring the server's stable code.
///
/// The code is what the web client translates and what a script can branch on, so it is more
/// useful than the prose — but the prose is what tells a person what to do, so both are
/// printed rather than one replacing the other.
fn describe(status: reqwest::StatusCode, body: &str) -> String {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let code = parsed
        .as_ref()
        .and_then(|value| value.get("code"))
        .and_then(serde_json::Value::as_str);
    let message = parsed
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(serde_json::Value::as_str);
    match (code, message) {
        (Some(code), Some(message)) => format!("{status}: {message} [{code}]"),
        (None, Some(message)) => format!("{status}: {message}"),
        _ if body.trim().is_empty() => format!("{status}"),
        _ => format!("{status}: {}", body.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Client, Failure, describe};

    #[test]
    fn a_server_address_needs_a_scheme() {
        // Without this a typo like `localhost:8710` produces a confusing reqwest error
        // rather than the one thing the user has to fix.
        assert!(Client::new("127.0.0.1:8710", None, 5).is_err());
        assert!(Client::new("http://127.0.0.1:8710", None, 5).is_ok());
        assert!(Client::new("https://nas.local/", None, 5).is_ok());
    }

    /// Finding 1 of the 2026-09-30 review: a proxy in the environment took the local control
    /// token. The environment is the child process's own, because a test may not change its
    /// own: the test binary runs this test once more with a dead proxy set.
    #[test]
    fn a_local_client_ignores_the_proxy_the_environment_names() {
        const CHILD: &str = "RD_TEST_LOCAL_CLIENT_CHILD";
        const NAME: &str =
            "remote::client::tests::a_local_client_ignores_the_proxy_the_environment_names";
        if std::env::var_os(CHILD).is_some() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind");
                let address = listener.local_addr().expect("address");
                let router = axum::Router::new().route(
                    "/ping",
                    axum::routing::get(|| async {
                        axum::Json(serde_json::json!({ "pong": true }))
                    }),
                );
                tokio::spawn(async move {
                    axum::serve(listener, router).await.expect("serve");
                });
                let client =
                    Client::local(&format!("http://{address}"), Some("token".to_owned()), 5)
                        .expect("client");
                let answer: serde_json::Value =
                    client.get("/ping").await.expect("reached past the proxy");
                assert_eq!(answer["pong"], true);
            });
            return;
        }
        // A port that was just free: a proxy there refuses every connection.
        let dead = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("bind")
            .local_addr()
            .expect("address");
        let proxy = format!("http://{dead}");
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", NAME, "--nocapture"])
            .env(CHILD, "1")
            .env("HTTP_PROXY", &proxy)
            .env("http_proxy", &proxy)
            .env("ALL_PROXY", &proxy)
            .env("all_proxy", &proxy)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .output()
            .expect("run the test binary");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn every_kind_of_failure_has_its_own_exit_code() {
        // A script branches on these; collapsing them into 1 makes "wrong token" and "host
        // is down" indistinguishable.
        assert_eq!(Failure::of_status(401), Failure::Unauthorized);
        assert_eq!(Failure::of_status(403), Failure::Unauthorized);
        assert_eq!(Failure::of_status(404), Failure::NotFound);
        assert_eq!(Failure::of_status(422), Failure::Usage);
        assert_eq!(Failure::of_status(500), Failure::Other);
        let codes = [
            Failure::Usage.code(),
            Failure::Unreachable.code(),
            Failure::Unauthorized.code(),
            Failure::NotFound.code(),
            Failure::Other.code(),
        ];
        let mut unique = codes.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), codes.len(), "two failures share an exit code");
    }

    #[test]
    fn an_error_response_keeps_both_the_message_and_the_code() {
        let described = describe(
            reqwest::StatusCode::NOT_FOUND,
            r#"{"code":"download.not_found","error":"Download not found"}"#,
        );
        assert!(described.contains("Download not found"), "{described}");
        assert!(described.contains("download.not_found"), "{described}");
    }

    #[test]
    fn a_response_that_is_not_json_still_says_something() {
        let described = describe(reqwest::StatusCode::BAD_GATEWAY, "<html>proxy error</html>");
        assert!(described.contains("502"), "{described}");
        assert!(described.contains("proxy error"), "{described}");
    }
}
