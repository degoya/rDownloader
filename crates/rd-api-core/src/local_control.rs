//! The local control token: how a launcher, the updater and `rdownloader stop` reach the running
//! service on this machine without an administrator's credential (RD-180-02, RD-180-03).
//!
//! Every start of `serve` draws 32 random bytes and writes them, with the address it listens on
//! and its process id, to `<data directory>/local-control.json` — on Unix readable by its owner
//! only (`0600`), on Windows protected by the data directory's access list, which the start sets
//! before this file is written (`rd_files::protect_private_dir`). The service keeps
//! only the SHA-256 of the token. The token opens exactly the routes in [`ROUTES`] and nothing
//! else, and only for a request from this machine (`client::from_this_machine`: a loopback peer,
//! or the listen address itself, and no forwarding header), so a copy that leaves the machine
//! opens nothing. Whoever can read
//! the data directory can already read the database beside it; the token adds stopping the
//! service, writing a backup before an update, switching the password sign-in back on and setting
//! a new administrator password — the last two of which writing the database directly could do
//! as well — to that, no more.
//!
//! The file is removed when the process ends normally — as the very last step, after the queue
//! was checkpointed — so its absence is what `rdownloader stop --wait` waits for. A file left by
//! a process that was killed is replaced by the next start.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};

use axum::http::Method;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The file below the data directory.
pub const FILE: &str = "local-control.json";

/// The routes the token opens: stopping the service, the backup before an update, switching
/// the password sign-in back on (`rdownloader auth password-login on`, RD-190-15) and a new
/// administrator password without the current one (`rdownloader auth reset-password`,
/// RD-190-24) — the ways back in that only this machine has.
pub const ROUTES: &[(&str, Method)] = &[
    ("/api/v1/auth/password-login/on", Method::POST),
    ("/api/v1/auth/password/reset", Method::POST),
    ("/api/v1/system/shutdown", Method::POST),
    ("/api/v1/system/update/prepare", Method::POST),
];

/// Whether the token opens this matched route.
#[must_use]
pub fn covers(path: &str, method: &Method) -> bool {
    ROUTES
        .iter()
        .any(|(route, allowed)| *route == path && allowed == method)
}

/// What the file holds. No `Debug`: the token must not reach a log by way of a `{:?}`.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ControlFile {
    /// Where to reach the service from this machine: the listen address, with an unspecified
    /// one (`0.0.0.0`, `::`) replaced by loopback.
    pub address: String,
    pub token: String,
    pub pid: u32,
}

/// The digest of the token this process issued; empty until the binary issues one, so a test
/// router and a service without the file accept no token at all.
#[derive(Clone, Debug, Default)]
pub struct LocalControl {
    digest: Option<String>,
}

impl LocalControl {
    /// Whether `bearer` is this process's token.
    #[must_use]
    pub fn accepts(&self, bearer: &str) -> bool {
        self.digest
            .as_deref()
            .is_some_and(|digest| digest == digest_of(bearer))
    }

    /// A control that accepts `token`, without writing any file. For tests.
    #[doc(hidden)]
    #[must_use]
    pub fn for_token(token: &str) -> Self {
        Self {
            digest: Some(digest_of(token)),
        }
    }

    /// Draws a token and writes the file for a service listening on `listen`.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn issue(
        data_directory: &Path,
        listen: SocketAddr,
    ) -> anyhow::Result<(Self, ControlFileGuard)> {
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token = hex::encode(bytes);
        let file = ControlFile {
            address: reachable(listen).to_string(),
            token: token.clone(),
            pid: std::process::id(),
        };
        let path = data_directory.join(FILE);
        write_private(&path, &serde_json::to_vec_pretty(&file)?)?;
        Ok((
            Self {
                digest: Some(digest_of(&token)),
            },
            ControlFileGuard { path, token },
        ))
    }
}

/// Removes the file when dropped, unless another start has replaced it since.
pub struct ControlFileGuard {
    path: PathBuf,
    token: String,
}

impl Drop for ControlFileGuard {
    fn drop(&mut self) {
        if read_at(&self.path).is_ok_and(|file| file.is_some_and(|file| file.token == self.token))
            && let Err(error) = std::fs::remove_file(&self.path)
        {
            tracing::warn!(%error, path = %self.path.display(), "the local control file could not be removed");
        }
    }
}

/// The file of the service whose data directory this is; `None` when no service wrote one.
///
/// # Errors
///
/// When the file exists but cannot be read or parsed.
pub fn read(data_directory: &Path) -> anyhow::Result<Option<ControlFile>> {
    read_at(&data_directory.join(FILE))
}

fn read_at(path: &Path) -> anyhow::Result<Option<ControlFile>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn digest_of(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// The address a client on this machine connects to.
#[must_use]
pub fn reachable(listen: SocketAddr) -> SocketAddr {
    match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), listen.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), listen.port())
        }
        _ => listen,
    }
}

/// Written beside its place and renamed, so a reader never sees half a file; on Unix created
/// `0600` before a byte is in it.
///
/// The staging file is always a new one (security review 2026-09-30, finding 7): an old `.tmp`
/// is removed first and `create_new` refuses whatever appears in its place meanwhile, so a
/// left-over file keeps no wider mode and a planted symbolic link redirects no token.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let mut staged = path.as_os_str().to_owned();
    staged.push(".tmp");
    let staged = PathBuf::from(staged);
    if let Err(error) = std::fs::remove_file(&staged)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&staged)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&staged, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_carries_a_token_only_its_own_control_accepts_and_goes_with_the_guard() {
        let directory = tempfile::tempdir().expect("tempdir");
        let listen = SocketAddr::from(([0, 0, 0, 0], 8710));
        let (control, guard) = LocalControl::issue(directory.path(), listen).expect("issue");
        let file = read(directory.path()).expect("read").expect("written");
        assert_eq!(file.address, "127.0.0.1:8710");
        assert_eq!(file.pid, std::process::id());
        assert_eq!(file.token.len(), 64);
        assert!(control.accepts(&file.token));
        assert!(!control.accepts("not the token"));
        assert!(!LocalControl::default().accepts(&file.token));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(directory.path().join(FILE))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        drop(guard);
        assert!(read(directory.path()).expect("read").is_none());
    }

    /// Finding 7 of the 2026-09-30 review: an existing `.tmp` kept its mode, a symbolic link
    /// there took the token wherever it pointed.
    #[cfg(unix)]
    #[test]
    fn a_left_over_staging_file_neither_widens_the_mode_nor_redirects_the_token() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().expect("tempdir");
        let staged = directory.path().join(format!("{FILE}.tmp"));
        let listen = SocketAddr::from(([127, 0, 0, 1], 8710));

        std::fs::write(&staged, b"left over").expect("write");
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let (_, _guard) = LocalControl::issue(directory.path(), listen).expect("issue");
        let mode = std::fs::metadata(directory.path().join(FILE))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        let elsewhere = directory.path().join("elsewhere.txt");
        std::fs::write(&elsewhere, b"untouched").expect("write");
        std::os::unix::fs::symlink(&elsewhere, &staged).expect("symlink");
        let (_, _second) = LocalControl::issue(directory.path(), listen).expect("issue");
        assert_eq!(std::fs::read(&elsewhere).expect("read"), b"untouched");
        let written = std::fs::symlink_metadata(directory.path().join(FILE)).expect("metadata");
        assert!(written.file_type().is_file(), "{written:?}");
        assert_eq!(written.permissions().mode() & 0o777, 0o600);
        assert!(read(directory.path()).expect("read").is_some());
    }

    #[test]
    fn a_guard_leaves_the_file_of_a_later_start_alone() {
        let directory = tempfile::tempdir().expect("tempdir");
        let listen = SocketAddr::from(([127, 0, 0, 1], 8710));
        let (_, first) = LocalControl::issue(directory.path(), listen).expect("first");
        let (_, _second) = LocalControl::issue(directory.path(), listen).expect("second");
        drop(first);
        assert!(read(directory.path()).expect("read").is_some());
    }

    #[test]
    fn only_the_lifecycle_routes_and_the_way_back_in_are_covered() {
        assert!(covers("/api/v1/system/shutdown", &Method::POST));
        assert!(covers("/api/v1/system/update/prepare", &Method::POST));
        assert!(covers("/api/v1/auth/password-login/on", &Method::POST));
        assert!(covers("/api/v1/auth/password/reset", &Method::POST));
        assert!(!covers("/api/v1/auth/password", &Method::POST));
        assert!(!covers("/api/v1/auth/password-login/off", &Method::POST));
        assert!(!covers("/api/v1/auth/oidc", &Method::PUT));
        assert!(!covers("/api/v1/system/shutdown", &Method::GET));
        assert!(!covers("/api/v1/settings", &Method::PUT));
        assert!(!covers("/api/v1/system/data-reset", &Method::POST));
    }

    #[test]
    fn an_unspecified_listen_address_is_reached_over_loopback() {
        assert_eq!(
            reachable(SocketAddr::from(([0, 0, 0, 0], 1))),
            SocketAddr::from(([127, 0, 0, 1], 1))
        );
        let v6: SocketAddr = "[::]:2".parse().expect("v6");
        assert_eq!(reachable(v6), "[::1]:2".parse::<SocketAddr>().expect("v6"));
        let lan = SocketAddr::from(([192, 168, 1, 5], 3));
        assert_eq!(reachable(lan), lan);
    }
}
