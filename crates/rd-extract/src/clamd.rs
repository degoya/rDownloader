//! A small `clamd` client (RD-190-14): `zPING`, `zVERSION` and `zINSTREAM`, over TCP or a
//! Unix socket.
//!
//! Written here rather than taken from a crate: the protocol is three commands and a chunk
//! framing, and the client that runs it is shorter than the dependency review of one that does.
//! Nothing but the bytes of the file being scanned goes to the address somebody configured; no
//! name, no path, no hash, and nowhere else.
//!
//! The `z` prefix asks clamd for NUL-terminated replies, so a reply is read up to its NUL rather
//! than up to a newline a signature name could in principle contain.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// clamd's port when an address names none.
pub const DEFAULT_PORT: u16 = 3310;
/// The size of one `INSTREAM` chunk. clamd accepts any size up to its `StreamMaxLength`.
const CHUNK: usize = 64 * 1024;
/// A reply longer than this is not a clamd reply.
const MAX_REPLY: usize = 4 * 1024;

/// Where `clamd` listens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClamdAddress {
    /// `host:port`; an IPv6 host is written in brackets, `[::1]:3310`.
    Tcp(String),
    /// A local socket, `unix:/run/clamav/clamd.ctl` or the absolute path alone.
    Unix(PathBuf),
}

impl ClamdAddress {
    /// Reads an address as the settings spell it.
    ///
    /// `tcp://` is accepted and dropped, a host without a port gets clamd's default, and a Unix
    /// socket is refused on a platform that has none rather than failing at the first scan.
    pub fn parse(text: &str) -> Result<Self, ClamdError> {
        let text = text.trim();
        let invalid = || ClamdError::Address(text.to_owned());
        if text.is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
            return Err(invalid());
        }
        let socket = text
            .strip_prefix("unix:")
            .or_else(|| (text.starts_with('/') || Path::new(text).is_absolute()).then_some(text));
        if let Some(path) = socket {
            let path = path.strip_prefix("//").unwrap_or(path);
            if !Path::new(path).is_absolute() {
                return Err(invalid());
            }
            if !cfg!(unix) {
                return Err(ClamdError::Address(format!(
                    "{text}: Unix sockets are not available on this platform"
                )));
            }
            return Ok(Self::Unix(PathBuf::from(path)));
        }
        let host_port = text.strip_prefix("tcp://").unwrap_or(text);
        let host_port = host_port.strip_suffix('/').unwrap_or(host_port);
        if host_port.contains('/') || host_port.contains('@') || host_port.contains(' ') {
            return Err(invalid());
        }
        let bracketed = host_port.starts_with('[');
        let (host, port) = match host_port.rsplit_once(':') {
            Some((host, port)) if !bracketed || host.ends_with(']') => (host, Some(port)),
            _ => (host_port, None),
        };
        // A bare IPv6 address has colons of its own and no brackets; it is ambiguous.
        if host.is_empty() || (!bracketed && host.contains(':')) {
            return Err(invalid());
        }
        if bracketed && (!host.ends_with(']') || host.len() <= 2) {
            return Err(invalid());
        }
        let port = match port {
            Some(port) => port
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(invalid)?,
            None => DEFAULT_PORT,
        };
        Ok(Self::Tcp(format!("{host}:{port}")))
    }
}

impl std::fmt::Display for ClamdAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp(address) => f.write_str(address),
            Self::Unix(path) => write!(f, "unix:{}", path.display()),
        }
    }
}

/// What clamd said about one stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Verdict {
    Clean,
    /// The signature clamd named, e.g. `Eicar-Test-Signature`.
    Found(String),
}

/// Why a scan has no verdict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClamdError {
    /// The configured address cannot be read.
    Address(String),
    /// Nothing answered, or the connection broke: the scanner is not there.
    Unavailable(String),
    /// clamd did not answer within the configured time.
    Timeout,
    /// The stream was longer than clamd's `StreamMaxLength`.
    SizeLimit,
    /// clamd answered something that is not a verdict, its own `ERROR` lines included.
    Protocol(String),
    /// The file to scan could not be read; clamd was never asked.
    Read(String),
}

impl std::fmt::Display for ClamdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Address(address) => write!(f, "not a clamd address: {address}"),
            Self::Unavailable(reason) => write!(f, "clamd is not reachable: {reason}"),
            Self::Timeout => f.write_str("clamd did not answer in time"),
            Self::SizeLimit => f.write_str("the file is larger than clamd's StreamMaxLength"),
            Self::Protocol(reply) => write!(f, "clamd answered: {reply}"),
            Self::Read(reason) => write!(f, "the file cannot be read: {reason}"),
        }
    }
}

impl std::error::Error for ClamdError {}

trait Connection: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Connection for T {}

/// One configured scanner. Every call opens its own connection, as clamd expects without
/// `IDSESSION`.
#[derive(Clone, Debug)]
pub struct Clamd {
    address: ClamdAddress,
    timeout: Duration,
}

impl Clamd {
    #[must_use]
    pub const fn new(address: ClamdAddress, timeout: Duration) -> Self {
        Self { address, timeout }
    }

    #[must_use]
    pub const fn address(&self) -> &ClamdAddress {
        &self.address
    }

    /// `PING`; `Ok` when clamd answered `PONG`.
    ///
    /// Whatever else answered is not repeated: the address may be anybody's port, and this is
    /// not a way to read another service's greeting.
    pub async fn ping(&self) -> Result<(), ClamdError> {
        let reply = self.command(b"zPING\0").await?;
        if reply == "PONG" {
            Ok(())
        } else {
            Err(ClamdError::Protocol("not a clamd reply".to_owned()))
        }
    }

    /// `VERSION`: the engine and the signature database, e.g.
    /// `ClamAV 1.5.4/27780/Wed Oct  1 08:25:02 2026`.
    pub async fn version(&self) -> Result<String, ClamdError> {
        let reply = self.command(b"zVERSION\0").await?;
        if reply.starts_with("ClamAV ") {
            Ok(reply)
        } else {
            Err(ClamdError::Protocol("not a clamd reply".to_owned()))
        }
    }

    /// Streams one file to clamd.
    ///
    /// At most `limit` bytes are sent; the caller decides beforehand what to do with a file
    /// larger than that, so the limit here only guards against one that grew meanwhile.
    pub async fn scan_file(&self, path: &Path, limit: u64) -> Result<Verdict, ClamdError> {
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|error| ClamdError::Read(error.to_string()))?;
        self.scan_reader(file.take(limit)).await
    }

    /// Streams any reader to clamd with `INSTREAM` and returns its verdict.
    pub async fn scan_reader<R: AsyncRead + Unpin>(
        &self,
        mut reader: R,
    ) -> Result<Verdict, ClamdError> {
        let mut connection = self.connect().await?;
        self.write(&mut connection, b"zINSTREAM\0").await?;
        let mut buffer = vec![0_u8; CHUNK];
        loop {
            let read = reader
                .read(&mut buffer)
                .await
                .map_err(|error| ClamdError::Read(error.to_string()))?;
            if read == 0 {
                break;
            }
            let length = u32::try_from(read).unwrap_or(u32::MAX).to_be_bytes();
            // clamd closes the connection once a stream passes its StreamMaxLength and says so
            // first; a write that fails is therefore answered by reading what it said.
            let sent = match self.write(&mut connection, &length).await {
                Ok(()) => self.write(&mut connection, &buffer[..read]).await,
                Err(error) => Err(error),
            };
            if let Err(error) = sent {
                return match self.reply(&mut connection).await {
                    Ok(reply) => parse_scan_reply(&reply),
                    Err(_) => Err(error),
                };
            }
        }
        self.write(&mut connection, &[0, 0, 0, 0]).await?;
        let reply = self.reply(&mut connection).await?;
        parse_scan_reply(&reply)
    }

    async fn command(&self, command: &[u8]) -> Result<String, ClamdError> {
        let mut connection = self.connect().await?;
        self.write(&mut connection, command).await?;
        self.reply(&mut connection).await
    }

    async fn connect(&self) -> Result<Box<dyn Connection>, ClamdError> {
        let connecting = async {
            match &self.address {
                ClamdAddress::Tcp(address) => tokio::net::TcpStream::connect(address)
                    .await
                    .map(|stream| Box::new(stream) as Box<dyn Connection>),
                #[cfg(unix)]
                ClamdAddress::Unix(path) => tokio::net::UnixStream::connect(path)
                    .await
                    .map(|stream| Box::new(stream) as Box<dyn Connection>),
                #[cfg(not(unix))]
                ClamdAddress::Unix(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "Unix sockets are not available on this platform",
                )),
            }
        };
        tokio::time::timeout(self.timeout, connecting)
            .await
            .map_err(|_| ClamdError::Timeout)?
            .map_err(|error| ClamdError::Unavailable(error.to_string()))
    }

    async fn write(&self, connection: &mut dyn Connection, bytes: &[u8]) -> Result<(), ClamdError> {
        tokio::time::timeout(self.timeout, connection.write_all(bytes))
            .await
            .map_err(|_| ClamdError::Timeout)?
            .map_err(|error| ClamdError::Unavailable(error.to_string()))
    }

    /// Reads one NUL-terminated reply (or up to the end of the connection).
    async fn reply(&self, connection: &mut dyn Connection) -> Result<String, ClamdError> {
        let reading = async {
            let mut reply = Vec::new();
            let mut chunk = [0_u8; 256];
            loop {
                let read = connection.read(&mut chunk).await?;
                if read == 0 {
                    break;
                }
                if let Some(end) = chunk[..read].iter().position(|value| *value == 0) {
                    reply.extend_from_slice(&chunk[..end]);
                    break;
                }
                reply.extend_from_slice(&chunk[..read]);
                if reply.len() > MAX_REPLY {
                    break;
                }
            }
            Ok::<_, std::io::Error>(reply)
        };
        let reply = tokio::time::timeout(self.timeout, reading)
            .await
            .map_err(|_| ClamdError::Timeout)?
            .map_err(|error| ClamdError::Unavailable(error.to_string()))?;
        if reply.is_empty() {
            return Err(ClamdError::Unavailable(
                "the connection closed without a reply".to_owned(),
            ));
        }
        Ok(
            String::from_utf8_lossy(&reply[..reply.len().min(MAX_REPLY)])
                .trim()
                .to_owned(),
        )
    }
}

/// Reads an `INSTREAM` reply: `stream: OK`, `stream: <signature> FOUND`, or an error line.
fn parse_scan_reply(reply: &str) -> Result<Verdict, ClamdError> {
    let reply = reply.trim();
    if let Some(found) = reply.strip_suffix(" FOUND") {
        let signature = found
            .split_once(": ")
            .map_or(found, |(_, signature)| signature)
            .trim();
        return Ok(Verdict::Found(if signature.is_empty() {
            "unknown".to_owned()
        } else {
            signature.to_owned()
        }));
    }
    if reply == "OK" || reply.ends_with(": OK") {
        return Ok(Verdict::Clean);
    }
    if reply.contains("size limit exceeded") {
        return Err(ClamdError::SizeLimit);
    }
    Err(ClamdError::Protocol(reply.chars().take(200).collect()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{ClamdAddress, ClamdError, Verdict, parse_scan_reply};

    #[test]
    fn addresses_read_the_way_the_settings_spell_them() {
        assert_eq!(
            ClamdAddress::parse("clamav:3310"),
            Ok(ClamdAddress::Tcp("clamav:3310".to_owned()))
        );
        assert_eq!(
            ClamdAddress::parse(" tcp://127.0.0.1:3311/ "),
            Ok(ClamdAddress::Tcp("127.0.0.1:3311".to_owned()))
        );
        // clamd's own port when none is named.
        assert_eq!(
            ClamdAddress::parse("scanner.lan"),
            Ok(ClamdAddress::Tcp("scanner.lan:3310".to_owned()))
        );
        assert_eq!(
            ClamdAddress::parse("[::1]:3310"),
            Ok(ClamdAddress::Tcp("[::1]:3310".to_owned()))
        );
        assert_eq!(
            ClamdAddress::parse("[::1]"),
            Ok(ClamdAddress::Tcp("[::1]:3310".to_owned()))
        );
        for bad in [
            "",
            "::1",
            "host:0",
            "host:99999",
            "host:port",
            "http://host:3310/x",
            "user@host:3310",
            "unix:relative/socket",
            "a\nb:3310",
        ] {
            assert!(ClamdAddress::parse(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_local_socket_is_named_with_or_without_its_prefix() {
        assert_eq!(
            ClamdAddress::parse("unix:/run/clamav/clamd.ctl"),
            Ok(ClamdAddress::Unix("/run/clamav/clamd.ctl".into()))
        );
        assert_eq!(
            ClamdAddress::parse("/run/clamav/clamd.ctl"),
            Ok(ClamdAddress::Unix("/run/clamav/clamd.ctl".into()))
        );
    }

    #[test]
    fn replies_become_verdicts() {
        assert_eq!(parse_scan_reply("stream: OK"), Ok(Verdict::Clean));
        assert_eq!(
            parse_scan_reply("stream: Eicar-Test-Signature FOUND"),
            Ok(Verdict::Found("Eicar-Test-Signature".to_owned()))
        );
        assert_eq!(
            parse_scan_reply("stream: Win.Test.EICAR_HDB-1 FOUND"),
            Ok(Verdict::Found("Win.Test.EICAR_HDB-1".to_owned()))
        );
        assert_eq!(
            parse_scan_reply("INSTREAM size limit exceeded. ERROR"),
            Err(ClamdError::SizeLimit)
        );
        assert!(matches!(
            parse_scan_reply("UNKNOWN COMMAND"),
            Err(ClamdError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn nothing_listening_is_unavailable_and_not_a_verdict() {
        // A port nobody listens on: bind one, learn its number, close it again.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        drop(listener);
        let clamd = super::Clamd::new(
            ClamdAddress::Tcp(address.to_string()),
            Duration::from_secs(2),
        );
        let result = clamd.scan_reader(&b"payload"[..]).await;
        assert!(
            matches!(
                result,
                Err(ClamdError::Unavailable(_) | ClamdError::Timeout)
            ),
            "{result:?}"
        );
    }
}
