//! The portable switch (RD-180-02): the program files of an unpacked archive, replaced by the
//! entries of the new one.
//!
//! 1. **Stage** ([`stage`]). The verified `.tar.gz`/`.zip` is unpacked into
//!    `<install>/.update-<version>/`; its top-level names are the entries the switch moves, and
//!    those that exist now are the ones it replaces. The journal says `staged`.
//! 2. **Switch** ([`switch`]). Per entry: the live one into `<install>/.previous/`, the new one
//!    into its place, then the emptied staging goes with whatever the switch never moves. Every
//!    step looks at what is where, so a switch stopped anywhere is finished
//!    by running it again (crash points `update.after_previous_set_aside`,
//!    `update.after_new_placed`), and taken back by [`roll_back`] from any point. The journal
//!    says `switching` before the first rename and `switched` after the last
//!    (`update.before_health_check`).
//! 3. **Roll back** ([`roll_back`]). A new entry that left the staging goes to
//!    `<install>/.failed-<version>/`, each entry in `.previous/` back to its place.
//!
//! The leftovers of an earlier update — its staging, `.previous/`, `.failed-…` — and those of
//! this one go through [`trash`]: moved aside, then removed where the system lets them go.
//!
//! Names the switch never moves, even when an archive carried them: the data a portable
//! installation keeps beside its program ([`KEPT`]) and anything hidden.

use std::fs;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail, ensure};

use super::{InstallError, Journal, Phase, exists, remove_any, rename, sync_directory, trash};

/// What lives beside a portable program and is never replaced by an update.
pub const KEPT: &[&str] = &["data", "downloads", "logs", "run", "vendor", "install-kind"];

/// Unpacks the verified artifact beside the program, records the entries it brings and which of
/// them exist now, and writes the journal as `staged`.
///
/// # Errors
///
/// [`InstallError`] `update.digest_mismatch` for a file that is not the one checked,
/// `update.archive_unknown` for a file that is no archive this knows,
/// `update.archive_incomplete` for one without the executable; otherwise what failed. Nothing
/// live has changed then.
pub fn stage(journal: &mut Journal) -> Result<()> {
    let staged = journal.staged_dir();
    let install = journal.plan.install_dir.clone();
    // Leftovers of an earlier update: the start that proved or rolled it back has passed, or
    // no new update could have been handed over. Moved into the trash, never removed in place:
    // a capture agent started before that update may still run from `.previous/` (live finding
    // 2026-10-02), and its folder can be moved but not removed.
    trash::sweep(&install);
    for leftover in [staged.clone(), journal.previous_dir(), journal.failed_dir()] {
        trash::discard(&leftover, &install)?;
    }
    // Unpacked from the handle that was hashed, never from the path again (security review
    // 2026-09-30, finding 6).
    let archive = super::steps::open_artifact(&journal.plan)?;
    fs::create_dir_all(&staged).with_context(|| format!("create {}", staged.display()))?;
    unpack(&journal.plan.artifact, archive, &staged)?;
    let entries = entries_of(&staged, &journal.plan.executable)?;
    journal.replaced = entries
        .iter()
        .filter(|name| exists(&install.join(name)))
        .cloned()
        .collect();
    journal.entries = entries;
    journal.advance(Phase::Staged)
}

/// Unpacks `file`, the opened `artifact`, whose name says what it is.
fn unpack(artifact: &Path, file: fs::File, into: &Path) -> Result<()> {
    let name = artifact
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(BufReader::new(file))
            .with_context(|| format!("read {}", artifact.display()))?;
        archive
            .extract(into)
            .with_context(|| format!("unpack {}", artifact.display()))?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(BufReader::new(file)));
        archive.set_overwrite(true);
        archive
            .unpack(into)
            .with_context(|| format!("unpack {}", artifact.display()))?;
    } else {
        bail!(InstallError::new(
            "update.archive_unknown",
            format!("{} is neither a .tar.gz nor a .zip", artifact.display()),
        ));
    }
    Ok(())
}

/// The top-level names of the unpacked archive, sorted, without what the switch never moves.
fn entries_of(staged: &Path, executable: &str) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(staged).with_context(|| format!("list {}", staged.display()))? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|name| anyhow!("the archive entry {name:?} is not UTF-8"))?;
        if name.starts_with('.') || KEPT.iter().any(|kept| kept.eq_ignore_ascii_case(&name)) {
            tracing::warn!(entry = %name, "the update archive carries an entry the switch never replaces; it is left out");
            continue;
        }
        entries.push(name);
    }
    entries.sort();
    let program = staged.join(executable);
    ensure!(
        program.is_file(),
        InstallError::new(
            "update.archive_incomplete",
            format!("the archive carries no {executable}"),
        )
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(&program)?.permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(&program, permissions)?;
    }
    Ok(entries)
}

/// Sets every entry that exists aside into `.previous/` and puts the new one in its place, then
/// writes the journal as `switched`. Idempotent: run again after a stop, it finishes.
///
/// # Errors
///
/// When a rename fails or the files are in a state no switch leaves them in; [`roll_back`]
/// takes back whatever was done.
pub fn switch(journal: &mut Journal) -> Result<()> {
    ensure!(
        matches!(journal.phase, Phase::Staged | Phase::Switching),
        "the update is {}, not staged",
        journal.phase.as_str()
    );
    if journal.phase == Phase::Staged {
        journal.advance(Phase::Switching)?;
    }
    let install = journal.plan.install_dir.clone();
    let staged = journal.staged_dir();
    let previous = journal.previous_dir();
    fs::create_dir_all(&previous).with_context(|| format!("create {}", previous.display()))?;
    for name in journal.entries.clone() {
        let live = install.join(&name);
        let aside = previous.join(&name);
        let incoming = staged.join(&name);
        // Once an entry left the staging it is the live one, and a second run leaves it alone.
        if !exists(&incoming) {
            continue;
        }
        if exists(&live) {
            ensure!(
                !exists(&aside),
                "{} and its previous copy both exist; the switch cannot tell them apart",
                live.display()
            );
            rename(&live, &aside)?;
            rd_core::failpoint!("update.after_previous_set_aside", || anyhow!("crash point"));
        }
        rename(&incoming, &live)?;
        rd_core::failpoint!("update.after_new_placed", || anyhow!("crash point"));
    }
    // Every entry has left the staging; what stays there is what the switch never moves (the
    // archive's `data/`, hidden names). A roll-back reads an absent staging as "every entry came
    // out", which is now true; one that cannot go yet goes with the clean-up of a later start.
    if let Err(error) = trash::discard(&staged, &install) {
        tracing::warn!(%error, "the staging of the update stays until a later start");
    }
    sync_directory(&previous);
    sync_directory(&install);
    journal.advance(Phase::Switched)?;
    rd_core::failpoint!("update.before_health_check", || anyhow!("crash point"));
    Ok(())
}

/// Takes a switch back from any point: every new entry that left the staging goes to
/// `.failed-<version>/`, every entry in `.previous/` back to its place; then the staging and
/// `.previous/` are removed, and `.failed-…` goes to the trash (a later start removes what the
/// system still holds). The journal is not written; the caller ends it with the reason.
///
/// # Errors
///
/// When a rename fails: the installation then needs the manual recovery of
/// `docs/development.md`, and the journal must say so.
pub fn roll_back(journal: &Journal) -> Result<()> {
    let install = &journal.plan.install_dir;
    let staged = journal.staged_dir();
    let previous = journal.previous_dir();
    let failed = journal.failed_dir();
    for name in &journal.entries {
        let live = install.join(name);
        let aside = previous.join(name);
        let came_out = !exists(&staged.join(name));
        // The live entry is the new one when the old one waits aside, or when the entry is one
        // the old version did not have and it has left the staging.
        let live_is_new =
            exists(&live) && (exists(&aside) || (came_out && !journal.replaced.contains(name)));
        if live_is_new {
            fs::create_dir_all(&failed).with_context(|| format!("create {}", failed.display()))?;
            let target = failed.join(name);
            remove_any(&target)?;
            rename(&live, &target)?;
        }
        if exists(&aside) {
            rename(&aside, &live)?;
        }
    }
    sync_directory(install);
    remove_any(&staged)?;
    remove_any(&previous)?;
    if let Err(error) = trash::discard(&failed, install) {
        tracing::warn!(%error, "the files of the rolled-back version stay until a later start");
    }
    Ok(())
}

#[cfg(test)]
#[path = "portable_tests.rs"]
mod tests;
