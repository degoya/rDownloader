//! The one keyless source this plugin asks, and how its answer becomes fields.
//!
//! Cinemeta is the Stremio project's public metadata addon. It was chosen for one reason that
//! decides the matter: an enricher is instantiated without an account — `self.runtime.store(None)`
//! in the host — so the host expands no `{{secret:...}}` on this path, and every service that
//! wants an API key is out of reach until that is decided on its own terms. Cinemeta wants
//! none, answers for films and series through the same two shapes, and carries exactly the
//! four values worth a row: rating, year, genre and runtime.
//!
//! Nothing here performs a request. Building an address and reading an answer are pure, so
//! both are tested without a WebAssembly target and without reaching anything.

use crate::json::{self, Json};

const SOURCE_BASE: &str = "https://v3-cinemeta.strem.io";

/// Field names, all under one namespace so the chips read as one plugin's work and none of
/// them can collide with a core field the online check resolved itself.
pub const FIELD_TITLE: &str = "metadata.title";
pub const FIELD_SERIES: &str = "metadata.series";
pub const FIELD_SEASON: &str = "metadata.season";
pub const FIELD_EPISODE: &str = "metadata.episode";
pub const FIELD_YEAR: &str = "metadata.year";
pub const FIELD_RATING: &str = "metadata.rating";
pub const FIELD_GENRE: &str = "metadata.genre";
pub const FIELD_RUNTIME: &str = "metadata.runtime";

/// Longest value this plugin will offer. The host truncates at 512; staying well inside that
/// means a chip stays a chip.
const MAX_VALUE: usize = 200;

/// Longest title sent as a query. A release name that needs more than this is not a title.
const MAX_QUERY: usize = 80;

/// How many genres are worth a chip. Cinemeta lists up to five or six; three is a row.
const MAX_GENRES: usize = 3;

/// How alike the name a hit carries and the name that was searched for have to read, in
/// hundredths, before the hit is believed to be the release at all.
///
/// The measure is a Dice coefficient over normalised words (see [`similarity`]), and 75 is
/// where it separates the two shapes that matter. A wrong hit that merely *sounds* right shares
/// all but one word — "Apollo Has Fallen" against "Paris Has Fallen" scores 66 — while a right
/// hit written differently keeps every word and adds or drops at most a subtitle: "Mission
/// Impossible Dead Reckoning" against "Mission: Impossible - Dead Reckoning Part One" scores 80.
/// Below 75 the reported defect comes straight back; much above it, a release whose name the
/// catalogue spells with one extra word loses its row for nothing.
const MIN_SIMILARITY: u32 = 75;

/// The same bar, lowered for a hit whose year is exactly the year in the release name.
///
/// A matching year is corroboration the name alone cannot give, and it is what keeps a title the
/// release name cut short — `Dune.Part.Two.2024...` searches for "Dune" and Cinemeta answers
/// "Dune: Part Two", a 50 — from being thrown away. Without a year in the name this bar is never
/// reached: both names RD-108-14 was reported with carry none, so neither profits from it.
const MIN_SIMILARITY_WITH_YEAR: u32 = 50;

/// Words that say nothing about which release a title names.
///
/// Articles and conjunctions in the five languages this catalogue answers in. They are dropped
/// from both sides, so "Beauty and the Beast" and "Beauty & the Beast" are the same two words,
/// and so that a leading article present on one side only cannot cost a real hit its row.
#[rustfmt::skip]
const IGNORED_WORDS: &[&str] = &[
    "the", "a", "an", "and", "of",
    "der", "die", "das", "den", "dem", "ein", "eine", "und",
    "le", "la", "les", "un", "une", "des", "du", "et",
    "el", "los", "las", "una", "il", "lo", "gli",
];

/// Which of the source's two catalogues to ask.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Movie,
    Series,
}

impl Kind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "series",
        }
    }

    /// The field the matched name goes into: a film has a title, an episode has a series.
    #[must_use]
    pub const fn name_field(self) -> &'static str {
        match self {
            Self::Movie => FIELD_TITLE,
            Self::Series => FIELD_SERIES,
        }
    }
}

/// What one answer yielded.
pub struct Lookup {
    /// The source's own identifier for the match, when it gave one. Only ever used to ask the
    /// same host for the detail; never sent anywhere else.
    pub id: Option<String>,
    pub fields: Vec<(String, String)>,
}

/// The catalogue address for a title, or nothing if the title is not one worth sending.
///
/// The title is reduced to letters, digits and spaces before it is encoded. That is narrower
/// than percent-encoding alone would require, and deliberately so: the query is spliced into a
/// path segment, and a value that cannot contain a slash, a dot or a percent cannot leave the
/// segment it was put in whatever the source does with it.
#[must_use]
pub fn catalogue_url(kind: Kind, title: &str) -> Option<String> {
    let query = searchable(title)?;
    Some(format!(
        "{SOURCE_BASE}/catalog/{}/top/search={}.json",
        kind.as_str(),
        percent_encode(&query)
    ))
}

/// The detail address for an identifier the source itself just handed back.
///
/// The identifier is checked against the one shape it may have rather than trusted, because it
/// arrived from outside and is about to become part of a path.
#[must_use]
pub fn meta_url(kind: Kind, id: &str) -> Option<String> {
    let digits = id.strip_prefix("tt")?;
    if digits.is_empty() || digits.len() > 12 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("{SOURCE_BASE}/meta/{}/{id}.json", kind.as_str()))
}

/// The title as it may be sent: letters, digits and single spaces, and not too much of it.
fn searchable(title: &str) -> Option<String> {
    let mut out = String::new();
    for character in title.chars() {
        if character.is_alphanumeric() {
            out.push(character);
        } else if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
        if out.chars().count() >= MAX_QUERY {
            break;
        }
    }
    let trimmed = out.trim();
    (trimmed.chars().filter(|c| c.is_alphabetic()).count() >= 2).then(|| trimmed.to_owned())
}

/// Percent-encoding, for the reduced alphabet [`searchable`] leaves behind.
fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// Reads a catalogue answer into fields, choosing the entry the release name pointed at.
///
/// Nothing here fails loudly. A body that is not JSON, an answer with no entries, an entry
/// with nothing worth showing, an answer about something else entirely — each is simply no
/// fields, because a source talking nonsense must leave the row exactly as the online check
/// left it.
#[must_use]
pub fn from_catalogue(
    body: &str,
    kind: Kind,
    want_title: &str,
    want_year: Option<u16>,
) -> Option<Lookup> {
    let document = json::parse(body)?;
    let entries = document.get("metas")?.as_array()?;
    let entry = best_entry(entries, kind, want_title, want_year)?;
    let mut fields = Vec::new();
    if let Some(name) = entry.text_field("name") {
        push(&mut fields, kind.name_field(), &name);
    }
    read_common(entry, &mut fields);
    Some(Lookup {
        id: entry.text_field("id").filter(|id| id.starts_with("tt")),
        fields,
    })
}

/// Reads a detail answer, which wraps one entry under `meta`.
#[must_use]
pub fn from_meta(body: &str) -> Vec<(String, String)> {
    let Some(entry) = json::parse(body).and_then(|document| document.get("meta").cloned()) else {
        return Vec::new();
    };
    let mut fields = Vec::new();
    read_common(&entry, &mut fields);
    fields
}

/// Whether the detail is worth a second request: only when the catalogue entry left out
/// something a person would actually read.
#[must_use]
pub fn wants_detail(fields: &[(String, String)]) -> bool {
    [FIELD_RUNTIME, FIELD_GENRE, FIELD_RATING]
        .iter()
        .any(|name| !fields.iter().any(|(field, _)| field == name))
}

/// The four values both shapes carry.
fn read_common(entry: &Json, fields: &mut Vec<(String, String)>) {
    if let Some(year) = entry
        .text_field("releaseInfo")
        .or_else(|| entry.text_field("year"))
        .as_deref()
        .and_then(release_year)
    {
        push(fields, FIELD_YEAR, &year);
    }
    if let Some(rating) = entry
        .text_field("imdbRating")
        .as_deref()
        .and_then(plausible_rating)
    {
        push(fields, FIELD_RATING, &rating);
    }
    if let Some(genres) = entry
        .text_list("genres", MAX_GENRES)
        .or_else(|| entry.text_list("genre", MAX_GENRES))
    {
        push(fields, FIELD_GENRE, &genres);
    }
    if let Some(runtime) = entry.text_field("runtime") {
        push(fields, FIELD_RUNTIME, &runtime);
    }
}

/// The entry the release name meant, or nothing when no entry is recognisably it.
///
/// Two things decide. The name has to read like the one searched for — Cinemeta's search is
/// fuzzy and answers "Paris Has Fallen" for "Apollo Has Fallen", and taking the first hit
/// unseen is how RD-108-14's wrong rows were written. And the year decides between entries
/// that all pass that bar: a search for a remade title otherwise lands on whichever version the
/// source ranks first, and a row saying 1974 under a file named 2019 is worse than no row.
///
/// Where nothing clears the bar the answer is `None`, which the caller turns into no field at
/// all. A thin row lets somebody look it up; a wrong row lets them decide on it.
fn best_entry<'a>(
    entries: &'a [Json],
    kind: Kind,
    want_title: &str,
    want_year: Option<u16>,
) -> Option<&'a Json> {
    let of_kind: Vec<&Json> = entries
        .iter()
        .filter(|entry| {
            entry
                .text_field("type")
                .is_none_or(|value| value == kind.as_str())
        })
        .collect();
    let candidates = if of_kind.is_empty() {
        entries.iter().collect()
    } else {
        of_kind
    };
    // The same string the request was built from, not the whole title: `searchable` cuts at
    // `MAX_QUERY`, and judging an answer on words the source was never asked about would
    // refuse a hit for a reason the source could not have known.
    let asked = searchable(want_title);
    let asked = asked.as_deref().unwrap_or(want_title);
    let wanted = normalise(asked);
    let wanted_plain = plain(asked);
    let mut best: Option<(&Json, bool, u32)> = None;
    for entry in candidates {
        let Some(name) = entry.text_field("name") else {
            // An entry that does not say what it is cannot be shown to be the right one.
            continue;
        };
        let score = title_score(&wanted, &wanted_plain, &name);
        let year_agrees = want_year
            .is_some_and(|year| entry_year(entry).is_some_and(|found| found == year.to_string()));
        if score < MIN_SIMILARITY && !(year_agrees && score >= MIN_SIMILARITY_WITH_YEAR) {
            continue;
        }
        // A year that agrees outranks a slightly better-reading name, and the source's own
        // ranking breaks a tie, because a strict comparison keeps the entry seen first.
        if best
            .is_none_or(|(_, best_year, best_score)| (year_agrees, score) > (best_year, best_score))
        {
            best = Some((entry, year_agrees, score));
        }
    }
    best.map(|(entry, _, _)| entry)
}

/// The year an entry claims, read the same way the fields are.
fn entry_year(entry: &Json) -> Option<String> {
    entry
        .text_field("releaseInfo")
        .or_else(|| entry.text_field("year"))
        .as_deref()
        .and_then(release_year)
}

/// Letters that are not in the alphabet a release name is written in, and what each becomes.
///
/// Release names are ASCII by convention — the scene and the trackers have written them
/// that way for twenty years — while a catalogue writes the title properly. So
/// `Shogun.S01E02` asks about a series the source calls "Sh\u{14d}gun", and without this the
/// two share no word at all and score zero. The characters are spelled as escapes because a
/// Rust source in this repository may not contain a German umlaut
/// (`crates/rdownloader/tests/no_german.rs`); each group is named in the comment instead.
#[rustfmt::skip]
const FOLDED: &[(&str, &str)] = &[
    // a: grave, acute, circumflex, tilde, diaeresis, ring, macron, breve, ogonek
    ("\u{e0}\u{e1}\u{e2}\u{e3}\u{e4}\u{e5}\u{101}\u{103}\u{105}", "a"),
    ("\u{e6}", "ae"),                                        // ae ligature
    ("\u{e7}\u{107}\u{109}\u{10d}", "c"),                    // c: cedilla, acute, circumflex, caron
    ("\u{10f}\u{111}\u{f0}", "d"),                           // d: caron, stroke, eth
    // e: grave, acute, circumflex, diaeresis, macron, breve, dot, ogonek, caron
    ("\u{e8}\u{e9}\u{ea}\u{eb}\u{113}\u{115}\u{117}\u{119}\u{11b}", "e"),
    ("\u{11d}\u{11f}\u{123}", "g"),                          // g: circumflex, breve, cedilla
    ("\u{ec}\u{ed}\u{ee}\u{ef}\u{129}\u{12b}\u{12f}", "i"),  // i: grave .. ogonek
    ("\u{135}", "j"),                                        // j circumflex
    ("\u{137}", "k"),                                        // k cedilla
    ("\u{13a}\u{13e}\u{142}", "l"),                          // l: acute, caron, stroke
    ("\u{f1}\u{144}\u{148}", "n"),                           // n: tilde, acute, caron
    // o: grave, acute, circumflex, tilde, diaeresis, stroke, macron, breve, double acute
    ("\u{f2}\u{f3}\u{f4}\u{f5}\u{f6}\u{f8}\u{14d}\u{14f}\u{151}", "o"),
    ("\u{153}", "oe"),                                       // oe ligature
    ("\u{155}\u{159}", "r"),                                 // r: acute, caron
    ("\u{15b}\u{15f}\u{161}", "s"),                          // s: acute, cedilla, caron
    ("\u{df}", "ss"),                                        // sharp s
    ("\u{163}\u{165}", "t"),                                 // t: cedilla, caron
    ("\u{fe}", "th"),                                        // thorn
    // u: grave, acute, circumflex, diaeresis, tilde, macron, breve, ring, double acute, ogonek
    ("\u{f9}\u{fa}\u{fb}\u{fc}\u{169}\u{16b}\u{16d}\u{16f}\u{171}\u{173}", "u"),
    ("\u{fd}\u{ff}\u{177}", "y"),                            // y: acute, diaeresis, circumflex
    ("\u{17a}\u{17c}\u{17e}", "z"),                          // z: acute, dot, caron
];

/// One lower-cased word with its letters folded onto the alphabet a release name uses.
fn fold_ascii(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    for letter in word.chars() {
        match FOLDED.iter().find(|(set, _)| set.contains(letter)) {
            Some((_, folded)) => out.push_str(folded),
            None => out.push(letter),
        }
    }
    out
}

/// A catalogue name without the qualifier a catalogue puts on it to tell two like-named
/// entries apart: "The Office (US)", "Doctor Who (2005)", "Skins (UK)".
///
/// A release name never carries one, and for a one-word title that qualifier is the whole
/// difference between 100 and 66 — while a series, unlike a film, has no year in its name
/// to be rescued by. Only a trailing bracket is taken, and only when something is left in
/// front of it.
fn without_qualifier(name: &str) -> &str {
    let trimmed = name.trim_end();
    let Some(open) = trimmed
        .strip_suffix(')')
        .and_then(|inside| inside.rfind('('))
    else {
        return name;
    };
    let head = trimmed[..open].trim_end();
    if head.is_empty() { name } else { head }
}

/// A title reduced to the words that identify it: lower case, folded onto ASCII, no
/// punctuation, no articles, and a plural folded onto its singular so "Alien" and "Aliens" are
/// not two different films.
///
/// The plural is folded *after* the articles are dropped, never before. The other order reads
/// "Loss" as "los" and "Dies" as "die", throws both away as a Spanish and a German article,
/// and can leave a title with nothing at all to compare.
fn normalise(title: &str) -> Vec<String> {
    let mut words: Vec<String> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| fold_ascii(&word.to_lowercase()))
        .filter(|word| !IGNORED_WORDS.contains(&word.as_str()))
        .map(|word| match word.strip_suffix('s') {
            Some(stem) if stem.chars().count() >= 3 => stem.to_owned(),
            _ => word,
        })
        .collect();
    words.sort_unstable();
    words.dedup();
    words
}

/// The whole title as one string, for the case where no words are left to compare.
fn plain(title: &str) -> String {
    fold_ascii(&title.to_lowercase())
        .chars()
        .filter(|letter| letter.is_alphanumeric())
        .collect()
}

/// How alike a catalogue entry's name reads against the title that was searched for.
fn title_score(wanted: &[String], wanted_plain: &str, name: &str) -> u32 {
    let name = without_qualifier(name);
    let found = normalise(name);
    if wanted.is_empty() || found.is_empty() {
        // A title that is nothing but articles normalises away entirely. Comparing what was
        // actually written is better than calling an identical name a mismatch.
        return u32::from(!wanted_plain.is_empty() && wanted_plain == plain(name)) * 100;
    }
    similarity(wanted, &found)
}

/// How alike two normalised titles read, in hundredths: twice the words they share over the
/// number of words they have between them.
///
/// A Dice coefficient rather than a character-by-character ratio, because release names and
/// catalogue names differ by whole words — a subtitle, a year, a franchise prefix —
/// and agree or disagree on the words that carry the identity. It is symmetric on purpose: a
/// short title must not be allowed to match a long one merely by being contained in it, which
/// is exactly how "Dragon Ball" would have found "Dragon Ball Z: Broly - Second Coming".
///
/// The value is a floor: the division truncates, so two shared words out of three read as 66
/// and not 66.7. That keeps the thresholds whole numbers, and anyone moving one should know
/// which side of it a truncated value falls on.
fn similarity(wanted: &[String], found: &[String]) -> u32 {
    let total = wanted.len() + found.len();
    if wanted.is_empty() || found.is_empty() || total == 0 {
        return 0;
    }
    let shared = wanted.iter().filter(|word| found.contains(word)).count();
    u32::try_from(shared * 200 / total).unwrap_or(0)
}

/// The year out of whatever the source wrote: `2010`, `2010-2015`, `2010-`.
fn release_year(value: &str) -> Option<String> {
    let digits: String = value.trim().chars().take(4).collect();
    (digits.len() == 4 && digits.chars().all(|c| c.is_ascii_digit())).then_some(digits)
}

/// A rating only if it reads like one. `0` through `10`, and nothing else gets a chip.
fn plausible_rating(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let number: f64 = trimmed.parse().ok()?;
    (0.0..=10.0).contains(&number).then(|| trimmed.to_owned())
}

/// Adds a field, once, trimmed and bounded.
fn push(fields: &mut Vec<(String, String)>, name: &str, value: &str) {
    let value: String = value.trim().chars().take(MAX_VALUE).collect();
    if value.is_empty() || fields.iter().any(|(field, _)| field == name) {
        return;
    }
    fields.push((name.to_owned(), value));
}

/// Adds every field of `extra` that is not already known.
pub fn merge(fields: &mut Vec<(String, String)>, extra: Vec<(String, String)>) {
    for (name, value) in extra {
        push(fields, &name, &value);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FIELD_GENRE, FIELD_RATING, FIELD_RUNTIME, FIELD_SERIES, FIELD_TITLE, FIELD_YEAR, Kind,
        catalogue_url, from_catalogue, from_meta, merge, meta_url, wants_detail,
    };

    const CATALOGUE: &str = r#"{"metas":[
        {"id":"tt0083658","type":"movie","name":"Blade Runner","releaseInfo":"1982",
         "imdbRating":"8.1","genres":["Action","Drama","Sci-Fi","Thriller"]},
        {"id":"tt1856101","type":"movie","name":"Blade Runner 2049","releaseInfo":"2017",
         "imdbRating":"8.0","genres":["Action","Drama","Mystery"]}
    ]}"#;

    fn value<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
        fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn the_address_carries_the_title_and_can_never_leave_its_path_segment() {
        let url = catalogue_url(Kind::Movie, "Some Film").expect("a query");
        assert_eq!(
            url,
            "https://v3-cinemeta.strem.io/catalog/movie/top/search=Some%20Film.json"
        );
        // A title full of path characters is reduced before it is encoded, so neither a slash
        // nor a dot nor a percent survives into the address.
        let hostile = catalogue_url(Kind::Series, "../../etc/passwd?x=1").expect("a query");
        assert!(hostile.starts_with("https://v3-cinemeta.strem.io/catalog/series/top/search="));
        assert_eq!(hostile.matches('/').count(), 6, "{hostile}");
        assert!(!hostile.contains('%') || hostile.contains("%20"));
        assert!(hostile.ends_with(".json"));
    }

    #[test]
    fn a_title_with_nothing_to_search_for_produces_no_address_at_all() {
        assert_eq!(catalogue_url(Kind::Movie, ""), None);
        assert_eq!(catalogue_url(Kind::Movie, "2019"), None);
        assert_eq!(catalogue_url(Kind::Movie, "-- --"), None);
    }

    #[test]
    fn an_identifier_is_checked_before_it_becomes_part_of_an_address() {
        assert_eq!(
            meta_url(Kind::Movie, "tt1856101").as_deref(),
            Some("https://v3-cinemeta.strem.io/meta/movie/tt1856101.json")
        );
        for hostile in ["tt../../x", "kitsu:1", "tt", "", "tt12345678901234"] {
            assert_eq!(meta_url(Kind::Movie, hostile), None, "{hostile}");
        }
    }

    #[test]
    fn the_year_in_the_name_decides_which_entry_the_row_describes() {
        // Without this the source's own ranking wins and the row says 1982 under a file
        // named 2017, which is worse than saying nothing.
        let found = from_catalogue(CATALOGUE, Kind::Movie, "Blade Runner 2049", Some(2017))
            .expect("a match");
        assert_eq!(value(&found.fields, FIELD_TITLE), Some("Blade Runner 2049"));
        assert_eq!(value(&found.fields, FIELD_YEAR), Some("2017"));
        assert_eq!(value(&found.fields, FIELD_RATING), Some("8.0"));
        assert_eq!(
            value(&found.fields, FIELD_GENRE),
            Some("Action, Drama, Mystery")
        );
        assert_eq!(found.id.as_deref(), Some("tt1856101"));

        let first = from_catalogue(CATALOGUE, Kind::Movie, "Blade Runner", None).expect("a match");
        assert_eq!(value(&first.fields, FIELD_TITLE), Some("Blade Runner"));
    }

    #[test]
    fn a_series_answer_lands_under_the_series_name_rather_than_a_film_title() {
        const BODY: &str = r#"{"metas":[{"id":"tt0903747","type":"series","name":"Breaking Bad",
            "releaseInfo":"2008-2013","imdbRating":"9.5","genres":["Crime","Drama"]}]}"#;
        let found = from_catalogue(BODY, Kind::Series, "Breaking Bad", None).expect("a match");
        assert_eq!(value(&found.fields, FIELD_SERIES), Some("Breaking Bad"));
        assert_eq!(value(&found.fields, FIELD_TITLE), None);
        // `2008-2013` is a run, and the year that belongs on the row is where it started.
        assert_eq!(value(&found.fields, FIELD_YEAR), Some("2008"));
    }

    #[test]
    fn a_hit_whose_name_is_not_the_one_searched_for_yields_no_field_at_all() {
        // The two answers RD-108-14 was reported with. Cinemeta's search is fuzzy and both of
        // these are what it really hands back; taking the first hit unseen wrote a 1994 cinema
        // release next to a 2024 episode, and another series' name next to an episode of this
        // one. Neither name reads like the one searched for, so there is no field.
        const PARIS: &str = r#"{"metas":[{"id":"tt21371866","type":"series",
            "name":"Paris Has Fallen","releaseInfo":"2024","imdbRating":"6.5",
            "genres":["Action","Drama"],"runtime":"48 min"}]}"#;
        assert!(from_catalogue(PARIS, Kind::Series, "Apollo Has Fallen", None).is_none());

        const BROLY: &str = r#"{"metas":[{"id":"tt0142242","type":"movie",
            "name":"Dragon Ball Z: Broly - Second Coming","releaseInfo":"1994",
            "imdbRating":"6.5","genres":["Animation"],"runtime":"48 min"}]}"#;
        assert!(from_catalogue(BROLY, Kind::Series, "Dragon Ball DAIMA", None).is_none());
        // And not even with the film catalogue's own answer, which is what the release used to
        // be looked up in.
        assert!(from_catalogue(BROLY, Kind::Movie, "Dragon Ball DAIMA", None).is_none());
    }

    #[test]
    fn a_name_the_catalogue_spells_differently_is_still_the_same_release() {
        // The other half of the bar. Too strict and a real hit written with one more word, or
        // with the punctuation the catalogue prefers, loses its row for nothing.
        const LONGER: &str = r#"{"metas":[{"id":"tt9603212","type":"movie",
            "name":"Mission: Impossible - Dead Reckoning Part One","releaseInfo":"2023",
            "imdbRating":"7.7","genres":["Action"],"runtime":"163 min"}]}"#;
        let found = from_catalogue(
            LONGER,
            Kind::Movie,
            "Mission Impossible Dead Reckoning",
            None,
        )
        .expect("the same release");
        assert_eq!(
            value(&found.fields, FIELD_TITLE),
            Some("Mission: Impossible - Dead Reckoning Part One")
        );

        // A title the release name cut short at a marker word. The name alone would not carry
        // it; the year in the name agreeing exactly is what does.
        const DUNE: &str = r#"{"metas":[{"id":"tt15239678","type":"movie","name":"Dune: Part Two",
            "releaseInfo":"2024","imdbRating":"8.5","genres":["Sci-Fi"]}]}"#;
        assert!(from_catalogue(DUNE, Kind::Movie, "Dune", None).is_none());
        let found = from_catalogue(DUNE, Kind::Movie, "Dune", Some(2024)).expect("the same film");
        assert_eq!(value(&found.fields, FIELD_TITLE), Some("Dune: Part Two"));
    }

    #[test]
    fn a_title_the_catalogue_writes_properly_is_the_one_the_release_spells_in_ascii() {
        // Release names are ASCII by convention and catalogues are not, so without folding the
        // two share no word at all and a perfectly good hit is thrown away.
        const SHOGUN: &str = r#"{"metas":[{"id":"tt2788316","type":"series",
            "name":"Sh\u00f4gun","releaseInfo":"2024","imdbRating":"8.6",
            "genres":["Drama"]}]}"#;
        let found = from_catalogue(SHOGUN, Kind::Series, "Shogun", None).expect("the same series");
        assert_eq!(value(&found.fields, FIELD_SERIES), Some("Sh\u{f4}gun"));

        const POKEMON: &str = r#"{"metas":[{"id":"tt0168366","type":"series",
            "name":"Pok\u00e9mon","releaseInfo":"1997","imdbRating":"7.5",
            "genres":["Animation"]}]}"#;
        let found =
            from_catalogue(POKEMON, Kind::Series, "Pokemon", None).expect("the same series");
        assert_eq!(value(&found.fields, FIELD_SERIES), Some("Pok\u{e9}mon"));
    }

    #[test]
    fn a_bracketed_qualifier_does_not_cost_a_one_word_title_its_row() {
        // The bar bites hardest on short titles: one word against two is 66 whatever the words
        // are, and a series has no year in its name to be rescued by. The qualifier a catalogue
        // adds to tell two like-named entries apart is not part of the title.
        const SKINS: &str = r#"{"metas":[{"id":"tt0840196","type":"series","name":"Skins (UK)",
            "releaseInfo":"2007-2013","imdbRating":"8.0","genres":["Drama"]}]}"#;
        let found = from_catalogue(SKINS, Kind::Series, "Skins", None).expect("the same series");
        assert_eq!(value(&found.fields, FIELD_SERIES), Some("Skins (UK)"));

        // And it is only the bracket that is forgiven, not the name in front of it.
        const OTHER: &str = r#"{"metas":[{"id":"tt1586680","type":"series",
            "name":"Shameless (UK)","releaseInfo":"2004-2013","imdbRating":"8.3",
            "genres":["Comedy"]}]}"#;
        assert!(from_catalogue(OTHER, Kind::Series, "Skins", None).is_none());
    }

    #[test]
    fn a_word_that_would_fold_onto_an_article_is_not_thrown_away() {
        // "Loss" folds to "los" and "Dies" to "die", which are a Spanish and a German article.
        // Dropping the articles first and folding the plural afterwards is what keeps such a
        // title from normalising to nothing and failing against its own name.
        const LOSS: &str = r#"{"metas":[{"id":"tt1234567","type":"movie","name":"Loss",
            "releaseInfo":"2008","imdbRating":"6.9","genres":["Drama"]}]}"#;
        let found = from_catalogue(LOSS, Kind::Movie, "Loss", None).expect("its own name");
        assert_eq!(value(&found.fields, FIELD_TITLE), Some("Loss"));
        // And the word still has to be there to be compared with, not merely rescued by the
        // whole-string fallback: an article in front of it and the two must still meet.
        let found = from_catalogue(LOSS, Kind::Movie, "The Loss", None).expect("its own name");
        assert_eq!(value(&found.fields, FIELD_TITLE), Some("Loss"));
    }

    #[test]
    fn nonsense_from_the_source_leaves_the_row_exactly_as_it_was() {
        for body in [
            "",
            "<html>502 Bad Gateway</html>",
            "{}",
            r#"{"metas":[]}"#,
            r#"{"metas":"soon"}"#,
            "null",
        ] {
            assert!(
                from_catalogue(body, Kind::Movie, "Blade Runner", None).is_none(),
                "{body} must yield nothing"
            );
            assert!(from_meta(body).is_empty(), "{body} must yield nothing");
        }
    }

    #[test]
    fn a_value_that_is_not_what_it_claims_to_be_is_left_out_rather_than_shown() {
        const BODY: &str = r#"{"metas":[{"id":"x","type":"movie","name":"Odd",
            "releaseInfo":"soon","imdbRating":"excellent","genres":[1,2],"runtime":"  "}]}"#;
        let found = from_catalogue(BODY, Kind::Movie, "Odd", None).expect("an entry");
        assert_eq!(value(&found.fields, FIELD_TITLE), Some("Odd"));
        assert_eq!(value(&found.fields, FIELD_YEAR), None);
        assert_eq!(value(&found.fields, FIELD_RATING), None);
        assert_eq!(value(&found.fields, FIELD_GENRE), None);
        assert_eq!(value(&found.fields, FIELD_RUNTIME), None);
        // An identifier that is not the source's own shape is not kept, so no second request
        // can be built from it.
        assert_eq!(found.id, None);
    }

    #[test]
    fn the_detail_fills_what_the_catalogue_left_out_and_overwrites_nothing() {
        let mut found = from_catalogue(CATALOGUE, Kind::Movie, "Blade Runner 2049", Some(2017))
            .expect("a match");
        assert!(wants_detail(&found.fields), "runtime is still missing");
        const DETAIL: &str = r#"{"meta":{"id":"tt1856101","type":"movie","name":"Blade Runner 2049",
            "releaseInfo":"2017","imdbRating":"7.9","runtime":"164 min",
            "genres":["Sci-Fi"]}}"#;
        merge(&mut found.fields, from_meta(DETAIL));
        assert_eq!(value(&found.fields, FIELD_RUNTIME), Some("164 min"));
        // The catalogue said 8.0 first, and a second answer does not get to move it.
        assert_eq!(value(&found.fields, FIELD_RATING), Some("8.0"));
        assert!(!wants_detail(&found.fields));
    }
}
