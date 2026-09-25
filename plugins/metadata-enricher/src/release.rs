//! Reading a film or an episode out of a release name, and — far more often — deciding that
//! there is none.
//!
//! This plugin is asked about every link, because an indexer hit's URL says nothing about what
//! the link contains: the address points at the indexer and the content is in the file name.
//! So the commonest answer by a wide margin is "this is not a film", and that answer has to be
//! free. Nothing in this module reaches outside; [`parse`] is the whole decision, and a `None`
//! from it means the guest returns an empty list without a single request.
//!
//! The rule that makes the negative cheap: a name is a release name only when it carries at
//! least one release marker — a year, a season/episode marker, or one of the quality, source
//! and codec tokens that scene and P2P naming has used for twenty years. `holiday.zip`,
//! `invoice-2024-03.pdf` and `setup.exe` carry none, and neither does the overwhelming
//! majority of what an online check resolves.

/// What a release name says about itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseName {
    /// The title, with the separators put back as spaces.
    pub title: String,
    pub year: Option<u16>,
    pub season: Option<u16>,
    pub episode: Option<u16>,
}

impl ReleaseName {
    /// Whether this is an episode rather than a film.
    ///
    /// Either half of the marker decides it. A season is the usual one, but an episode number
    /// on its own is just as conclusive: anime and a good deal of German television number
    /// straight through, so `Dragon.Ball.DAIMA.E15` carries no `S..` and is still an episode.
    /// Reading it as a film sent its title to a film catalogue and put a 1994 cinema release
    /// next to a 2024 episode (RD-108-14). What stays unknown is the season, and it stays
    /// unknown: [`ReleaseName::season`] is `None` and the guest shows no season at all rather
    /// than inventing one.
    #[must_use]
    pub const fn is_series(&self) -> bool {
        self.season.is_some() || self.episode.is_some()
    }
}

/// Extensions that settle the question before any parsing starts.
///
/// Deliberately a list of what a film is *not*, rather than a list of video containers: a
/// release often arrives as `.rar` parts, as an `.iso`, as an `.nzb` or as a folder with no
/// extension at all, and a whitelist of containers would refuse all of those. Archives are
/// therefore allowed through and caught by the marker rule instead — `holiday-photos.zip` has
/// no marker and is refused a line later, at no cost.
#[rustfmt::skip]
const NOT_A_FILM: &[&str] = &[
    // Audio.
    "mp3", "flac", "m4a", "ogg", "opus", "wav", "wma", "aac", "ape", "alac", "aiff", "mid",
    // Text and books.
    "pdf", "epub", "mobi", "azw", "azw3", "cbz", "cbr", "djvu", "doc", "docx", "odt", "rtf",
    "txt", "md", "csv", "xls", "xlsx", "ppt", "pptx",
    // Pictures.
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "svg", "tif", "tiff", "psd", "raw",
    // Programs and packages.
    "exe", "msi", "apk", "ipa", "dmg", "pkg", "deb", "rpm", "appimage", "jar", "bat", "sh",
    "dll", "so",
    // The files that travel next to a release rather than being one.
    "nfo", "sfv", "srt", "sub", "idx", "ass", "ssa", "vtt", "par2", "md5", "sha256", "sig",
    "url", "log", "xml", "json", "html", "htm", "css", "js", "ini", "cfg", "diz", "yml",
    "yaml", "toml", "conf", "sql", "sqlite", "db", "bak", "dat", "tmp",
];

/// Tokens that mark where the title ends: quality, source, codec, audio, language, edition.
///
/// A release name is title, then markers, then group. Finding the first marker finds the end
/// of the title, and finding *any* marker is what says the name is a release name at all.
#[rustfmt::skip]
const MARKERS: &[&str] = &[
    // Resolution and format.
    "2160p", "1080p", "1080i", "720p", "576p", "540p", "480p", "360p", "4k", "8k", "uhd", "hd",
    "sd", "hdr", "hdr10", "sdr", "dv", "imax", "3d",
    // Source.
    "bluray", "blueray", "bdrip", "bdremux", "brrip", "bd25", "bd50", "dvdrip", "dvdr", "dvd5",
    "dvd9", "dvd", "webrip", "webdl", "web", "hdtv", "pdtv", "sdtv", "dsr", "hdrip", "dvdscr",
    "screener", "r5", "cam", "camrip", "ts", "tc", "telesync", "telecine", "workprint",
    "remux", "vhsrip", "tvrip", "satrip", "dtheater", "uhdbd",
    // Codec and container hints.
    "x264", "x265", "h264", "h265", "avc", "hevc", "xvid", "divx", "vp9", "av1", "mpeg2",
    "10bit", "8bit", "hi10p",
    // Audio.
    "aac", "ac3", "eac3", "dts", "dtshd", "truehd", "atmos", "flac", "mp3", "dd", "ddp",
    "dd5", "ddp5", "dd2", "lpcm", "opus",
    // Language and edition.
    "german", "english", "french", "spanish", "italian", "dutch", "swedish", "danish",
    "norwegian", "finnish", "polish", "czech", "hungarian", "russian", "portuguese", "korean",
    "japanese", "chinese", "hindi", "multi", "dl", "dubbed", "subbed", "subs", "sub",
    "ac3d", "dtsd",
    // Edition and state.
    "extended", "unrated", "uncut", "uncensored", "remastered", "restored", "directors",
    "theatrical", "proper", "repack", "rerip", "internal", "limited", "festival", "anniversary",
    "criterion", "complete", "season", "staffel", "series", "boxset", "collection", "trilogy",
    "duology", "quadrilogy", "part", "disc", "cd1", "cd2", "sample", "untouched",
];

/// Titles that are not titles, whatever the rest of the name says.
const NOT_A_TITLE: &[&str] = &["sample", "www", "http", "https", "rarbg", "proof", "subs"];

/// Reads a release name, or decides there is none.
///
/// `None` is the answer for anything that is not recognisably a film or an episode, and the
/// caller must treat it as "ask nobody". That is the whole negative path: no allocation worth
/// the name, no network, no service told about a link it could not answer for.
#[must_use]
pub fn parse(file_name: &str) -> Option<ReleaseName> {
    let stem = last_segment(file_name);
    if stem.is_empty() || is_not_a_film(stem) {
        return None;
    }
    let tokens = tokenise(stem);
    if tokens.is_empty() {
        return None;
    }
    let scan = scan(&tokens);
    // No marker at all: an ordinary file name, which is what most links carry. This is the
    // branch that keeps the common case free.
    let cut = scan.cut?;
    let title = title_of(&tokens[..cut])?;
    Some(ReleaseName {
        title,
        year: scan.year,
        season: scan.season,
        episode: scan.episode,
    })
}

/// The file name without any directory in front of it, and without a query or fragment.
fn last_segment(file_name: &str) -> &str {
    let trimmed = file_name.trim();
    let trimmed = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    trimmed
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or("")
}

/// Whether the extension settles it: audio, a book, a picture, a program, a sidecar.
///
/// Numeric archive parts (`.001`, `.r07`) are looked past rather than judged, so that
/// `Some.Film.2019.1080p.BluRay.x264-GRP.part03.rar` is still the film it plainly is.
fn is_not_a_film(stem: &str) -> bool {
    let mut rest = stem;
    loop {
        let Some((head, extension)) = rest.rsplit_once('.') else {
            return false;
        };
        let extension = extension.to_ascii_lowercase();
        if NOT_A_FILM.contains(&extension.as_str()) {
            return true;
        }
        if is_archive_part(&extension) {
            rest = head;
            continue;
        }
        return false;
    }
}

/// `rar`, `zip`, `7z`, `001`, `r07`, `part2` — a wrapper, not an answer.
fn is_archive_part(extension: &str) -> bool {
    matches!(
        extension,
        "rar" | "zip" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zipx"
    ) || (extension.len() == 3 && extension.chars().all(|c| c.is_ascii_digit()))
        || extension
            .strip_prefix('r')
            .is_some_and(|rest| rest.len() == 2 && rest.chars().all(|c| c.is_ascii_digit()))
        || extension
            .strip_prefix("part")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

/// Splits a name into the words a release name is made of.
///
/// Dots, underscores, plus signs, spaces and brackets all separate; a hyphen does too, which
/// removes the trailing group without having to recognise group names. Everything is lowered
/// for matching, and the original casing is rebuilt for the title later.
fn tokenise(stem: &str) -> Vec<String> {
    stem.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

/// What the scan found: where the title ends, and the markers worth keeping.
#[derive(Default)]
struct Scan {
    cut: Option<usize>,
    year: Option<u16>,
    season: Option<u16>,
    episode: Option<u16>,
}

/// Walks the tokens once, left to right.
///
/// It does not stop at the first marker, because the year is often behind one — a German
/// release writes `Some.Film.German.2019.1080p` — and the season and the episode may be in
/// separate tokens. What it *does* fix at the first marker is where the title ended.
fn scan(tokens: &[String]) -> Scan {
    let mut found = Scan::default();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index].to_ascii_lowercase();
        if let Some((season, episode)) = season_episode(&token) {
            found.cut.get_or_insert(index);
            if found.season.is_none() {
                found.season = Some(season);
                found.episode = episode;
            }
        } else if let Some(year) = year_token(&token) {
            // `Blade.Runner.2049.2017.1080p`: a year followed by another year is part of the
            // title, not the release year. Without this the title would be "Blade Runner".
            let next_is_year = tokens
                .get(index + 1)
                .is_some_and(|next| year_token(&next.to_ascii_lowercase()).is_some());
            if next_is_year && found.cut.is_none() {
                index += 1;
                continue;
            }
            found.cut.get_or_insert(index);
            if found.year.is_none() {
                found.year = Some(year);
            }
        } else if let Some(episode) = episode_token(&token) {
            // `Show.Name.S01.E02` writes the two halves apart, and `Show.Name.E15` writes the
            // episode without ever naming a season. Both are episodes.
            //
            // The second case is only believed where an episode number can credibly stand:
            // inside the name, before any other marker has ended the title. Past that point a
            // lone `e`-and-digits token is far more often a group or an edition — the `E11` in
            // `...x264-E11` — and reading that as an episode would turn films into series.
            if found.episode.is_none() && (found.season.is_some() || found.cut.is_none()) {
                found.episode = Some(episode);
            }
            found.cut.get_or_insert(index);
        } else if MARKERS.contains(&token.as_str()) {
            found.cut.get_or_insert(index);
        }
        index += 1;
    }
    found
}

/// A four-digit year that could plausibly be a release year.
fn year_token(token: &str) -> Option<u16> {
    if token.len() != 4 || !token.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let year: u16 = token.parse().ok()?;
    (1900..=2099).contains(&year).then_some(year)
}

/// `s01e02`, `s1e2`, `s01e02e03`, `1x02`, `s01`.
fn season_episode(token: &str) -> Option<(u16, Option<u16>)> {
    if let Some(rest) = token.strip_prefix('s') {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() || digits.len() > 3 {
            return None;
        }
        let season = digits.parse().ok()?;
        let after = &rest[digits.len()..];
        let episode = after.strip_prefix('e').and_then(|tail| {
            let tail: String = tail.chars().take_while(char::is_ascii_digit).collect();
            (!tail.is_empty() && tail.len() <= 3).then(|| tail.parse().ok())?
        });
        // `s01` alone is a season pack; `s01e02` an episode. Anything else after the digits
        // means this was never a marker — `sample`, for one.
        if !after.is_empty() && episode.is_none() {
            return None;
        }
        return Some((season, episode));
    }
    let (left, right) = token.split_once('x')?;
    if left.is_empty() || left.len() > 2 || right.is_empty() || right.len() > 3 {
        return None;
    }
    if !left.chars().all(|c| c.is_ascii_digit()) || !right.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((left.parse().ok()?, right.parse().ok()))
}

/// A standalone `e02`, whose weight depends on where in the name it stands; see [`scan`].
fn episode_token(token: &str) -> Option<u16> {
    let digits = token.strip_prefix('e')?;
    if digits.is_empty() || digits.len() > 3 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The title the tokens before the first marker spell, or nothing if they spell no title.
fn title_of(tokens: &[String]) -> Option<String> {
    let words: Vec<&str> = tokens
        .iter()
        .map(String::as_str)
        .filter(|word| !NOT_A_TITLE.contains(&word.to_ascii_lowercase().as_str()))
        .collect();
    if words.is_empty() {
        return None;
    }
    let title = words.join(" ");
    // A title has to be something a person could search for: at least two characters and at
    // least one letter. `2` and `---` are neither.
    let letters = title.chars().filter(|c| c.is_alphabetic()).count();
    (title.chars().count() >= 2 && letters >= 2).then_some(title)
}

#[cfg(test)]
mod tests {
    use super::{ReleaseName, parse};

    #[test]
    fn the_release_naming_out_of_usenet_and_torrents_yields_title_and_year() {
        let parsed = parse("Some.Film.2019.1080p.BluRay.x264-GRUPPE").expect("a release name");
        assert_eq!(parsed.title, "Some Film");
        assert_eq!(parsed.year, Some(2019));
        assert!(!parsed.is_series());
    }

    #[test]
    fn the_shapes_a_film_release_actually_arrives_in_are_all_read() {
        for (name, title, year) in [
            (
                "Some.Film.2019.1080p.BluRay.x264-GRUPPE.mkv",
                "Some Film",
                2019,
            ),
            ("Some_Film_2019_720p_WEB-DL_x265.mkv", "Some Film", 2019),
            ("Some Film (2019) [1080p] [BluRay]", "Some Film", 2019),
            (
                "The.Longer.Title.Here.2004.German.DL.1080p.BluRay.x264-GRP",
                "The Longer Title Here",
                2004,
            ),
            // The year is behind a marker here, which is why the scan does not stop at the
            // first one it sees.
            (
                "Another.Film.German.2011.COMPLETE.UHD.BLURAY-GROUP",
                "Another Film",
                2011,
            ),
            (
                "Some.Film.2019.1080p.BluRay.x264-GRP.part03.rar",
                "Some Film",
                2019,
            ),
            (
                "/downloads/incoming/Some.Film.2019.1080p.WEB.h264-GRP/",
                "Some Film",
                2019,
            ),
        ] {
            let parsed = parse(name).unwrap_or_else(|| panic!("{name} should parse"));
            assert_eq!(parsed.title, title, "{name}");
            assert_eq!(parsed.year, Some(year), "{name}");
            assert!(!parsed.is_series(), "{name}");
        }
    }

    #[test]
    fn a_number_in_the_title_is_not_mistaken_for_the_year() {
        let parsed = parse("Blade.Runner.2049.2017.2160p.UHD.BluRay.x265-GRP").expect("parses");
        assert_eq!(parsed.title, "Blade Runner 2049");
        assert_eq!(parsed.year, Some(2017));
    }

    #[test]
    fn an_episode_is_recognised_as_one_with_its_season_and_number() {
        for (name, title, season, episode) in [
            (
                "Some.Show.S02E05.1080p.WEB-DL.x264-GRP",
                "Some Show",
                2,
                Some(5),
            ),
            ("Some.Show.s2e5.HDTV.x264", "Some Show", 2, Some(5)),
            ("Some.Show.2x05.720p.HDTV", "Some Show", 2, Some(5)),
            ("Some.Show.S02.E05.1080p", "Some Show", 2, Some(5)),
            // A season pack: the season is known, the episode is not, and that is a truthful
            // answer rather than a guess at episode one.
            ("Some.Show.S02.COMPLETE.1080p.WEB-DL", "Some Show", 2, None),
        ] {
            let parsed = parse(name).unwrap_or_else(|| panic!("{name} should parse"));
            assert!(parsed.is_series(), "{name}");
            assert_eq!(parsed.title, title, "{name}");
            assert_eq!(parsed.season, Some(season), "{name}");
            assert_eq!(parsed.episode, episode, "{name}");
        }
    }

    #[test]
    fn an_episode_number_without_a_season_is_an_episode_with_an_unknown_season() {
        // The two names RD-108-14 was reported with. Both were read as films, so both were
        // looked up in a film catalogue, and both came back wearing a 1994 cinema release.
        for (name, episode) in [
            (
                "Dragon.Ball.DAIMA.E15.Das.dritte.Auge.German.DL.1080p.WEB.h264-GRP",
                15,
            ),
            (
                "Dragon.Ball.DAIMA.E14.Tabu.German.DL.1080p.WEB.h264-GRP",
                14,
            ),
        ] {
            let parsed = parse(name).unwrap_or_else(|| panic!("{name} should parse"));
            assert!(parsed.is_series(), "{name} is an episode, not a film");
            assert_eq!(parsed.title, "Dragon Ball DAIMA", "{name}");
            assert_eq!(parsed.episode, Some(episode), "{name}");
            // Nothing in the name says which season, so nothing is said about it.
            assert_eq!(parsed.season, None, "{name}");
            assert_eq!(parsed.year, None, "{name}");
        }
    }

    #[test]
    fn a_season_and_episode_marker_still_carries_both_halves() {
        // The third reported name. Its season was never in doubt; what was wrong sat one layer
        // up, in which catalogue entry the title was matched against.
        let parsed =
            parse("Apollo.Has.Fallen.S02E02.DL.GERMAN.1080p.WEB.h264-GRP").expect("parses");
        assert!(parsed.is_series());
        assert_eq!(parsed.title, "Apollo Has Fallen");
        assert_eq!(parsed.season, Some(2));
        assert_eq!(parsed.episode, Some(2));
    }

    #[test]
    fn an_e_token_behind_the_markers_is_a_group_and_leaves_the_film_a_film() {
        // The cost of believing a lone episode number: a group or an edition written the same
        // way would make a series out of every film carrying one. Only an episode number that
        // still stands inside the title counts.
        for name in [
            "Some.Film.2019.1080p.BluRay.x264-E11",
            "Some.Film.2019.1080p.BluRay.x264.E5.mkv",
        ] {
            let parsed = parse(name).unwrap_or_else(|| panic!("{name} should parse"));
            assert!(!parsed.is_series(), "{name}");
            assert_eq!(parsed.episode, None, "{name}");
            assert_eq!(parsed.title, "Some Film", "{name}");
        }
    }

    #[test]
    fn a_name_that_gives_no_title_is_answered_without_asking_anybody() {
        // The decision this plugin makes most often, and the one that has to be free: every
        // one of these returns `None`, which is the guest's instruction to make no request.
        for name in [
            "setup.exe",
            "holiday-photos.zip",
            "invoice.pdf",
            "Some.Album.2019.FLAC.mp3",
            "readme.nfo",
            "Some.Film.2019.1080p.BluRay.x264-GRP.srt",
            "archive.tar.gz",
            "document",
            "IMG_20190812_144501.jpg",
            "backup-2019-08-12.sql",
            "",
            "   ",
            "https://example.com/download?id=4711",
        ] {
            assert_eq!(parse(name), None, "{name} must not reach a service");
        }
    }

    #[test]
    fn a_marker_without_a_title_in_front_of_it_is_still_nothing() {
        // `1080p.mkv` carries a marker but nothing to search for, and a request built from an
        // empty title is a request that tells a service something for no possible answer.
        assert_eq!(parse("1080p.mkv"), None);
        assert_eq!(parse("sample.1080p.mkv"), None);
        assert_eq!(parse("2019.mkv"), None);
    }

    #[test]
    fn an_ordinary_video_file_without_any_marker_asks_nobody() {
        // A file that is plainly a video but carries no release marker: there is no title to
        // look up with any confidence, and guessing would mean sending file names of a
        // person's own recordings to a service.
        assert_eq!(parse("holiday.mkv"), None);
        assert_eq!(parse("clip.mp4"), None);
        assert_eq!(
            parse("Some.Film.mkv"),
            None,
            "no year, no resolution, no source: not a release name"
        );
    }

    #[test]
    fn the_parsed_name_is_the_whole_decision() {
        // Stated as a test because it is the contract the guest relies on: a `Some` carries a
        // title that is worth a query, and a `None` means no query at all.
        let parsed: ReleaseName = parse("Some.Film.2019.1080p.BluRay.x264-GRP").expect("parses");
        assert!(!parsed.title.is_empty());
        assert!(parsed.title.chars().any(char::is_alphabetic));
    }
}
