//! Opening an authenticated SFTP session.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use rd_core::{Failure, RemoteAuthMode, RemoteCredential};
use rd_db::HostKeyVerdict;
use russh::client::{self, Handle};
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate, decode_secret_key};
use russh_sftp::client::SftpSession;
use secrecy::{ExposeSecret, SecretString};

use crate::error;
use crate::hostkey::{self, Observed, Rejection};

/// Default SSH user when a credential names none; matches what `ssh` itself would do only
/// loosely, so it is really a fallback for a misconfigured entry.
const DEFAULT_USER: &str = "root";

/// The `russh` callback side of host-key checking.
///
/// It only records what it saw; the decision text is produced by the caller, because a
/// `false` here becomes an opaque protocol error that cannot explain itself.
pub struct HostKeyHandler {
    verdict: Arc<dyn Fn(&hostkey::OfferedKey) -> HostKeyVerdict + Send + Sync>,
    auto_trust: bool,
    observed: Observed,
}

impl client::Handler for HostKeyHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let decision = match hostkey::describe(server_public_key) {
            Ok(offered) => {
                let verdict = (self.verdict)(&offered);
                hostkey::decide(&verdict, offered, self.auto_trust)
            }
            Err(rejection) => Err(rejection),
        };
        let accepted = decision.is_ok();
        *self
            .observed
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(decision);
        Ok(accepted)
    }
}

/// An authenticated SSH connection with its SFTP subsystem open.
pub struct Connection {
    /// Held so the SSH transport outlives the SFTP session running on it.
    _session: Handle<HostKeyHandler>,
    pub sftp: SftpSession,
}

/// Everything needed to reach one server.
pub struct ConnectSpec<'a> {
    pub credential: &'a RemoteCredential,
    pub password: Option<&'a SecretString>,
    pub private_key: Option<&'a SecretString>,
    pub passphrase: Option<&'a SecretString>,
    pub auto_trust: bool,
    pub timeout: Duration,
}

/// Connects, verifies the host key and authenticates.
///
/// Returns the observed host key alongside the connection so the caller can record a
/// first sighting that `auto_trust` allowed through.
pub async fn connect(
    spec: ConnectSpec<'_>,
    verdict: Arc<dyn Fn(&hostkey::OfferedKey) -> HostKeyVerdict + Send + Sync>,
) -> Result<Result<(Connection, hostkey::OfferedKey), Failure>> {
    let credential = spec.credential;
    let observed: Observed = Arc::new(Mutex::new(None));
    let handler = HostKeyHandler {
        verdict,
        auto_trust: spec.auto_trust,
        observed: Arc::clone(&observed),
    };
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(spec.timeout),
        ..client::Config::default()
    });
    let address = format!("{}:{}", credential.host, credential.port);

    let connected =
        tokio::time::timeout(spec.timeout, client::connect(config, address, handler)).await;
    let mut session = match connected {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            // A refused host key surfaces here as a generic protocol error; the handler's
            // note says what actually happened, so it wins over the transport message.
            if let Some(Err(rejection)) = take(&observed) {
                return Ok(Err(
                    rejection.into_failure(&credential.host, credential.port)
                ));
            }
            return Ok(Err(error::classify_transport(&error)));
        }
        Err(_) => {
            return Ok(Err(rd_core::Failure::coded(
                rd_core::FailureKind::Transient {
                    retry_after_seconds: None,
                },
                error::CONNECT_FAILED,
                "The SSH server did not answer in time",
            )));
        }
    };
    let Some(Ok(offered)) = take(&observed) else {
        // Defensive: a connection that came up without the callback recording an accepted
        // key would mean the trust check was bypassed.
        return Ok(Err(rd_core::Failure::coded(
            rd_core::FailureKind::Permanent,
            hostkey::HOST_KEY_UNKNOWN,
            "The SSH host key was not verified",
        )));
    };

    let user = credential.username.as_deref().unwrap_or(DEFAULT_USER);
    if let Err(failure) = authenticate(&mut session, user, &spec).await? {
        return Ok(Err(failure));
    }

    let channel = match session.channel_open_session().await {
        Ok(channel) => channel,
        Err(error) => return Ok(Err(error::classify_transport(&error))),
    };
    if let Err(error) = channel.request_subsystem(true, "sftp").await {
        return Ok(Err(error::classify_transport(&error)));
    }
    let sftp = match SftpSession::new(channel.into_stream()).await {
        Ok(sftp) => sftp,
        Err(error) => return Ok(Err(error::classify_sftp(&error))),
    };
    Ok(Ok((
        Connection {
            _session: session,
            sftp,
        },
        offered,
    )))
}

/// Runs the authentication method the credential asks for.
async fn authenticate(
    session: &mut Handle<HostKeyHandler>,
    user: &str,
    spec: &ConnectSpec<'_>,
) -> Result<Result<(), Failure>> {
    let authenticated = match spec.credential.auth_mode {
        RemoteAuthMode::Password | RemoteAuthMode::Anonymous => {
            let password = spec
                .password
                .map_or_else(String::new, |value| value.expose_secret().to_owned());
            match session.authenticate_password(user, password).await {
                Ok(result) => result.success(),
                Err(error) => return Ok(Err(error::classify_transport(&error))),
            }
        }
        RemoteAuthMode::PrivateKey => {
            let Some(pem) = spec.private_key else {
                return Ok(Err(error::key_invalid()));
            };
            let passphrase = spec.passphrase.map(|value| value.expose_secret());
            let Ok(key) = decode_secret_key(pem.expose_secret(), passphrase) else {
                return Ok(Err(error::key_invalid()));
            };
            // `None` lets russh negotiate the RSA signature hash; for ed25519 it is ignored.
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), None);
            match session.authenticate_publickey(user, key).await {
                Ok(result) => result.success(),
                Err(error) => return Ok(Err(error::classify_transport(&error))),
            }
        }
        RemoteAuthMode::Agent => return agent_authenticate(session, user).await,
    };
    if authenticated {
        Ok(Ok(()))
    } else {
        Ok(Err(rd_core::Failure::coded(
            rd_core::FailureKind::AuthRequired,
            error::AUTH_FAILED,
            "The SSH server rejected the login",
        )))
    }
}

/// An agent connection with its transport boxed away, so the Unix socket and the Windows named
/// pipe reach [`agent_authenticate`] as one type.
type DynAgentClient = russh::keys::agent::client::AgentClient<
    Box<dyn russh::keys::agent::client::AgentStream + Send + Unpin>,
>;

/// Connects to the SSH agent the way the platform exposes it: a Unix socket named by
/// `SSH_AUTH_SOCK`, or on Windows a named pipe — OpenSSH's, or Pageant for the PuTTY family.
#[cfg(unix)]
async fn connect_agent() -> Option<DynAgentClient> {
    russh::keys::agent::client::AgentClient::connect_env()
        .await
        .ok()
        .map(russh::keys::agent::client::AgentClient::dynamic)
}

#[cfg(windows)]
async fn connect_agent() -> Option<DynAgentClient> {
    /// Where Windows' own OpenSSH agent listens; unlike on Unix there is no environment
    /// variable for it, the path is fixed.
    const OPENSSH_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

    // A relay (Git Bash, a WSL forwarder) can name a different pipe, so honour SSH_AUTH_SOCK
    // first — but fall through rather than give up if it names something unopenable, otherwise a
    // stale variable would hide a perfectly good agent.
    let candidates = std::env::var_os("SSH_AUTH_SOCK")
        .into_iter()
        .chain(std::iter::once(std::ffi::OsString::from(
            OPENSSH_AGENT_PIPE,
        )));
    for pipe in candidates {
        if let Ok(agent) = russh::keys::agent::client::AgentClient::connect_named_pipe(pipe).await {
            return Some(agent.dynamic());
        }
    }
    russh::keys::agent::client::AgentClient::connect_pageant()
        .await
        .ok()
        .map(russh::keys::agent::client::AgentClient::dynamic)
}

/// Tries every identity the running SSH agent offers, in the order it lists them.
async fn agent_authenticate(
    session: &mut Handle<HostKeyHandler>,
    user: &str,
) -> Result<Result<(), Failure>> {
    let Some(mut agent) = connect_agent().await else {
        return Ok(Err(error::agent_unavailable()));
    };
    let Ok(identities) = agent.request_identities().await else {
        return Ok(Err(error::agent_unavailable()));
    };
    if identities.is_empty() {
        return Ok(Err(error::agent_unavailable()));
    }
    for identity in identities {
        let russh::keys::agent::AgentIdentity::PublicKey { key, .. } = identity else {
            // Certificate identities need a different auth flow; skip rather than fail, so
            // one unusable entry does not shadow a usable one further down the list.
            continue;
        };
        match session
            .authenticate_publickey_with(user, key, None, &mut agent)
            .await
        {
            Ok(result) if result.success() => return Ok(Ok(())),
            // A single identity being refused is normal; the agent commonly holds several.
            Ok(_) | Err(_) => {}
        }
    }
    Ok(Err(rd_core::Failure::coded(
        rd_core::FailureKind::AuthRequired,
        error::AUTH_FAILED,
        "No identity offered by the SSH agent was accepted",
    )))
}

fn take(observed: &Observed) -> Option<Result<hostkey::OfferedKey, Rejection>> {
    observed
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
}
