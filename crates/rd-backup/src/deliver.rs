//! Handing one archive to every destination (RD-160-02).
//!
//! Each destination is its own attempt, run side by side with the others: one that is down,
//! slow or refusing costs its own delivery and nothing else — not the other destinations, and
//! not a download, since none of this touches the queue. An outage ([`is_transient`]) is tried
//! again after a pause that doubles each time; a refusal that another attempt cannot change —
//! an unusable folder, a deleted profile, a taken name — is not.
//!
//! [`is_transient`]: crate::destination::DestinationError::is_transient

use std::path::Path;
use std::time::Duration;

use crate::destination::{BackupDestination, DestinationError, StoredBackup};

/// How often and how patiently a destination is tried.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    /// Attempts in all, the first included; at least one is made.
    pub attempts: u32,
    /// The pause before the second attempt; each further pause is twice the one before.
    pub first_delay: Duration,
}

impl Default for RetryPolicy {
    /// Three attempts, thirty seconds and then a minute apart: long enough for a NAS that is
    /// waking up or a connection that is back, short enough to finish within the run.
    fn default() -> Self {
        Self {
            attempts: 3,
            first_delay: Duration::from_secs(30),
        }
    }
}

/// How one destination's delivery ended.
#[derive(Debug)]
pub struct Delivery {
    /// Attempts made.
    pub attempts: u32,
    pub result: Result<StoredBackup, DestinationError>,
}

/// Stores `archive` under `name` at one destination, retrying an outage.
pub async fn deliver(
    destination: &dyn BackupDestination,
    archive: &Path,
    name: &str,
    policy: RetryPolicy,
) -> Delivery {
    let mut delay = policy.first_delay;
    let mut attempts = 0;
    loop {
        attempts += 1;
        let result = destination.store(archive, name).await;
        match result {
            Err(error) if error.is_transient() && attempts < policy.attempts.max(1) => {
                tracing::warn!(
                    destination = %destination.describe(),
                    attempt = attempts,
                    code = error.code(),
                    %error,
                    "a backup destination failed; trying again"
                );
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            result => return Delivery { attempts, result },
        }
    }
}

/// [`deliver`] to every destination at once; the deliveries in the order given.
pub async fn deliver_all(
    destinations: &[&dyn BackupDestination],
    archive: &Path,
    name: &str,
    policy: RetryPolicy,
) -> Vec<Delivery> {
    futures_util::future::join_all(
        destinations
            .iter()
            .map(|destination| deliver(*destination, archive, name, policy)),
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;

    use super::{RetryPolicy, deliver_all};
    use crate::destination::{
        BackupDestination, DestinationError, ListedArchive, LocalFolder, StoredBackup,
    };

    /// A destination that is down for the first `failures` attempts, or for good.
    struct Flaky {
        failures: u32,
        calls: AtomicU32,
        refuse: bool,
    }

    #[async_trait]
    impl BackupDestination for Flaky {
        fn kind(&self) -> &'static str {
            "flaky"
        }

        fn describe(&self) -> String {
            "flaky".to_owned()
        }

        async fn store(&self, _: &Path, name: &str) -> Result<StoredBackup, DestinationError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.refuse {
                return Err(DestinationError::NameTaken(name.to_owned()));
            }
            if call < self.failures {
                return Err(DestinationError::Unavailable {
                    code: "backup.destination_unreachable",
                    detail: "connection refused".to_owned(),
                });
            }
            Ok(StoredBackup {
                location: format!("flaky/{name}"),
            })
        }

        async fn list(&self) -> Result<Vec<ListedArchive>, DestinationError> {
            Ok(Vec::new())
        }

        async fn fetch(&self, name: &str, _: &Path) -> Result<u64, DestinationError> {
            Err(DestinationError::NotFound(name.to_owned()))
        }

        async fn remove(&self, name: &str) -> Result<(), DestinationError> {
            Err(DestinationError::NotFound(name.to_owned()))
        }
    }

    fn flaky(failures: u32, refuse: bool) -> Flaky {
        Flaky {
            failures,
            calls: AtomicU32::new(0),
            refuse,
        }
    }

    const QUICK: RetryPolicy = RetryPolicy {
        attempts: 3,
        first_delay: Duration::from_millis(1),
    };

    #[tokio::test]
    async fn an_outage_costs_its_own_destination_and_nothing_else() {
        let directory = tempfile::tempdir().expect("temp");
        let archive = directory.path().join("staged.rdbackup");
        std::fs::write(&archive, b"sealed").expect("stage");
        let folder = LocalFolder::open(&directory.path().join("nas"))
            .await
            .expect("folder");
        let down = flaky(u32::MAX, false);
        let back = flaky(2, false);
        let refusing = flaky(0, true);
        let deliveries = deliver_all(
            &[&down, &folder, &back, &refusing],
            &archive,
            "a.rdbackup",
            QUICK,
        )
        .await;
        // Down for good: three attempts, then its own failure.
        assert_eq!(deliveries[0].attempts, 3);
        assert_eq!(
            deliveries[0]
                .result
                .as_ref()
                .map_err(DestinationError::code),
            Err("backup.destination_unreachable")
        );
        // The folder got the archive regardless.
        assert!(deliveries[1].result.is_ok());
        assert!(directory.path().join("nas/a.rdbackup").exists());
        // Back after two failures: the third attempt succeeds.
        assert_eq!(deliveries[2].attempts, 3);
        assert!(deliveries[2].result.is_ok());
        // A refusal is not tried again.
        assert_eq!(deliveries[3].attempts, 1);
        assert_eq!(refusing.calls.load(Ordering::SeqCst), 1);
    }
}
