use super::{ToolVersion, parse_output};

fn version(text: &str) -> ToolVersion {
    ToolVersion::parse(text).expect("parses")
}

/// Padding with zeros is what makes `6.1` and `6.1.0` the same version, and a fourth
/// component (yt-dlp's nightly) newer than the release it extends.
#[test]
fn missing_components_count_as_zero() {
    assert_eq!(version("6.1"), version("6.1.0"));
    assert!(version("2024.08.06.232855") > version("2024.08.06"));
    assert!(version("2024.08.06") > version("2023.12.31"));
}

/// A date-style version is not SemVer and must not be read as one: `2024.8.6` has to be
/// older than `2024.10.1`, which a string compare gets wrong.
#[test]
fn date_style_versions_order_numerically() {
    assert!(version("2024.10.01") > version("2024.8.6"));
}

/// A distribution suffix is packaging metadata, not a pre-release; treating it as one
/// would put Debian's build of 6.1.1 below 6.1.1 and make that minimum unreachable.
#[test]
fn a_packaging_suffix_does_not_sort_below_the_release() {
    assert_eq!(version("6.1.1-3ubuntu5"), version("6.1.1"));
    assert!(!version("6.1.1-3ubuntu5").is_pre_release());
}

/// A named pre-release does sort below the release.
#[test]
fn a_named_pre_release_sorts_below_the_release() {
    assert!(version("7.0-rc1") < version("7.0"));
    assert!(version("1.28.0-dev") < version("1.28.0"));
    assert!(version("1.28.0-dev").is_pre_release());
}

/// Nothing numeric to read means no version, never version zero.
#[test]
fn a_token_without_numbers_is_not_a_version() {
    assert!(ToolVersion::parse("N-113522-g8b0a3d5c").is_none());
    assert!(ToolVersion::parse("nightly").is_none());
    assert!(ToolVersion::parse("").is_none());
    assert!(ToolVersion::parse("   ").is_none());
}

/// yt-dlp: bare date, nightly, garbage, empty, unexpected prefix.
#[test]
fn yt_dlp_output_matrix() {
    assert_eq!(
        parse_output("yt-dlp", "2024.08.06"),
        Some(version("2024.08.06"))
    );
    assert_eq!(
        parse_output("yt-dlp", "2024.08.06.232855\n"),
        Some(version("2024.08.06.232855"))
    );
    assert!(parse_output("yt-dlp", "ERROR: unable to start").is_none());
    assert!(parse_output("yt-dlp", "").is_none());
    assert!(parse_output("yt-dlp", "   \n\n").is_none());
}

/// FFmpeg: release, distribution build, git description, empty, a usage banner that also
/// begins with the tool's name.
#[test]
fn ffmpeg_output_matrix() {
    assert_eq!(
        parse_output("ffmpeg", "ffmpeg version 6.1.1 Copyright (c) 2000-2023"),
        Some(version("6.1.1"))
    );
    assert_eq!(
        parse_output("ffmpeg", "ffmpeg version n7.0.2-1 Copyright (c)"),
        Some(version("n7.0.2-1"))
    );
    assert_eq!(
        parse_output("ffprobe", "ffprobe version 6.1.1-3ubuntu5 Copyright (c)"),
        Some(version("6.1.1-3ubuntu5"))
    );
    assert!(parse_output("ffmpeg", "ffmpeg version N-113522-g8b0a3d5c").is_none());
    assert!(parse_output("ffmpeg", "").is_none());
    assert!(parse_output("ffmpeg", "Usage: ffmpeg [options] [[infile]]").is_none());
}

/// Streamlink and gallery-dl: release, dev build, garbage, empty, wrong program.
#[test]
fn streamlink_and_gallery_dl_output_matrix() {
    assert_eq!(
        parse_output("streamlink", "streamlink 6.7.4"),
        Some(version("6.7.4"))
    );
    assert_eq!(
        parse_output("gallery-dl", "gallery-dl 1.27.1"),
        Some(version("1.27.1"))
    );
    assert_eq!(
        parse_output("gallery-dl", "gallery-dl 1.28.0-dev"),
        Some(version("1.28.0-dev"))
    );
    assert!(parse_output("streamlink", "streamlink: error: no such option").is_none());
    assert!(parse_output("streamlink", "").is_none());
    // A different program answering on that path must not be read as this one.
    assert!(parse_output("streamlink", "yt-dlp 2024.08.06").is_none());
}

/// The other vendor tools, whose banners are the least regular of the set.
#[test]
fn vendor_tool_output_matrix() {
    assert_eq!(
        parse_output("unrar", "UNRAR 6.24 freeware      Copyright (c) 1993-2023"),
        Some(version("6.24"))
    );
    assert_eq!(
        parse_output("7z", "7-Zip (z) 23.01 (x64) : Copyright (c) 1999-2023"),
        Some(version("23.01"))
    );
    assert_eq!(
        parse_output("rclone", "rclone v1.66.0"),
        Some(version("v1.66.0"))
    );
    assert_eq!(
        parse_output(
            "apprise",
            "Apprise v1.13.1\nCopyright (c) 2026, Chris Caron <lead2gold@gmail.com>"
        ),
        Some(version("v1.13.1"))
    );
    assert!(parse_output("7z", "7-Zip").is_none());
    assert!(parse_output("rclone", "").is_none());
}

/// The archive tools' banners as their builds print them when started bare, including the
/// old ones the version floor exists to refuse (review 2026-09-28, finding 5).
#[test]
fn archive_tool_banner_matrix() {
    let cases = [
        (
            "unrar",
            "\nUNRAR 6.11 freeware      Copyright (c) 1993-2022 Alexander Roshal\n",
            "6.11",
        ),
        (
            "unrar",
            "UNRAR 7.01 freeware      Copyright (c) 1993-2024 Alexander Roshal",
            "7.01",
        ),
        (
            "unrar",
            "RAR 7.12   Copyright (c) 1993-2025 Alexander Roshal   23 Jun 2025",
            "7.12",
        ),
        (
            "7z",
            "\n7-Zip [64] 16.02 : Copyright (c) 1999-2016 Igor Pavlov : 2016-05-21",
            "16.02",
        ),
        (
            "7z",
            "\n7-Zip (a) 24.09 (x64) : Copyright (c) 1999-2024 Igor Pavlov : 2024-11-29",
            "24.09",
        ),
        (
            "7z",
            "7-Zip 25.01 (x64) : Copyright (c) 1999-2025 Igor Pavlov : 2025-08-03",
            "25.01",
        ),
    ];
    for (tool, banner, expected) in cases {
        assert_eq!(
            parse_output(tool, banner),
            Some(version(expected)),
            "{banner}"
        );
    }
    // A usage error without the banner names no version, so it stays unknown.
    assert!(parse_output("unrar", "ERROR: Unknown option: -version").is_none());
    assert!(parse_output("7z", "Command Line Error:").is_none());
}
