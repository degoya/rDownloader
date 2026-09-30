//! What a journal may name before anything acts on it (security review 2026-09-30, finding 5).

use std::fs;
use std::path::{Component, Path};

use super::{
    INSTALLER_DIR, InstallError, Journal, PRE_UPDATE_DIR, UPDATE_DIR, portable, update_dir,
};

impl Journal {
    /// Refuses a journal that reaches outside the installation it belongs to: whatever the start
    /// does with it moves program files, puts a database in place and starts a program, as the
    /// account the service runs as. On top of [`super::Plan::validate`]: the journal lies in the
    /// data directory it names, the database in it, the database copy in its `pre-update` folder
    /// and the kept installer in `update/installer`; the executable, the entries and the replaced
    /// ones are single names in the program folder.
    ///
    /// Whether the program folder is the running executable's is the start's question
    /// (`recover::recover_at_start`); the updater runs from a copy elsewhere.
    ///
    /// # Errors
    ///
    /// `update.journal_invalid` with what is wrong.
    pub fn check(&self, data: &Path) -> Result<(), InstallError> {
        let invalid = |detail: String| Err(InstallError::new("update.journal_invalid", detail));
        if let Err(error) = self.plan.validate() {
            return invalid(error.detail);
        }
        let plan = &self.plan;
        let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if canonical(&plan.data_dir) != canonical(data) {
            return invalid(format!(
                "the journal in {} names the data directory {}",
                data.display(),
                plan.data_dir.display()
            ));
        }
        let database_name = plan
            .database
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if plan.database.parent() != Some(plan.data_dir.as_path()) || !is_name(database_name) {
            return invalid(format!(
                "the database {} is not in the data directory",
                plan.database.display()
            ));
        }
        if let Some(copy) = &plan.database_copy
            && !is_below(copy, &plan.data_dir.join(PRE_UPDATE_DIR))
        {
            return invalid(format!(
                "the database copy {} is not in {PRE_UPDATE_DIR}",
                copy.display()
            ));
        }
        if let Some(installer) = &plan.previous_installer
            && !is_below(installer, &update_dir(&plan.data_dir).join(INSTALLER_DIR))
        {
            return invalid(format!(
                "the previous installer {} is not in {UPDATE_DIR}/{INSTALLER_DIR}",
                installer.display()
            ));
        }
        for name in std::iter::once(&plan.executable)
            .chain(&self.entries)
            .chain(&self.replaced)
        {
            if !is_name(name) {
                return invalid(format!("{name:?} is not a name in the program folder"));
            }
        }
        if let Some(name) = self.entries.iter().find(|name| {
            portable::KEPT
                .iter()
                .any(|kept| kept.eq_ignore_ascii_case(name))
        }) {
            return invalid(format!("{name:?} is never replaced by an update"));
        }
        if let Some(name) = self
            .replaced
            .iter()
            .find(|name| !self.entries.contains(name))
        {
            return invalid(format!("{name:?} is replaced but no entry of the update"));
        }
        Ok(())
    }
}

/// One plain, visible name: a single normal path component that does not start with `.`, so
/// joined to a folder it stays inside it on every platform (`C:x` and `a\b` are two on Windows).
fn is_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
        && matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
}

/// Whether `path` lies below `folder`, lexically: no `..`, no second root.
fn is_below(path: &Path, folder: &Path) -> bool {
    path.strip_prefix(folder).is_ok_and(|rest| {
        rest.components().next().is_some()
            && rest
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
    })
}
