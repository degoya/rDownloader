//! Reading a checksum sidecar of one [`Algorithm`].
//!
//! The format is the one `md5sum` and `sha256sum` write: one line per file, `<hex>  <name>`. A
//! leading `*` on the name marks binary mode and means nothing here — every read is binary.
//!
//! Entries name their files relative to the sidecar's own folder, the way the tool wrote them
//! there; [`Algorithm::wanted`] turns them into the names the host lists for the package.

use std::collections::BTreeSet;

/// What tells one checksum plugin's sidecars and codes from another's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Algorithm {
    /// The extension that marks a sidecar of this kind, `.md5`.
    pub extension: &'static str,
    /// Length of the hex digest the hash produces.
    pub digest_hex_len: usize,
    /// The plugin's slug, which every code it reports starts with: `md5_postprocess`.
    pub slug: &'static str,
    /// The hash's name in an English message: `MD5`.
    pub label: &'static str,
}

/// One `<hex>  <name>` line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub digest: String,
    pub file: String,
}

/// What one sidecar asks this step to verify.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Plan {
    /// Entries whose file is in the package, each named as the host lists it.
    pub entries: Vec<Entry>,
    /// Entries, as the sidecar spells them, whose file the package lacks.
    pub unchecked: Vec<String>,
}

/// Why a sidecar cannot be verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Unverifiable {
    /// None of the files the sidecar lists is in the package; the first entry, as spelt.
    Missing(String),
    /// Not one line of the sidecar is a checksum of this kind.
    Empty,
}

impl Algorithm {
    /// The code `<slug>.<name>` a failure or a warning of this plugin is reported under.
    #[must_use]
    pub fn code(&self, name: &str) -> String {
        format!("{}.{name}", self.slug)
    }

    /// Whether a package file is a sidecar of this kind.
    #[must_use]
    pub fn is_sidecar(&self, name: &str) -> bool {
        name.to_ascii_lowercase().ends_with(self.extension)
    }

    /// Parses a sidecar's text.
    ///
    /// A malformed line is skipped rather than failing the file: a stray comment or a blank
    /// line is no reason to refuse to check the entries that *are* well formed. A digest of the
    /// wrong length is skipped for the same reason — it cannot match anything, and treating it
    /// as a mismatch would report a file as corrupt on the strength of a typo.
    #[must_use]
    pub fn parse(&self, text: &str) -> Vec<Entry> {
        let mut entries = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((digest, file)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let digest = digest.trim().to_ascii_lowercase();
            if digest.len() != self.digest_hex_len || !digest.chars().all(|c| c.is_ascii_hexdigit())
            {
                continue;
            }
            // `*name` is binary mode; the marker is not part of the name.
            let file = file.trim().trim_start_matches('*').trim();
            if file.is_empty() {
                continue;
            }
            entries.push(Entry {
                digest,
                file: file.to_owned(),
            });
        }
        entries
    }

    /// The entries of one sidecar this step verifies, each naming its file as the host lists it.
    ///
    /// `files` is the package as the host offered it: relative to the package, with `/` between
    /// folders (`Film/film.mkv`, RD-170-16). `sidecar` is one of those names, so `film.mkv` in
    /// `Film/film.md5` is `Film/film.mkv`. `removed` names, the same way, the files the
    /// pipeline removed before this step: unpacked volumes, PAR2 files, what the cleanup deleted.
    ///
    /// An entry whose file is not in the package, or that [`resolve`] refuses, is left
    /// [`Plan::unchecked`] while others of the sidecar are there to verify: a release split
    /// across packages, say. The caller names those, never passes them over silently. A sidecar
    /// none of whose files is there fails, as does one without a single readable line: both
    /// promise a check and deliver none. An entry the host says post-processing removed counts
    /// as neither — the unpack or repair that read it succeeded before this step ran — so a
    /// sidecar listing only such files is left with nothing to do (RD-190-06; before, the
    /// extension was guessed at).
    ///
    /// # Errors
    ///
    /// [`Unverifiable::Empty`] for a sidecar without a readable line, [`Unverifiable::Missing`]
    /// for one none of whose files is in the package.
    pub fn wanted(
        &self,
        files: &BTreeSet<&str>,
        removed: &BTreeSet<&str>,
        sidecar: &str,
        text: &str,
    ) -> Result<Plan, Unverifiable> {
        let entries = self.parse(text);
        if entries.is_empty() {
            return Err(Unverifiable::Empty);
        }
        let mut plan = Plan::default();
        for entry in entries {
            match resolve(sidecar, &entry.file) {
                Some(file) if files.contains(file.as_str()) => plan.entries.push(Entry {
                    digest: entry.digest,
                    file,
                }),
                Some(file) if removed.contains(file.as_str()) => {}
                _ => plan.unchecked.push(entry.file),
            }
        }
        if plan.entries.is_empty()
            && let Some(first) = plan.unchecked.first()
        {
            return Err(Unverifiable::Missing(first.clone()));
        }
        Ok(plan)
    }
}

/// Where an entry of `sidecar` lives in the package, `/`-separated like the host's list.
///
/// A `\` counts as a separator, since a checksum written on Windows uses it, and `.` is dropped.
/// `None` for an entry that could leave the sidecar's folder — one with `..`, an absolute path or
/// a drive letter — which is never followed: it names no file of this package.
#[must_use]
pub fn resolve(sidecar: &str, file: &str) -> Option<String> {
    let drive = file.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && file.as_bytes().get(1) == Some(&b':');
    if drive || file.starts_with(['/', '\\']) {
        return None;
    }
    let mut parts: Vec<&str> = sidecar.split('/').collect();
    // The sidecar's own name; what is left is its folder.
    parts.pop();
    for part in file.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
#[path = "sidecar_tests.rs"]
mod tests;
