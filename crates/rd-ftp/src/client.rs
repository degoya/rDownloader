//! Connecting and logging in, across the three FTP flavours.
//!
//! Plain FTP and FTPS are distinct types in `suppaftp` (`ImplAsyncFtpStream` is generic over
//! its stream), so they are unified behind one enum here instead of leaking the difference
//! into the probe and the runner.

use std::time::Duration;

use rd_core::{RemoteAuthMode, RemoteCredential, RemoteProtocol};
use secrecy::{ExposeSecret, SecretString};
use suppaftp::tokio::{AsyncFtpStream, AsyncRustlsFtpStream};
use suppaftp::types::FileType;
use suppaftp::{FtpResult, Mode, types::Features};

/// Anonymous FTP convention: the user name is fixed and the password is a contact address.
/// Sending a real-looking mail address is the norm, but it would be someone's data, so a
/// literal placeholder goes out instead.
const ANONYMOUS_USER: &str = "anonymous";
const ANONYMOUS_PASSWORD: &str = "anonymous@example.invalid";

/// An established, logged-in control connection.
pub enum Connection {
    Plain(Box<AsyncFtpStream>),
    Secure(Box<AsyncRustlsFtpStream>),
}

/// Dispatches one method over both stream types.
///
/// The two arms are identical apart from the receiver's type, which no amount of generics
/// removes without a trait `suppaftp` does not provide.
macro_rules! dispatch {
    ($self:expr, $stream:ident => $call:expr) => {
        match $self {
            Self::Plain($stream) => $call,
            Self::Secure($stream) => $call,
        }
    };
}

impl Connection {
    /// Connects, negotiates TLS where the protocol asks for it, and logs in.
    ///
    /// The whole handshake is bounded by `timeout`: an FTP server that accepts the socket
    /// and then says nothing would otherwise hold a queue slot open indefinitely.
    pub async fn open(
        credential: &RemoteCredential,
        password: Option<&SecretString>,
        custom_ca_pem: &[Vec<u8>],
        timeout: Duration,
    ) -> anyhow::Result<FtpResult<Self>> {
        match tokio::time::timeout(
            timeout,
            Self::handshake(credential, password, custom_ca_pem),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Ok(Err(suppaftp::FtpError::ConnectionError(
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the FTP server did not complete the login in time",
                ),
            ))),
        }
    }

    async fn handshake(
        credential: &RemoteCredential,
        password: Option<&SecretString>,
        custom_ca_pem: &[Vec<u8>],
    ) -> anyhow::Result<FtpResult<Self>> {
        let address = format!("{}:{}", credential.host, credential.port);
        let mut connection = match credential.protocol {
            RemoteProtocol::Ftp => match AsyncFtpStream::connect(&address).await {
                Ok(stream) => Self::Plain(Box::new(stream)),
                Err(error) => return Ok(Err(error)),
            },
            RemoteProtocol::Ftps => {
                let connector = crate::tls::connector(custom_ca_pem)?;
                // Explicit FTPS starts as plaintext but has to be typed as the TLS stream
                // from the start: `into_secure` upgrades in place and cannot change the
                // stream type of the connection it is called on.
                let plain = match AsyncRustlsFtpStream::connect(&address).await {
                    Ok(stream) => stream,
                    Err(error) => return Ok(Err(error)),
                };
                match plain.into_secure(connector, &credential.host).await {
                    Ok(stream) => Self::Secure(Box::new(stream)),
                    Err(error) => return Ok(Err(error)),
                }
            }
            RemoteProtocol::FtpsImplicit => {
                let connector = crate::tls::connector(custom_ca_pem)?;
                match AsyncRustlsFtpStream::connect_secure_implicit(
                    &address,
                    connector,
                    &credential.host,
                )
                .await
                {
                    Ok(stream) => Self::Secure(Box::new(stream)),
                    Err(error) => return Ok(Err(error)),
                }
            }
            other => anyhow::bail!("{} is not an FTP protocol", other.as_str()),
        };
        connection.set_mode(if credential.passive {
            Mode::Passive
        } else {
            Mode::Active
        });
        let (user, secret) = match credential.auth_mode {
            RemoteAuthMode::Anonymous => (ANONYMOUS_USER.to_owned(), ANONYMOUS_PASSWORD.to_owned()),
            _ => (
                credential
                    .username
                    .clone()
                    .unwrap_or_else(|| ANONYMOUS_USER.to_owned()),
                password.map_or_else(String::new, |value| value.expose_secret().to_owned()),
            ),
        };
        if let Err(error) = connection.login(&user, &secret).await {
            return Ok(Err(error));
        }
        // Everything this crate transfers is a file, and ASCII mode would rewrite line
        // endings inside it. The default is server-dependent, so it is set explicitly.
        if let Err(error) = connection.transfer_type(FileType::Binary).await {
            return Ok(Err(error));
        }
        Ok(Ok(connection))
    }

    fn set_mode(&mut self, mode: Mode) {
        dispatch!(self, stream => stream.set_mode(mode));
    }

    async fn login(&mut self, user: &str, password: &str) -> FtpResult<()> {
        dispatch!(self, stream => stream.login(user, password).await)
    }

    async fn transfer_type(&mut self, file_type: FileType) -> FtpResult<()> {
        dispatch!(self, stream => stream.transfer_type(file_type).await)
    }

    pub async fn features(&mut self) -> FtpResult<Features> {
        dispatch!(self, stream => stream.feat().await)
    }

    pub async fn size(&mut self, path: &str) -> FtpResult<usize> {
        dispatch!(self, stream => stream.size(path).await)
    }

    pub async fn modified_at(&mut self, path: &str) -> FtpResult<chrono::NaiveDateTime> {
        dispatch!(self, stream => stream.mdtm(path).await)
    }

    pub async fn cwd(&mut self, path: &str) -> FtpResult<()> {
        dispatch!(self, stream => stream.cwd(path).await)
    }

    pub async fn pwd(&mut self) -> FtpResult<String> {
        dispatch!(self, stream => stream.pwd().await)
    }

    pub async fn mlsd(&mut self, path: Option<&str>) -> FtpResult<Vec<String>> {
        dispatch!(self, stream => stream.mlsd(path).await)
    }

    pub async fn list(&mut self, path: Option<&str>) -> FtpResult<Vec<String>> {
        dispatch!(self, stream => stream.list(path).await)
    }

    /// Sends `REST <offset>`, which the next `RETR` continues from.
    pub async fn resume_from(&mut self, offset: usize) -> FtpResult<()> {
        dispatch!(self, stream => stream.resume_transfer(offset).await)
    }

    pub async fn quit(&mut self) -> FtpResult<()> {
        dispatch!(self, stream => stream.quit().await)
    }

    /// Streams `path` from the offset a preceding [`Self::resume_from`] established.
    #[allow(clippy::too_many_arguments)]
    pub async fn retrieve(
        &mut self,
        path: &str,
        staging: &rd_transfer_file::Staging<'_>,
        sink: &mut tokio::fs::File,
        limiter: &rd_limits::ScopedLimiter,
        cancellation: &tokio_util::sync::CancellationToken,
        read_timeout: Duration,
    ) -> anyhow::Result<FtpResult<rd_transfer_file::TransferEnd>> {
        dispatch!(self, stream => crate::transfer::retrieve(
            stream,
            path,
            staging,
            sink,
            limiter,
            cancellation,
            read_timeout,
        )
        .await)
    }
}

/// Whether the server advertised `REST STREAM`, i.e. whether an interrupted transfer can
/// be continued rather than restarted.
#[must_use]
pub fn advertises_rest(features: &Features) -> bool {
    features.iter().any(|(name, argument)| {
        name.eq_ignore_ascii_case("REST")
            && argument
                .as_deref()
                .is_none_or(|value| value.to_ascii_uppercase().contains("STREAM"))
    })
}

#[cfg(test)]
mod tests {
    use super::{ANONYMOUS_PASSWORD, ANONYMOUS_USER};

    #[test]
    fn the_anonymous_password_is_not_a_real_address() {
        // Sending a plausible mail address would publish someone's data to every server
        // an anonymous link points at.
        assert_eq!(ANONYMOUS_USER, "anonymous");
        assert!(ANONYMOUS_PASSWORD.ends_with(".invalid"));
    }
}
