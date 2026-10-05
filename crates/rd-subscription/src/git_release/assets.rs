//! Which files of a release the options select, and what its checksum files say.

use std::collections::BTreeMap;

use rd_core::{GitArchitecture, GitPlatform, GitReleaseOptions};

use super::is_sha256;

/// Whether the options select a file of this name.
#[must_use]
pub fn selects(options: &GitReleaseOptions, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if !options.asset_patterns.is_empty()
        && !options
            .asset_patterns
            .iter()
            .any(|pattern| glob_matches(pattern, &lower))
    {
        return false;
    }
    // A checksum file is no build for any platform, whatever its name repeats of the file it
    // covers; it is read for the checksums either way.
    if !options.platforms.is_empty()
        && (checksum_file(name).is_some()
            || !platform_of(&lower).is_some_and(|platform| options.platforms.contains(&platform)))
    {
        return false;
    }
    if !options.architectures.is_empty()
        && let Some(architecture) = architecture_of(&lower)
        && !options.architectures.contains(&architecture)
    {
        return false;
    }
    true
}

/// Whether `pattern` (`*`, `?`) matches the whole of `name_lowercase`, ignoring case.
///
/// Matched directly rather than through a regular expression built and compiled for every
/// asset of every release (audit 1.9.1, INTAKE-10). The semantics are the ones that expression
/// had: `*` is any run of characters and `?` any one character, a line break excepted, and
/// everything else stands for itself.
#[must_use]
pub fn glob_matches(pattern: &str, name_lowercase: &str) -> bool {
    let pattern: Vec<char> = pattern.trim().to_lowercase().chars().collect();
    let name: Vec<char> = name_lowercase.chars().collect();
    let (mut at_pattern, mut at_name) = (0, 0);
    // The last `*` seen and the first name character it does not cover yet: where a failed
    // literal goes back to, letting that star take one character more.
    let mut star: Option<(usize, usize)> = None;
    while at_name < name.len() {
        let current = name[at_name];
        match pattern.get(at_pattern).copied() {
            Some('*') => {
                star = Some((at_pattern, at_name));
                at_pattern += 1;
            }
            Some('?') if current != '\n' => {
                at_pattern += 1;
                at_name += 1;
            }
            Some(literal) if literal != '*' && literal != '?' && literal == current => {
                at_pattern += 1;
                at_name += 1;
            }
            _ => match star {
                Some((star_at, covered)) if name[covered] != '\n' => {
                    star = Some((star_at, covered + 1));
                    at_pattern = star_at + 1;
                    at_name = covered + 1;
                }
                _ => return false,
            },
        }
    }
    pattern[at_pattern..]
        .iter()
        .all(|character| *character == '*')
}

/// Whether `token` stands in `name` as a word of its own — not inside a longer one. A token
/// written as an extension (`.exe`) has to end the name: `.pkg` is a macOS installer, while
/// `.pkg.tar.zst` is an Arch Linux package.
fn has_word(name: &str, token: &str) -> bool {
    if token.starts_with('.') {
        return name.ends_with(token);
    }
    name.match_indices(token).any(|(start, _)| {
        let before = name[..start].chars().next_back();
        let after = name[start + token.len()..].chars().next();
        !before.is_some_and(|character| character.is_ascii_alphanumeric())
            && !after.is_some_and(|character| character.is_ascii_alphanumeric())
    })
}

/// The platform a file name names, by word or by a telling extension.
#[must_use]
pub fn platform_of(name_lowercase: &str) -> Option<GitPlatform> {
    const TABLE: &[(GitPlatform, &[&str])] = &[
        (
            GitPlatform::Windows,
            &[
                "windows", "win", "win32", "win64", "msvc", "mingw", ".exe", ".msi", ".msix",
            ],
        ),
        (
            GitPlatform::Macos,
            &["macos", "darwin", "osx", "mac", "apple", ".dmg", ".pkg"],
        ),
        (
            GitPlatform::Linux,
            &[
                "linux",
                "musl",
                ".appimage",
                ".deb",
                ".rpm",
                ".flatpak",
                ".snap",
                ".pkg.tar.zst",
                ".pkg.tar.xz",
            ],
        ),
    ];
    TABLE.iter().find_map(|(platform, tokens)| {
        tokens
            .iter()
            .any(|token| has_word(name_lowercase, token))
            .then_some(*platform)
    })
}

/// The architecture a file name names. The 64-bit spellings are tried first, because the
/// 32-bit ones are their prefixes (`x86` in `x86_64`, `arm` in `arm64`).
#[must_use]
pub fn architecture_of(name_lowercase: &str) -> Option<GitArchitecture> {
    const TABLE: &[(GitArchitecture, &[&str])] = &[
        (GitArchitecture::Aarch64, &["aarch64", "arm64", "armv8"]),
        (
            GitArchitecture::X86_64,
            &["x86_64", "x86-64", "amd64", "x64", "win64", "64bit"],
        ),
        (
            GitArchitecture::X86,
            &[
                "i386", "i486", "i586", "i686", "x86", "ia32", "win32", "32bit", "386",
            ],
        ),
        (
            GitArchitecture::Arm,
            &["armv7", "armv7l", "armhf", "armv6", "arm"],
        ),
    ];
    TABLE.iter().find_map(|(architecture, tokens)| {
        tokens
            .iter()
            .any(|token| has_word(name_lowercase, token))
            .then_some(*architecture)
    })
}

/// What a checksum file covers, by its name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChecksumFile {
    /// A list for every file of the release: `SHA256SUMS`, `checksums.txt`, `…_checksums.txt`.
    List,
    /// The checksum of one file, `<file>.sha256`.
    Single(String),
}

/// Whether a release file is a SHA-256 checksum file, and of what.
#[must_use]
pub fn checksum_file(name: &str) -> Option<ChecksumFile> {
    let lower = name.to_ascii_lowercase();
    for suffix in [".sha256", ".sha256sum"] {
        if lower.ends_with(suffix)
            && let Some(covered) = name.get(..name.len() - suffix.len())
            && !covered.is_empty()
        {
            return Some(ChecksumFile::Single(covered.to_owned()));
        }
    }
    let list = matches!(
        lower.as_str(),
        "sha256sums" | "sha256sums.txt" | "sha256sum.txt" | "checksums.txt" | "checksums.sha256"
    ) || lower.ends_with("_checksums.txt")
        || lower.ends_with("-checksums.txt")
        || lower.ends_with(".sha256sums");
    list.then_some(ChecksumFile::List)
}

/// Reads a SHA-256 checksum file: `<hex>  <name>` (GNU, `*` for binary mode), `SHA256 (<name>)
/// = <hex>` (BSD), or a lone `<hex>`, which is filed under the empty name.
///
/// Names are kept as their last path segment, since lists are often written from a build
/// directory (`./dist/tool.tar.gz`).
#[must_use]
pub fn parse_checksums(text: &str) -> BTreeMap<String, String> {
    let mut sums = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(rest) = line.strip_prefix("SHA256 (")
            && let Some((name, hex)) = rest.split_once(") = ")
            && is_sha256(hex.trim())
        {
            sums.insert(base_name(name), hex.trim().to_ascii_lowercase());
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(hex) = parts.next().filter(|hex| is_sha256(hex)) else {
            continue;
        };
        let name = parts.next().map(str::trim).unwrap_or_default();
        let name = name.strip_prefix('*').unwrap_or(name);
        sums.insert(base_name(name), hex.to_ascii_lowercase());
    }
    sums
}

fn base_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_owned()
}
