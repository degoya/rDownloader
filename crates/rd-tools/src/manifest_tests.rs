use super::*;

fn entry(version: &str, min: Option<&str>, max: Option<&str>) -> ToolEntry {
    ToolEntry {
        name: "yt-dlp".to_owned(),
        version: version.to_owned(),
        platform: "x86_64-unknown-linux-gnu".to_owned(),
        url: "https://example.invalid/yt-dlp".to_owned(),
        sha256: "0".repeat(64),
        size: 1024,
        archive: ArchiveFormat::Raw,
        members: Vec::new(),
        min_app_version: min.map(str::to_owned),
        max_app_version: max.map(str::to_owned),
    }
}

fn manifest(tools: Vec<ToolEntry>) -> ToolManifest {
    ToolManifest {
        schema_version: TOOL_MANIFEST_SCHEMA_VERSION,
        sequence: 1,
        issued_at: Utc::now(),
        not_after: None,
        tools,
        compatibility: Vec::new(),
    }
}

/// The whole point of the compatibility window: a build outside it is not offered.
#[test]
fn a_release_outside_the_compatibility_window_is_not_offered() {
    let manifest = manifest(vec![
        entry("2024.01.01", None, Some("1.0.0")),
        entry("2024.06.01", Some("1.0.0"), None),
    ]);
    let offered = manifest.releases("yt-dlp", "x86_64-unknown-linux-gnu", "1.0.1");
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].version, "2024.06.01");
}

/// An unreadable bound refuses the entry rather than installing it anyway.
#[test]
fn an_unparseable_compatibility_bound_refuses_the_entry() {
    assert!(!entry("2024.01.01", Some("whenever"), None).suits_application("1.0.1"));
}

/// Newest first, and a non-semver tool version still orders sensibly.
#[test]
fn releases_are_offered_newest_first() {
    let manifest = manifest(vec![
        entry("2024.01.01", None, None),
        entry("2024.09.07", None, None),
        entry("2024.06.01", None, None),
    ]);
    let offered = manifest.releases("yt-dlp", "x86_64-unknown-linux-gnu", "1.0.1");
    let versions: Vec<&str> = offered.iter().map(|entry| entry.version.as_str()).collect();
    assert_eq!(versions, ["2024.09.07", "2024.06.01", "2024.01.01"]);
}

/// A name outside the closed list must not reach the download path at all.
#[test]
fn an_unmanaged_tool_name_is_refused_by_validation() {
    let mut foreign = entry("1.0.0", None, None);
    foreign.name = "curl".to_owned();
    assert!(matches!(
        foreign.validate(),
        Err(ToolError::NotManaged(name)) if name == "curl"
    ));
}

/// http:// in a signed document is still http://; the signature does not make it safe to
/// take bytes over it.
#[test]
fn a_plain_http_url_is_refused() {
    let mut insecure = entry("1.0.0", None, None);
    insecure.url = "http://example.invalid/yt-dlp".to_owned();
    assert!(matches!(
        insecure.validate(),
        Err(ToolError::ManifestUntrusted(_))
    ));
}

/// The build's own manifest has to verify against the build's own root, or the feature
/// ships broken.
#[test]
fn the_embedded_manifest_verifies_against_the_compiled_in_root() {
    let manifest = embedded(Utc::now()).expect("embedded manifest verifies");
    assert_eq!(manifest.schema_version, TOOL_MANIFEST_SCHEMA_VERSION);
}

/// Every shipped entry has to pass the validation an install applies. A hash of the wrong
/// length or a name outside [`MANAGED_TOOLS`] would otherwise only surface at the moment
/// somebody tries to install it.
#[test]
fn every_entry_of_the_embedded_manifest_validates() {
    let manifest = embedded(Utc::now()).expect("embedded manifest verifies");
    assert!(
        !manifest.tools.is_empty(),
        "the shipped manifest has to carry the builds it promises"
    );
    for entry in &manifest.tools {
        if let Err(error) = entry.validate() {
            panic!(
                "{} {} on {} is unusable: {error}",
                entry.name, entry.version, entry.platform
            );
        }
    }
}

/// Every archive entry names, for its own platform, the program the resolver looks for:
/// `<name>.exe` on Windows, `<name>` elsewhere (RD-140-08). A raw entry is written under
/// that name by the installer itself, so only archives can get it wrong.
#[test]
fn every_archive_entry_names_its_program_for_its_platform() {
    let manifest = embedded(Utc::now()).expect("embedded manifest verifies");
    for entry in &manifest.tools {
        if entry.archive == ArchiveFormat::Raw {
            continue;
        }
        let program = if entry.platform.contains("-windows-") {
            format!("{}.exe", entry.name)
        } else {
            entry.name.clone()
        };
        assert!(
            entry
                .members
                .iter()
                .any(|member| member.rsplit(['/', '\\']).next() == Some(program.as_str())),
            "{} {} on {} names no member called {program}",
            entry.name,
            entry.version,
            entry.platform
        );
    }
}

/// BtbN keeps a daily build for 14 days and the last build of each month for two years, and
/// `latest` floats, so a BtbN entry is pinned to an `autobuild-<YYYY-MM-DD>-…` release dated
/// the last day of its month. 1.10.0 shipped a mid-month build that answered 404 three
/// weeks later (RD-1101-13). Whether the URL answers today is
/// `scripts/tools-manifest-check.sh`'s question, asked weekly in CI.
#[test]
fn every_btbn_entry_is_pinned_to_a_month_end_release() {
    use chrono::{Datelike, NaiveDate};

    const BTBN: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/";
    let manifest = embedded(Utc::now()).expect("embedded manifest verifies");
    for entry in &manifest.tools {
        let Some(release) = entry.url.strip_prefix(BTBN) else {
            continue;
        };
        let date = release
            .strip_prefix("autobuild-")
            .and_then(|tag| tag.get(..10))
            .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok());
        let Some(date) = date else {
            panic!(
                "{} on {} is not a dated autobuild: {}",
                entry.name, entry.platform, entry.url
            );
        };
        assert!(
            date.succ_opt()
                .is_some_and(|next| next.month() != date.month()),
            "{} on {} is pinned to {date}, a daily build BtbN deletes after 14 days",
            entry.name,
            entry.platform
        );
    }
}

/// What the shipped manifest actually covers, written down as a test so the support
/// matrix in `docs/external-tools.md` cannot drift away from the document.
#[test]
fn the_embedded_manifest_covers_the_platforms_the_documentation_claims() {
    let manifest = embedded(Utc::now()).expect("embedded manifest verifies");
    let application = env!("CARGO_PKG_VERSION");
    for platform in ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"] {
        for tool in ["yt-dlp", "ffmpeg", "ffprobe"] {
            assert!(
                manifest
                    .newest_release(tool, platform, application)
                    .is_some(),
                "{tool} on {platform}"
            );
        }
    }
    for platform in ["aarch64-unknown-linux-gnu", "aarch64-pc-windows-msvc"] {
        assert!(
            manifest
                .newest_release("yt-dlp", platform, application)
                .is_some(),
            "yt-dlp on {platform}"
        );
    }
    // gallery-dl and streamlink are managed tools with no manageable release; see
    // `docs/external-tools.md` for why. They resolve through the vendor folders instead.
    for tool in ["gallery-dl", "streamlink"] {
        assert!(
            manifest
                .newest_release(tool, "x86_64-unknown-linux-gnu", application)
                .is_none(),
            "{tool}"
        );
    }
}
