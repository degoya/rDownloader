//! Reading, parsing and comparing the version an external tool reports about itself
//! (RD-102-03).
//!
//! Three things make this its own module rather than a helper next to the settings page.
//!
//! **Tool versions are not SemVer, and pretending otherwise loses.** yt-dlp publishes dates
//! (`2024.08.06`, and a nightly appends a fourth component), FFmpeg publishes `6.1.1` with a
//! distribution suffix glued on (`6.1.1-3ubuntu5`) or a git description with no numbers at all
//! (`N-113522-g8b0a3d5c`), Streamlink and gallery-dl publish plain triples. So the comparison
//! is over a list of numbers of any length, padded with zeros, and a suffix is a pre-release
//! only when it *says* it is — `rc`, `beta`, `dev` and friends. Anything else is packaging
//! metadata and must not make a build sort below the release it is.
//!
//! **An unreadable version is not an old version.** Parsing returns `None` rather than a
//! zero, because "we could not tell" and "it is ancient" lead to opposite decisions: the first
//! must warn, the second may block. [`crate::compat`] keeps them apart.
//!
//! **Asking costs a process.** The settings page lists eight tools, and before this module the
//! status endpoint spawned eight processes on every load. The answer only changes when the
//! binary does, so it is cached against the file's modification time and size — a rebuilt,
//! replaced or newly activated binary invalidates its own entry without anything having to
//! remember to.

use std::{
    cmp::Ordering,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{LazyLock, RwLock},
    time::SystemTime,
};

/// Suffixes that mark a build as coming *before* the release it names.
///
/// Everything else after the numbers — `-3ubuntu5`, `+deb12`, `_1` — is packaging metadata
/// and is ignored for ordering. Treating it as a pre-release would sort a distribution's
/// build of 6.1.1 below plain 6.1.1 and make a `min_version` of 6.1.1 unreachable on Debian.
const PRE_RELEASE_MARKERS: [&str; 7] = ["rc", "alpha", "beta", "dev", "pre", "nightly", "snapshot"];

/// The longest raw line kept for display. The status endpoint shows this verbatim.
const MAX_RAW_CHARS: usize = 120;

/// How long a `--version` call may take before it is given up on.
const VERSION_TIMEOUT_SECONDS: u64 = 5;

/// Entries kept in the version cache before it is dropped wholesale.
///
/// The cache exists to stop one settings page from spawning eight processes, not to remember
/// every binary a long-running process ever looked at. Well above the number of tools this
/// application knows, so in practice it never trips.
const MAX_CACHE_ENTRIES: usize = 64;

/// A tool version parsed into comparable parts.
///
/// Ordering compares the numeric components position by position, padding the shorter list
/// with zeros, so `6.1` and `6.1.0` are equal and `2024.8.6.232855` is newer than `2024.8.6`.
/// A pre-release sorts below the same numbers without one.
///
/// Equality is defined by that same comparison rather than derived, so the text a tool
/// happened to print never decides whether two versions are the same one.
#[derive(Clone, Debug)]
pub struct ToolVersion {
    numbers: Vec<u64>,
    pre_release: Option<String>,
    text: String,
}

impl ToolVersion {
    /// Parses one version token, e.g. `2024.08.06`, `v1.66.0`, `n6.1.1` or `6.1.1-3ubuntu5`.
    ///
    /// Returns `None` when the token carries no leading number at all, which is the honest
    /// answer for a git description like `N-113522-g8b0a3d5c`.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return None;
        }
        // `v1.66.0` (rclone) and `n6.1.1` (some FFmpeg builds) prefix the number with a
        // letter. Only strip it when a digit actually follows, so `nightly` stays unparsed.
        let body = match trimmed.strip_prefix(['v', 'V', 'n', 'N']) {
            Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => rest,
            _ => trimmed,
        };
        let head_length = body
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(body.len());
        let (head, tail) = body.split_at(head_length);
        let numbers: Vec<u64> = head
            .split('.')
            .filter(|part| !part.is_empty())
            .map(str::parse::<u64>)
            .collect::<Result<_, _>>()
            .ok()?;
        if numbers.is_empty() {
            return None;
        }
        let suffix = tail
            .trim_start_matches(['-', '_', '+', '.', '~'])
            .to_lowercase();
        let pre_release = PRE_RELEASE_MARKERS
            .iter()
            .any(|marker| suffix.starts_with(marker))
            .then_some(suffix);
        Some(Self {
            numbers,
            pre_release,
            text: trimmed.to_owned(),
        })
    }

    /// The token this was parsed from, as the tool wrote it.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the suffix marks this as a build published before the release it names.
    #[cfg(test)]
    #[must_use]
    pub fn is_pre_release(&self) -> bool {
        self.pre_release.is_some()
    }
}

impl std::fmt::Display for ToolVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.text)
    }
}

impl Ord for ToolVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let width = self.numbers.len().max(other.numbers.len());
        for index in 0..width {
            let left = self.numbers.get(index).copied().unwrap_or(0);
            let right = other.numbers.get(index).copied().unwrap_or(0);
            match left.cmp(&right) {
                Ordering::Equal => {}
                decided => return decided,
            }
        }
        match (&self.pre_release, &other.pre_release) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(left), Some(right)) => left.cmp(right),
        }
    }
}

impl PartialOrd for ToolVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ToolVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ToolVersion {}

/// What one tool reported about itself.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DetectedVersion {
    /// The first line the tool printed, trimmed and capped. Shown verbatim in the interface,
    /// because a line this could not parse is still the most useful thing to show a person.
    pub raw: Option<String>,
    /// The comparable reading of that line, when there was one.
    pub parsed: Option<ToolVersion>,
}

impl DetectedVersion {
    /// The best text to show: the normalised version when it parsed, else the raw line.
    #[must_use]
    pub fn display(&self) -> Option<String> {
        self.parsed
            .as_ref()
            .map(|version| version.text().to_owned())
            .or_else(|| self.raw.clone())
    }
}

/// Turns a tool's `--version` output into a comparable version.
///
/// Per tool rather than generically, because the prefix is what tells a version line from a
/// banner, a usage message or an error. `ffmpeg version 6.1.1` and `Usage: ffmpeg [options]`
/// both start with `ffmpeg`; only one of them is answering the question.
#[must_use]
pub fn parse_output(tool: &str, output: &str) -> Option<ToolVersion> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let tokens: Vec<&str> = line.split_whitespace().collect();
    match tool {
        // yt-dlp prints the bare version and nothing else.
        "yt-dlp" => ToolVersion::parse(tokens.first()?),
        // `gallery-dl 1.27.1`, `streamlink 6.7.4`, `rclone v1.66.0`, `Apprise v1.13.1`.
        "gallery-dl" | "streamlink" | "rclone" | "apprise" => {
            names(tokens.first()?, tool).then_some(())?;
            ToolVersion::parse(tokens.get(1)?)
        }
        // `UNRAR 6.24 freeware`, or `RAR 7.01` when the full `rar` is configured as the unpacker:
        // the same code, and the same version floor.
        "unrar" => {
            let name = tokens.first()?;
            (names(name, "unrar") || names(name, "rar")).then_some(())?;
            ToolVersion::parse(tokens.get(1)?)
        }
        // `ffmpeg version 6.1.1-3ubuntu5 Copyright (c) …`
        "ffmpeg" | "ffprobe" => {
            names(tokens.first()?, tool).then_some(())?;
            (*tokens.get(1)? == "version").then_some(())?;
            ToolVersion::parse(tokens.get(2)?)
        }
        // `7-Zip (z) 23.01 (x64) : Copyright …`. The program name itself starts with a digit,
        // so it has to be skipped rather than parsed.
        "7z" => {
            names(tokens.first()?, "7-zip").then_some(())?;
            tokens
                .iter()
                .skip(1)
                .find_map(|token| ToolVersion::parse(token))
        }
        _ => tokens.iter().find_map(|token| ToolVersion::parse(token)),
    }
}

/// Whether `token` is the tool's own name, ignoring case.
fn names(token: &str, tool: &str) -> bool {
    token.eq_ignore_ascii_case(tool)
}

/// One cached answer, valid for as long as the binary behind it is unchanged.
struct CacheEntry {
    modified: Option<SystemTime>,
    size: u64,
    detected: DetectedVersion,
}

static CACHE: LazyLock<RwLock<HashMap<PathBuf, CacheEntry>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Runs `path --version` and reads what it says, answering from the cache when the binary has
/// not changed since it was last asked.
///
/// The cache key is the path together with the file's modification time and size, so
/// replacing a binary in place, activating another managed version or upgrading a system
/// package all invalidate the entry on their own. A file that cannot be stat'ed answers
/// "nothing detected" without spawning anything.
pub async fn detect(tool: &str, path: &Path) -> DetectedVersion {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return DetectedVersion::default();
    };
    let modified = metadata.modified().ok();
    let size = metadata.len();
    if let Ok(cache) = CACHE.read()
        && let Some(entry) = cache.get(path)
        && entry.modified == modified
        && entry.size == size
    {
        return entry.detected.clone();
    }
    let detected = run(tool, path).await;
    if let Ok(mut cache) = CACHE.write() {
        if cache.len() >= MAX_CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(
            path.to_path_buf(),
            CacheEntry {
                modified,
                size,
                detected: detected.clone(),
            },
        );
    }
    detected
}

/// Forgets every cached answer. Called when a change could not have moved a file's timestamp.
#[cfg(any(test, feature = "test-support"))]
pub fn clear_cache() {
    if let Ok(mut cache) = CACHE.write() {
        cache.clear();
    }
}

/// Whether the tool answers with its banner when started without arguments, and knows no
/// version flag at all.
///
/// `unrar` and 7-Zip read `--version` as an unknown switch and exit with a usage error, so
/// asking them that way never learned anything; started bare, both print their banner and
/// the usage text. Their exit status is not read either: the banner parser only accepts the
/// tool's own first line, and a usage error is not a reason to forget a version that the
/// banner stated. The archive tools' version is a security floor (review 2026-09-28,
/// finding 5), so reading it matters more here than anywhere else.
fn answers_with_banner(tool: &str) -> bool {
    matches!(tool, "unrar" | "7z")
}

/// Spawns the tool's version flag and reads the first line of what it prints.
async fn run(tool: &str, path: &Path) -> DetectedVersion {
    let mut command = tokio::process::Command::new(path);
    // FFmpeg and ffprobe take a single dash, the archive tools none at all; everything else
    // here takes two.
    if tool.starts_with("ff") {
        command.arg("-version");
    } else if !answers_with_banner(tool) {
        command.arg("--version");
    }
    let output = crate::process::run_to_output(
        &mut command,
        std::time::Duration::from_secs(VERSION_TIMEOUT_SECONDS),
    )
    .await;
    // A timeout and a binary that will not start are the same answer here: nothing was
    // learned, so nothing is claimed. The caller must not read that as "an old version".
    let Ok(Ok(output)) = output else {
        return DetectedVersion::default();
    };
    if !output.status.success() && !answers_with_banner(tool) {
        return DetectedVersion::default();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // The first line that says anything: 7-Zip opens its banner with an empty one.
    let raw = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(MAX_RAW_CHARS).collect::<String>());
    DetectedVersion {
        parsed: parse_output(tool, &text),
        raw,
    }
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod tests;
