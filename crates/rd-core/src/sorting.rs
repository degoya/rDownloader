//! Sort and rename templates for series and films (RD-1100-08).
//!
//! Only the vocabulary lives here: what a category stores and what a preview names. Recognising
//! a release name and expanding a template into a path is `rd-files`' work, beside the safe-path
//! rules every produced name goes through.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What a release name was recognised as.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SortKind {
    /// An episode by season and number, `Show.S01E02` or `Show.1x02`; several numbers make a
    /// multi-episode file.
    Series,
    /// An episode by air date, `Show.2024.03.15`.
    Dated,
    /// A film by title and year, `Film.2010`.
    Movie,
}

impl SortKind {
    pub const ALL: [Self; 3] = [Self::Series, Self::Dated, Self::Movie];

    /// The stored word, which is also the translation key suffix.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Series => "series",
            Self::Dated => "dated",
            Self::Movie => "movie",
        }
    }
}

/// A category's sort templates, one per kind of release. A kind without a template is not
/// sorted: its files stay where the package put them.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct SortTemplates {
    /// For episodes by season and number, e.g.
    /// `{show}/Season {season:00}/{show} - S{season:00}E{episode:00} - {title}`.
    #[serde(default)]
    pub series: Option<String>,
    /// For episodes by air date, e.g. `{show}/{year}/{show} - {date} - {title}`.
    #[serde(default)]
    pub dated: Option<String>,
    /// For films, e.g. `{movie} ({year})/{movie} ({year})`.
    #[serde(default)]
    pub movie: Option<String>,
}

impl SortTemplates {
    /// The template for `kind`, if the category has one.
    #[must_use]
    pub fn for_kind(&self, kind: SortKind) -> Option<&str> {
        match kind {
            SortKind::Series => self.series.as_deref(),
            SortKind::Dated => self.dated.as_deref(),
            SortKind::Movie => self.movie.as_deref(),
        }
        .filter(|template| !template.trim().is_empty())
    }

    /// Trimmed, a blank template dropped, and `None` when none is left — so "no sorting" has one
    /// spelling in the database rather than three.
    #[must_use]
    pub fn normalized(self) -> Option<Self> {
        let clean = |value: Option<String>| {
            value
                .map(|template| template.trim().to_owned())
                .filter(|template| !template.is_empty())
        };
        let templates = Self {
            series: clean(self.series),
            dated: clean(self.dated),
            movie: clean(self.movie),
        };
        (templates.series.is_some() || templates.dated.is_some() || templates.movie.is_some())
            .then_some(templates)
    }
}

#[cfg(test)]
mod tests {
    use super::{SortKind, SortTemplates};

    #[test]
    fn blank_templates_normalise_to_no_sorting() {
        let blank = SortTemplates {
            series: Some("  ".to_owned()),
            dated: None,
            movie: Some(String::new()),
        };
        assert_eq!(blank.normalized(), None);
        let one = SortTemplates {
            series: None,
            dated: None,
            movie: Some(" {movie} ({year}) ".to_owned()),
        }
        .normalized()
        .expect("one template is left");
        assert_eq!(one.for_kind(SortKind::Movie), Some("{movie} ({year})"));
        assert_eq!(one.for_kind(SortKind::Series), None);
    }
}
