//! One `serve` per data directory (audit 1.9.1, INTAKE-03).
//!
//! Only the launchers kept a second service from starting; `rdownloader serve` run by hand, by
//! a service manager or by a script did not, and two services on one database recover each
//! other's running downloads as interrupted and race for the same files. The lock is the
//! operating system's own (`File::try_lock`), so a process that ends — however it ends —
//! releases it, and no stale file ever has to be cleaned up.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// The lock file in the data directory, beside the database and the local control file.
pub(crate) const FILE: &str = "serve.lock";

/// Held for as long as the service runs; dropping it releases the lock.
#[derive(Debug)]
pub(crate) struct InstanceLock {
    _file: std::fs::File,
}

/// Takes the data directory's lock, or refuses when another service holds it.
///
/// # Errors
///
/// When another process holds the lock, or the lock file cannot be opened.
pub(crate) fn acquire(data_directory: &Path) -> Result<InstanceLock> {
    match try_acquire(data_directory)? {
        Some(lock) => Ok(lock),
        None => bail!(
            "another rDownloader service already runs on {}; stop it first (`rdownloader stop`)",
            directory_of(data_directory).display()
        ),
    }
}

fn directory_of(data_directory: &Path) -> &Path {
    if data_directory.as_os_str().is_empty() {
        Path::new(".")
    } else {
        data_directory
    }
}

/// The lock, or `None` when another process holds it.
///
/// A file system that cannot lock at all — some CIFS and NFS mounts answer `ENOLCK` or
/// `EOPNOTSUPP` — gets a warning and an unlocked handle rather than a refusal: a data
/// directory on a network share started before the lock existed, and refusing it would take
/// the service away for a guard it cannot have there (audit 1.9.1, RA-IN-04). Every other
/// error still refuses.
fn try_acquire(data_directory: &Path) -> Result<Option<InstanceLock>> {
    let path = directory_of(data_directory).join(FILE);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(InstanceLock { _file: file })),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) if locking_unsupported(&error) => {
            tracing::warn!(
                %error,
                path = %path.display(),
                "the file system cannot lock; a second service on this data directory is not refused"
            );
            Ok(Some(InstanceLock { _file: file }))
        }
        Err(std::fs::TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("lock {}", path.display()))
        }
    }
}

/// Whether a lock error says the file system does not lock, rather than that something failed.
fn locking_unsupported(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::Unsupported {
        return true;
    }
    #[cfg(unix)]
    {
        use rustix::io::Errno;
        Errno::from_io_error(error)
            .is_some_and(|errno| [Errno::NOLCK, Errno::OPNOTSUPP, Errno::NOTSUP].contains(&errno))
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// What `doctor` found on a data directory. No `Debug`: the control file holds the token.
pub(crate) enum Probe {
    /// No service holds the lock. `doctor` holds it now, so none starts while it works; `None`
    /// when the lock file could not be opened, and only the control file was asked.
    Free(
        // Never read: holding it is the point, until `doctor` drops the probe.
        #[allow(dead_code)] Option<InstanceLock>,
    ),
    /// A service holds the lock — starting, running or stopping — with its control file when
    /// it answers there. One still starting has written none yet.
    Held(Option<rd_api::local_control::ControlFile>),
}

/// Whether a service runs on `data_directory`, decided by its lock first (audit 1.9.1,
/// RA-IN-03).
///
/// The control file alone missed a service that was starting: the lock was taken, the file not
/// written yet, and `doctor` checkpointed and migrated beside it. The address in the file is
/// asked as well, for the process and address to name, and as the only answer where the lock
/// file cannot be opened.
///
/// For `doctor`, which must not touch the queue of a live service and says which one it found.
pub(crate) async fn probe(data_directory: &Path) -> Probe {
    match try_acquire(data_directory) {
        Ok(Some(lock)) => Probe::Free(Some(lock)),
        Ok(None) => Probe::Held(running(data_directory).await),
        Err(error) => {
            tracing::warn!(%error, "doctor could not open the instance lock; asking the control file");
            match running(data_directory).await {
                Some(control) => Probe::Held(Some(control)),
                None => Probe::Free(None),
            }
        }
    }
}

/// The service running on `data_directory`, if one answers at the address its local control
/// file names. A file nobody listens behind is what a process ended by force leaves.
async fn running(data_directory: &Path) -> Option<rd_api::local_control::ControlFile> {
    let control = rd_api::local_control::read(data_directory).ok().flatten()?;
    let reached = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(control.address.as_str()),
    )
    .await;
    matches!(reached, Ok(Ok(_))).then_some(control)
}

#[cfg(test)]
mod tests {
    use super::{Probe, acquire, locking_unsupported, probe, running};

    fn control_file(data: &std::path::Path, address: std::net::SocketAddr) {
        let file = rd_api::local_control::ControlFile {
            address: address.to_string(),
            token: "the-token".to_owned(),
            pid: 4242,
        };
        std::fs::write(
            data.join(rd_api::local_control::FILE),
            serde_json::to_vec(&file).expect("json"),
        )
        .expect("write");
    }

    #[tokio::test]
    async fn doctor_sees_a_service_that_answers_and_not_a_file_left_behind() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(running(directory.path()).await.is_none(), "no file");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        control_file(directory.path(), listener.local_addr().expect("address"));
        let found = running(directory.path()).await.expect("a live service");
        assert_eq!(found.pid, 4242);

        // The process ended by force: the file names an address nothing listens on.
        let address = listener.local_addr().expect("address");
        drop(listener);
        control_file(directory.path(), address);
        assert!(running(directory.path()).await.is_none(), "stale file");
    }

    #[test]
    fn a_second_service_on_the_same_data_directory_is_refused_until_the_first_ends() {
        let directory = tempfile::tempdir().expect("tempdir");
        let first = acquire(directory.path()).expect("the first service starts");
        let error = acquire(directory.path()).expect_err("the second is refused");
        assert!(error.to_string().contains("already runs"), "{error:#}");
        drop(first);
        acquire(directory.path()).expect("free again once the first ended");
    }

    #[test]
    fn services_on_different_data_directories_do_not_meet() {
        let one = tempfile::tempdir().expect("tempdir");
        let other = tempfile::tempdir().expect("tempdir");
        let _first = acquire(one.path()).expect("first");
        acquire(other.path()).expect("second, elsewhere");
    }

    /// RA-IN-03: a service that holds the lock is running, whether or not its control file
    /// exists yet; a free lock is held by `doctor` until it is done.
    #[tokio::test]
    async fn doctor_sees_a_starting_service_by_its_lock() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = acquire(directory.path()).expect("the service starts");
        // Started, no control file written yet.
        assert!(matches!(probe(directory.path()).await, Probe::Held(None)));
        drop(service);

        let Probe::Free(Some(held)) = probe(directory.path()).await else {
            panic!("a free data directory");
        };
        acquire(directory.path()).expect_err("no service starts while doctor works");
        drop(held);
        acquire(directory.path()).expect("free again after doctor");
    }

    /// RA-IN-04: a file system without locking is a warning; other lock errors still refuse.
    #[test]
    fn only_a_file_system_without_locking_is_let_through() {
        use std::io::{Error, ErrorKind};
        assert!(locking_unsupported(&Error::from(ErrorKind::Unsupported)));
        assert!(!locking_unsupported(&Error::from(
            ErrorKind::PermissionDenied
        )));
        #[cfg(unix)]
        {
            use rustix::io::Errno;
            for errno in [Errno::NOLCK, Errno::OPNOTSUPP, Errno::NOTSUP] {
                assert!(locking_unsupported(&Error::from_raw_os_error(
                    errno.raw_os_error()
                )));
            }
            assert!(!locking_unsupported(&Error::from_raw_os_error(
                Errno::IO.raw_os_error()
            )));
        }
    }
}
