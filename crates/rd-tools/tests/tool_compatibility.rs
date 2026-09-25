//! RD-102-03 against real processes: fake binaries that print a chosen version string, the
//! cache that stops the settings page spawning eight of them per load, and the degradation
//! rule that keeps the compiled-in base in force when a delivered rule set cannot be read.
//!
//! Unix only, because the fixtures are shell scripts. The parsing matrix itself is
//! platform-independent and lives in `rd_tools::version`'s unit tests, which do run on
//! Windows; what needs a real process is the spawn, the timeout and the cache.

#![cfg(unix)]

use std::{os::unix::fs::PermissionsExt, path::Path, path::PathBuf};

use rd_tools::{
    Capability, CompatRule, CompatRules, Verdict, compat,
    manifest::{TOOL_MANIFEST_SCHEMA_VERSION, ToolManifest},
    version,
};

/// Writes an executable shell script and returns its path.
fn script(directory: &Path, name: &str, body: &str) -> PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, body).expect("write script");
    let mut permissions = std::fs::metadata(&path).expect("stat").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).expect("chmod");
    path
}

/// A fake tool that prints one line and exits.
fn tool(directory: &Path, name: &str, line: &str) -> PathBuf {
    script(directory, name, &format!("#!/bin/sh\necho '{line}'\n"))
}

fn manifest(rules: Vec<CompatRule>) -> ToolManifest {
    ToolManifest {
        schema_version: TOOL_MANIFEST_SCHEMA_VERSION,
        sequence: 1,
        issued_at: chrono::Utc::now(),
        not_after: None,
        tools: Vec::new(),
        compatibility: rules,
    }
}

/// The version really is read from the process, for each tool's own output shape.
#[tokio::test]
async fn a_real_process_reports_its_version_in_the_shape_its_tool_uses() {
    let directory = tempfile::tempdir().expect("tempdir");
    let cases = [
        ("yt-dlp", "2024.08.06", "2024.08.06"),
        (
            "ffmpeg",
            "ffmpeg version 6.1.1-3ubuntu5 Copyright (c) 2000-2023 the FFmpeg developers",
            "6.1.1-3ubuntu5",
        ),
        ("streamlink", "streamlink 6.7.4", "6.7.4"),
        ("gallery-dl", "gallery-dl 1.27.1", "1.27.1"),
    ];
    for (name, line, expected) in cases {
        let path = tool(directory.path(), name, line);
        let detected = version::detect(name, &path).await;
        assert_eq!(
            detected.parsed.as_ref().map(|value| value.text()),
            Some(expected),
            "{name} reported {line:?}"
        );
        assert_eq!(detected.raw.as_deref(), Some(line));
    }
}

/// A binary that fails, hangs briefly or prints nothing usable yields no version — and never
/// a version zero, which would read as "ancient" and could block.
#[tokio::test]
async fn a_binary_that_answers_nothing_usable_yields_no_version() {
    let directory = tempfile::tempdir().expect("tempdir");
    let failing = script(directory.path(), "yt-dlp", "#!/bin/sh\nexit 1\n");
    assert!(version::detect("yt-dlp", &failing).await.parsed.is_none());
    let silent = script(directory.path(), "gallery-dl", "#!/bin/sh\nexit 0\n");
    assert!(
        version::detect("gallery-dl", &silent)
            .await
            .parsed
            .is_none()
    );
    let missing = directory.path().join("not-here");
    assert_eq!(version::detect("yt-dlp", &missing).await.parsed, None);
}

/// The cache is the reason the settings page stopped spawning a process per tool per load:
/// the same unchanged binary is asked once, and a replaced one is asked again.
#[tokio::test]
async fn the_version_of_an_unchanged_binary_is_read_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let counter = directory.path().join("runs.txt");
    let path = script(
        directory.path(),
        "yt-dlp",
        &format!(
            "#!/bin/sh\necho run >> '{}'\necho '2024.08.06'\n",
            counter.display()
        ),
    );
    version::clear_cache();
    for _ in 0..5 {
        assert_eq!(
            version::detect("yt-dlp", &path)
                .await
                .parsed
                .map(|value| value.text().to_owned()),
            Some("2024.08.06".to_owned())
        );
    }
    let runs = std::fs::read_to_string(&counter).unwrap_or_default();
    assert_eq!(runs.lines().count(), 1, "spawned more than once: {runs:?}");

    // Replacing the binary changes its size and modification time, which is exactly what the
    // key is made of, so nothing has to remember to invalidate anything.
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\necho run >> '{}'\necho '2025.02.02'\n# a longer script\n",
            counter.display()
        ),
    )
    .expect("replace script");
    assert_eq!(
        version::detect("yt-dlp", &path)
            .await
            .parsed
            .map(|value| value.text().to_owned()),
        Some("2025.02.02".to_owned())
    );
    let runs = std::fs::read_to_string(&counter).unwrap_or_default();
    assert_eq!(runs.lines().count(), 2, "did not re-read: {runs:?}");
}

/// The rules in force are process-wide, so everything that reads or writes them is asserted in
/// one test rather than in several that could interleave.
///
/// Covers, in order: the compiled-in base being in force by default; a usable delivered rule
/// set replacing only what it names; every kind of unusable rule degrading back to that base;
/// and an override keeping the verdict while dropping the block, which is the auditable half
/// of the override rule.
#[tokio::test]
async fn the_rules_in_force_degrade_to_the_embedded_base_and_honour_an_override() {
    let directory = tempfile::tempdir().expect("tempdir");
    let old = tool(directory.path(), "yt-dlp", "2020.01.01");
    version::clear_cache();

    // The compiled-in base is what an installation that never saw a manifest runs on.
    compat::set_rules(CompatRules::base());
    compat::set_overrides(&[]);
    let assessment = compat::assess("yt-dlp", &old).await;
    assert_eq!(assessment.verdict, Verdict::TooOld);
    assert!(assessment.blocks(Capability::MediaDownload));
    assert!(!assessment.blocks(Capability::GalleryDownload));

    // A verified manifest may carry a policy, and it lays over the base per tool.
    compat::adopt_manifest(&manifest(vec![CompatRule {
        tool: "yt-dlp".to_owned(),
        min_version: Some("2019.01.01".to_owned()),
        known_bad: vec!["2020.02.02".to_owned()],
        affects: vec![Capability::MediaDownload],
    }]));
    assert_eq!(
        compat::assess("yt-dlp", &old).await.verdict,
        Verdict::Supported,
        "the delivered floor was not adopted"
    );
    assert_eq!(
        compat::rules()
            .rule("ffmpeg")
            .and_then(|rule| rule.min_version.clone()),
        Some("4.4".to_owned()),
        "adopting a rule for one tool dropped the base rule for another"
    );

    // Every way a rule can be unusable leaves the compiled-in base in force rather than a
    // half-read set: an unknown tool, an unreadable version, and a rule that gates nothing.
    for broken in [
        CompatRule {
            tool: "curl".to_owned(),
            min_version: Some("8.0.0".to_owned()),
            known_bad: Vec::new(),
            affects: vec![Capability::MediaDownload],
        },
        CompatRule {
            tool: "yt-dlp".to_owned(),
            min_version: Some("whenever".to_owned()),
            known_bad: Vec::new(),
            affects: vec![Capability::MediaDownload],
        },
        CompatRule {
            tool: "yt-dlp".to_owned(),
            min_version: Some("2019.01.01".to_owned()),
            known_bad: Vec::new(),
            affects: Vec::new(),
        },
    ] {
        compat::adopt_manifest(&manifest(vec![broken]));
        assert_eq!(
            compat::assess("yt-dlp", &old).await.verdict,
            Verdict::TooOld,
            "a broken rule set did not degrade to the compiled-in base"
        );
    }

    // An override is explicit: the verdict and the warning stay, the block goes.
    compat::set_overrides(&["yt-dlp".to_owned()]);
    let overridden = compat::assess("yt-dlp", &old).await;
    assert_eq!(overridden.verdict, Verdict::TooOld);
    assert!(overridden.warns());
    assert!(overridden.overridden);
    assert!(!overridden.blocks(Capability::MediaDownload));

    // An override names a tool, not every tool.
    let stale_ffmpeg = tool(
        directory.path(),
        "ffmpeg",
        "ffmpeg version 3.4.8 Copyright (c)",
    );
    let other = compat::assess("ffmpeg", &stale_ffmpeg).await;
    assert!(other.blocks(Capability::MediaMerge));

    compat::set_overrides(&[]);
    compat::set_rules(CompatRules::base());
}
