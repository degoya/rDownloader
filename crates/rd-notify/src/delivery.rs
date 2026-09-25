//! Sending one notification to one target.
//!
//! Every path here is written so a secret cannot leak: the webhook secret only ever becomes
//! an HMAC, the SMTP password only reaches `lettre`, and the apprise target URL — which
//! carries the service token — is handed over in the child's environment so it never appears
//! in the process list. What comes back is truncated and redacted before it is stored.

use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use secrecy::ExposeSecret;
use sha2::Sha256;

use crate::model::{NotificationTarget, TargetKind};

/// Header carrying the HMAC of the body, so a receiver can verify the call came from here.
pub const SIGNATURE_HEADER: &str = "X-RDownloader-Signature";
/// Header carrying the delivery's idempotency key, so a receiver can drop a repeat.
pub const IDEMPOTENCY_HEADER: &str = "X-RDownloader-Idempotency-Key";

/// The environment variable the apprise CLI takes its target URLs from.
const APPRISE_URLS_VARIABLE: &str = "APPRISE_URLS";

/// How much of a response is kept for the history.
const MAX_EXCERPT: usize = 500;

/// What a target's own configuration adds on top of the endpoint.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct TargetConfig {
    /// SMTP: sender address.
    pub from: Option<String>,
    /// SMTP: recipients.
    pub to: Vec<String>,
    /// SMTP: username; the password is a vault reference on the target.
    pub username: Option<String>,
    /// SMTP: `starttls` (default), `tls` or `none`.
    pub tls: Option<String>,
    /// SMTP: port; `None` picks 587 for STARTTLS, 465 for implicit TLS, 25 for none.
    pub port: Option<u16>,
    /// Apprise: absolute path of the executable; empty = vendor folders and `PATH`.
    pub executable: Option<String>,
    /// Plugin: which installed notification destination delivers this target.
    pub plugin_id: Option<String>,
}

/// The message being delivered.
#[derive(Clone, Debug)]
pub struct Message {
    pub title: String,
    pub body: String,
    pub event: crate::model::NotificationEvent,
    pub idempotency_key: String,
    pub payload: serde_json::Value,
}

/// The outcome of one attempt.
#[derive(Clone, Debug)]
pub struct Attempt {
    pub ok: bool,
    pub status: Option<u16>,
    pub excerpt: Option<String>,
    /// Whether trying again could succeed. A rejected address never will.
    pub retryable: bool,
}

impl Attempt {
    fn ok(status: Option<u16>) -> Self {
        Self {
            ok: true,
            status,
            excerpt: None,
            retryable: false,
        }
    }

    /// A delivery that a caller outside this crate performed successfully.
    ///
    /// The plugin transport lives in the service, not here, but its outcome has to be the
    /// same shape or the retry policy would need to know which transport answered.
    #[must_use]
    pub fn succeeded() -> Self {
        Self::ok(None)
    }

    /// A delivery that a caller outside this crate could not perform.
    #[must_use]
    pub fn could_not_deliver(detail: impl Into<String>, retryable: bool) -> Self {
        Self::failed(None, detail, retryable)
    }

    fn failed(status: Option<u16>, detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            ok: false,
            status,
            excerpt: Some(excerpt(detail.into())),
            retryable,
        }
    }
}

/// Truncates and redacts what is kept from a response.
fn excerpt(value: String) -> String {
    let redacted = rd_core::redact_text(value.trim());
    redacted.chars().take(MAX_EXCERPT).collect()
}

/// Delivers one message; `secret` is the resolved vault value, if the target has one.
///
/// `vendor_directory` is the tool folder configured under Settings → Tools. Apprise is looked
/// up there first, like every other helper binary (RD-120-62); `None` leaves the built-in
/// vendor folders and `PATH`.
pub async fn send(
    http: &reqwest::Client,
    target: &NotificationTarget,
    config: &TargetConfig,
    message: &Message,
    secret: Option<&secrecy::SecretString>,
    vendor_directory: Option<&str>,
) -> Attempt {
    let result = match target.kind {
        TargetKind::Webhook => send_webhook(http, target, message, secret).await,
        TargetKind::Smtp => send_smtp(target, config, message, secret).await,
        TargetKind::Apprise => send_apprise(config, message, secret, vendor_directory).await,
        // Never reached in practice: the service dispatches plugin targets before it gets
        // here. Answering rather than panicking means a target whose kind was changed while
        // a delivery was in flight fails once instead of taking the worker down.
        TargetKind::Plugin => Ok(Attempt::failed(
            None,
            "a plugin destination is delivered by the service, not by this transport",
            false,
        )),
    };
    match result {
        Ok(attempt) => attempt,
        // A transport error (DNS, TLS, connection refused) is worth another attempt.
        Err(error) => Attempt::failed(None, error.to_string(), true),
    }
}

async fn send_webhook(
    http: &reqwest::Client,
    target: &NotificationTarget,
    message: &Message,
    secret: Option<&secrecy::SecretString>,
) -> Result<Attempt> {
    let body = serde_json::to_vec(&message.payload)?;
    let mut request = http
        .post(&target.endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(IDEMPOTENCY_HEADER, &message.idempotency_key);
    if let Some(secret) = secret {
        request = request.header(SIGNATURE_HEADER, sign(secret.expose_secret(), &body));
    }
    let response = request.body(body).send().await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if status.is_success() {
        return Ok(Attempt::ok(Some(status.as_u16())));
    }
    // 4xx other than 408/429 means the request itself is wrong; repeating it will not help.
    let retryable = status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
    Ok(Attempt::failed(Some(status.as_u16()), text, retryable))
}

/// `sha256=<hex>` over the exact body that is sent, the shape most receivers expect.
fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        <Hmac<Sha256>>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

async fn send_smtp(
    target: &NotificationTarget,
    config: &TargetConfig,
    message: &Message,
    secret: Option<&secrecy::SecretString>,
) -> Result<Attempt> {
    use lettre::{
        AsyncSmtpTransport, AsyncTransport, Message as Mail, Tokio1Executor,
        transport::smtp::authentication::Credentials,
    };

    let from = config.from.as_deref().context("SMTP sender is not set")?;
    anyhow::ensure!(!config.to.is_empty(), "SMTP has no recipients");
    let mut mail = Mail::builder()
        .from(from.parse().context("SMTP sender address")?)
        .subject(message.title.clone());
    for recipient in &config.to {
        mail = mail.to(recipient.parse().context("SMTP recipient address")?);
    }
    let mail = mail.body(message.body.clone())?;

    let host = target.endpoint.trim();
    let tls = config.tls.as_deref().unwrap_or("starttls");
    let mut builder = match tls {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(host)?,
        "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host),
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)?,
    };
    if let Some(port) = config.port {
        builder = builder.port(port);
    }
    if let (Some(username), Some(secret)) = (config.username.as_deref(), secret) {
        builder = builder.credentials(Credentials::new(
            username.to_owned(),
            secret.expose_secret().to_owned(),
        ));
    }
    match builder.build().send(mail).await {
        Ok(_) => Ok(Attempt::ok(None)),
        Err(error) => {
            // A permanent SMTP reply (5xx) will not become acceptable on a retry.
            let retryable = !error.is_permanent();
            Ok(Attempt::failed(None, error.to_string(), retryable))
        }
    }
}

async fn send_apprise(
    config: &TargetConfig,
    message: &Message,
    secret: Option<&secrecy::SecretString>,
    vendor_directory: Option<&str>,
) -> Result<Attempt> {
    let secret = secret.context("apprise target URL is not stored")?;
    let tool = rd_core::locate_tool(config.executable.as_deref(), vendor_directory, "apprise")
        .context("apprise is not installed or not configured")?;
    let mut command = tokio::process::Command::new(&tool.path);
    // The target URL carries the service token, so it must not be an argument: argv stands in
    // the process list for every other user on the machine to read, the environment only for
    // the same user. `APPRISE_URLS` is the CLI's own source for URLs when argv names none and
    // no `--config` is given; stdin is no way in, because apprise reads it as the body.
    command
        .arg("--input-format")
        .arg("text")
        .arg("--title")
        .arg(&message.title)
        .arg("--body")
        .arg(&message.body)
        .env(APPRISE_URLS_VARIABLE, secret.expose_secret())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = command.spawn().context("start apprise")?;
    let output = child.wait_with_output().await.context("run apprise")?;
    if output.status.success() {
        return Ok(Attempt::ok(None));
    }
    // apprise names a URL it cannot parse in full ("Unparseable URL tgram://…"), token and all.
    let text = String::from_utf8_lossy(&output.stderr)
        .replace(secret.expose_secret(), rd_core::REDACTION_PLACEHOLDER);
    // A wrong token looks the same as a service outage from here, so apprise failures are
    // always retried and give up through the attempt limit instead.
    Ok(Attempt::failed(None, text, true))
}

#[cfg(test)]
mod tests {
    use super::{excerpt, sign};

    /// Apprise is found in the folder configured under Settings -> Tools (RD-120-62), not only
    /// beside the program, in the data folder and on `PATH`.
    #[cfg(unix)]
    #[tokio::test]
    async fn apprise_is_found_in_the_configured_vendor_folder() {
        use std::os::unix::fs::PermissionsExt;

        use super::{Message, TargetConfig, send};
        use crate::model::{NotificationEvent, NotificationTarget, TargetKind};

        let vendor = tempfile::tempdir().expect("tempdir");
        // The stand-in records its arguments, the `APPRISE_URLS` it was handed and its stdin:
        // the real CLI takes the target URL from that variable and reads stdin as the body.
        let received = vendor.path().join("received");
        let arguments = vendor.path().join("arguments");
        let input = vendor.path().join("input");
        let script = vendor.path().join("apprise");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat > '{}'\nprintf '%s' \"$APPRISE_URLS\" > '{}'\n",
                arguments.display(),
                input.display(),
                received.display(),
            ),
        )
        .expect("write stand-in");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make it executable");
        let target = NotificationTarget {
            id: rd_core::NotificationTargetId::new(),
            name: "apprise".to_owned(),
            kind: TargetKind::Apprise,
            enabled: true,
            endpoint: "tgram".to_owned(),
            config: serde_json::json!({}),
            secret_ref: Some("vault:apprise".to_owned()),
            has_secret: true,
        };
        let message = Message {
            title: "Package finished".to_owned(),
            body: "example.iso".to_owned(),
            event: NotificationEvent::PackageCompleted,
            idempotency_key: "test:1".to_owned(),
            payload: serde_json::json!({}),
        };
        let secret = secrecy::SecretString::from("tgram://token/chat");
        let http = reqwest::Client::new();
        let folder = vendor.path().to_string_lossy().into_owned();

        // Without the setting the stand-in is out of reach, whatever else the machine has.
        send(
            &http,
            &target,
            &TargetConfig::default(),
            &message,
            Some(&secret),
            None,
        )
        .await;
        assert!(!received.exists(), "the stand-in ran without the setting");

        let attempt = send(
            &http,
            &target,
            &TargetConfig::default(),
            &message,
            Some(&secret),
            Some(&folder),
        )
        .await;
        assert!(attempt.ok, "{attempt:?}");
        assert_eq!(
            std::fs::read_to_string(&received).expect("the stand-in ran"),
            "tgram://token/chat"
        );
        // The URL is nowhere in argv, where the process list would show it, and stdin — which
        // apprise would send as the body if `--body` were missing — stays empty.
        let argv = std::fs::read_to_string(&arguments).expect("arguments recorded");
        assert!(!argv.contains("token"), "{argv}");
        assert_eq!(
            argv.lines().collect::<Vec<_>>(),
            [
                "--input-format",
                "text",
                "--title",
                "Package finished",
                "--body",
                "example.iso"
            ]
        );
        assert_eq!(std::fs::read_to_string(&input).expect("stdin recorded"), "");
    }

    /// What apprise prints about a URL it cannot parse contains the URL; the history must not.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_apprise_failure_keeps_the_target_url_out_of_the_history() {
        use std::os::unix::fs::PermissionsExt;

        use super::{Message, TargetConfig, send};
        use crate::model::{NotificationEvent, NotificationTarget, TargetKind};

        let vendor = tempfile::tempdir().expect("tempdir");
        let script = vendor.path().join("apprise");
        std::fs::write(
            &script,
            "#!/bin/sh\necho \"ERROR - Unparseable URL $APPRISE_URLS\" >&2\nexit 1\n",
        )
        .expect("write stand-in");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make it executable");
        let target = NotificationTarget {
            id: rd_core::NotificationTargetId::new(),
            name: "apprise".to_owned(),
            kind: TargetKind::Apprise,
            enabled: true,
            endpoint: "tgram".to_owned(),
            config: serde_json::json!({}),
            secret_ref: Some("vault:apprise".to_owned()),
            has_secret: true,
        };
        let message = Message {
            title: "Package finished".to_owned(),
            body: "example.iso".to_owned(),
            event: NotificationEvent::PackageCompleted,
            idempotency_key: "test:2".to_owned(),
            payload: serde_json::json!({}),
        };
        let secret = secrecy::SecretString::from("tgram://123456:sekrit/999");
        let folder = vendor.path().to_string_lossy().into_owned();

        let attempt = send(
            &reqwest::Client::new(),
            &target,
            &TargetConfig::default(),
            &message,
            Some(&secret),
            Some(&folder),
        )
        .await;
        assert!(!attempt.ok);
        let excerpt = attempt.excerpt.expect("stderr kept");
        assert!(excerpt.contains("Unparseable URL"), "{excerpt}");
        assert!(!excerpt.contains("sekrit"), "{excerpt}");
    }

    #[test]
    fn the_signature_is_a_stable_hmac_over_the_exact_body() {
        let first = sign("topsecret", b"{\"a\":1}");
        assert_eq!(first, sign("topsecret", b"{\"a\":1}"));
        assert!(first.starts_with("sha256="));
        assert_ne!(first, sign("other", b"{\"a\":1}"));
        assert_ne!(first, sign("topsecret", b"{\"a\":2}"));
        // The secret itself is nowhere in the header value.
        assert!(!first.contains("topsecret"));
    }

    #[test]
    fn a_response_excerpt_is_truncated_and_redacted() {
        let long = "x".repeat(5_000);
        assert_eq!(excerpt(long).chars().count(), 500);
        let signed = excerpt("failed for https://host/file?X-Amz-Signature=abcdef".to_owned());
        assert!(!signed.contains("abcdef"), "{signed}");
    }
}
