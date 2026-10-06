use std::path::Path;

const MAX_PASSWORDS: usize = 10_000;

/// The most of the file that is read: ten thousand passwords fit many times over. A path that
/// names something without an end — a device, a huge log — must not fill the memory (audit
/// 2026-10-05, S9).
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

/// Loads one password per line (trimmed, deduplicated, blank lines ignored).
/// A missing file yields an empty list, and so does anything that is not a regular file; of a
/// file longer than [`MAX_FILE_BYTES`] the complete lines within it are read.
///
/// Trimming is deliberate and confined to this function: a line-based file cannot express a
/// leading or trailing space, and every editor that writes one writes it by accident. A password
/// that genuinely carries whitespace belongs on the package, where it survives verbatim
/// (`password_candidates`, RD-107-11).
#[must_use]
pub fn load_password_file(path: &Path) -> Vec<String> {
    let Ok(content) = rd_files::read_text_capped(path, MAX_FILE_BYTES) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| seen.insert((*line).to_owned()))
        .take(MAX_PASSWORDS)
        .map(str::to_owned)
        .collect()
}

/// Candidate order: explicit package password, no password, then the shared list.
///
/// The package password is offered **verbatim** first — it is the one value a user can type with
/// a deliberate leading or trailing space, and the old unconditional `trim()` silently destroyed
/// it (RD-107-11). The trimmed form follows as a second candidate when it differs, so a space
/// pasted in by accident still costs nothing but one extra attempt. This is the single place
/// where a password's whitespace is touched.
#[must_use]
pub fn password_candidates(package: Option<&str>, list: &[String]) -> Vec<Option<String>> {
    let mut candidates = Vec::with_capacity(list.len() + 3);
    let push = |value: String, into: &mut Vec<Option<String>>| {
        let entry = Some(value);
        if !into.contains(&entry) {
            into.push(entry);
        }
    };
    if let Some(explicit) = package.filter(|value| !value.is_empty()) {
        push(explicit.to_owned(), &mut candidates);
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            push(trimmed.to_owned(), &mut candidates);
        }
    }
    candidates.push(None);
    for password in list {
        push(password.clone(), &mut candidates);
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::{MAX_FILE_BYTES, load_password_file, password_candidates};

    #[test]
    fn candidates_prefer_package_password_then_none_then_list() {
        let list = vec!["a".to_owned(), "b".to_owned(), "a".to_owned()];
        assert_eq!(
            password_candidates(Some("b"), &list),
            [Some("b".to_owned()), None, Some("a".to_owned())]
        );
        assert_eq!(password_candidates(None, &[]), [None]);
    }

    #[test]
    fn a_package_password_keeps_its_whitespace_and_gains_a_trimmed_fallback() {
        // RD-107-11: the verbatim value first, because it is the one the user can have meant.
        assert_eq!(
            password_candidates(Some(" b "), &[]),
            [Some(" b ".to_owned()), Some("b".to_owned()), None]
        );
        // A password made of nothing but whitespace survives; trimming it away would leave the
        // package with no password at all.
        assert_eq!(
            password_candidates(Some("  "), &[]),
            [Some("  ".to_owned()), None]
        );
        assert_eq!(password_candidates(Some(""), &[]), [None]);
    }

    #[test]
    fn password_file_is_trimmed_and_deduplicated() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("passwords.txt");
        std::fs::write(&path, " one \n\ntwo\none\n").expect("write");
        assert_eq!(load_password_file(&path), ["one", "two"]);
        assert!(load_password_file(&temp.path().join("missing.txt")).is_empty());
    }

    /// A list longer than [`MAX_FILE_BYTES`] is read up to its last complete line inside the
    /// limit, never as a whole (RD-1120-19, mutation probe B10: the device test alone could not
    /// tell a capped read from an unbounded one, because both end in an error there).
    #[test]
    fn a_list_past_the_limit_is_read_only_up_to_the_limit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("passwords.txt");
        let mut content = String::from("first\n");
        content.push_str(&"x".repeat(MAX_FILE_BYTES));
        content.push_str("\nbeyond-the-limit\n");
        std::fs::write(&path, content).expect("write");
        let passwords = load_password_file(&path);
        assert_eq!(passwords.first().map(String::as_str), Some("first"));
        assert!(
            !passwords
                .iter()
                .any(|password| password == "beyond-the-limit"),
            "a line past the limit was read"
        );
    }

    /// A path the settings point at a device is no list (audit 2026-10-05, S9): read as one,
    /// `/dev/zero` never ends and fills the memory.
    #[cfg(unix)]
    #[test]
    fn a_device_is_no_password_list() {
        assert!(load_password_file(std::path::Path::new("/dev/zero")).is_empty());
    }
}
