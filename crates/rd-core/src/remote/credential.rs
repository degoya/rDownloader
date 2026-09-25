//! Reusable credentials for the non-HTTP transfer protocols (FTP, FTPS, SFTP) and the
//! SSH host-key trust store.
//!
//! These deliberately do **not** reuse [`crate::AuthProfile`]: that type is scoped to
//! http/https by construction (`AuthScope::parse` rejects every other scheme) and the
//! HTTP client pool keys on it. FTP and SSH authenticate against one `host:port` with one
//! user, which is the same shape as an NNTP server, so this follows that model instead.
//!
//! WebDAV is the exception and keeps using [`crate::AuthProfile`], because it *is* HTTP.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::RemoteCredentialId;

/// Longest password accepted for a remote credential.
pub const MAX_REMOTE_SECRET: usize = 4 * 1024;
/// Longest private-key PEM accepted for an SFTP credential.
pub const MAX_REMOTE_KEY: usize = 64 * 1024;
/// Longest host name accepted, matching the DNS limit.
pub const MAX_REMOTE_HOST: usize = 253;

/// Transfer protocol a credential and a link belong to.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteProtocol {
    /// Plain FTP, no TLS at any point.
    Ftp,
    /// FTP that upgrades the control channel with `AUTH TLS` before logging in.
    Ftps,
    /// FTP that is TLS from the first byte (historically port 990).
    FtpsImplicit,
    Sftp,
    Webdav,
}

impl RemoteProtocol {
    /// Port used when neither the URL nor the credential names one.
    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            Self::Ftp | Self::Ftps => 21,
            Self::FtpsImplicit => 990,
            Self::Sftp => 22,
            Self::Webdav => 443,
        }
    }

    /// Stable snake_case name used in storage and on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ftp => "ftp",
            Self::Ftps => "ftps",
            Self::FtpsImplicit => "ftps_implicit",
            Self::Sftp => "sftp",
            Self::Webdav => "webdav",
        }
    }

    /// Whether the control connection is encrypted (explicitly or implicitly).
    #[must_use]
    pub const fn is_encrypted(self) -> bool {
        !matches!(self, Self::Ftp)
    }

    /// Which transport family handles a link, so a `ftp://` URL can still be served by a
    /// credential that was configured to require `AUTH TLS` on the same port.
    #[must_use]
    pub const fn family(self) -> RemoteFamily {
        match self {
            Self::Ftp | Self::Ftps | Self::FtpsImplicit => RemoteFamily::Ftp,
            Self::Sftp => RemoteFamily::Sftp,
            Self::Webdav => RemoteFamily::Webdav,
        }
    }

    /// Maps a URL scheme to the protocol it names and whether that scheme was the secure
    /// variant. `ftp://` is deliberately the plain protocol: whether TLS is required is a
    /// property of the configured credential, not of the address someone pasted.
    ///
    /// `webdav://`/`dav://` mean plain HTTP and `webdavs://`/`davs://` mean HTTPS. Guessing
    /// the other way round would silently downgrade or upgrade a share the user named
    /// explicitly, so the scheme decides and nothing is inferred.
    #[must_use]
    pub fn from_url_scheme(scheme: &str) -> Option<(Self, bool)> {
        match scheme {
            "ftp" => Some((Self::Ftp, false)),
            "ftps" => Some((Self::Ftps, true)),
            "sftp" | "ssh" => Some((Self::Sftp, true)),
            "webdav" | "dav" => Some((Self::Webdav, false)),
            "webdavs" | "davs" => Some((Self::Webdav, true)),
            _ => None,
        }
    }
}

/// The transport that executes a link, independent of the exact TLS variant.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RemoteFamily {
    Ftp,
    Sftp,
    Webdav,
}

/// How a remote credential authenticates.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAuthMode {
    /// FTP `anonymous` login; carries no secret at all.
    Anonymous,
    Password,
    /// SFTP public-key authentication with a stored private key.
    PrivateKey,
    /// SFTP authentication delegated to a running SSH agent; nothing is stored.
    Agent,
}

impl RemoteAuthMode {
    /// Stable snake_case name used in storage.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::Password => "password",
            Self::PrivateKey => "private_key",
            Self::Agent => "agent",
        }
    }

    /// Whether this mode is meaningful for `protocol`.
    #[must_use]
    pub const fn is_valid_for(self, protocol: RemoteProtocol) -> bool {
        match self {
            Self::Anonymous | Self::Password => true,
            // Keys and agents are SSH concepts; FTP and WebDAV have no equivalent.
            Self::PrivateKey | Self::Agent => matches!(protocol, RemoteProtocol::Sftp),
        }
    }
}

/// A stored login for one `protocol://host:port` and user.
///
/// Secret values live in the secret store; this struct only ever carries opaque
/// `vault://` references, and those are never serialized.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct RemoteCredential {
    pub id: RemoteCredentialId,
    pub name: String,
    pub protocol: RemoteProtocol,
    /// Lowercased, IDNA-encoded host without a trailing dot.
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub auth_mode: RemoteAuthMode,
    /// FTP data connections use PASV/EPSV; turning this off asks for an active PORT/EPRT
    /// connection instead. Ignored by SFTP and WebDAV.
    #[serde(default = "default_true")]
    pub passive: bool,
    #[serde(skip_serializing)]
    #[schema(ignore)]
    pub secret_ref: Option<String>,
    #[serde(skip_serializing)]
    #[schema(ignore)]
    pub key_ref: Option<String>,
    #[serde(skip_serializing)]
    #[schema(ignore)]
    pub passphrase_ref: Option<String>,
    pub has_secret: bool,
    pub has_key: bool,
    pub has_passphrase: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const fn default_true() -> bool {
    true
}

impl RemoteCredential {
    /// Whether this credential can serve `target`.
    ///
    /// Host and port must agree exactly and the family must match; the exact TLS variant
    /// is the credential's decision, not the link's. A credential with a user name only
    /// matches a target that names no user or the same one.
    #[must_use]
    pub fn matches(&self, target: &RemoteTarget) -> bool {
        if !self.enabled
            || self.protocol.family() != target.protocol.family()
            || !self.host.eq_ignore_ascii_case(&target.host)
            || self.port != target.port
        {
            return false;
        }
        match (&target.username, &self.username) {
            (Some(wanted), Some(stored)) => wanted == stored,
            (Some(_), None) => false,
            (None, _) => true,
        }
    }

    /// Ranking key for picking between several matching credentials: one that names the
    /// same user is more specific than a catch-all for the host.
    #[must_use]
    pub const fn specificity(&self) -> u8 {
        if self.username.is_some() { 1 } else { 0 }
    }
}

/// What a link addresses, after its credentials have been split off the URL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteTarget {
    pub protocol: RemoteProtocol,
    pub host: String,
    pub port: u16,
    /// Percent-decoded, always starting with `/`.
    pub path: String,
    /// User name taken from the URL's userinfo, if it carried one.
    pub username: Option<String>,
    /// Whether the scheme asked for TLS. Only meaningful for WebDAV, where it decides
    /// between `http` and `https`; the FTP variants carry that in `protocol`.
    pub secure: bool,
}

impl RemoteTarget {
    /// Parses a remote URL. A password in the userinfo is deliberately **not** returned:
    /// it must never travel further than the parser, because everything downstream (queue
    /// rows, events, logs) would then carry it. Credentials come from the store.
    #[must_use]
    pub fn parse(url: &Url) -> Option<Self> {
        let (protocol, secure) = RemoteProtocol::from_url_scheme(url.scheme())?;
        let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() || host.len() > MAX_REMOTE_HOST {
            return None;
        }
        let username = percent_decode(url.username()).filter(|value| !value.is_empty());
        let path = percent_decode(url.path()).unwrap_or_else(|| "/".to_owned());
        let path = if path.starts_with('/') {
            path
        } else {
            format!("/{path}")
        };
        let default_port = if protocol == RemoteProtocol::Webdav && !secure {
            80
        } else {
            protocol.default_port()
        };
        Some(Self {
            port: url.port().unwrap_or(default_port),
            protocol,
            host,
            path,
            username,
            secure,
        })
    }

    /// The address without userinfo, which is what is persisted on the queue row and shown
    /// in the UI. A WebDAV target becomes the `http(s)` URL its files are fetched from, so
    /// the queue row is an ordinary HTTP download.
    #[must_use]
    pub fn sanitized_url(&self) -> Option<Url> {
        let scheme = match (self.protocol, self.secure) {
            (RemoteProtocol::Webdav, true) => "https",
            (RemoteProtocol::Webdav, false) => "http",
            (RemoteProtocol::Sftp, _) => "sftp",
            (RemoteProtocol::Ftp, _) => "ftp",
            (RemoteProtocol::Ftps | RemoteProtocol::FtpsImplicit, _) => "ftps",
        };
        let mut url = Url::parse(&format!("{scheme}://{}:{}", self.host, self.port)).ok()?;
        url.set_path(&self.path);
        Some(url)
    }

    /// Last path segment, used as the default file name.
    #[must_use]
    pub fn file_name(&self) -> Option<String> {
        self.path
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .map(str::to_owned)
    }
}

fn percent_decode(value: &str) -> Option<String> {
    percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .ok()
        .map(std::borrow::Cow::into_owned)
}

/// A server host key the user confirmed once, so a later change is visible as a change.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SshHostKey {
    pub host: String,
    pub port: u16,
    /// Key algorithm as SSH names it (`ssh-ed25519`, `rsa-sha2-512`, …).
    pub algorithm: String,
    /// `SHA256:<base64>` exactly as OpenSSH prints it, so it can be compared by eye.
    pub fingerprint: String,
    pub first_seen: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{RemoteAuthMode, RemoteFamily, RemoteProtocol, RemoteTarget};

    fn target(input: &str) -> RemoteTarget {
        RemoteTarget::parse(&Url::parse(input).expect("url")).expect("target")
    }

    #[test]
    fn default_ports_follow_the_scheme() {
        assert_eq!(target("ftp://example.com/f").port, 21);
        assert_eq!(target("ftps://example.com/f").port, 21);
        assert_eq!(target("sftp://example.com/f").port, 22);
        assert_eq!(target("ftp://example.com:2121/f").port, 2121);
    }

    #[test]
    fn a_password_in_the_url_is_never_carried_along() {
        // The bug this locks in: keeping the userinfo would put the password into the
        // queue row, the SSE event and every log line that mentions the source URL.
        let parsed = target("ftp://bob:hunter2@example.com/dir/file.bin");
        assert_eq!(parsed.username.as_deref(), Some("bob"));
        let sanitized = parsed.sanitized_url().expect("url");
        assert!(!sanitized.as_str().contains("hunter2"));
        assert!(!sanitized.as_str().contains("bob"));
        // `url` drops the default port of a special scheme during normalisation.
        assert_eq!(sanitized.as_str(), "ftp://example.com/dir/file.bin");
    }

    #[test]
    fn a_webdav_link_becomes_the_http_url_its_files_are_fetched_from() {
        assert_eq!(
            target("davs://cloud.example.com/remote.php/dav/files/a.bin")
                .sanitized_url()
                .expect("url")
                .as_str(),
            "https://cloud.example.com/remote.php/dav/files/a.bin"
        );
        // The insecure alias must not be silently upgraded, or a user who typed `dav://`
        // would believe they were on TLS.
        let plain = target("dav://intranet.example/share/a.bin");
        assert_eq!(plain.port, 80);
        assert_eq!(
            plain.sanitized_url().expect("url").as_str(),
            "http://intranet.example/share/a.bin"
        );
    }

    #[test]
    fn percent_escapes_are_decoded_in_path_and_user() {
        let parsed = target("sftp://a%40b.com@example.com/a%20b/c.bin");
        assert_eq!(parsed.username.as_deref(), Some("a@b.com"));
        assert_eq!(parsed.path, "/a b/c.bin");
        assert_eq!(parsed.file_name().as_deref(), Some("c.bin"));
    }

    #[test]
    fn hosts_normalize_like_auth_scopes() {
        assert_eq!(target("ftp://Example.COM./f").host, "example.com");
    }

    #[test]
    fn tls_variants_share_one_transport_family() {
        assert_eq!(RemoteProtocol::Ftps.family(), RemoteFamily::Ftp);
        assert_eq!(RemoteProtocol::FtpsImplicit.family(), RemoteFamily::Ftp);
        assert_eq!(RemoteProtocol::Sftp.family(), RemoteFamily::Sftp);
        assert!(!RemoteProtocol::Ftp.is_encrypted());
        assert!(RemoteProtocol::Ftps.is_encrypted());
    }

    #[test]
    fn keys_and_agents_are_ssh_only() {
        assert!(RemoteAuthMode::PrivateKey.is_valid_for(RemoteProtocol::Sftp));
        assert!(!RemoteAuthMode::PrivateKey.is_valid_for(RemoteProtocol::Ftp));
        assert!(!RemoteAuthMode::Agent.is_valid_for(RemoteProtocol::Webdav));
        assert!(RemoteAuthMode::Password.is_valid_for(RemoteProtocol::Ftp));
    }

    #[test]
    fn unknown_schemes_are_not_remote_links() {
        assert!(RemoteTarget::parse(&Url::parse("https://example.com/f").expect("url")).is_none());
        assert!(RemoteProtocol::from_url_scheme("magnet").is_none());
        assert!(RemoteProtocol::from_url_scheme("https").is_none());
    }
}
