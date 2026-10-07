use super::*;

fn version(text: &str) -> semver::Version {
    semver::Version::parse(text).expect("version")
}

const NOTES: &str = "# Release notes\n\n<!-- How to write a section: see the release skill. -->\n\n\
## 1.9.0\n\n<!-- draft: the coordinator completes it -->\n\n- Something new.\n\n\
## 1.8.0\n\n- Downloads from file hosters run in parallel.\n- A long point that wraps\n  onto \
a second line.\n\n## 1.7.1\n\nMaintenance release: internal changes only, no change in \
behaviour.\n";

#[test]
fn the_points_of_the_version_become_the_notes() {
    assert_eq!(
        user_notes(NOTES, &version("1.8.0"))
            .expect("notes")
            .as_deref(),
        Some(
            "- Downloads from file hosters run in parallel.\n- A long point that wraps onto a \
             second line."
        )
    );
    assert_eq!(
        user_notes(NOTES, &version("1.7.1"))
            .expect("notes")
            .as_deref(),
        Some(MAINTENANCE)
    );
    // A beta falls back to its release's section; a version without one has no notes.
    assert!(
        user_notes(NOTES, &version("1.8.0-beta.1"))
            .expect("notes")
            .is_some_and(|notes| notes.starts_with("- Downloads"))
    );
    assert_eq!(user_notes(NOTES, &version("2.0.0")).expect("notes"), None);
}

#[test]
fn a_draft_section_does_not_ship() {
    let error = user_notes(NOTES, &version("1.9.0")).expect_err("a draft");
    assert!(error.to_string().contains("draft"), "{error}");
}

/// What the 1.13.0 dialog showed, and what else a developer's note carries, is refused.
#[test]
fn developer_notes_are_refused() {
    for (point, problem) in [
        (
            "Narrower visibility in every crate (RD-1120-12, CR-9).",
            "names a job",
        ),
        ("The plugin PL-12 starts.", "names a job"),
        (
            "The chain no longer stops at a stale `web/dist`.",
            "names a code identifier",
        ),
        (
            "A new setting unwrap_package_folder.",
            "names a code identifier",
        ),
        ("Fixed in crates/rd-http/src/lib.rs.", "names a path"),
        ("See scripts/release.sh for details.", "names a path"),
        ("Without a full stop", "does not end a sentence"),
        ("Neue Funktion f\u{fc}r alle.", "is not English"),
    ] {
        let problems = problems(&parse(&format!("- {point}\n")));
        assert!(
            problems.iter().any(|found| found.contains(problem)),
            "{point}: {problems:?}"
        );
    }
    let long = format!("- {}.\n", "word ".repeat(45));
    assert!(problems(&parse(&long))[0].contains("at most 200"));
    let many = "- A point.\n".repeat(MAX_POINTS + 1);
    assert!(problems(&parse(&many))[0].contains("at most 8"));
    assert!(problems(&parse("Some other paragraph.\n"))[0].contains("not a list of points"));
    assert!(problems(&parse("Intro.\n\n- A point.\n"))[0].contains("mixes"));
    assert!(problems(&parse("\n"))[0].contains("no points"));
}

/// Plain words that look a little like code pass: a version, a domain, "and/or", a time.
#[test]
fn ordinary_words_pass() {
    let parsed = parse(
        "- Version 1.15 checks example.com and/or its mirrors at 10:30.\n\
         - The **Updates** page links the [full changes](https://example.test/x.md).\n",
    );
    assert_eq!(problems(&parsed), Vec::<String>::new());
    assert_eq!(parsed.points[1], "The Updates page links the full changes.");
}

/// GitHub's anchor of a `## [X.Y.Z] - YYYY-MM-DD` heading: the dots and brackets dropped, each
/// space a hyphen.
#[test]
fn the_changelog_anchor_follows_githubs_rule() {
    assert_eq!(github_anchor("[1.15.0] - 2026-10-10"), "1150---2026-10-10");
    assert_eq!(
        github_anchor("[1.8.0-beta.1] - 2026-09-30"),
        "180-beta1---2026-09-30"
    );
    let changelog = "# Changelog\n\n## [Unreleased]\n\n## [1.8.0] - 2026-10-10\n\n### Added\n\n\
                     ## [1.7.0] - 2026-09-30\n";
    assert_eq!(
        changelog_anchor(changelog, &version("1.8.0")).as_deref(),
        Some("180---2026-10-10")
    );
    assert_eq!(
        changelog_anchor(changelog, &version("1.8.0-beta.2")).as_deref(),
        Some("180---2026-10-10")
    );
    assert_eq!(changelog_anchor(changelog, &version("1.9.0")), None);
}
