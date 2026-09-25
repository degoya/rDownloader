//! What a release name says about the episode inside it (RD-110-21).
//!
//! A release page names the same episode again and again: another group's release the same
//! evening, a second quality the next day, a rerip three weeks later. A subscription that
//! recognises items by address enqueues every one of them, which is the defect this module
//! exists to prevent. So recognition is by *name*: the series, the season and the episode,
//! and those three are what "already had" means.
//!
//! **Nothing is guessed.** The quality and the language come from the closed token lists
//! RD-110-18 established in `rd_collector::mirrors` — reused rather than copied, so
//! `Show.2160.mkv` is not 2160p here either and a token added there is added once.
//!
//! **A name that is not an episode has no episode key.** Recognition then falls back to the
//! address, which is what every other subscription kind already does. A wrong key is far
//! worse than no key: it would silence a genuinely new release forever, and nothing would
//! say why.

use rd_collector::{language_of, quality_of};

/// Above this, a number in the season position is a year or a resolution, not a season.
const MAX_SEASON: u32 = 99;
/// Above this, a number in the episode position is not an episode.
const MAX_EPISODE: u32 = 999;

/// What a release name spelled out.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReleaseName {
    /// The part before the season marker, normalised: lower case, single spaces, no
    /// punctuation. Empty when the name begins with its marker.
    pub series: String,
    pub season: Option<u32>,
    /// The first episode the marker names. A season pack has a season and no episode.
    pub episode: Option<u32>,
    /// The marker in one spelling, `s01e02` or `s01e02e03` or `s01`, whatever the name used.
    /// This rather than the two numbers is what the key is built from, so a double episode
    /// is its own thing instead of colliding with the single one that opens it.
    pub marker: Option<String>,
    /// The quality token, as `rd_collector::quality_of` names it.
    pub quality: Option<String>,
    /// The language token, as `rd_collector::language_of` names it.
    pub language: Option<String>,
}

impl ReleaseName {
    /// The key every release of this episode shares, or `None` when the name is not one.
    ///
    /// Deliberately the series and the marker and nothing else. Adding the quality would
    /// make the 720p and the 1080p release two episodes, which is exactly the duplication a
    /// release-page subscription suffers from; somebody who wants that says so through
    /// `Subscription::every_release` and gets the address as the key instead.
    #[must_use]
    pub fn key(&self) -> Option<String> {
        let marker = self.marker.as_deref()?;
        if self.series.is_empty() {
            return None;
        }
        Some(format!("{}|{marker}", self.series))
    }

    /// The vertical resolution the quality token stands for, so the existing `min_height`
    /// filter judges a release name without a second mechanism.
    #[must_use]
    pub fn height(&self) -> Option<u32> {
        // Every name in the closed list is written `<n>p`.
        self.quality.as_deref()?.trim_end_matches('p').parse().ok()
    }
}

/// Reads a release name.
///
/// The three spellings that actually occur, tried on every token until one matches:
/// `S01E02` (with further `E` parts for a double episode), `1x02`, and a bare `S01` for a
/// season pack. A name carrying none of them is not refused — it comes back with a series
/// and no marker, and [`ReleaseName::key`] then says `None`.
#[must_use]
pub fn parse(name: &str) -> ReleaseName {
    let tokens: Vec<String> = name
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let found = tokens
        .iter()
        .enumerate()
        .find_map(|(index, token)| marker_of(token).map(|marker| (index, marker)));
    let (cut, marker) = match found {
        Some((index, marker)) => (index, Some(marker)),
        None => (tokens.len(), None),
    };
    ReleaseName {
        series: tokens[..cut].join(" "),
        season: marker.as_ref().map(|marker| marker.season),
        episode: marker
            .as_ref()
            .and_then(|marker| marker.episodes.first().copied()),
        marker: marker.as_ref().map(Marker::spelled),
        quality: quality_of(name),
        language: language_of(name),
    }
}

/// A season and the episodes one token named.
struct Marker {
    season: u32,
    episodes: Vec<u32>,
}

impl Marker {
    /// The one spelling every recognised form is written back as.
    fn spelled(&self) -> String {
        let mut out = format!("s{:02}", self.season);
        for episode in &self.episodes {
            out.push_str(&format!("e{episode:02}"));
        }
        out
    }
}

/// Reads one token as a season/episode marker.
///
/// A four-digit number after `s` is a year in a name like `Show.S2026.1080p` far more often
/// than a season, and seasons that high do not exist, so [`MAX_SEASON`] is what keeps a year
/// out. A bare number is never a season: `1x02` is recognised because the `x` holds the two
/// halves together in one token, and nothing else does.
fn marker_of(token: &str) -> Option<Marker> {
    if let Some(rest) = token.strip_prefix('s') {
        return se_marker(rest);
    }
    // `1x02`, and only with both halves present.
    let (season, episode) = token.split_once('x')?;
    let season = number(season, MAX_SEASON)?;
    let episode = number(episode, MAX_EPISODE)?;
    Some(Marker {
        season,
        episodes: vec![episode],
    })
}

/// `01`, `01e02` or `01e02e03`, the part of an `s…` token after its `s`.
fn se_marker(rest: &str) -> Option<Marker> {
    let mut parts = rest.split('e');
    let season = number(parts.next()?, MAX_SEASON)?;
    let mut episodes = Vec::new();
    for part in parts {
        episodes.push(number(part, MAX_EPISODE)?);
    }
    Some(Marker { season, episodes })
}

/// A run of digits no larger than `limit`, or nothing.
fn number(text: &str, limit: u32) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value: u32 = text.parse().ok()?;
    (value <= limit).then_some(value)
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod release_tests;
