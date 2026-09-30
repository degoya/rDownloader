//! Reading a `.sha256` sidecar.
//!
//! The format is the one `sha256sum` writes: one line per file, `<hex>  <name>`. A leading
//! `*` on the name marks binary mode and means nothing here — every read is binary.
//!
//! Entries name their files relative to the sidecar's own folder, the way the tool wrote them
//! there; [`wanted`] turns them into the names the host lists for the package.
//!
//! Deliberately duplicated in the MD5 plugin rather than shared. The two are separate plugins
//! so each can be updated, versioned and switched off on its own; a shared crate would quietly
//! make them one thing again, and a hundred lines is a cheap price for keeping them apart.

use std::collections::BTreeSet;

/// The extension that marks a sidecar of this kind.
pub const EXTENSION: &str = ".sha256";
/// Length of the hex digest SHA-256 produces.
pub const DIGEST_HEX_LEN: usize = 64;

/// One `<hex>  <name>` line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub digest: String,
    pub file: String,
}

/// Whether a package file is a sidecar of this kind.
#[must_use]
pub fn is_sidecar(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(EXTENSION)
}

/// Parses a sidecar's text.
///
/// A malformed line is skipped rather than failing the file: a stray comment or a blank line
/// is no reason to refuse to check the entries that *are* well formed. A digest of the wrong
/// length is skipped for the same reason — it cannot match anything, and treating it as a
/// mismatch would report a file as corrupt on the strength of a typo.
#[must_use]
pub fn parse(text: &str) -> Vec<Entry> {
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
        if digest.len() != DIGEST_HEX_LEN || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
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

/// The entries of one sidecar this step verifies, each naming its file as the host lists it.
///
/// `files` is the package as the host offered it: relative to the package, with `/` between
/// folders (`Film/film.mkv`, RD-170-16). `sidecar` is one of those names, so `film.mkv` in
/// `Film/film.sha256` is `Film/film.mkv`.
///
/// An entry whose file is not in the package, or that [`resolve`] refuses, is left
/// [`Plan::unchecked`] while others of the sidecar are there to verify: a release split across
/// packages, or a file a cleanup rule removed. The caller names those, never passes them over
/// silently. A sidecar none of whose files is there fails, as does one without a single readable
/// line: both promise a check and deliver none. A file post-processing removes itself once the
/// unpack or repair that read it has succeeded counts as neither — this step only runs after
/// that — so a sidecar listing only such files is left with nothing to do.
///
/// # Errors
///
/// [`Unverifiable::Empty`] for a sidecar without a readable line, [`Unverifiable::Missing`] for
/// one none of whose files is in the package.
pub fn wanted(files: &BTreeSet<&str>, sidecar: &str, text: &str) -> Result<Plan, Unverifiable> {
    let entries = parse(text);
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
            _ if removed_by_postprocessing(&entry.file) => {}
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

/// Whether post-processing deletes a file of this name itself: an archive volume once it is
/// unpacked (`.rar`, `.r00`, `.zip`, `.z01`, `.7z`, `.001`), a PAR2 file once the repair is
/// through, or one of the default cleanup extensions. A sidecar listing it is not wrong
/// for that, and the unpack already checked what the volume held.
fn removed_by_postprocessing(file: &str) -> bool {
    let name = file.to_ascii_lowercase();
    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };
    let digits = |text: &str| text.len() >= 2 && text.bytes().all(|byte| byte.is_ascii_digit());
    matches!(
        extension,
        "rar" | "zip" | "7z" | "par2" | "nfo" | "sfv" | "srr" | "url" | "nzb"
    ) || extension.strip_prefix('r').is_some_and(digits)
        || extension.strip_prefix('z').is_some_and(digits)
        || (extension.len() == 3 && digits(extension))
}

/// Lower-case hex of a digest, for comparing with what a sidecar recorded.
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{Entry, Plan, Unverifiable, is_sidecar, parse, resolve, to_hex, wanted};

    /// SHA-256 of the empty input, used only for its shape.
    const DIGEST: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn a_sidecar_is_recognised_by_its_extension() {
        assert!(is_sidecar("release.sha256"));
        assert!(is_sidecar("RELEASE.SHA256"));
        assert!(!is_sidecar("release.sfv"));
        assert!(!is_sidecar("sha256"));
    }

    #[test]
    fn the_usual_two_space_form_is_read() {
        assert_eq!(
            parse(&format!("{DIGEST}  release.bin\n")),
            vec![Entry {
                digest: DIGEST.to_owned(),
                file: "release.bin".to_owned(),
            }]
        );
    }

    #[test]
    fn binary_mode_and_upper_case_digests_are_accepted() {
        let entries = parse(&format!("{}  *release.bin", DIGEST.to_uppercase()));
        assert_eq!(entries[0].digest, DIGEST);
        assert_eq!(entries[0].file, "release.bin");
    }

    #[test]
    fn a_malformed_line_costs_only_itself() {
        let text = format!("# a comment\n\nnothex  release.bin\n{DIGEST}  good.bin\n");
        let entries = parse(&text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file, "good.bin");
    }

    fn package<'a>(files: &[&'a str]) -> BTreeSet<&'a str> {
        files.iter().copied().collect()
    }

    #[test]
    fn a_top_level_entry_keeps_its_name() {
        assert_eq!(
            resolve("release.sha256", "release.bin").as_deref(),
            Some("release.bin")
        );
        assert_eq!(
            resolve("release.sha256", "CD1/a.bin").as_deref(),
            Some("CD1/a.bin")
        );
    }

    #[test]
    fn an_entry_in_a_subfolder_is_read_from_the_sidecars_folder() {
        assert_eq!(
            resolve("Film/film.sha256", "film.mkv").as_deref(),
            Some("Film/film.mkv")
        );
        assert_eq!(
            resolve("A/B/x.sha256", "./c\\d.bin").as_deref(),
            Some("A/B/c/d.bin")
        );
    }

    #[test]
    fn an_entry_that_could_leave_its_folder_is_not_resolved() {
        assert_eq!(resolve("Film/film.sha256", "../other.bin"), None);
        assert_eq!(resolve("Film/film.sha256", "a/../../b.bin"), None);
        assert_eq!(resolve("Film/film.sha256", "/etc/passwd"), None);
        assert_eq!(resolve("Film/film.sha256", "\\server\\x.bin"), None);
        assert_eq!(resolve("Film/film.sha256", "C:\\film.mkv"), None);
    }

    #[test]
    fn a_sidecar_in_a_subfolder_checks_the_files_beside_it() {
        let files = package(&["Film/film.mkv", "Film/film.sha256", "film.mkv"]);
        let entries = wanted(&files, "Film/film.sha256", &format!("{DIGEST}  film.mkv\n"));
        assert_eq!(
            entries,
            Ok(Plan {
                entries: vec![Entry {
                    digest: DIGEST.to_owned(),
                    file: "Film/film.mkv".to_owned(),
                }],
                unchecked: Vec::new(),
            })
        );
    }

    #[test]
    fn a_top_level_sidecar_reads_as_before() {
        let files = package(&["release.bin", "release.sha256"]);
        let entries = wanted(
            &files,
            "release.sha256",
            &format!("{DIGEST}  release.bin\n"),
        );
        assert_eq!(
            entries.map(|plan| plan.entries[0].file.clone()),
            Ok("release.bin".to_owned())
        );
    }

    #[test]
    fn a_listed_file_the_package_lacks_is_named_when_others_verify() {
        // A release split across two packages: this one holds the first part only.
        let files = package(&["release.sha256", "CD1/a.bin"]);
        let text = format!("{DIGEST}  CD1/a.bin\n{DIGEST}  CD2/b.bin\n{DIGEST}  ../c.bin\n");
        let plan = wanted(&files, "release.sha256", &text).expect("plan");
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(
            plan.unchecked,
            vec!["CD2/b.bin".to_owned(), "../c.bin".to_owned()]
        );
    }

    #[test]
    fn a_sidecar_none_of_whose_files_is_there_fails() {
        // `film.mkv` exists, but at the top, not beside the sidecar that names it.
        let files = package(&["Film/film.sha256", "film.mkv"]);
        let text = format!("{DIGEST}  film.mkv\n{DIGEST}  ../film.mkv\n");
        assert_eq!(
            wanted(&files, "Film/film.sha256", &text),
            Err(Unverifiable::Missing("film.mkv".to_owned()))
        );
    }

    #[test]
    fn volumes_the_unpack_removed_are_not_missing() {
        let files = package(&["release.sha256", "release/film.mkv"]);
        let text = format!(
            "{DIGEST}  release.part1.rar\n{DIGEST}  release.r00\n{DIGEST}  release.7z.001\n\
             {DIGEST}  release.vol0+1.par2\n{DIGEST}  release.nfo\n"
        );
        assert_eq!(wanted(&files, "release.sha256", &text), Ok(Plan::default()));
    }

    #[test]
    fn a_sidecar_without_a_readable_line_is_not_a_pass() {
        let files = package(&["release.sha256"]);
        assert_eq!(
            wanted(&files, "release.sha256", "# nothing here\n"),
            Err(Unverifiable::Empty)
        );
    }

    #[test]
    fn hex_is_lower_case_and_zero_padded() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
    }
}
