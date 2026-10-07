//! The notes of the update manifest (RD-1150-02): the version's section of `RELEASE-NOTES.md`,
//! written for users, and the anchor of its `CHANGELOG.md` section for the full changes.
//!
//! `RELEASE-NOTES.md` keeps a `## X.Y.Z` section per version, newest first: up to
//! [`MAX_POINTS`] `- ` points of what a user notices, or [`MAINTENANCE`] alone. Until 1.14 the
//! notes were the CHANGELOG section's headlines, and the 1.13.0 dialog read "Narrower visibility
//! in every crate (RD-1120-12, CR-9)" and an open "**A file waiting …". The rules here are
//! `scripts/release-notes.sh --check`'s, which the preflight and the release's docs gate run;
//! this refuses the same section once more when the manifest is built, so a rule broken past
//! both never reaches an installation.

use anyhow::{Result, bail};

/// Most points per version.
const MAX_POINTS: usize = 8;
/// Longest point, in characters of plain text.
const MAX_POINT_CHARS: usize = 200;
/// The one sentence of a version with nothing a user notices.
const MAINTENANCE: &str = "Maintenance release: internal changes only, no change in behaviour.";

/// The version's points as the manifest carries them, one `- point` per line, or the
/// maintenance sentence; `None` when `RELEASE-NOTES.md` has no section for it. A pre-release
/// falls back to its release's section. A section the rules refuse, or one still marked as a
/// draft, is an error.
pub(super) fn user_notes(text: &str, version: &semver::Version) -> Result<Option<String>> {
    let Some((found, body)) = candidates(version)
        .iter()
        .find_map(|candidate| section(text, candidate).map(|body| (candidate.clone(), body)))
    else {
        return Ok(None);
    };
    let parsed = parse(body);
    if parsed.draft {
        bail!("RELEASE-NOTES.md: the section {found} is still marked as a draft");
    }
    let broken = problems(&parsed);
    if !broken.is_empty() {
        bail!(
            "RELEASE-NOTES.md: the section {found} breaks the rules: {}",
            broken.join("; ")
        );
    }
    Ok(Some(match parsed.sentence {
        Some(sentence) => sentence,
        None => parsed
            .points
            .iter()
            .map(|point| format!("- {point}"))
            .collect::<Vec<_>>()
            .join("\n"),
    }))
}

/// GitHub's anchor of the version's `## [X.Y.Z] - YYYY-MM-DD` heading in `CHANGELOG.md`
/// (`1150---2026-10-10`); a pre-release falls back to its release's heading.
pub(super) fn changelog_anchor(changelog: &str, version: &semver::Version) -> Option<String> {
    candidates(version).iter().find_map(|candidate| {
        let marker = format!("## [{candidate}]");
        changelog
            .lines()
            .find(|line| line.starts_with(&marker))
            .map(|line| github_anchor(&line[3..]))
    })
}

/// GitHub's heading anchor: lower case, every character but letters, digits, `-`, `_` and the
/// space dropped, each space a hyphen. `[1.15.0] - 2026-10-10` becomes `1150---2026-10-10`.
fn github_anchor(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric() || matches!(*character, '-' | '_' | ' '))
        .map(|character| if character == ' ' { '-' } else { character })
        .collect()
}

fn candidates(version: &semver::Version) -> Vec<String> {
    let base = format!("{}.{}.{}", version.major, version.minor, version.patch);
    let exact = version.to_string();
    if exact == base {
        vec![exact]
    } else {
        vec![exact, base]
    }
}

/// The lines under `## <version>` up to the next heading of that level or above.
fn section<'a>(text: &'a str, version: &str) -> Option<&'a str> {
    let mut offset = 0;
    let mut start = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if let Some(begin) = start {
            if trimmed.starts_with("## ") || trimmed.starts_with("# ") {
                return Some(&text[begin..offset]);
            }
        } else if trimmed.strip_prefix("## ").map(str::trim) == Some(version) {
            start = Some(offset + line.len());
        }
        offset += line.len();
    }
    start.map(|begin| &text[begin..])
}

#[derive(Debug, Default)]
struct Parsed {
    points: Vec<String>,
    /// The section is one paragraph rather than points: the maintenance sentence, or a mistake.
    sentence: Option<String>,
    draft: bool,
}

/// Points (`- ` or `* ` lines, indented lines continuing them) and paragraph text, as plain
/// text; HTML comments are left out, and one saying `draft` marks the section.
fn parse(body: &str) -> Parsed {
    let mut parsed = Parsed::default();
    let mut paragraph = Vec::new();
    let mut in_comment = false;
    for line in body.lines() {
        if in_comment || line.trim_start().starts_with("<!--") {
            parsed.draft |= line.to_ascii_lowercase().contains("draft");
            in_comment = !line.contains("-->");
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        if let Some(point) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            parsed.points.push(point.trim().to_owned());
        } else if line.starts_with(' ')
            && let Some(last) = parsed.points.last_mut()
        {
            last.push(' ');
            last.push_str(line.trim());
        } else {
            paragraph.push(line.trim());
        }
    }
    for point in &mut parsed.points {
        *point = plain(point);
    }
    if !paragraph.is_empty() {
        parsed.sentence = Some(plain(&paragraph.join(" ")));
    }
    parsed
}

/// Bold and link targets dropped, runs of white space one space.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let linked = rest[open..].find("](").and_then(|close| {
            let target = &rest[open + close + 2..];
            target
                .find(')')
                .map(|end| (&rest[open + 1..open + close], open + close + 2 + end + 1))
        });
        match linked {
            Some((label, after)) => {
                out.push_str(&rest[..open]);
                out.push_str(label);
                rest = &rest[after..];
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    out.replace("**", "")
        .replace("__", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every rule the section breaks, worded for the release's author.
fn problems(parsed: &Parsed) -> Vec<String> {
    let mut found = Vec::new();
    match (&parsed.sentence, parsed.points.len()) {
        (None, 0) => found.push("has no points".to_owned()),
        (Some(sentence), 0) => {
            if sentence != MAINTENANCE {
                found.push(format!(
                    "is not a list of points; a version without visible change says \"{MAINTENANCE}\""
                ));
            }
        }
        (Some(_), _) => found.push("mixes points with other text".to_owned()),
        (None, count) if count > MAX_POINTS => {
            found.push(format!("has {count} points, at most {MAX_POINTS}"));
        }
        _ => {}
    }
    for (index, point) in parsed.points.iter().enumerate() {
        found.extend(
            point_problems(point)
                .into_iter()
                .map(|problem| format!("point {} {problem}", index + 1)),
        );
    }
    found
}

fn point_problems(point: &str) -> Vec<String> {
    let mut found = Vec::new();
    let length = point.chars().count();
    if length > MAX_POINT_CHARS {
        found.push(format!("is {length} characters, at most {MAX_POINT_CHARS}"));
    }
    if !point.ends_with(['.', '!', '?']) {
        found.push("does not end a sentence".to_owned());
    }
    // The umlauts and the sharp s: German text, as the plugin notes' check reads it.
    if point.contains([
        '\u{e4}', '\u{f6}', '\u{fc}', '\u{c4}', '\u{d6}', '\u{dc}', '\u{df}',
    ]) {
        found.push("is not English".to_owned());
    }
    if names_a_job(point) {
        found.push("names a job".to_owned());
    }
    if point.split_whitespace().any(is_path) {
        found.push("names a path".to_owned());
    }
    if point.contains('`') || point.contains("::") || point.split_whitespace().any(is_snake_case) {
        found.push("names a code identifier".to_owned());
    }
    found
}

/// `RD-1150`, `CR-9`, `PL-12`: two to five capitals, a hyphen and a digit, at a word start.
fn names_a_job(text: &str) -> bool {
    let bytes = text.as_bytes();
    (0..bytes.len()).any(|start| {
        if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
            return false;
        }
        let capitals = bytes[start..]
            .iter()
            .take_while(|byte| byte.is_ascii_uppercase())
            .count();
        (2..=5).contains(&capitals)
            && bytes.get(start + capitals) == Some(&b'-')
            && bytes
                .get(start + capitals + 1)
                .is_some_and(u8::is_ascii_digit)
    })
}

/// The repository's top directories and `src`, each followed by a slash in a path.
const SOURCE_DIRECTORIES: [&str; 7] = ["crates", "plugins", "scripts", "docs", "sdk", "src", "web"];
const SOURCE_EXTENSIONS: [&str; 13] = [
    "rs", "toml", "md", "sh", "json", "wit", "ts", "vue", "yml", "yaml", "py", "txt", "sql",
];

/// A word that is a repository path: under a source directory, or with a slash and a source
/// file's extension.
fn is_path(word: &str) -> bool {
    let word =
        word.trim_matches(|character: char| matches!(character, '(' | ')' | ',' | ';' | ':' | '"'));
    let word = word.strip_suffix('.').unwrap_or(word);
    SOURCE_DIRECTORIES.iter().any(|directory| {
        word.starts_with(&format!("{directory}/")) || word.contains(&format!("/{directory}/"))
    }) || (word.contains('/')
        && word
            .rsplit_once('.')
            .is_some_and(|(_, extension)| SOURCE_EXTENSIONS.contains(&extension)))
}

/// `snake_case`: a lower-case word with an underscore between letters or digits.
fn is_snake_case(word: &str) -> bool {
    let word =
        word.trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '_');
    word.starts_with(|character: char| character.is_ascii_lowercase())
        && word.contains('_')
        && !word.ends_with('_')
        && word
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
#[path = "update_manifest_notes_tests.rs"]
mod tests;
