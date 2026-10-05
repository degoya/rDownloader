//! Archive passwords in the vault, the facade half (RD-190-04): storing, revealing, the
//! takeover a start runs, and the sweep. The SQL and the order of the steps are in
//! `crate::archive_password`.
//!
//! The password is plaintext in exactly two places: in memory between a request and the vault,
//! and in the vault's encrypted files. The rows hold references; the models leave the store with
//! `password: None`, and only the calls that answer a person who looks at the password (the
//! REST lists and edits the web UI renders, RD-104-04) or that need it (the extraction, the
//! hand-over from the LinkGrabber) read it back from the vault.

use anyhow::{Context, Result, bail};
use rd_secrets::{ExposeSecret, SecretStore, SecretString};

use crate::{
    Database,
    archive_password::{self, PasswordTable},
    commands::{ArchivePasswordsCommand, MaintenanceCommand},
    writer,
};

/// No vault is installed, so a password has nowhere to go but the plain column it left.
pub const NO_VAULT: &str = "db.archive_password_no_vault";

/// A model that carries an archive password out of the store.
pub(crate) trait ArchivePassword {
    const TABLE: PasswordTable;
    fn row_id(&self) -> String;
    /// `false` only where the row says it has none; asking the store is never wrong.
    fn may_have_password(&self) -> bool;
    fn reveal(&mut self, password: String);
}

impl ArchivePassword for rd_core::DownloadPackage {
    const TABLE: PasswordTable = PasswordTable::Packages;
    fn row_id(&self) -> String {
        self.id.to_string()
    }
    fn may_have_password(&self) -> bool {
        self.has_password
    }
    fn reveal(&mut self, password: String) {
        self.password = Some(password);
    }
}

impl ArchivePassword for rd_core::CollectorPackage {
    const TABLE: PasswordTable = PasswordTable::CollectorPackages;
    fn row_id(&self) -> String {
        self.id.to_string()
    }
    fn may_have_password(&self) -> bool {
        self.has_password
    }
    fn reveal(&mut self, password: String) {
        self.password = Some(password);
    }
}

impl ArchivePassword for rd_core::NzbImport {
    const TABLE: PasswordTable = PasswordTable::NzbImports;
    fn row_id(&self) -> String {
        self.id.to_string()
    }
    fn may_have_password(&self) -> bool {
        self.has_password
    }
    fn reveal(&mut self, password: String) {
        self.password = Some(password);
    }
}

impl ArchivePassword for rd_core::SubscriptionItem {
    const TABLE: PasswordTable = PasswordTable::SubscriptionItems;
    fn row_id(&self) -> String {
        self.id.to_string()
    }
    fn may_have_password(&self) -> bool {
        true
    }
    fn reveal(&mut self, password: String) {
        self.password = Some(password);
    }
}

impl Database {
    /// Stores each row's password in the vault and points the row at it; `None` or an empty
    /// value clears the row's password. A value equal to the stored one is left as it is, so a
    /// poll that lists a release again or a form saved unchanged writes nothing.
    pub(crate) async fn store_archive_passwords(
        &self,
        table: PasswordTable,
        entries: Vec<(String, Option<String>)>,
    ) -> Result<()> {
        self.write_archive_passwords(table, entries, true).await
    }

    async fn write_archive_passwords(
        &self,
        table: PasswordTable,
        entries: Vec<(String, Option<String>)>,
        skip_unchanged: bool,
    ) -> Result<()> {
        let entries: Vec<(String, Option<String>)> = entries
            .into_iter()
            .map(|(id, password)| (id, password.filter(|value| !value.is_empty())))
            .collect();
        if entries.is_empty() {
            return Ok(());
        }
        let vault = self.secret_vault();
        let pending = if skip_unchanged {
            let ids: Vec<String> = entries.iter().map(|(id, _)| id.clone()).collect();
            let stored = archive_password::references(&self.readers, table, &ids).await?;
            let mut pending = Vec::with_capacity(entries.len());
            for (id, password) in entries {
                let unchanged = match (&password, stored.get(&id)) {
                    (None, None) => true,
                    (Some(value), Some(reference)) => match vault {
                        Some(vault) => vault
                            .get(reference)
                            .await
                            .is_ok_and(|current| current.expose_secret() == value),
                        None => false,
                    },
                    _ => false,
                };
                if !unchanged {
                    pending.push((id, password));
                }
            }
            pending
        } else {
            entries
        };
        if pending.is_empty() {
            return Ok(());
        }
        let writes: Vec<(String, String)> = pending
            .iter()
            .filter_map(|(_, password)| password.clone())
            .map(|password| (SecretStore::new_reference(), password))
            .collect();
        let mut adopted = Vec::with_capacity(pending.len());
        let mut references = writes.iter().map(|(reference, _)| reference.clone());
        for (id, password) in &pending {
            let reference = password.as_ref().and_then(|_| references.next());
            adopted.push((id.clone(), reference));
        }
        if !writes.is_empty() {
            let Some(vault) = vault else {
                bail!(NO_VAULT);
            };
            let reserved: Vec<String> = writes
                .iter()
                .map(|(reference, _)| reference.clone())
                .collect();
            writer::request(&self.writer, |reply| {
                ArchivePasswordsCommand::ReserveArchivePasswords {
                    references: reserved.clone(),
                    reply,
                }
            })
            .await?;
            for (reference, password) in writes {
                if let Err(error) = vault.put_at(&reference, SecretString::from(password)).await {
                    let released = writer::request(&self.writer, |reply| {
                        ArchivePasswordsCommand::ReleaseArchivePasswords {
                            references: reserved,
                            reply,
                        }
                    })
                    .await;
                    if released.is_ok() {
                        self.sweep_archive_passwords().await;
                    }
                    return Err(error.context("an archive password could not be put in the vault"));
                }
            }
        }
        rd_core::failpoint!("archive_password.before_reference_adopted", || {
            anyhow::anyhow!("crash point: the vault holds the passwords and no row points at them")
        });
        writer::request(&self.writer, |reply| {
            ArchivePasswordsCommand::AdoptArchivePasswords {
                table,
                entries: adopted,
                reply,
            }
        })
        .await?;
        // What the rows pointed at before is released by now.
        self.sweep_archive_passwords().await;
        Ok(())
    }

    /// Fills in the password of every item whose row points into the vault. Never fails: a
    /// password that cannot be read is logged by row, never by value, and left out.
    pub(crate) async fn reveal_archive_passwords<T: ArchivePassword>(&self, items: &mut [T]) {
        let Some(vault) = self.secret_vault() else {
            return;
        };
        let ids: Vec<String> = items
            .iter()
            .filter(|item| item.may_have_password())
            .map(ArchivePassword::row_id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let stored = match archive_password::references(&self.readers, T::TABLE, &ids).await {
            Ok(stored) => stored,
            Err(error) => {
                tracing::warn!(%error, table = T::TABLE.name(), "archive password references could not be read");
                return;
            }
        };
        for item in items {
            let id = item.row_id();
            let Some(reference) = stored.get(&id) else {
                continue;
            };
            match vault.get(reference).await {
                Ok(password) => item.reveal(password.expose_secret().to_owned()),
                Err(error) => tracing::warn!(
                    %error,
                    table = T::TABLE.name(),
                    %id,
                    "an archive password could not be read from the vault"
                ),
            }
        }
    }

    /// One row's password, read from the vault; `None` when it has none, when no vault is
    /// installed, or when the vault cannot open it (logged by row).
    pub(crate) async fn archive_password(
        &self,
        table: PasswordTable,
        id: String,
    ) -> Result<Option<String>> {
        let Some(vault) = self.secret_vault() else {
            return Ok(None);
        };
        let stored =
            archive_password::references(&self.readers, table, std::slice::from_ref(&id)).await?;
        let Some(reference) = stored.get(&id) else {
            return Ok(None);
        };
        match vault.get(reference).await {
            Ok(password) => Ok(Some(password.expose_secret().to_owned())),
            Err(error) => {
                tracing::warn!(%error, table = table.name(), %id, "an archive password could not be read from the vault");
                Ok(None)
            }
        }
    }

    /// Every archive password reference with the table and the row id that own it, for the
    /// full backup, which seals the values under its own key (RD-190-04). The table names are
    /// the ones `restore_copy::BUNDLED_SECRET_COLUMNS` matches them by.
    pub async fn archive_password_references(&self) -> Result<Vec<(&'static str, String, String)>> {
        let mut references = Vec::new();
        for table in PasswordTable::ALL {
            for (id, reference) in archive_password::all_references(&self.readers, table).await? {
                references.push((table.name(), id, reference));
            }
        }
        Ok(references)
    }

    /// Moves every archive password still in a plain column into the vault (RD-190-04).
    ///
    /// Run by every start, after the vault is installed and before anything else writes:
    /// first the reservations a stopped write left are removed with their vault entries,
    /// then each table's plain values go through the same three steps as any write, so the
    /// plain column of a row is emptied in the transaction that gives it its reference. A
    /// stop anywhere leaves the plain value where it was and the next start does it again.
    /// When anything moved, the file is rewritten without free pages and checkpointed, so no
    /// page that held a password survives in it. Without a vault this does nothing.
    ///
    /// Returns how many rows were taken over.
    pub async fn take_over_archive_passwords(&self) -> Result<usize> {
        if self.secret_vault().is_none() {
            return Ok(0);
        }
        self.sweep(true).await?;
        let mut moved = 0;
        for table in PasswordTable::ALL {
            let rows = archive_password::plain(&self.readers, table).await?;
            if rows.is_empty() {
                continue;
            }
            moved += rows.len();
            let entries = rows
                .into_iter()
                .map(|(id, password)| (id, Some(password)))
                .collect();
            self.write_archive_passwords(table, entries, false)
                .await
                .with_context(|| format!("take over the archive passwords of {}", table.name()))?;
        }
        if moved > 0 {
            // The takeover's own pages are zeroed (`secure_delete`); older free pages may still
            // hold a password a deleted row had, and only a rewrite drops them.
            if let Err(error) =
                writer::request(&self.writer, |reply| MaintenanceCommand::Vacuum { reply }).await
            {
                tracing::warn!(%error, "the database could not be rewritten after the archive password takeover");
            }
            self.checkpoint_wal().await?;
        }
        Ok(moved)
    }

    /// Removes the vault entries no row points at any more. Never fails an operation: a
    /// failure is logged and the entry stays recorded for the next sweep.
    pub async fn sweep_archive_passwords(&self) {
        if let Err(error) = self.sweep(false).await {
            tracing::warn!(%error, "released archive passwords could not be swept");
        }
    }

    async fn sweep(&self, with_reservations: bool) -> Result<usize> {
        let Some(vault) = self.secret_vault() else {
            return Ok(0);
        };
        let references = archive_password::swept(&self.readers, with_reservations).await?;
        if references.is_empty() {
            return Ok(0);
        }
        let mut removed = Vec::with_capacity(references.len());
        for reference in references {
            match vault.remove(&reference).await {
                Ok(()) => removed.push(reference),
                Err(error) => {
                    tracing::warn!(%error, "a released archive password stays in the vault until the next sweep");
                }
            }
        }
        rd_core::failpoint!("archive_password.after_secret_removed", || {
            anyhow::anyhow!("crash point: the vault entries are removed and still recorded")
        });
        let count = removed.len();
        writer::request(&self.writer, |reply| {
            ArchivePasswordsCommand::ForgetArchivePasswords {
                references: removed,
                reply,
            }
        })
        .await?;
        Ok(count)
    }
}
