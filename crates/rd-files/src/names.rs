use std::collections::HashSet;
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

const MAX_UTF16_UNITS: usize = 240;

/// Budget for a complete path. Windows' classic limit is 260 including the drive and the
/// terminating NUL; long-path support needs both an opted-in manifest and a machine-wide
/// registry switch, and external tools such as yt-dlp open their files through runtimes that
/// do not opt in at all. Staying below the classic limit is the only portable option.
const MAX_PATH_UTF16_UNITS: usize = 250;

/// Never shorten a name below this, otherwise deeply nested destinations would erase it.
const MIN_NAME_UTF16_UNITS: usize = 24;

/// Produces a portable filename safe on Windows, Linux and macOS.
#[must_use]
pub fn sanitize_file_name(input: &str) -> String {
    sanitize_to_limit(input, MAX_UTF16_UNITS)
}

/// Sanitizes `name` and shortens it so `directory/<name>` still fits the path budget.
///
/// `reserve` keeps room for suffixes a downloader appends to the very same path — yt-dlp for
/// instance writes `<name>.f137.mp4.part` before moving the result into place.
#[must_use]
pub fn sanitize_file_name_within(directory: &Path, name: &str, reserve: usize) -> String {
    let directory_units = directory.to_string_lossy().encode_utf16().count();
    let available = MAX_PATH_UTF16_UNITS
        .saturating_sub(directory_units + 1 + reserve)
        .max(MIN_NAME_UTF16_UNITS);
    sanitize_to_limit(name, available.min(MAX_UTF16_UNITS))
}

fn sanitize_to_limit(input: &str, limit: usize) -> String {
    let normalized: String = input.nfc().collect();
    let mut sanitized: String = normalized
        .chars()
        .map(|character| {
            // U+FFFD marks text that was already decoded wrongly upstream; keeping it would
            // carry the damage into the file name.
            if character.is_control()
                || character == '\u{FFFD}'
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    sanitized = sanitized.trim().trim_end_matches(['.', ' ']).to_owned();

    if sanitized.is_empty() {
        sanitized.push_str("download");
    }
    if is_windows_reserved(&sanitized) {
        sanitized.insert(0, '_');
    }
    truncate_utf16(&sanitized, limit)
}

/// Extensions stripped when a file name becomes a package (folder) name. Only known
/// extensions are listed so release names such as `Show.2023` keep their suffix.
const PACKAGE_NAME_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "mpg", "mpeg", "m4v", "ts", "mp3", "m4a",
    "aac", "flac", "wav", "ogg", "opus", "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso",
    "nzb", "par2", "pdf", "jpg", "jpeg", "png", "gif", "srt", "sub", "idx", "bin", "exe",
];

/// Turns a file name into a package name by removing trailing file extensions
/// (`Video.mp4` → `Video`, `backup.tar.gz` → `backup`).
#[must_use]
pub fn package_name_from_file_name(file_name: &str) -> String {
    let mut name = file_name.trim();
    while let Some((stem, extension)) = name.rsplit_once('.') {
        let stem = stem.trim_end_matches(['.', ' ']);
        if stem.is_empty()
            || !PACKAGE_NAME_EXTENSIONS.contains(&extension.trim().to_ascii_lowercase().as_str())
        {
            break;
        }
        name = stem;
    }
    if name.is_empty() {
        file_name.trim().to_owned()
    } else {
        name.to_owned()
    }
}

/// Directory that holds every file of one package: `<base>/<sanitized package name>`.
///
/// The folder name is shortened so the files inside it still have room: a package whose name
/// is a full video title would otherwise consume the entire path budget.
#[must_use]
pub fn package_directory(base: &Path, package_name: &str) -> PathBuf {
    let name = sanitize_file_name_within(base, package_name, MIN_NAME_UTF16_UNITS * 2);
    let name = if name == "download" && package_name.trim().is_empty() {
        "package".to_owned()
    } else {
        name
    };
    base.join(name)
}

/// The folder a package moves into when it is renamed to `package_name`.
///
/// A package folder always sits directly below the directory its category resolved to, so a
/// rename keeps the parent and changes only the last component — which is what makes it a
/// cheaper operation than a category change, and what lets it reuse the same name rules.
/// `None` when `current` has no parent at all (a bare root), because there is then no
/// directory to rename inside.
#[must_use]
pub fn renamed_package_directory(current: &Path, package_name: &str) -> Option<PathBuf> {
    let parent = current.parent()?;
    Some(package_directory(parent, package_name))
}

/// Finds a non-existing path by appending a deterministic numeric suffix.
///
/// "Non-existing" means no entry at all, a dangling symlink included: `exists()` follows the
/// link and calls it free, and the download would then be written through it to wherever the
/// link points (audit 2026-10-08, CORE-03).
#[must_use]
pub fn collision_free_path(directory: &Path, file_name: &str) -> PathBuf {
    let sanitized = sanitize_file_name(file_name);
    let direct = directory.join(&sanitized);
    if !is_taken(&direct) {
        return direct;
    }

    let path = Path::new(&sanitized);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1..=10_000_u32 {
        let candidate = match extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let path = directory.join(candidate);
        if !is_taken(&path) {
            return path;
        }
    }

    directory.join(format!("{}-{}", stem, uuid::Uuid::now_v7()))
}

/// Whether anything sits at `path`, the link itself rather than its target.
fn is_taken(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// The folder one archive set is unpacked into when every set gets its own (RD-170-16):
/// `<directory>/<archive base>`, `Film.part1.rar` → `Film`.
///
/// The name comes from a downloaded file, so it goes through the package-folder rules and is
/// shortened so the extracted files below it still fit the path budget. An existing directory
/// of that name is the answer — a second run after a crash merges into it. Anything else in the
/// way (a file, a symlink) moves the name on to ` (1)`, ` (2)`, … in the same order every time,
/// so a rerun lands in the folder the first run chose.
#[must_use]
pub fn extraction_subfolder(directory: &Path, archive_base: &str) -> PathBuf {
    free_subfolder(directory, archive_base, &HashSet::new())
}

/// The folders of all archive sets of one package, one per base and in the same order
/// (RD-190-06).
///
/// Each is [`extraction_subfolder`], except that a name an earlier set already took counts as in
/// the way: `Film.zip` and `Film.rar` unpack into `Film` and `Film (1)` rather than into one
/// folder. Names compare without regard to case, as they do on Windows and macOS. The order of
/// `archive_bases` decides who keeps the plain name, so a rerun over the same sets in the same
/// order lands every set in the folder the first run chose.
#[must_use]
pub fn extraction_subfolders(directory: &Path, archive_bases: &[&str]) -> Vec<PathBuf> {
    let mut taken = HashSet::new();
    archive_bases
        .iter()
        .map(|base| {
            let folder = free_subfolder(directory, base, &taken);
            if let Some(name) = folder.file_name() {
                taken.insert(name.to_string_lossy().to_lowercase());
            }
            folder
        })
        .collect()
}

/// The first of `<name>`, `<name> (1)`, `<name> (2)`, … that is a directory or free and not
/// in `taken` (lower-case names).
fn free_subfolder(directory: &Path, archive_base: &str, taken: &HashSet<String>) -> PathBuf {
    let name = sanitize_file_name_within(directory, archive_base, MIN_NAME_UTF16_UNITS * 2);
    // `symlink_metadata`: a link to a directory elsewhere is in the way, not a place to unpack.
    let usable = |candidate: &str| {
        !taken.contains(&candidate.to_lowercase())
            && !std::fs::symlink_metadata(directory.join(candidate))
                .is_ok_and(|meta| !meta.is_dir())
    };
    if usable(&name) {
        return directory.join(&name);
    }
    for index in 1..=10_000_u32 {
        let candidate = format!("{name} ({index})");
        if usable(&candidate) {
            return directory.join(candidate);
        }
    }
    directory.join(format!("{name}-{}", uuid::Uuid::now_v7()))
}

/// Device names Windows opens instead of a file, whatever the extension: `CON`, `CONIN$`,
/// `COM1`, `LPT` with a superscript digit and the rest of Microsoft's list.
#[must_use]
pub fn is_windows_reserved(file_name: &str) -> bool {
    let base = file_name
        .split_once('.')
        .map_or(file_name, |(base, _)| base)
        .trim_end_matches(['.', ' '])
        .to_ascii_uppercase();
    if matches!(
        base.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    // Compared by characters, never sliced by bytes: a four-byte base can be one emoji or
    // `42` and a degree sign, and a byte index through it panicked (audit 2026-10-05, S3).
    let Some(port) = base
        .strip_prefix("COM")
        .or_else(|| base.strip_prefix("LPT"))
    else {
        return false;
    };
    let mut digits = port.chars();
    matches!(
        (digits.next(), digits.next()),
        (Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
    )
}

fn truncate_utf16(input: &str, limit: usize) -> String {
    if input.encode_utf16().count() <= limit {
        return input.to_owned();
    }

    let path = Path::new(input);
    let extension = path.extension().and_then(|value| value.to_str());
    let suffix = extension.map_or(String::new(), |value| format!(".{value}"));
    let suffix_units = suffix.encode_utf16().count();
    let stem_limit = limit.saturating_sub(suffix_units);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let mut result = String::new();
    let mut units = 0;
    for character in stem.chars() {
        let width = character.len_utf16();
        if units + width > stem_limit {
            break;
        }
        units += width;
        result.push(character);
    }
    result.push_str(&suffix);
    result
}

#[cfg(test)]
#[path = "names_tests.rs"]
mod tests;
