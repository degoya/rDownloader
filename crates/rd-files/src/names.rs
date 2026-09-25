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
        "Paket".to_owned()
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
#[must_use]
pub fn collision_free_path(directory: &Path, file_name: &str) -> PathBuf {
    let sanitized = sanitize_file_name(file_name);
    let direct = directory.join(&sanitized);
    if !direct.exists() {
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
        if !path.exists() {
            return path;
        }
    }

    directory.join(format!("{}-{}", stem, uuid::Uuid::now_v7()))
}

fn is_windows_reserved(file_name: &str) -> bool {
    let base = file_name
        .split_once('.')
        .map_or(file_name, |(base, _)| base)
        .trim_end_matches(['.', ' '])
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && matches!(&base[..3], "COM" | "LPT")
            && matches!(base.as_bytes()[3], b'1'..=b'9'))
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
mod tests {
    use std::path::Path;

    use super::{
        MAX_PATH_UTF16_UNITS, package_directory, package_name_from_file_name,
        renamed_package_directory, sanitize_file_name, sanitize_file_name_within,
    };

    #[test]
    fn strips_known_extensions_from_package_names() {
        assert_eq!(
            package_name_from_file_name("Great Video.mp4"),
            "Great Video"
        );
        assert_eq!(package_name_from_file_name("backup.tar.gz"), "backup");
        assert_eq!(package_name_from_file_name("Show.2023"), "Show.2023");
        assert_eq!(
            package_name_from_file_name("Show.S01.1080p.WEB-DL"),
            "Show.S01.1080p.WEB-DL"
        );
        assert_eq!(package_name_from_file_name(".mp4"), ".mp4");
        assert_eq!(package_name_from_file_name("watch"), "watch");
    }

    #[test]
    fn sanitizes_windows_names() {
        assert_eq!(sanitize_file_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_file_name("bad<name>. "), "bad_name_");
        assert_eq!(sanitize_file_name("   "), "download");
    }

    #[test]
    fn replaces_broken_decoding_markers() {
        assert_eq!(
            sanitize_file_name("251K views \u{FFFD} 562"),
            "251K views _ 562"
        );
    }

    #[test]
    fn keeps_the_whole_path_inside_the_budget() {
        // The title that made yt-dlp fail with "unable to open for writing" on Windows.
        let title = "251K views \u{FFFD} 562 reactions _ Seit Jahren reisen Menschen nach \
                     Altschauerberg, um dort ein ganz besonderes Haus zu sehen und heute \
                     zeigen wir euch warum das so ist und was es damit auf sich hat";
        let directory = Path::new("C:\\Tools\\rdownloader\\downloads\\facebook.com");
        let name = sanitize_file_name_within(directory, title, 20);
        let full = directory
            .join(&name)
            .to_string_lossy()
            .encode_utf16()
            .count();
        assert!(
            full + 20 <= MAX_PATH_UTF16_UNITS,
            "path {full} + reserve exceeds the budget: {name}"
        );
        assert!(!name.is_empty());
        assert!(!name.contains('\u{FFFD}'));
    }

    #[test]
    fn a_long_package_name_leaves_room_for_its_files() {
        let base = Path::new("C:\\Tools\\rdownloader\\downloads");
        let directory = package_directory(base, &"very long package name ".repeat(20));
        let used = directory.to_string_lossy().encode_utf16().count();
        assert!(
            MAX_PATH_UTF16_UNITS - used >= 24,
            "package folder leaves only {} units for file names",
            MAX_PATH_UTF16_UNITS - used
        );
    }

    #[test]
    fn a_short_name_is_left_alone() {
        let directory = Path::new("/downloads/youtube.com");
        assert_eq!(
            sanitize_file_name_within(directory, "clip.mp4", 20),
            "clip.mp4"
        );
    }

    #[test]
    fn a_renamed_package_folder_stays_beside_the_one_it_replaces() {
        let current = Path::new("/downloads/movies/Old Name");
        assert_eq!(
            renamed_package_directory(current, "New Name"),
            Some(Path::new("/downloads/movies/New Name").to_path_buf())
        );
    }

    /// The rename runs the requested name through the very same rules a new folder gets, so a
    /// separator in it can never walk the package out of its category directory.
    #[test]
    fn a_rename_cannot_escape_the_category_directory() {
        let current = Path::new("/downloads/movies/Old Name");
        let renamed = renamed_package_directory(current, "../../etc/Escaped").expect("renamed");
        assert_eq!(renamed.parent(), Some(Path::new("/downloads/movies")));
        assert_eq!(
            renamed.file_name().and_then(|value| value.to_str()),
            Some(".._.._etc_Escaped")
        );
    }

    #[test]
    fn a_root_directory_has_nothing_to_rename_inside() {
        assert_eq!(renamed_package_directory(Path::new("/"), "New Name"), None);
    }
}
