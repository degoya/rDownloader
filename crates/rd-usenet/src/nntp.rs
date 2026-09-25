use std::{pin::Pin, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use rustls::pki_types::ServerName;
use secrecy::{ExposeSecret, SecretString};
use tokio::{
    io::{
        AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadHalf,
        WriteHalf,
    },
    net::TcpStream,
};
use tokio_rustls::TlsConnector;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_STATUS_LINE: usize = 16 * 1024;

trait AsyncTransport: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> AsyncTransport for T {}
type Transport = Pin<Box<dyn AsyncTransport + Send>>;

/// Optional SOCKS5 hop, with credentials retained in secret wrappers.
#[derive(Clone)]
pub struct Socks5Proxy {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<SecretString>,
}

/// One NNTP endpoint. Lower priority values should be attempted first.
#[derive(Clone)]
pub struct NntpServerConfig {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    /// The operator's custom CA bundles, in the `Vec<Vec<u8>>` shape `rd_http::NetworkDefaults`
    /// carries them.
    ///
    /// Carried per endpoint rather than read from somewhere global because that is how the
    /// setting reaches every other transport: a news server is trusted by the same rule as an
    /// HTTP host and an FTPS server, and the rule is `rd_http::tls_client_config`. Empty means
    /// the platform store alone, which is what an installation without a custom CA gets.
    pub custom_ca_pem: Vec<Vec<u8>>,
    pub username: Option<String>,
    pub password: Option<SecretString>,
    pub proxy: Option<Socks5Proxy>,
    pub max_article_bytes: usize,
    pub max_connections: u16,
}

/// Builds the TLS connector for a news server.
///
/// The trust decision itself lives in `rd_http::tls_client_config`, so NNTP, ordinary HTTP and
/// FTPS all augment the platform store with the operator's custom CA in exactly the same way.
/// Building a verifier here instead is what made "custom CA" mean something different depending
/// on which protocol happened to be carrying the bytes: it worked for HTTP and FTPS and silently
/// did not for the news server.
fn tls_connector(custom_ca_pem: &[Vec<u8>]) -> Result<TlsConnector> {
    let config = rd_http::tls_client_config(custom_ca_pem)?;
    Ok(TlsConnector::from(Arc::new(config)))
}

impl NntpServerConfig {
    #[must_use]
    pub fn tls(host: String) -> Self {
        Self {
            host,
            port: 563,
            tls: true,
            custom_ca_pem: Vec::new(),
            username: None,
            password: None,
            proxy: None,
            max_article_bytes: 128 * 1024 * 1024,
            max_connections: 8,
        }
    }
}

/// Authenticated asynchronous NNTP connection.
///
/// Two halves under one name: NNTP answers in the order the commands went out, so a
/// connection can carry a second `BODY` while the first body is still arriving - if the
/// sending and the reading side can be held by different tasks. [`Self::into_halves`] hands
/// them out; [`Self::body`] is the one-at-a-time form for callers that need no more.
pub struct NntpClient {
    writer: NntpWriter,
    reader: NntpReader,
}

/// The sending half of a connection.
pub(crate) struct NntpWriter {
    stream: WriteHalf<Transport>,
}

/// The receiving half: status lines and bodies, in the order the commands went out.
pub(crate) struct NntpReader {
    stream: BufReader<ReadHalf<Transport>>,
    max_article_bytes: usize,
}

/// A body, with the message-id the server put on its `222` line.
///
/// RFC 3977 section 6.2.3 answers `BODY` with `222 n message-id`, and that id is the one
/// handle a client has to tell whether the answer it is reading belongs to the command it
/// sent (RD-108-27). `None` when the server names no id, which the pool treats as a server
/// whose answers cannot be verified.
pub(crate) struct Body {
    pub message_id: Option<String>,
    pub data: Vec<u8>,
}

/// Statuses that mean the article is not on this server, and nothing more.
///
/// RFC 3977 §6.2.3: `430` for a message-id that is not here, `423` for an article number
/// that is not. Every other refusal is about the server, not about the article (RD-108-29).
const ARTICLE_ABSENT: [u16; 2] = [423, 430];

/// Why a body did not arrive, and what that means for the connection and for the article.
///
/// Two distinctions live here. The first is what pipelining hinges on: after a status line
/// the connection is in step - the line was the whole answer and the next answer follows it -
/// whereas after a break, a timeout, a closed stream, a body abandoned at the size limit,
/// unread bytes of one answer may still be on the line and whoever reads next would take them
/// for the answer to a different command. Such a connection must not be used again.
///
/// The second is what the answer says about the article. `430` says the server does not have
/// it; that is a fact worth acting on. `400 Archive server temporarily offline.` says the
/// server cannot serve right now and says nothing whatsoever about the article - RFC 3977
/// §3.2.1 even has the server close the connection after it. Treating the two alike is what
/// wrote 2313 zero-filled segments in one afternoon (RD-108-29).
#[derive(Debug)]
pub(crate) enum BodyError {
    /// `430`/`423`: this server does not have the article. Nothing follows the status line.
    Unavailable(String),
    /// Any other status: the server could not answer this request. The article's fate is
    /// unknown and the connection is given up, because a `400` closes it anyway.
    ServerFault(String),
    /// The connection is no longer in step and has to be dropped.
    Broken(anyhow::Error),
}

impl std::fmt::Display for BodyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(status) | Self::ServerFault(status) => {
                write!(formatter, "NNTP server returned {status:?}")
            }
            Self::Broken(error) => write!(formatter, "{error:#}"),
        }
    }
}

// Deliberately not `std::error::Error`: anyhow's blanket conversion would wrap the error,
// and a `Broken` carries an `anyhow::Error` whose context chain is worth keeping as it is.
impl From<BodyError> for anyhow::Error {
    fn from(error: BodyError) -> Self {
        match error {
            BodyError::Unavailable(status) | BodyError::ServerFault(status) => {
                anyhow::anyhow!("NNTP server returned {status:?}")
            }
            BodyError::Broken(error) => error,
        }
    }
}

impl NntpClient {
    /// Connects through direct TCP or SOCKS5, negotiates TLS and authenticates.
    pub async fn connect(config: &NntpServerConfig) -> Result<Self> {
        let tcp = connect_tcp(config).await?;
        let transport: Transport = if config.tls {
            let server_name = ServerName::try_from(config.host.clone())
                .context("invalid NNTP TLS server name")?;
            let stream = tls_connector(&config.custom_ca_pem)?
                .connect(server_name, tcp)
                .await
                .context("NNTP TLS handshake")?;
            Box::pin(stream)
        } else {
            Box::pin(tcp)
        };
        let (read, write) = tokio::io::split(transport);
        let mut client = Self {
            writer: NntpWriter { stream: write },
            reader: NntpReader {
                stream: BufReader::new(read),
                max_article_bytes: config.max_article_bytes,
            },
        };
        let greeting = client.reader.read_status().await?;
        expect_status(&greeting, &[200, 201])?;
        if let Some(username) = &config.username {
            let response = client
                .command(&format!("AUTHINFO USER {username}"), false)
                .await?;
            if status_code(&response)? == 381 {
                let password = config
                    .password
                    .as_ref()
                    .context("NNTP password is missing")?;
                let response = client
                    .command(&format!("AUTHINFO PASS {}", password.expose_secret()), true)
                    .await?;
                expect_status(&response, &[281])?;
            } else {
                expect_status(&response, &[281])?;
            }
        }
        Ok(client)
    }

    /// Retrieves a dot-stuffed article body by message ID, one command at a time.
    pub async fn body(&mut self, message_id: &str) -> Result<Vec<u8>> {
        self.writer.send_body(message_id).await?;
        Ok(self.reader.read_body().await?.data)
    }

    /// Splits the connection so one task can send while another reads.
    #[must_use]
    pub(crate) fn into_halves(self) -> (NntpWriter, NntpReader) {
        (self.writer, self.reader)
    }

    async fn command(&mut self, command: &str, sensitive: bool) -> Result<String> {
        self.writer.send(command, sensitive).await?;
        self.reader.read_status().await
    }
}

impl NntpWriter {
    /// Sends `BODY` for a message ID; the answer is read with [`NntpReader::read_body`].
    pub(crate) async fn send_body(&mut self, message_id: &str) -> Result<()> {
        if message_id.contains(['\r', '\n']) {
            bail!("invalid NNTP message id");
        }
        // RFC 3977 §6.2.3: the message-id argument must be enclosed in angle brackets;
        // NZB files usually carry it without them and many servers answer 430 otherwise.
        let message_id = message_id.trim_matches(['<', '>']);
        self.send(&format!("BODY <{message_id}>"), false).await
    }

    async fn send(&mut self, command: &str, sensitive: bool) -> Result<()> {
        if command.contains(['\r', '\n']) {
            bail!("invalid NNTP command");
        }
        if sensitive {
            tracing::trace!("sending redacted NNTP command");
        } else {
            tracing::trace!(command, "sending NNTP command");
        }
        // One write, so the command and its line ending leave in one TLS record and one
        // segment, the way SABnzbd sends it (RD-108-27). Two writes were two records; a
        // server that answered a second `BODY` out of step saw them that way.
        let line = format!("{command}\r\n");
        tokio::time::timeout(COMMAND_TIMEOUT, async {
            self.stream.write_all(line.as_bytes()).await?;
            self.stream.flush().await
        })
        .await
        .context("NNTP command timeout")??;
        Ok(())
    }
}

impl NntpReader {
    /// Reads the answer to the oldest unanswered `BODY`: a dot-unstuffed article body.
    pub(crate) async fn read_body(&mut self) -> Result<Body, BodyError> {
        let response = self.read_status().await.map_err(BodyError::Broken)?;
        match status_code(&response) {
            Ok(222) => {}
            Ok(code) if ARTICLE_ABSENT.contains(&code) => {
                return Err(BodyError::Unavailable(response));
            }
            Ok(_) => return Err(BodyError::ServerFault(response)),
            Err(error) => return Err(BodyError::Broken(error)),
        }
        let message_id = answered_message_id(&response);
        let data = self.read_lines().await.map_err(BodyError::Broken)?;
        Ok(Body { message_id, data })
    }

    async fn read_lines(&mut self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        loop {
            let mut line = Vec::new();
            let read =
                tokio::time::timeout(COMMAND_TIMEOUT, self.stream.read_until(b'\n', &mut line))
                    .await
                    .context("NNTP body timeout")??;
            if read == 0 {
                bail!("NNTP connection closed during article");
            }
            if line == b".\r\n" || line == b".\n" {
                break;
            }
            if line.starts_with(b"..") {
                line.remove(0);
            }
            if body.len().saturating_add(line.len()) > self.max_article_bytes {
                bail!("NNTP article exceeds configured size limit");
            }
            body.extend_from_slice(&line);
        }
        Ok(body)
    }

    async fn read_status(&mut self) -> Result<String> {
        let mut line = String::new();
        tokio::time::timeout(COMMAND_TIMEOUT, self.stream.read_line(&mut line))
            .await
            .context("NNTP status timeout")??;
        if line.len() > MAX_STATUS_LINE {
            bail!("NNTP status line exceeds limit");
        }
        if line.is_empty() {
            bail!("NNTP connection closed");
        }
        Ok(line)
    }
}

async fn connect_tcp(config: &NntpServerConfig) -> Result<TcpStream> {
    if let Some(proxy) = &config.proxy {
        let mut stream = TcpStream::connect((&*proxy.host, proxy.port)).await?;
        socks_handshake(&mut stream, proxy, &config.host, config.port).await?;
        Ok(stream)
    } else {
        TcpStream::connect((&*config.host, config.port))
            .await
            .with_context(|| format!("connect NNTP server {}:{}", config.host, config.port))
    }
}

async fn socks_handshake(
    stream: &mut TcpStream,
    proxy: &Socks5Proxy,
    target: &str,
    port: u16,
) -> Result<()> {
    let authenticated = proxy.username.is_some();
    stream
        .write_all(if authenticated {
            &[5, 1, 2]
        } else {
            &[5, 1, 0]
        })
        .await?;
    let mut selection = [0_u8; 2];
    stream.read_exact(&mut selection).await?;
    if selection != [5, if authenticated { 2 } else { 0 }] {
        bail!("SOCKS5 proxy rejected authentication method");
    }
    if authenticated {
        socks_authenticate(stream, proxy).await?;
    }
    let host = target.as_bytes();
    let host_length = u8::try_from(host.len()).context("SOCKS5 hostname is too long")?;
    let mut request = vec![5, 1, 0, 3, host_length];
    request.extend_from_slice(host);
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).await?;
    if header[0] != 5 || header[1] != 0 {
        bail!("SOCKS5 proxy connect failed with code {}", header[1]);
    }
    consume_socks_address(stream, header[3]).await?;
    Ok(())
}

async fn socks_authenticate(stream: &mut TcpStream, proxy: &Socks5Proxy) -> Result<()> {
    let username = proxy.username.as_deref().unwrap_or_default().as_bytes();
    let password = proxy
        .password
        .as_ref()
        .map(ExposeSecret::expose_secret)
        .map_or(b"".as_slice(), str::as_bytes);
    let username_length = u8::try_from(username.len()).context("SOCKS5 username is too long")?;
    let password_length = u8::try_from(password.len()).context("SOCKS5 password is too long")?;
    let mut request = vec![1, username_length];
    request.extend_from_slice(username);
    request.push(password_length);
    request.extend_from_slice(password);
    stream.write_all(&request).await?;
    let mut response = [0_u8; 2];
    stream.read_exact(&mut response).await?;
    if response != [1, 0] {
        bail!("SOCKS5 authentication failed");
    }
    Ok(())
}

async fn consume_socks_address(stream: &mut TcpStream, kind: u8) -> Result<()> {
    let address_bytes = match kind {
        1 => 4,
        3 => {
            let mut length = [0_u8; 1];
            stream.read_exact(&mut length).await?;
            usize::from(length[0])
        }
        4 => 16,
        _ => bail!("invalid SOCKS5 address kind"),
    };
    let mut remainder = vec![0_u8; address_bytes + 2];
    stream.read_exact(&mut remainder).await?;
    Ok(())
}

fn expect_status(line: &str, expected: &[u16]) -> Result<()> {
    let code = status_code(line)?;
    if !expected.contains(&code) {
        // Typed, not just worded: `rd-api` distinguishes a provider refusing this node (502)
        // from every other connection failure, and it used to do that by searching this
        // sentence for the characters "502".
        return Err(crate::error::NntpStatusError::new(code, line).into());
    }
    Ok(())
}

/// The message-id a `222 n message-id ...` line names, without its angle brackets.
fn answered_message_id(line: &str) -> Option<String> {
    let token = line.split_whitespace().nth(2)?;
    let inner = token.strip_prefix('<')?.strip_suffix('>')?;
    (!inner.is_empty()).then(|| inner.to_owned())
}

fn status_code(line: &str) -> Result<u16> {
    line.get(..3)
        .context("short NNTP status")?
        .parse()
        .context("invalid NNTP status")
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::{NntpClient, NntpServerConfig, answered_message_id, status_code, tls_connector};

    /// The news server is trusted by the same rule as everything else: a bundle that parses to
    /// nothing is refused here exactly as `rd_http::tls_client_config` refuses it, instead of
    /// quietly falling back to the platform roots the way a verifier built here once did.
    #[test]
    fn the_connector_comes_from_the_one_place_tls_trust_is_decided() {
        assert!(tls_connector(&[]).is_ok());
        assert!(tls_connector(&[b"not a certificate".to_vec()]).is_err());
    }

    #[test]
    fn parses_status_codes() {
        assert_eq!(status_code("222 0 <id> body follows\r\n").ok(), Some(222));
        assert!(status_code("no").is_err());
    }

    #[test]
    fn the_message_id_on_the_222_line_is_read_and_its_absence_is_told_apart() {
        assert_eq!(
            answered_message_id("222 0 <part-1@example.test> body follows\r\n").as_deref(),
            Some("part-1@example.test")
        );
        assert_eq!(
            answered_message_id("222 12345 <a@b>\r\n").as_deref(),
            Some("a@b")
        );
        assert_eq!(answered_message_id("222 body follows\r\n"), None);
        assert_eq!(answered_message_id("222 0 <> body follows\r\n"), None);
        assert_eq!(answered_message_id("222\r\n"), None);
    }

    #[tokio::test]
    async fn authenticates_and_reads_a_dot_stuffed_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fake NNTP listener");
        let address = listener.local_addr().expect("listener address");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("client");
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            write
                .write_all(b"200 fake server ready\r\n")
                .await
                .expect("greeting");
            let mut line = String::new();
            read.read_line(&mut line).await.expect("username");
            assert_eq!(line, "AUTHINFO USER reader\r\n");
            write
                .write_all(b"381 password required\r\n")
                .await
                .expect("user response");
            line.clear();
            read.read_line(&mut line).await.expect("password");
            assert_eq!(line, "AUTHINFO PASS secret\r\n");
            write
                .write_all(b"281 authentication accepted\r\n")
                .await
                .expect("auth response");
            line.clear();
            read.read_line(&mut line).await.expect("body command");
            assert_eq!(line, "BODY <message-id@example.test>\r\n");
            write
                .write_all(b"222 body follows\r\nfirst\r\n..second\r\n.\r\n")
                .await
                .expect("article");
        });
        let config = NntpServerConfig {
            host: "127.0.0.1".to_owned(),
            port: address.port(),
            tls: false,
            custom_ca_pem: Vec::new(),
            username: Some("reader".to_owned()),
            password: Some(SecretString::from("secret".to_owned())),
            proxy: None,
            max_article_bytes: 1024,
            max_connections: 1,
        };

        let mut client = NntpClient::connect(&config).await.expect("connect");
        let body = client.body("message-id@example.test").await.expect("body");
        assert_eq!(body, b"first\r\n.second\r\n");
    }
}
