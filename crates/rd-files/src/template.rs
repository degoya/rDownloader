//! A small, closed template language for output paths.
//!
//! Deliberately *not* yt-dlp's own `-o` syntax. That language can reach into arbitrary
//! metadata, do arithmetic, and address the filesystem; handing a user-supplied string to it
//! makes the extractor's output a code path. Here, a template is expanded to a literal path
//! *before* anything reaches a tool, and only from an allowlisted field set.
//!
//! The rules that matter:
//!
//! * `{field}` and nothing else. An unknown field is refused at edit time, not silently left
//!   as literal text where it would end up in a filename.
//! * A `/` separates directories and is the only structure a template can create. `..`,
//!   absolute paths, drive letters and UNC prefixes are refused — before *and* after
//!   expansion, because a title is attacker-controlled and `..` is a perfectly ordinary
//!   thing for one to contain.
//! * Every produced segment goes through [`crate::sanitize_file_name`], so reserved device
//!   names, control characters and trailing dots are handled by the same code the rest of
//!   the application uses. There is no second sanitiser here.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use crate::names::{sanitize_file_name, sanitize_file_name_within};

/// Longest template accepted, before expansion.
pub const MAX_TEMPLATE_LENGTH: usize = 512;
/// Deepest directory nesting a template may create.
pub const MAX_TEMPLATE_DEPTH: usize = 8;

/// Fields a template may reference. Anything outside this list is refused.
///
/// These are exactly the values a probe reliably supplies; adding more means teaching the
/// probe to supply them, not loosening the check.
pub const TEMPLATE_FIELDS: [&str; 8] = [
    "title",
    "uploader",
    "upload_date",
    "upload_year",
    "extractor",
    "id",
    "resolution",
    "ext",
];

/// Why a template was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TemplateError {
    #[error("the template is empty")]
    Empty,
    #[error("the template is longer than {MAX_TEMPLATE_LENGTH} characters")]
    TooLong,
    #[error("`{{{field}}}` is not a field a template can use")]
    UnknownField { field: String },
    #[error("a `{{` is never closed")]
    Unterminated,
    #[error("`{{` and `}}` cannot be nested")]
    Nested,
    #[error("the template must not start at the filesystem root")]
    Absolute,
    #[error("the template must not leave its destination directory")]
    Traversal,
    #[error("the template nests more than {MAX_TEMPLATE_DEPTH} directories deep")]
    TooDeep,
    #[error("the template expands to an empty path")]
    EmptyResult,
}

/// The values a template is expanded against.
pub type TemplateValues = BTreeMap<String, String>;

/// Checks a template without expanding it.
///
/// # Errors
///
/// Returns [`TemplateError`] for an unknown field, unbalanced braces, an absolute path, a
/// traversal segment, or a template that is too long or too deeply nested.
pub fn validate(template: &str) -> Result<(), TemplateError> {
    let trimmed = template.trim();
    if trimmed.is_empty() {
        return Err(TemplateError::Empty);
    }
    if trimmed.chars().count() > MAX_TEMPLATE_LENGTH {
        return Err(TemplateError::TooLong);
    }
    if starts_at_root(trimmed) {
        return Err(TemplateError::Absolute);
    }
    let mut field = None::<String>;
    for character in trimmed.chars() {
        match (character, &mut field) {
            ('{', None) => field = Some(String::new()),
            ('{', Some(_)) => return Err(TemplateError::Nested),
            ('}', None) => return Err(TemplateError::Nested),
            ('}', Some(name)) => {
                if !TEMPLATE_FIELDS.contains(&name.as_str()) {
                    return Err(TemplateError::UnknownField {
                        field: name.clone(),
                    });
                }
                field = None;
            }
            (character, Some(name)) => name.push(character),
            (_, None) => {}
        }
    }
    if field.is_some() {
        return Err(TemplateError::Unterminated);
    }
    // A literal `..` segment is refused here; one that only appears after expansion is
    // caught by `expand`, which is why both checks exist.
    if trimmed
        .split('/')
        .any(|segment| segment.trim() == ".." || segment.trim() == ".")
    {
        return Err(TemplateError::Traversal);
    }
    if trimmed
        .split('/')
        .filter(|segment| !segment.is_empty())
        .count()
        > MAX_TEMPLATE_DEPTH
    {
        return Err(TemplateError::TooDeep);
    }
    Ok(())
}

/// Expands `template` into a path relative to `base`.
///
/// The same function is used for the preview and for the actual download, so what a person
/// is shown is what they get. A missing field expands to nothing rather than to the literal
/// `{field}`; a segment that ends up empty is dropped, and a template that produces nothing
/// at all is refused rather than silently writing into the package root.
///
/// # Errors
///
/// Returns [`TemplateError`] when [`validate`] would, or when expansion produces a
/// traversal segment or an empty path.
pub fn expand(
    base: &Path,
    template: &str,
    values: &TemplateValues,
    reserve: usize,
) -> Result<PathBuf, TemplateError> {
    validate(template)?;
    let mut path = base.to_path_buf();
    let segments: Vec<&str> = template
        .trim()
        .split('/')
        .filter(|segment| !segment.trim().is_empty())
        .collect();
    let last = segments.len().saturating_sub(1);
    let mut produced = 0_usize;
    for (index, segment) in segments.iter().enumerate() {
        let expanded = substitute(segment, values);
        // Expansion can produce traversal that the literal template did not contain: a
        // video titled `..` is not exotic. A separator inside a value is *not* traversal —
        // it is an ordinary character in a title, and the sanitiser replaces it — so only
        // the parent-directory shapes are refused.
        if is_traversal(&expanded) {
            return Err(TemplateError::Traversal);
        }
        // Checked before sanitising: the sanitiser substitutes a fallback name for an empty
        // input, which would turn a missing field into a literal `download` directory.
        if expanded.trim().is_empty() {
            continue;
        }
        let sanitized = if index == last {
            sanitize_file_name_within(&path, &expanded, reserve)
        } else {
            sanitize_file_name(&expanded)
        };
        // The sanitiser is not asked to understand traversal; checking its output keeps
        // that assumption from ever becoming load-bearing.
        if is_traversal(&sanitized) {
            return Err(TemplateError::Traversal);
        }
        path.push(sanitized);
        produced += 1;
    }
    if produced == 0 {
        return Err(TemplateError::EmptyResult);
    }
    // Belt and braces: whatever came out must still be under `base`.
    if !path.starts_with(base) || path.components().any(|part| part == Component::ParentDir) {
        return Err(TemplateError::Traversal);
    }
    Ok(path)
}

/// Replaces every `{field}` with its value, or with nothing when there is none.
fn substitute(segment: &str, values: &TemplateValues) -> String {
    let mut output = String::with_capacity(segment.len());
    let mut field = None::<String>;
    for character in segment.chars() {
        match (character, &mut field) {
            ('{', _) => field = Some(String::new()),
            ('}', Some(name)) => {
                if let Some(value) = values.get(name.as_str()) {
                    output.push_str(value);
                }
                field = None;
            }
            (character, Some(name)) => name.push(character),
            (character, None) => output.push(character),
        }
    }
    output
}

/// Whether a segment addresses a parent or the current directory.
fn is_traversal(segment: &str) -> bool {
    let trimmed = segment.trim();
    trimmed == ".." || trimmed == "." || trimmed.starts_with("../") || trimmed.starts_with("..\\")
}

/// Whether a template tries to start at a filesystem root, including a Windows drive or UNC
/// prefix, which `Path::is_absolute` does not catch when running on Linux.
fn starts_at_root(template: &str) -> bool {
    let template = template.trim();
    template.starts_with('/')
        || template.starts_with('\\')
        || template.as_bytes().get(1).is_some_and(|byte| *byte == b':')
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::Path};

    use super::{TemplateError, TemplateValues, expand, validate};

    fn values() -> TemplateValues {
        BTreeMap::from([
            ("title".to_owned(), "Idle Immortal / Trailer".to_owned()),
            ("uploader".to_owned(), "Studio".to_owned()),
            ("upload_date".to_owned(), "20260904".to_owned()),
            ("upload_year".to_owned(), "2026".to_owned()),
            ("id".to_owned(), "abc123".to_owned()),
            ("resolution".to_owned(), "1080p".to_owned()),
            ("ext".to_owned(), "mp4".to_owned()),
        ])
    }

    #[test]
    fn a_template_expands_into_directories_and_a_file_name() {
        let base = Path::new("/downloads/pkg");
        let path = expand(base, "{uploader}/{upload_year}/{title}", &values(), 0).expect("expands");
        assert_eq!(
            path,
            Path::new("/downloads/pkg/Studio/2026/Idle Immortal _ Trailer"),
            "the `/` in the title is sanitised, not treated as a directory"
        );
    }

    #[test]
    fn an_unknown_field_is_refused_at_edit_time() {
        // Leaving it as literal text would put `{shell}` in a filename, which is worse than
        // saying no.
        assert_eq!(
            validate("{title}-{shell}"),
            Err(TemplateError::UnknownField {
                field: "shell".to_owned()
            })
        );
        assert_eq!(validate("{title"), Err(TemplateError::Unterminated));
        assert_eq!(validate("{{title}}"), Err(TemplateError::Nested));
        assert_eq!(validate("   "), Err(TemplateError::Empty));
    }

    #[test]
    fn absolute_paths_and_traversal_are_refused_before_expansion() {
        for template in ["/etc/passwd", "\\\\server\\share", "C:/Windows/x", "c:x"] {
            assert_eq!(
                validate(template),
                Err(TemplateError::Absolute),
                "{template} must not be accepted"
            );
        }
        assert_eq!(validate("../{title}"), Err(TemplateError::Traversal));
        assert_eq!(validate("a/./{title}"), Err(TemplateError::Traversal));
        assert_eq!(
            validate("a/b/c/d/e/f/g/h/i/{title}"),
            Err(TemplateError::TooDeep)
        );
    }

    #[test]
    fn traversal_that_only_appears_after_expansion_is_caught_too() {
        // A video titled `..` is not exotic, and the literal template is blameless.
        let mut values = values();
        values.insert("title".to_owned(), "..".to_owned());
        assert_eq!(
            expand(Path::new("/downloads/pkg"), "{title}", &values, 0),
            Err(TemplateError::Traversal)
        );

        values.insert("uploader".to_owned(), "../../etc".to_owned());
        assert_eq!(
            expand(
                Path::new("/downloads/pkg"),
                "{uploader}/{title}",
                &values,
                0
            ),
            Err(TemplateError::Traversal)
        );
    }

    #[test]
    fn a_missing_field_expands_to_nothing_and_an_empty_segment_is_dropped() {
        let mut values = values();
        values.remove("uploader");
        let path = expand(
            Path::new("/downloads/pkg"),
            "{uploader}/{title}",
            &values,
            0,
        )
        .expect("expands");
        assert_eq!(path, Path::new("/downloads/pkg/Idle Immortal _ Trailer"));

        // A template that produces nothing at all would silently write into the package
        // root, so it is refused instead.
        let empty = BTreeMap::new();
        assert_eq!(
            expand(Path::new("/downloads/pkg"), "{uploader}/{title}", &empty, 0),
            Err(TemplateError::EmptyResult)
        );
    }

    #[test]
    fn reserved_names_and_length_limits_come_from_the_shared_sanitiser() {
        let mut values = values();
        values.insert("title".to_owned(), "CON".to_owned());
        let path = expand(Path::new("/downloads/pkg"), "{title}", &values, 0).expect("expands");
        assert_ne!(
            path.file_name().and_then(|name| name.to_str()),
            Some("CON"),
            "a Windows device name must not survive"
        );

        values.insert("title".to_owned(), "x".repeat(600));
        let path = expand(Path::new("/downloads/pkg"), "{title}", &values, 40).expect("expands");
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("name");
        assert!(name.len() < 600, "the shared limit applies: {}", name.len());
    }

    #[test]
    fn the_preview_and_the_download_use_the_same_evaluator() {
        // Not a tautology: it is the reason `expand` takes the base path rather than
        // formatting a display string separately.
        let base = Path::new("/downloads/pkg");
        let template = "{uploader}/{title} [{resolution}]";
        let first = expand(base, template, &values(), 40).expect("expands");
        let second = expand(base, template, &values(), 40).expect("expands");
        assert_eq!(first, second);
        assert_eq!(
            first,
            Path::new("/downloads/pkg/Studio/Idle Immortal _ Trailer [1080p]")
        );
    }
}
