//! Golden tests for [`rd_core::redact_text`].
//!
//! Each case is a pair of files: `redaction/inputs/<case>.txt` holds a realistic log line,
//! error message or URL, and `redaction/expected/<case>.txt` holds exactly what redaction
//! must turn it into.
//!
//! The expected files are hand-maintained on purpose. There is deliberately **no**
//! `UPDATE_GOLDEN` environment escape: a wrong redaction must not be blessable by re-running
//! the suite, because the whole point of these files is that a human looked at them and
//! confirmed no secret survives.

use std::{fs, path::Path};

/// Secret values planted in the inputs. None of them may appear in any expected file.
///
/// This is the assertion that actually protects users; the byte-for-byte comparison only
/// protects the formatting.
const FIXTURE_SECRETS: &[&str] = &[
    "fe5f80f77d5fa3beca038a248ff027d0445342fe2855ddc963176630326f1024",
    "AKIAIOSFODNN7EXAMPLE",
    "abcdEFGH1234",
    "1a2b3c4d5e6f",
    "Gp7uBcQ~zzz__",
    "APKAIEXAMPLE",
    "eyJTdGF0ZW1lbnQiOlt7fV19",
    "abc123~def",
    "9f8e7d6c5b4a",
    "deadbeefcafebabe",
    "0123456789abcdef",
    "sk-live-9f8e7d6c5b4a3f2e1d0c",
    "dXNlcjpodW50ZXIy",
    "abc123; csrftoken=xyz789",
    "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
    "deadbeef",
    "abcdef123",
    "hunter2",
    "secretvalue",
    "zzz999",
];

fn case_directory() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/redaction"))
}

/// Every case name, taken from the input files so a new fixture cannot be forgotten.
fn cases() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(case_directory().join("inputs"))
        .expect("inputs directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
        .map(|path| {
            path.file_stem()
                .expect("stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no redaction fixtures found");
    names
}

#[test]
fn every_case_matches_its_golden_file() {
    for case in cases() {
        let input = fs::read_to_string(case_directory().join("inputs").join(format!("{case}.txt")))
            .expect("input");
        let expected_path = case_directory()
            .join("expected")
            .join(format!("{case}.txt"));
        let expected = fs::read_to_string(&expected_path).unwrap_or_else(|_| {
            panic!(
                "missing golden file {}; add it by hand",
                expected_path.display()
            )
        });
        assert_eq!(
            rd_core::redact_text(&input),
            expected,
            "redaction changed for case `{case}`"
        );
    }
}

#[test]
fn redaction_is_idempotent_for_every_case() {
    // The same value passes engine -> scheduler -> database -> SSE, so a second pass over
    // an already-redacted string must be a no-op.
    for case in cases() {
        let input = fs::read_to_string(case_directory().join("inputs").join(format!("{case}.txt")))
            .expect("input");
        let once = rd_core::redact_text(&input);
        assert_eq!(
            rd_core::redact_text(&once),
            once,
            "redaction is not idempotent for case `{case}`"
        );
    }
}

#[test]
fn no_golden_file_contains_a_planted_secret() {
    for case in cases() {
        let expected = fs::read_to_string(
            case_directory()
                .join("expected")
                .join(format!("{case}.txt")),
        )
        .unwrap_or_else(|_| panic!("missing golden file for `{case}`"));
        for secret in FIXTURE_SECRETS {
            assert!(
                !expected.contains(secret),
                "golden file for `{case}` leaks `{secret}`"
            );
        }
    }
}
