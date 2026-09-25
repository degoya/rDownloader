//! Reading what the provider answers, and turning its vocabulary into the contract's five.
//!
//! No JSON parser, for the reason the crawler scaffold gives: a sandboxed guest pays for
//! every dependency in code size and in surface, and a job document is a flat object with two
//! small arrays in it. Scanned rather than parsed, therefore. Replace this module wholesale if
//! your provider answers in XML or in anything else that wants a real parser.
//!
//! The mapping is the part worth thinking about. A provider names more states than the
//! contract does — ten at one of them, against `preparing`, `awaiting-choice`, `working`,
//! `ready` and `failed` — so three decisions are made here rather than in the component:
//!
//! - Anything that happens *before* the bytes move and anything that happens *after* they
//!   have moved but before the addresses exist is `preparing`. "Queued" is not "working at
//!   zero percent": a person watching a speed of zero for an hour concludes it is broken.
//! - A state this plugin does not recognise **waits**. A provider that adds a word to its
//!   vocabulary must not cost somebody a job that was going perfectly well.
//! - Only a state the provider calls terminal is `failed`, and the reason travels as a
//!   stable code — never as the provider's own sentence, which nobody has translated.

/// Where the provider says a job stands, in the contract's vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    /// Nothing to do but wait. The number is a suggested wait in seconds, which the host
    /// treats as a suggestion: it owns the clock.
    Preparing(u64),
    /// Nothing moves until a person has chosen which entries they want.
    AwaitingChoice,
    /// The provider is fetching.
    Working,
    /// Finished; the addresses are in the document.
    Ready,
    /// The provider ended it. Terminal.
    Failed,
}

/// Suggested wait while a job is being prepared.
///
/// Short enough that a magnet which converts in seconds is not left sitting, long enough that
/// a slow one does not spend an account's request budget. The host clamps it either way.
const PREPARING_SECONDS: u64 = 15;

/// The stage a provider's own state word means.
///
/// Replace the words, keep the shape — above all the final arm: an unknown state waits.
#[must_use]
pub fn stage_of(state: &str) -> Stage {
    match state.trim().to_ascii_lowercase().as_str() {
        // Before the bytes move: accepted, queued, and a magnet still being turned into a
        // torrent. None of these is "downloading at zero percent".
        "queued" | "waiting" | "converting" => Stage::Preparing(PREPARING_SECONDS),
        // After the bytes have moved and before the addresses exist. Also `preparing`: the
        // job is not finished and there is nothing to show a speed for.
        "compressing" | "uploading" => Stage::Preparing(PREPARING_SECONDS),
        "waiting_for_selection" => Stage::AwaitingChoice,
        "downloading" => Stage::Working,
        "finished" => Stage::Ready,
        "error" | "dead" | "removed" => Stage::Failed,
        _ => Stage::Preparing(PREPARING_SECONDS),
    }
}

/// A percentage the provider reports, as the thousandths the contract carries.
///
/// Above 100 is dropped rather than clamped: a provider that answers 4000 is not reporting
/// progress, and a bar that jumps to full and stays there is worse than no bar.
#[must_use]
pub fn permille(percent: Option<u64>) -> Option<u16> {
    percent
        .filter(|value| *value <= 100)
        .and_then(|value| u16::try_from(value * 10).ok())
}

/// The string value of a JSON field inside one object's text.
#[must_use]
pub fn string_field(body: &str, name: &str) -> Option<String> {
    let mut rest = value_after(body, name)?.strip_prefix('"')?;
    let mut out = String::new();
    loop {
        let mut chars = rest.chars();
        let character = chars.next()?;
        rest = chars.as_str();
        match character {
            '"' => return Some(out),
            '\\' => {
                let mut escaped = rest.chars();
                match escaped.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    other => out.push(other),
                }
                rest = escaped.as_str();
            }
            other => out.push(other),
        }
    }
}

/// The numeric value of a JSON field, quoted or not — providers spell sizes both ways.
#[must_use]
pub fn number_field(body: &str, name: &str) -> Option<u64> {
    let rest = value_after(body, name)?;
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    let end = rest
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Whether a JSON field is the literal `true`.
#[must_use]
pub fn flag_field(body: &str, name: &str) -> bool {
    let Some(rest) = value_after(body, name) else {
        return false;
    };
    let value = rest.trim_start_matches('"');
    let end = value
        .find(|character: char| !character.is_ascii_alphanumeric())
        .unwrap_or(value.len());
    // Both spellings, because a provider that answers `1` and a provider that answers `true`
    // mean the same thing and neither is going to change for this plugin. Matched whole
    // rather than by prefix: `10` is not `1`, and a size is not a flag.
    matches!(&value[..end], "true" | "1")
}

fn value_after<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("\"{name}\"");
    let mut from = 0;
    while let Some(at) = body[from..].find(&needle) {
        let after = &body[from + at + needle.len()..];
        if let Some(value) = after.trim_start().strip_prefix(':') {
            return Some(value.trim_start());
        }
        from += at + needle.len();
    }
    None
}

/// Splits the objects of the named JSON array, respecting nesting and strings.
///
/// Written out rather than taken from a parser because the alternative — cutting on `},{` —
/// is wrong the moment an entry carries a nested object or a brace inside a file name, and
/// wrong in the direction that silently loses files.
#[must_use]
pub fn objects(body: &str, array_field: &str) -> Vec<String> {
    let Some(rest) = value_after(body, array_field).and_then(|rest| rest.strip_prefix('[')) else {
        return Vec::new();
    };
    let mut objects = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in rest.char_indices() {
        if in_string {
            match character {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = index;
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    objects.push(rest[start..=index].to_owned());
                }
            }
            ']' if depth == 0 => break,
            _ => {}
        }
    }
    objects
}

/// A path inside the job, split into the file name and the package it belongs under.
///
/// The host reduces both to something that cannot leave the package, so this does not have to
/// be defensive — but it does have to be *useful*: a finished job with a season in it should
/// arrive as a package and not as forty loose links, and the only thing that says so is the
/// path the provider kept.
#[must_use]
pub fn place(path: &str) -> (Option<String>, Option<String>) {
    let cleaned = path.trim().trim_matches('/');
    if cleaned.is_empty() {
        return (None, None);
    }
    match cleaned.rsplit_once('/') {
        Some((parent, name)) => (
            Some(name.to_owned()).filter(|name| !name.is_empty()),
            Some(parent.to_owned()).filter(|parent| !parent.is_empty()),
        ),
        None => (Some(cleaned.to_owned()), None),
    }
}

/// Whether an identifier is safe to splice into a request path.
///
/// It comes back from the provider and goes out again in a URL, which is the shape of a
/// request built for somebody else's endpoint. Checked rather than escaped: an identifier is
/// opaque to this plugin, so the narrow spelling the provider issues is the whole of what it
/// ever has to accept.
#[must_use]
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::{
        Stage, flag_field, is_safe_id, number_field, objects, permille, place, stage_of,
        string_field,
    };

    const JOB: &str = r#"{
      "id": "j-1",
      "state": "waiting_for_selection",
      "progress": 42,
      "files": [
        {"id": 1, "path": "Show/S01/e01 {1080p}.mkv", "size": "1024", "selected": false},
        {"id": 2, "path": "Show/S01/e02.mkv", "size": 2048, "selected": true, "meta": {"a": 1}}
      ],
      "links": []
    }"#;

    #[test]
    fn a_job_document_is_read_without_a_parser() {
        assert_eq!(string_field(JOB, "id").as_deref(), Some("j-1"));
        assert_eq!(number_field(JOB, "progress"), Some(42));
        let files = objects(JOB, "files");
        // Two, not three: the brace inside the first path does not split it, and the nested
        // object in the second does not close it early.
        assert_eq!(files.len(), 2);
        assert_eq!(
            string_field(&files[0], "path").as_deref(),
            Some("Show/S01/e01 {1080p}.mkv")
        );
        assert_eq!(number_field(&files[0], "size"), Some(1024));
        assert!(!flag_field(&files[0], "selected"));
        assert!(flag_field(&files[1], "selected"));
        assert!(objects(JOB, "links").is_empty());
    }

    #[test]
    fn an_unknown_state_waits_instead_of_failing_the_job() {
        // The arm that matters most. A provider that adds a word must not cost somebody a
        // job that was going perfectly well.
        assert_eq!(
            stage_of("something_new"),
            Stage::Preparing(super::PREPARING_SECONDS)
        );
        assert_eq!(stage_of(""), Stage::Preparing(super::PREPARING_SECONDS));
    }

    #[test]
    fn the_states_that_are_not_downloading_are_not_working() {
        // "Queued" as `working` shows a speed of zero for an hour, which reads as broken.
        assert_eq!(
            stage_of("queued"),
            Stage::Preparing(super::PREPARING_SECONDS)
        );
        assert_eq!(
            stage_of("Compressing"),
            Stage::Preparing(super::PREPARING_SECONDS)
        );
        assert_eq!(
            stage_of("uploading"),
            Stage::Preparing(super::PREPARING_SECONDS)
        );
        assert_eq!(stage_of("downloading"), Stage::Working);
        assert_eq!(stage_of("waiting_for_selection"), Stage::AwaitingChoice);
        assert_eq!(stage_of("finished"), Stage::Ready);
        assert_eq!(stage_of("dead"), Stage::Failed);
    }

    #[test]
    fn progress_is_thousandths_and_nonsense_is_dropped() {
        assert_eq!(permille(Some(0)), Some(0));
        assert_eq!(permille(Some(42)), Some(420));
        assert_eq!(permille(Some(100)), Some(1000));
        assert_eq!(permille(Some(4000)), None);
        assert_eq!(permille(None), None);
    }

    #[test]
    fn a_path_becomes_a_name_and_the_package_it_belongs_under() {
        assert_eq!(
            place("Show/S01/e01.mkv"),
            (Some("e01.mkv".to_owned()), Some("Show/S01".to_owned()))
        );
        assert_eq!(place("single.mkv"), (Some("single.mkv".to_owned()), None));
        assert_eq!(
            place("/lead/and/trail/"),
            (Some("trail".to_owned()), Some("lead/and".to_owned()))
        );
        assert_eq!(place("   "), (None, None));
    }

    #[test]
    fn an_identifier_that_is_not_one_never_reaches_a_url() {
        assert!(is_safe_id("j-1"));
        assert!(!is_safe_id(""));
        assert!(!is_safe_id("../../account/delete"));
        assert!(!is_safe_id("j 1"));
        assert!(!is_safe_id(&"x".repeat(65)));
    }
}
