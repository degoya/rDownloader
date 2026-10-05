//! The sort template language (RD-1100-08): where a recognised episode or film is placed, and
//! under which name.
//!
//! The same closed design as the media output template ([`crate::template`]), with formatting:
//!
//! * `{field}` or `{field:format}`, and nothing else. A field the template's kind does not have
//!   is refused when the template is saved, never left as literal text in a file name.
//! * Formats: `00`, `000` pad a number to that many digits; `lower`, `upper`; `dots` writes the
//!   spaces of a value as dots.
//! * `/` separates folders and is the only structure a template creates. The last segment is
//!   the file name without its extension, which the file keeps.
//! * `..`, `.`, absolute paths, drive letters and UNC prefixes are refused before *and* after
//!   expansion; every produced segment goes through [`crate::sanitize_file_name`], so reserved
//!   device names, control characters and trailing dots are handled by the one sanitiser the
//!   application has. The result is checked to stay below the root it was expanded against.
//! * A placeholder without a value expands to nothing; brackets left empty and doubled ` - `
//!   separators are tidied away, so `{show} ({year})` reads `Show` when the year is unknown.

use std::path::{Component, Path, PathBuf};

use rd_core::SortKind;

use crate::{
    SortMatch,
    names::{sanitize_file_name, sanitize_file_name_within},
    template::{MAX_TEMPLATE_DEPTH, MAX_TEMPLATE_LENGTH, is_traversal, starts_at_root},
};

/// Room kept behind a produced file name for a companion's suffix, `.forced.en.srt` and the
/// like, so a subtitle still fits the path budget its video was shortened to.
pub const SORT_COMPANION_RESERVE: usize = 24;

const SERIES_FIELDS: &[&str] = &[
    "show",
    "season",
    "episode",
    "title",
    "year",
    "resolution",
    "source",
];
const DATED_FIELDS: &[&str] = &[
    "show",
    "date",
    "year",
    "month",
    "day",
    "title",
    "resolution",
    "source",
];
const MOVIE_FIELDS: &[&str] = &["movie", "year", "resolution", "source"];
const NUMBER_FIELDS: &[&str] = &["season", "episode", "year", "month", "day"];

/// The fields a template of `kind` may use.
#[must_use]
pub const fn sort_fields(kind: SortKind) -> &'static [&'static str] {
    match kind {
        SortKind::Series => SERIES_FIELDS,
        SortKind::Dated => DATED_FIELDS,
        SortKind::Movie => MOVIE_FIELDS,
    }
}

/// Why a sort template was refused, or could not be expanded for one name.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SortTemplateError {
    #[error("the template is empty")]
    Empty,
    #[error("the template is longer than {MAX_TEMPLATE_LENGTH} characters")]
    TooLong,
    #[error("`{{{field}}}` is not a field this template can use")]
    UnknownField { field: String },
    #[error("`{format}` is not a format `{{{field}}}` can use")]
    UnknownFormat { field: String, format: String },
    #[error("a `{{` or `}}` is unbalanced, or a `\\` is used instead of `/`")]
    Syntax,
    #[error("the template must not leave the category's folder")]
    Outside,
    #[error("the template nests more than {MAX_TEMPLATE_DEPTH} folders deep")]
    TooDeep,
    #[error("the file name (the last part) uses no field, so every file would get the same name")]
    FixedName,
    #[error("the template expands to an empty file name")]
    EmptyResult,
}

impl SortTemplateError {
    /// The stable code the interface translates.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Empty => "sort.template_empty",
            Self::TooLong => "sort.template_too_long",
            Self::UnknownField { .. } => "sort.template_unknown_field",
            Self::UnknownFormat { .. } => "sort.template_unknown_format",
            Self::Syntax => "sort.template_syntax",
            Self::Outside => "sort.template_outside",
            Self::TooDeep => "sort.template_too_deep",
            Self::FixedName => "sort.template_fixed_name",
            Self::EmptyResult => "sort.template_empty_result",
        }
    }
}

/// Where a recognised file goes: a folder below the root and the file name without extension.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SortTarget {
    pub directory: PathBuf,
    pub stem: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Format {
    Plain,
    Pad(usize),
    Lower,
    Upper,
    Dots,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Piece {
    Text(String),
    Field { name: String, format: Format },
}

/// Checks a template of `kind` without expanding it.
///
/// # Errors
///
/// Returns [`SortTemplateError`] for an unknown field or format, unbalanced braces, a
/// backslash, an absolute path, a traversal segment, a template that is too long or too deep,
/// or one whose file name uses no field.
pub fn validate_sort_template(kind: SortKind, template: &str) -> Result<(), SortTemplateError> {
    segments(kind, template).map(|_| ())
}

/// Expands `template` for `found` into a target below `root`.
///
/// The same function serves the preview and the sort itself, so what a person is shown is what
/// a file gets.
///
/// # Errors
///
/// Returns [`SortTemplateError`] when [`validate_sort_template`] would, when a value expands to
/// a traversal, or when the file name comes out empty.
pub fn expand_sort_template(
    root: &Path,
    template: &str,
    found: &SortMatch,
) -> Result<SortTarget, SortTemplateError> {
    let segments = segments(found.kind, template)?;
    let last = segments.len().saturating_sub(1);
    let mut directory = root.to_path_buf();
    let mut stem = None;
    for (index, pieces) in segments.iter().enumerate() {
        let rendered = render(pieces, found);
        // A value can carry what the literal template could not: a show called `..` is
        // refused here, before the tidying or the sanitiser could turn it into something else.
        if is_traversal(&rendered) {
            return Err(SortTemplateError::Outside);
        }
        let expanded = tidy(&rendered);
        if is_traversal(&expanded) {
            return Err(SortTemplateError::Outside);
        }
        if expanded.is_empty() {
            if index == last {
                return Err(SortTemplateError::EmptyResult);
            }
            continue;
        }
        if index == last {
            stem = Some(sanitize_file_name_within(
                &directory,
                &expanded,
                SORT_COMPANION_RESERVE,
            ));
        } else {
            let sanitized = sanitize_file_name(&expanded);
            if is_traversal(&sanitized) {
                return Err(SortTemplateError::Outside);
            }
            directory.push(sanitized);
        }
    }
    let stem = stem.ok_or(SortTemplateError::EmptyResult)?;
    // Whatever came out must still be below `root`.
    if is_traversal(&stem)
        || !directory.starts_with(root)
        || directory
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(SortTemplateError::Outside);
    }
    Ok(SortTarget { directory, stem })
}

/// The template's segments, parsed and checked.
fn segments(kind: SortKind, template: &str) -> Result<Vec<Vec<Piece>>, SortTemplateError> {
    let trimmed = template.trim();
    if trimmed.is_empty() {
        return Err(SortTemplateError::Empty);
    }
    if trimmed.chars().count() > MAX_TEMPLATE_LENGTH {
        return Err(SortTemplateError::TooLong);
    }
    if starts_at_root(trimmed) {
        return Err(SortTemplateError::Outside);
    }
    if trimmed.contains('\\') {
        return Err(SortTemplateError::Syntax);
    }
    let raw: Vec<&str> = trimmed
        .split('/')
        .filter(|segment| !segment.trim().is_empty())
        .collect();
    if raw.iter().any(|segment| is_traversal(segment)) {
        return Err(SortTemplateError::Outside);
    }
    if raw.len() > MAX_TEMPLATE_DEPTH {
        return Err(SortTemplateError::TooDeep);
    }
    let parsed = raw
        .iter()
        .map(|segment| parse_segment(kind, segment))
        .collect::<Result<Vec<_>, _>>()?;
    let names_a_file = parsed.last().is_some_and(|pieces| {
        pieces
            .iter()
            .any(|piece| matches!(piece, Piece::Field { .. }))
    });
    if !names_a_file {
        return Err(SortTemplateError::FixedName);
    }
    Ok(parsed)
}

fn parse_segment(kind: SortKind, segment: &str) -> Result<Vec<Piece>, SortTemplateError> {
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut field: Option<String> = None;
    for character in segment.chars() {
        match (character, field.as_mut()) {
            ('{', None) => {
                if !text.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                }
                field = Some(String::new());
            }
            ('{', Some(_)) | ('}', None) => return Err(SortTemplateError::Syntax),
            ('}', Some(inner)) => {
                let inner = std::mem::take(inner);
                pieces.push(placeholder(kind, &inner)?);
                field = None;
            }
            (character, Some(inner)) => inner.push(character),
            (character, None) => text.push(character),
        }
    }
    if field.is_some() {
        return Err(SortTemplateError::Syntax);
    }
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
    Ok(pieces)
}

fn placeholder(kind: SortKind, inner: &str) -> Result<Piece, SortTemplateError> {
    let (name, spec) = match inner.split_once(':') {
        Some((name, spec)) => (name.trim(), Some(spec.trim())),
        None => (inner.trim(), None),
    };
    if !sort_fields(kind).contains(&name) {
        return Err(SortTemplateError::UnknownField {
            field: name.to_owned(),
        });
    }
    let refused = || SortTemplateError::UnknownFormat {
        field: name.to_owned(),
        format: spec.unwrap_or_default().to_owned(),
    };
    let format = match spec {
        None => Format::Plain,
        Some("lower") => Format::Lower,
        Some("upper") => Format::Upper,
        Some("dots") => Format::Dots,
        Some(zeros)
            if !zeros.is_empty()
                && zeros.len() <= 4
                && zeros.bytes().all(|byte| byte == b'0')
                && NUMBER_FIELDS.contains(&name) =>
        {
            Format::Pad(zeros.len())
        }
        Some(_) => return Err(refused()),
    };
    Ok(Piece::Field {
        name: name.to_owned(),
        format,
    })
}

fn render(pieces: &[Piece], found: &SortMatch) -> String {
    let mut output = String::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => output.push_str(text),
            Piece::Field { name, format } => {
                // A multi-episode file names its first and last episode, joined the way the
                // template writes the first: `S01E01-E02` after an `E`, `1x01-02` otherwise.
                let joiner = match output.chars().last() {
                    Some('E') => "-E",
                    Some('e') => "-e",
                    _ => "-",
                };
                output.push_str(&value(name, *format, found, joiner));
            }
        }
    }
    output
}

fn value(name: &str, format: Format, found: &SortMatch, joiner: &str) -> String {
    let number = |value: u32| match format {
        Format::Pad(width) => format!("{value:0width$}"),
        _ => value.to_string(),
    };
    let text = match name {
        "show" => found.show.clone(),
        "movie" => found.movie.clone(),
        "title" => found.title.clone(),
        "resolution" => found.resolution.clone(),
        "source" => found.source.clone(),
        "season" => found.season.map(number),
        "episode" => match found.episodes.as_slice() {
            [] => None,
            [single] => Some(number(*single)),
            [first, .., last] => Some(format!("{}{joiner}{}", number(*first), number(*last))),
        },
        "year" => found.year.map(number),
        "month" => found.date.map(|(_, month, _)| number(month)),
        "day" => found.date.map(|(_, _, day)| number(day)),
        "date" => found
            .date
            .map(|(year, month, day)| format!("{year:04}-{month:02}-{day:02}")),
        _ => None,
    }
    .unwrap_or_default();
    match format {
        Format::Lower => text.to_lowercase(),
        Format::Upper => text.to_uppercase(),
        Format::Dots => text.split_whitespace().collect::<Vec<_>>().join("."),
        Format::Plain | Format::Pad(_) => text,
    }
}

/// Drops what a missing value leaves behind: empty brackets, doubled ` - ` separators, runs of
/// spaces and separators at either end.
fn tidy(segment: &str) -> String {
    let mut text = segment.to_owned();
    loop {
        let before = text.len();
        for empty in ["()", "[]", "( )", "[ ]"] {
            text = text.replace(empty, "");
        }
        text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        text = text.replace(" - - ", " - ");
        if text.len() == before {
            break;
        }
    }
    text.trim_matches([' ', '-', '.', '_']).to_owned()
}
