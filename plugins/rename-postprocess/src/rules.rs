//! The naming rules, kept free of the guest bindings so they can be tested on the host.

/// Which clean-ups to apply, in the order they are listed here.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// `Big Buck Bunny.mkv` -> `Big.Buck.Bunny.mkv`
    pub spaces_to_dots: bool,
    /// `Big..Buck._.Bunny.mkv` -> `Big.Buck.Bunny.mkv`
    pub collapse_separators: bool,
    /// `Big.Buck.Bunny.[1080p].[x264].mkv` -> `Big.Buck.Bunny.mkv`
    pub strip_bracket_tags: bool,
    /// `Big.Buck.Bunny.MKV` -> `big.buck.bunny.mkv`
    pub lowercase: bool,
}

impl Default for Rules {
    fn default() -> Self {
        // Only the rule that was actually asked for is on by default. A step that silently
        // rewrote more than the user expects is worse than one that does too little.
        Self {
            spaces_to_dots: true,
            collapse_separators: true,
            strip_bracket_tags: false,
            lowercase: false,
        }
    }
}

/// The extension is never rewritten: tooling keys off it, and a lowercased or de-spaced
/// extension can make a file unopenable.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        // A leading dot is a hidden file, not an extension.
        Some(index) if index > 0 => (&name[..index], &name[index..]),
        _ => (name, ""),
    }
}

fn strip_bracket_tags(stem: &str) -> String {
    let mut out = String::with_capacity(stem.len());
    let mut depth = 0_u32;
    for character in stem.chars() {
        match character {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(character),
            _ => {}
        }
    }
    out
}

fn collapse_separators(stem: &str) -> String {
    let mut out = String::with_capacity(stem.len());
    let mut previous_separator = false;
    for character in stem.chars() {
        let separator = matches!(character, '.' | '_' | '-' | ' ');
        if separator {
            if !previous_separator {
                out.push(if character == ' ' { ' ' } else { '.' });
            }
        } else {
            out.push(character);
        }
        previous_separator = separator;
    }
    out.trim_matches(['.', ' ', '_', '-']).to_owned()
}

/// The name this file should have, or `None` when it already has it.
///
/// Returning `None` rather than the unchanged name keeps the caller from asking the host to
/// rename a file to what it is already called, which the host refuses as a name clash.
#[must_use]
pub fn rename_to(name: &str, rules: Rules) -> Option<String> {
    // A leading dot marks a hidden file; it is held back so the separator rules cannot trim it
    // away, which would change what the file is rather than just tidy its name.
    let (hidden, name_body) = match name.strip_prefix('.') {
        Some(rest) => (".", rest),
        None => ("", name),
    };
    let (stem, extension) = split_extension(name_body);
    let mut stem = stem.to_owned();
    if rules.strip_bracket_tags {
        stem = strip_bracket_tags(&stem);
    }
    if rules.spaces_to_dots {
        stem = stem.replace(' ', ".");
    }
    if rules.collapse_separators {
        stem = collapse_separators(&stem);
    }
    if rules.lowercase {
        stem = stem.to_lowercase();
    }
    if stem.is_empty() {
        return None;
    }
    let renamed = format!("{hidden}{stem}{extension}");
    (renamed != name).then_some(renamed)
}

#[cfg(test)]
mod tests {
    use super::{Rules, rename_to};

    #[test]
    fn spaces_become_dots_and_the_extension_is_left_alone() {
        assert_eq!(
            rename_to("Big Buck Bunny.MKV", Rules::default()).as_deref(),
            Some("Big.Buck.Bunny.MKV")
        );
    }

    #[test]
    fn a_name_that_already_matches_is_not_renamed() {
        assert_eq!(rename_to("Big.Buck.Bunny.mkv", Rules::default()), None);
    }

    #[test]
    fn runs_of_separators_collapse_into_one() {
        assert_eq!(
            rename_to("Big..Buck_-_Bunny.mkv", Rules::default()).as_deref(),
            Some("Big.Buck.Bunny.mkv")
        );
    }

    #[test]
    fn bracket_tags_are_only_stripped_when_asked_for() {
        let name = "Big Buck Bunny [1080p] (x264).mkv";
        assert_eq!(
            rename_to(name, Rules::default()).as_deref(),
            Some("Big.Buck.Bunny.[1080p].(x264).mkv"),
            "off by default: the tags are often what identifies a release"
        );
        assert_eq!(
            rename_to(
                name,
                Rules {
                    strip_bracket_tags: true,
                    ..Rules::default()
                }
            )
            .as_deref(),
            Some("Big.Buck.Bunny.mkv")
        );
    }

    #[test]
    fn lowercasing_leaves_the_extension_as_it_is() {
        assert_eq!(
            rename_to(
                "Big Buck Bunny.MKV",
                Rules {
                    lowercase: true,
                    ..Rules::default()
                }
            )
            .as_deref(),
            Some("big.buck.bunny.MKV")
        );
    }

    #[test]
    fn a_hidden_file_keeps_its_leading_dot() {
        assert_eq!(
            rename_to(".hidden file", Rules::default()).as_deref(),
            Some(".hidden.file")
        );
    }

    #[test]
    fn a_name_that_would_become_empty_is_left_alone() {
        assert_eq!(
            rename_to(
                "[1080p].mkv",
                Rules {
                    strip_bracket_tags: true,
                    ..Rules::default()
                }
            ),
            None
        );
    }
}
