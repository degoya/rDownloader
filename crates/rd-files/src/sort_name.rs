//! Recognising a series episode or a film from a release name (RD-1100-08).
//!
//! Only what the name says — no online database is asked. The limits that follow from that are
//! deliberate and documented in `docs/feature-list.md`:
//!
//! * Episodes are recognised by `S01E02` (also `S01E01E02`, `S01E01-E02`, `S01E01-02`) or
//!   `1x02` (also `1x02-03`), and dated episodes by `2024.03.15` / `2024-03-15`. Absolute
//!   numbering (anime) and season packs without an episode number are not recognised.
//! * A film needs a year: `Film.2010.1080p…` or `Film (2010)`. A name with no year is no film.
//! * The show or film name is the text before the marker, with `.` and `_` read as spaces; a
//!   year at its end becomes `year`. Punctuation the release name dropped stays dropped.
//! * The episode title is the text after the marker up to the first quality tag (resolution,
//!   source, codec, `PROPER`, …).

use std::sync::LazyLock;

use rd_core::SortKind;
use regex::Regex;

/// Video files a sort places; everything else is a companion or stays where it is.
pub const SORT_VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "wmv", "mpg", "mpeg", "ts", "m2ts", "webm", "flv",
];

/// Files that travel with their video: subtitles and the NFO.
pub const SORT_COMPANION_EXTENSIONS: &[&str] =
    &["srt", "sub", "idx", "ass", "ssa", "vtt", "sup", "nfo"];

/// What a release name was recognised as, with the values a sort template can use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortMatch {
    pub kind: SortKind,
    /// The show of an episode.
    pub show: Option<String>,
    pub season: Option<u32>,
    /// One number, or several for a multi-episode file, ascending.
    pub episodes: Vec<u32>,
    /// The air date of a dated episode: year, month, day.
    pub date: Option<(u32, u32, u32)>,
    /// The episode title, when the name carries one.
    pub title: Option<String>,
    /// The title of a film.
    pub movie: Option<String>,
    /// A film's year, a dated episode's year, or the year a show name ended with.
    pub year: Option<u32>,
    /// `720p`, `1080p`, `2160p`, …
    pub resolution: Option<String>,
    /// `BluRay`, `WEB-DL`, `HDTV`, …
    pub source: Option<String>,
}

static EPISODE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?P<show>.*?)\s*-?\s*\bs(?P<season>\d{1,2})\s?e(?P<first>\d{1,3})(?P<more>(?:\s?-?\s?e\d{1,3}|-\d{1,3})*)\b(?P<rest>.*)$",
    )
    .ok()
});

static CROSS: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?P<show>.*?)\s*-?\s*\b(?P<season>\d{1,2})x(?P<first>\d{2,3})(?P<more>(?:-(?:\d{1,2}x)?\d{2,3})*)\b(?P<rest>.*)$",
    )
    .ok()
});

static DATED: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<show>.*?)\s*-?\s*\b(?P<year>(?:19|20)\d{2})[\s-](?P<month>\d{2})[\s-](?P<day>\d{2})\b(?P<rest>.*)$",
    )
    .ok()
});

/// Recognises `name` — a file name, a folder name or a package name.
///
/// A trailing video or companion extension is ignored. `None` when the name is neither an
/// episode nor a film by the rules above; such a file is not sorted.
#[must_use]
pub fn recognize_release(name: &str) -> Option<SortMatch> {
    let words = words(strip_extension(name.trim()));
    if words.is_empty() {
        return None;
    }
    episode(&words)
        .or_else(|| dated(&words))
        .or_else(|| movie(&words))
}

/// The values of a match by field name, as a preview shows them.
#[must_use]
pub fn sort_values(found: &SortMatch) -> Vec<(&'static str, String)> {
    let mut values = Vec::new();
    let mut text = |field: &'static str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            values.push((field, value));
        }
    };
    text("show", found.show.clone());
    text("movie", found.movie.clone());
    text("season", found.season.map(|season| season.to_string()));
    text(
        "episode",
        (!found.episodes.is_empty()).then(|| {
            found
                .episodes
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("-")
        }),
    );
    text(
        "date",
        found
            .date
            .map(|(year, month, day)| format!("{year:04}-{month:02}-{day:02}")),
    );
    text("year", found.year.map(|year| year.to_string()));
    text("title", found.title.clone());
    text("resolution", found.resolution.clone());
    text("source", found.source.clone());
    values
}

/// The extension of `name` when it is one a sort knows, lower-cased.
#[must_use]
pub fn sort_extension(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    let lower = extension.to_ascii_lowercase();
    (SORT_VIDEO_EXTENSIONS.contains(&lower.as_str())
        || SORT_COMPANION_EXTENSIONS.contains(&lower.as_str()))
    .then_some(lower)
}

fn strip_extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() && sort_extension(name).is_some() => stem,
        _ => name,
    }
}

/// `.` and `_` read as spaces, runs of space collapsed, a leading `[tag]` dropped.
fn words(name: &str) -> String {
    let spaced: String = name
        .chars()
        .map(|character| match character {
            '.' | '_' => ' ',
            other => other,
        })
        .collect();
    let mut text = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    while text.starts_with('[') {
        match text.find(']') {
            Some(end) => text = text[end + 1..].trim_start().to_owned(),
            None => break,
        }
    }
    text
}

fn episode(words: &str) -> Option<SortMatch> {
    let captures = EPISODE
        .as_ref()
        .and_then(|pattern| pattern.captures(words))
        .or_else(|| CROSS.as_ref().and_then(|pattern| pattern.captures(words)))?;
    let (show, show_year) = show_name(captures.name("show")?.as_str())?;
    let season = captures.name("season")?.as_str().parse().ok()?;
    let first: u32 = captures.name("first")?.as_str().parse().ok()?;
    let mut episodes = vec![first];
    if let Some(more) = captures.name("more") {
        // `E01E02`, `E01-E02`, `E01-02`, `1x02-03`, `1x02-1x03`: every part after the first
        // number ends in an episode number; a `1x` before it repeats the season.
        let tail = more.as_str().to_ascii_lowercase();
        for part in tail.split(|character: char| {
            character == '-' || character == 'e' || character.is_whitespace()
        }) {
            let number = part.rsplit('x').next().unwrap_or(part);
            if let Ok(value) = number.parse() {
                episodes.push(value);
            }
        }
    }
    episodes.sort_unstable();
    episodes.dedup();
    let rest = captures.name("rest").map_or("", |rest| rest.as_str());
    Some(SortMatch {
        kind: SortKind::Series,
        show: Some(show),
        season: Some(season),
        episodes,
        date: None,
        title: title(rest),
        movie: None,
        year: show_year,
        resolution: resolution(rest),
        source: source(rest),
    })
}

fn dated(words: &str) -> Option<SortMatch> {
    let captures = DATED.as_ref()?.captures(words)?;
    let (show, _) = show_name(captures.name("show")?.as_str())?;
    let year: u32 = captures.name("year")?.as_str().parse().ok()?;
    let month: u32 = captures.name("month")?.as_str().parse().ok()?;
    let day: u32 = captures.name("day")?.as_str().parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let rest = captures.name("rest").map_or("", |rest| rest.as_str());
    Some(SortMatch {
        kind: SortKind::Dated,
        show: Some(show),
        season: None,
        episodes: Vec::new(),
        date: Some((year, month, day)),
        title: title(rest),
        movie: None,
        year: Some(year),
        resolution: resolution(rest),
        source: source(rest),
    })
}

fn movie(words: &str) -> Option<SortMatch> {
    let tokens: Vec<&str> = words.split(' ').collect();
    // The year is the last one before the first quality tag, and never the first word:
    // `2001.A.Space.Odyssey.1968.1080p` is a film from 1968, `Blade.Runner.2049.2017` one
    // from 2017.
    let quality = tokens
        .iter()
        .position(|token| is_quality(token))
        .unwrap_or(tokens.len());
    let (index, year) = tokens[..quality]
        .iter()
        .enumerate()
        .skip(1)
        .rev()
        .find_map(|(index, token)| year_token(token).map(|year| (index, year)))?;
    let title = tokens[..index]
        .join(" ")
        .trim_matches([' ', '-', '(', '['])
        .to_owned();
    if title.is_empty() {
        return None;
    }
    // Only what follows the year: a film called `The Web` has no source.
    let rest = tokens[index + 1..].join(" ");
    Some(SortMatch {
        kind: SortKind::Movie,
        show: None,
        season: None,
        episodes: Vec::new(),
        date: None,
        title: None,
        movie: Some(title),
        year: Some(year),
        resolution: resolution(&rest),
        source: source(&rest),
    })
}

/// The show name and a year it ended with; `None` when nothing is left of it.
fn show_name(raw: &str) -> Option<(String, Option<u32>)> {
    let mut tokens: Vec<&str> = raw
        .split(' ')
        .filter(|token| !token.is_empty() && *token != "-")
        .collect();
    let year = match tokens.as_slice() {
        [.., last] if tokens.len() > 1 => year_token(last),
        _ => None,
    };
    if year.is_some() {
        tokens.pop();
    }
    let name = tokens.join(" ").trim_matches([' ', '-']).to_owned();
    (!name.is_empty()).then_some((name, year))
}

/// The episode title: what follows the marker up to the first quality tag.
fn title(rest: &str) -> Option<String> {
    let words: Vec<&str> = rest
        .split(' ')
        .filter(|token| !token.is_empty())
        .take_while(|token| !is_quality(token))
        .collect();
    let title = words.join(" ").trim_matches([' ', '-']).to_owned();
    (!title.is_empty()).then_some(title)
}

fn year_token(token: &str) -> Option<u32> {
    let digits = token.trim_matches(['(', ')', '[', ']']);
    if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year: u32 = digits.parse().ok()?;
    (1900..=2099).contains(&year).then_some(year)
}

fn resolution(words: &str) -> Option<String> {
    words.split(' ').find_map(|token| {
        let lower = token
            .split('-')
            .next()
            .unwrap_or(token)
            .to_ascii_lowercase();
        match lower.as_str() {
            "4k" | "uhd" | "2160p" => Some("2160p".to_owned()),
            _ => {
                let digits = lower
                    .strip_suffix('p')
                    .or_else(|| lower.strip_suffix('i'))?;
                ((3..=4).contains(&digits.len())
                    && digits.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| format!("{digits}p"))
            }
        }
    })
}

/// Sources by the spellings release names use, and the one a template gets.
const SOURCES: &[(&str, &str)] = &[
    ("bluray", "BluRay"),
    ("blu-ray", "BluRay"),
    ("bdrip", "BluRay"),
    ("brrip", "BluRay"),
    ("bdremux", "BluRay"),
    ("web-dl", "WEB-DL"),
    ("webdl", "WEB-DL"),
    ("webrip", "WEBRip"),
    ("web", "WEB"),
    ("hdtv", "HDTV"),
    ("pdtv", "SDTV"),
    ("sdtv", "SDTV"),
    ("dsr", "SDTV"),
    ("dvdrip", "DVDRip"),
    ("dvd", "DVD"),
    ("dvdr", "DVD"),
    ("dvd5", "DVD"),
    ("dvd9", "DVD"),
    ("hdrip", "HDRip"),
];

fn source(words: &str) -> Option<String> {
    words.split(' ').find_map(|token| {
        let lower = token.to_ascii_lowercase();
        let without_group = lower
            .rsplit_once('-')
            .map_or(lower.as_str(), |(head, _)| head);
        SOURCES
            .iter()
            .find(|(spelling, _)| *spelling == lower || *spelling == without_group)
            .map(|(_, canonical)| (*canonical).to_owned())
    })
}

/// Tags that end a title, in any case: resolution, codec, dynamic range and audio.
const QUALITY_TAGS: &[&str] = &[
    "4k", "uhd", "8k", "x264", "x265", "h264", "h265", "hevc", "avc", "xvid", "divx", "av1", "vp9",
    "10bit", "8bit", "hdr", "hdr10", "hdr10+", "dovi", "remux", "atmos", "truehd", "flac", "eac3",
];

/// Scene tags that are also ordinary words — `Real`, `Complete`, `German` — and so end a title
/// only in the capitals a release name writes them in.
const SCENE_TAGS: &[&str] = &[
    "PROPER",
    "REPACK",
    "RERIP",
    "REAL",
    "INTERNAL",
    "LIMITED",
    "EXTENDED",
    "UNCUT",
    "REMASTERED",
    "UNRATED",
    "THEATRICAL",
    "IMAX",
    "MULTI",
    "DUAL",
    "DL",
    "GERMAN",
    "DUBBED",
    "SUBBED",
    "COMPLETE",
    "READNFO",
    "NFOFIX",
    "AMZN",
    "NF",
    "DSNP",
    "HMAX",
    "ATVP",
    "HULU",
    "DV",
    "SDR",
];

/// Audio tags arrive with their channel count glued on: `DDP5.1` reads as `DDP5 1`.
const QUALITY_PREFIXES: &[&str] = &["aac", "ac3", "dts", "ddp", "dd5", "dd2", "dd+"];

fn is_quality(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    let head = lower.split('-').next().unwrap_or(lower.as_str());
    let resolution = head
        .strip_suffix('p')
        .or_else(|| head.strip_suffix('i'))
        .is_some_and(|digits| {
            (3..=4).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit())
        });
    let scene_head = token.split('-').next().unwrap_or(token);
    resolution
        || QUALITY_TAGS.contains(&lower.as_str())
        || QUALITY_TAGS.contains(&head)
        || SCENE_TAGS.contains(&scene_head)
        || SOURCES
            .iter()
            .any(|(spelling, _)| *spelling == lower || *spelling == head)
        || QUALITY_PREFIXES
            .iter()
            .any(|prefix| head.starts_with(prefix))
}
