//! A stored value the vault holds but cannot open (RD-1240-36).
//!
//! The master key is per installation -- in the OS keyring wherever one is reachable -- so a
//! data folder copied to another machine or user account arrives with every entry and without
//! the key they were sealed with. Such an entry is neither missing nor a fault of the service:
//! the credential has to be entered again, or a full backup restored with its passphrase, which
//! carries the key. [`SecretUnreadable`] says exactly that, so a caller can tell it apart from
//! every other failure without matching on text.

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;

use crate::{KeyringRead, SecretStore, existing_master_key, load_keyring_key};

/// The stable code a request or a background run answers with when a stored credential cannot
/// be opened.
pub const SECRET_UNREADABLE: &str = "secret.unreadable";

/// Why an existing entry could not be opened. None of them names the entry or its value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnreadableReason {
    /// The cipher refused it: sealed under another master key, or changed since.
    Undecryptable,
    /// The file is not an envelope this build can read.
    Malformed,
    /// The envelope names a version this build does not know.
    UnknownVersion,
}

/// An entry exists under the reference, but the current master key cannot open it.
#[derive(Debug)]
pub struct SecretUnreadable {
    reason: UnreadableReason,
}

impl SecretUnreadable {
    pub(crate) fn error(reason: UnreadableReason) -> anyhow::Error {
        anyhow::Error::new(Self { reason })
    }

    /// Why the entry could not be opened.
    #[must_use]
    pub const fn reason(&self) -> UnreadableReason {
        self.reason
    }
}

impl std::fmt::Display for SecretUnreadable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self.reason {
            UnreadableReason::Undecryptable => "the vault master key does not open it",
            UnreadableReason::Malformed => "its file is damaged",
            UnreadableReason::UnknownVersion => "it was written by a newer version",
        };
        write!(
            formatter,
            "{SECRET_UNREADABLE}: a stored credential cannot be read ({why}); enter it again"
        )
    }
}

impl std::error::Error for SecretUnreadable {}

/// The [`SecretUnreadable`] among `error` and its causes -- through every context a caller added
/// on the way up -- for a record that should carry its code rather than the outermost context.
#[must_use]
pub fn find_unreadable(error: &anyhow::Error) -> Option<&SecretUnreadable> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<SecretUnreadable>())
}

/// Whether `error` or any of its causes is a [`SecretUnreadable`].
#[must_use]
pub fn is_unreadable(error: &anyhow::Error) -> bool {
    find_unreadable(error).is_some()
}

/// How many of the vault's entries the current master key opens, and how many it does not.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VaultReadability {
    pub readable: usize,
    pub unreadable: usize,
}

impl SecretStore {
    /// Tries every entry once and counts those the current master key cannot open
    /// ([`SecretUnreadable`]). No value leaves this function; a temporary a stopped write left
    /// behind is no entry and counts as neither.
    ///
    /// # Errors
    ///
    /// When the folder cannot be read.
    pub async fn readability(&self) -> Result<VaultReadability> {
        let mut counts = VaultReadability::default();
        for reference in self.stored_references().await? {
            match self.get(&reference).await {
                Ok(_) => counts.readable += 1,
                Err(error) if is_unreadable(&error) => counts.unreadable += 1,
                Err(_) => {}
            }
        }
        Ok(counts)
    }

    /// [`SecretStore::readability`] of the service's vault for `rdownloader doctor`, without
    /// minting a master key or creating the folder: a diagnosis run on a copied data folder, or
    /// as another user, must not seal the vault under a key the service would then not hold.
    ///
    /// # Errors
    ///
    /// When the keyring refuses the read or the folder cannot be read.
    pub async fn inspect_with_os_keyring(root: PathBuf) -> Result<VaultReadability> {
        inspect(root, Some(load_keyring_key as KeyringRead)).await
    }
}

pub(crate) async fn inspect(
    root: PathBuf,
    read_keyring: Option<KeyringRead>,
) -> Result<VaultReadability> {
    if !tokio::fs::try_exists(&root).await.unwrap_or(false) {
        return Ok(VaultReadability::default());
    }
    // Without a master key nothing stored opens. An all-zero key classifies every entry the way
    // a wrong key does -- the cipher refuses it -- through the same path as a present key.
    let key = existing_master_key(&root, read_keyring)
        .await?
        .unwrap_or([0_u8; 32]);
    SecretStore {
        root: Arc::new(root),
        key: Arc::new(key),
    }
    .readability()
    .await
}
