//! Sending one notification to one target.
//!
//! Every path here is written so a secret cannot leak: the webhook secret only ever becomes
//! an HMAC, the SMTP password only reaches `lettre`, and the apprise target URL — which
//! carries the service token — is handed over in the child's environment so it never appears
//! in the process list. What comes back is truncated and redacted before it is stored.

use anyhow::{Context, Result};
use rd_files::NoConsoleWindow as _;
use secrecy::ExposeSecret;

use crate::model::{NotificationTarget, TargetKind};

pub(crate) mod web_push;
mod webhook;

/// Header carrying the HMAC of the body, so a receiver can verify the call came from here.
pub const SIGNATURE_HEADER: &str = "X-RDownloader-Signature";
/// Header carrying the moment of the attempt, in seconds since the epoch, so a receiver can refuse
/// a call that was recorded and sent again (RD-1480-11).
pub const TIMESTAMP_HEADER: &str = "X-RDownloader-Timestamp";
/// Header carrying `t=<timestamp>,v1=<hex>`: the HMAC of `<timestamp>.<body>`, which vouches for
/// the moment as [`SIGNATURE_HEADER`] does for the body alone (RD-1480-11).
pub const TIMED_SIGNATURE_HEADER: &str = "X-RDownloader-Timed-Signature";
/// Header carrying the delivery's idempotency key, so a receiver can drop a repeat.
pub const IDEMPOTENCY_HEADER: &str = "X-RDownloader-Idempotency-Key";

/// The environment variable the apprise CLI takes its target URLs from.
const APPRISE_URLS_VARIABLE: &str = "APPRISE_URLS";

/// How much of a response is kept for the history.
const MAX_EXCERPT: usize = 500;

/// How long one apprise run may take before it is killed and the attempt counted as failed.
///
/// apprise sends to one service and exits; a minute is far beyond any working call. Without a
/// limit a hung run kept its target `in_flight` for good and the test action never answered
/// (audit 1.9.1, INTAKE-05).
const APPRISE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

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
    /// Apprise: executable path, run once an administrator sealed it; empty = vendor and `PATH`.
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
/// `reach` is the address rule a webhook keeps to (audit 2026-10-05, S2): the service's rule for
/// an address the person entered, `rd_plugin_host::entered_address_policy`.
///
/// `vendor_directory` is the tool folder configured under Settings → Tools. Apprise is looked
/// up there first, like every other helper binary (RD-120-62); `None` leaves the built-in
/// vendor folders and `PATH`.
pub async fn send(
    reach: &rd_http::AddressPolicy,
    target: &NotificationTarget,
    config: &TargetConfig,
    message: &Message,
    secret: Option<&secrecy::SecretString>,
    vendor_directory: Option<&str>,
) -> Attempt {
    let result = match target.kind {
        TargetKind::Webhook => webhook::send(reach, target, message, secret).await,
        TargetKind::Smtp => send_smtp(target, config, message, secret).await,
        TargetKind::Apprise => {
            send_apprise(target, config, message, secret, vendor_directory).await
        }
        // Never reached in practice: the service dispatches plugin and browser push targets
        // before it gets here. Answering rather than panicking means a target whose kind was
        // changed while a delivery was in flight fails once instead of taking the worker down.
        TargetKind::Plugin | TargetKind::WebPush => Ok(Attempt::failed(
            None,
            "this destination is delivered by the service, not by this transport",
            false,
        )),
    };
    match result {
        Ok(attempt) => attempt,
        // A transport error (DNS, TLS, connection refused) is worth another attempt.
        Err(error) => Attempt::failed(None, error.to_string(), true),
    }
}

async fn send_smtp(
    target: &NotificationTarget,
    config: &TargetConfig,
    message: &Message,
    secret: Option<&secrecy::SecretString>,
) -> Result<Attempt> {
    use lettre::{
        AsyncSmtpTransport, AsyncTransport, Message as Mail, Tokio1Executor,
        message::header::ContentType, transport::smtp::authentication::Credentials,
    };

    let from = config.from.as_deref().context("SMTP sender is not set")?;
    anyhow::ensure!(!config.to.is_empty(), "SMTP has no recipients");
    let mut mail = Mail::builder()
        // UTF-8 named, or a translated text reads as US-ASCII with broken letters.
        .header(ContentType::TEXT_PLAIN)
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
    target: &NotificationTarget,
    config: &TargetConfig,
    message: &Message,
    secret: Option<&secrecy::SecretString>,
    vendor_directory: Option<&str>,
) -> Result<Attempt> {
    let secret = secret.context("apprise target URL is not stored")?;
    let explicit = crate::executable::runnable_executable(target, config)?;
    let tool = rd_core::locate_tool(explicit, vendor_directory, "apprise")
        .context("apprise is not installed or not configured")?;
    let mut command = tokio::process::Command::new(&tool.path);
    // Like the downloaders, apprise gets the allowlist and its network settings, not the
    // service's environment; the one variable it is meant to read is set below.
    rd_files::restrict_environment(&mut command, rd_files::TOOL_VARIABLES);
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
        .stderr(std::process::Stdio::piped())
        // A run that is given up on — the limit below, or a service stopping and dropping
        // the attempt — must not leave apprise behind.
        .kill_on_drop(true)
        .no_console_window();
    run_apprise(command, secret, APPRISE_TIMEOUT).await
}

/// Runs a prepared apprise command within `limit`.
async fn run_apprise(
    mut command: tokio::process::Command,
    secret: &secrecy::SecretString,
    limit: std::time::Duration,
) -> Result<Attempt> {
    let child = command.spawn().context("start apprise")?;
    let Ok(output) = tokio::time::timeout(limit, child.wait_with_output()).await else {
        // Dropping the future dropped the child, which `kill_on_drop` ends. Retried like any
        // other apprise failure: the service on the other end may only be slow today.
        return Ok(Attempt::failed(
            None,
            format!("apprise did not finish within {} seconds", limit.as_secs()),
            true,
        ));
    };
    let output = output.context("run apprise")?;
    if output.status.success() {
        return Ok(Attempt::ok(None));
    }
    // apprise prints most of its errors on stdout, so both streams are kept, stderr first;
    // `Attempt::failed` bounds the excerpt. It names a URL it cannot parse in full
    // ("Unparseable URL tgram://…"), token and all.
    let text = [&output.stderr, &output.stdout]
        .map(|stream| String::from_utf8_lossy(stream).trim().to_owned())
        .into_iter()
        .filter(|stream| !stream.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .replace(secret.expose_secret(), rd_core::REDACTION_PLACEHOLDER);
    // A wrong token looks the same as a service outage from here, so apprise failures are
    // always retried and give up through the attempt limit instead.
    Ok(Attempt::failed(None, text, true))
}

#[cfg(test)]
#[path = "delivery/delivery_tests.rs"]
mod tests;
