//! The pure half of the git-release adapter, against release lists recorded from the forges
//! (`tests/fixtures/`, RD-1400-05). Recorded on 2026-11-04 without a token and sanitized —
//! accounts, commit authors and release notes replaced, everything the adapter reads kept:
//!
//! - `github-releases.json`: `GET https://api.github.com/repos/neovim/neovim/releases?per_page=4`
//! - `gitlab-releases.json`: `GET https://gitlab.com/api/v4/projects/gitlab-org%2Fcli/releases?per_page=3`
//! - `gitlab-checksums.txt`: the `checksums.txt` of glab v1.128.0, as published
//!
//! What a recorded public list cannot show (a draft, a file still uploading, an upcoming GitLab
//! release, a pre-release tag) is a small document in the documented shape, built here.

use rd_core::{GitArchitecture, GitForge, GitPlatform, GitReleaseOptions};
use url::Url;

use super::{
    ChecksumFile, Repository, architecture_of, checksum_file, glob_matches, looks_like_prerelease,
    parse_checksums, parse_releases, platform_of, selects,
};

const GITHUB: &str = include_str!("../tests/fixtures/github-releases.json");
const GITLAB: &str = include_str!("../tests/fixtures/gitlab-releases.json");
const SUMS: &str = include_str!("../tests/fixtures/gitlab-checksums.txt");

/// A draft and a file still being uploaded, which only a list read with a token shows; built in
/// the documented shape, not recorded.
const GITHUB_DRAFT_AND_UPLOAD: &str = r#"[
  {"id": 2, "tag_name": "v2.0.0", "draft": true, "prerelease": false, "published_at": null,
   "created_at": "2026-11-01T10:00:00Z", "tarball_url": null, "zipball_url": null, "assets": []},
  {"id": 1, "tag_name": "v1.0.0", "draft": false, "prerelease": false,
   "published_at": "2026-10-01T10:00:00Z", "created_at": "2026-10-01T09:00:00Z",
   "tarball_url": null, "zipball_url": null,
   "assets": [
     {"id": 11, "name": "tool-linux-x86_64.tar.gz", "state": "uploaded", "size": 10,
      "browser_download_url": "https://github.com/example/tool/releases/download/v1.0.0/tool-linux-x86_64.tar.gz",
      "url": "https://api.github.com/repos/example/tool/releases/assets/11",
      "digest": "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"},
     {"id": 12, "name": "tool-windows-x86_64.zip", "state": "open", "size": 0,
      "browser_download_url": "https://github.com/example/tool/releases/download/v1.0.0/tool-windows-x86_64.zip",
      "url": "https://api.github.com/repos/example/tool/releases/assets/12", "digest": null}
   ]}
]"#;

/// An upcoming release and a pre-release tag, which the recorded project has neither of, and a
/// link without a permanent address; built in the documented shape, not recorded.
const GITLAB_UPCOMING_AND_BETA: &str = r#"[
  {"tag_name": "v3.0.0", "released_at": "2026-12-01T00:00:00Z", "upcoming_release": true,
   "assets": {"sources": [], "links": []}},
  {"tag_name": "v3.0.0-beta.1", "released_at": "2026-11-01T00:00:00Z", "upcoming_release": false,
   "assets": {"sources": [], "links": [
     {"id": 7, "name": "app-linux-amd64", "url": "https://downloads.example.test/app/app-linux-amd64"}
   ]}}
]"#;

fn url(value: &str) -> Url {
    value.parse().expect("url")
}

#[test]
fn a_github_address_is_read_however_it_was_copied() {
    for address in [
        "https://github.com/example/tool",
        "https://github.com/example/tool/",
        "https://github.com/example/tool.git",
        "https://github.com/example/tool/releases/latest",
    ] {
        let repository = Repository::parse(&url(address), None).expect(address);
        assert_eq!(repository.forge, GitForge::Github);
        assert_eq!(repository.path, "example/tool", "{address}");
        assert_eq!(
            repository.releases_url(10).as_str(),
            "https://api.github.com/repos/example/tool/releases?per_page=10"
        );
    }
}

#[test]
fn a_gitlab_project_keeps_its_groups_and_is_named_by_its_encoded_path() {
    let repository = Repository::parse(
        &url("https://gitlab.example.test/group/sub/app/-/releases"),
        Some(GitForge::Gitlab),
    )
    .expect("repository");
    assert_eq!(repository.path, "group/sub/app");
    assert_eq!(
        repository.releases_url(10).as_str(),
        "https://gitlab.example.test/api/v4/projects/group%2Fsub%2Fapp/releases?per_page=10"
    );
}

#[test]
fn a_github_enterprise_server_has_its_api_under_its_own_address() {
    let repository = Repository::parse(
        &url("https://git.example.test:8443/team/tool"),
        Some(GitForge::Github),
    )
    .expect("repository");
    assert_eq!(
        repository.releases_url(5).as_str(),
        "https://git.example.test:8443/api/v3/repos/team/tool/releases?per_page=5"
    );
}

#[test]
fn an_unknown_host_or_a_missing_name_is_refused() {
    assert!(Repository::parse(&url("https://git.example.test/team/tool"), None).is_err());
    assert!(Repository::parse(&url("https://github.com/example"), None).is_err());
}

#[test]
fn github_releases_carry_their_flags_digests_and_source_archives() {
    let releases = parse_releases(GitForge::Github, GITHUB, "neovim/neovim").expect("parse");
    assert_eq!(releases.len(), 4);
    assert!(
        releases.iter().all(|release| !release.draft),
        "a list read without a token shows no drafts"
    );
    assert_eq!(releases[0].tag, "nightly");
    assert!(releases[0].prerelease);
    let stable = &releases[1];
    assert_eq!(stable.id, "408064517");
    assert_eq!(stable.tag, "v0.12.6");
    assert!(!stable.prerelease);
    assert_eq!(
        stable.published_raw.as_deref(),
        Some("2026-10-09T15:43:25Z")
    );
    assert!(stable.published_at.is_some());
    assert_eq!(stable.assets.len(), 12);
    let tarball = stable
        .assets
        .iter()
        .find(|asset| asset.name == "nvim-linux-x86_64.tar.gz")
        .expect("the Linux tarball");
    assert_eq!(tarball.id, "625399155");
    assert_eq!(tarball.size, Some(11_404_582));
    assert_eq!(
        tarball.sha256.as_deref(),
        Some("474430d53e6264f6d6dd18db42d6dc9df3a1b56ca9e88a325bbf860e1a811d87")
    );
    assert_eq!(
        tarball.url.as_str(),
        "https://github.com/neovim/neovim/releases/download/v0.12.6/nvim-linux-x86_64.tar.gz"
    );
    assert_eq!(
        tarball.api_url.as_ref().map(Url::as_str),
        Some("https://api.github.com/repos/neovim/neovim/releases/assets/625399155")
    );
    // GitHub states the digest of every file of the recorded list.
    assert!(
        releases
            .iter()
            .flat_map(|release| &release.assets)
            .all(|asset| asset.sha256.is_some())
    );
    let names: Vec<&str> = stable
        .sources
        .iter()
        .map(|source| source.name.as_str())
        .collect();
    assert_eq!(names, ["neovim-v0.12.6.tar.gz", "neovim-v0.12.6.zip"]);
    assert_eq!(
        stable.sources[0].url.as_str(),
        "https://api.github.com/repos/neovim/neovim/tarball/v0.12.6"
    );
}

#[test]
fn a_github_draft_is_flagged_and_a_file_still_uploading_is_not_a_file_yet() {
    let releases =
        parse_releases(GitForge::Github, GITHUB_DRAFT_AND_UPLOAD, "example/tool").expect("parse");
    assert!(releases[0].draft);
    // A draft has no publication date; its creation stands in.
    assert_eq!(
        releases[0].published_raw.as_deref(),
        Some("2026-11-01T10:00:00Z")
    );
    let published = &releases[1];
    let names: Vec<&str> = published
        .assets
        .iter()
        .map(|asset| asset.name.as_str())
        .collect();
    assert_eq!(names, ["tool-linux-x86_64.tar.gz"]);
    assert_eq!(
        published.assets[0].sha256.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert!(published.sources.is_empty());
}

#[test]
fn gitlab_releases_are_named_by_tag_and_their_files_by_the_permanent_address() {
    let releases = parse_releases(GitForge::Gitlab, GITLAB, "gitlab-org/cli").expect("parse");
    assert_eq!(releases.len(), 3);
    let newest = &releases[0];
    assert_eq!(newest.id, "v1.128.0");
    assert!(!newest.draft);
    assert!(!newest.prerelease);
    assert_eq!(
        newest.published_raw.as_deref(),
        Some("2026-11-03T12:18:50.742Z")
    );
    assert!(newest.published_at.is_some());
    assert_eq!(newest.assets.len(), 36);
    let tarball = newest
        .assets
        .iter()
        .find(|asset| asset.name == "glab_1.128.0_linux_amd64.tar.gz")
        .expect("the Linux tarball");
    assert_eq!(tarball.id, "13999616");
    // The permanent release address, not the package registry the link points at.
    assert_eq!(
        tarball.url.as_str(),
        "https://gitlab.com/gitlab-org/cli/-/releases/v1.128.0/downloads/glab_1.128.0_linux_amd64.tar.gz"
    );
    assert!(tarball.api_url.is_none());
    assert!(tarball.sha256.is_none(), "GitLab states no digest");
    let names: Vec<&str> = newest
        .sources
        .iter()
        .map(|source| source.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "cli-v1.128.0.zip",
            "cli-v1.128.0.tar.gz",
            "cli-v1.128.0.tar.bz2",
            "cli-v1.128.0.tar"
        ]
    );
}

#[test]
fn gitlab_has_an_upcoming_release_as_a_draft_and_a_pre_release_only_by_its_tag() {
    let releases =
        parse_releases(GitForge::Gitlab, GITLAB_UPCOMING_AND_BETA, "group/app").expect("parse");
    assert!(
        releases[0].draft,
        "an upcoming release is not published yet"
    );
    assert!(releases[1].prerelease, "the tag names a beta");
    // Without a permanent address the link's own target is the file.
    assert_eq!(
        releases[1].assets[0].url.as_str(),
        "https://downloads.example.test/app/app-linux-amd64"
    );
}

#[test]
fn a_list_that_is_not_one_fails_rather_than_reading_as_empty() {
    assert!(parse_releases(GitForge::Github, "{\"message\":\"Not Found\"}", "a/b").is_err());
    assert!(parse_releases(GitForge::Gitlab, "<html>", "a/b").is_err());
}

#[test]
fn pre_releases_are_recognised_by_their_tag() {
    for tag in [
        "v1.0.0-rc.1",
        "v2.0.0-beta2",
        "1.0-preview",
        "v3.0.0-alpha",
        "v1.0-dev",
    ] {
        assert!(looks_like_prerelease(tag), "{tag}");
    }
    for tag in ["v1.0.0", "release-2026", "v1.0-prerelease-notes", "rc"] {
        assert!(!looks_like_prerelease(tag), "{tag}");
    }
}

#[test]
fn platforms_and_architectures_are_read_from_words_and_extensions() {
    assert_eq!(
        platform_of("tool-linux-x86_64.tar.gz"),
        Some(GitPlatform::Linux)
    );
    assert_eq!(platform_of("tool-setup.exe"), Some(GitPlatform::Windows));
    assert_eq!(
        platform_of("tool-1.0-universal.dmg"),
        Some(GitPlatform::Macos)
    );
    assert_eq!(
        platform_of("tool_darwin_arm64.tar.gz"),
        Some(GitPlatform::Macos)
    );
    // An Arch Linux package is not a macOS installer.
    assert_eq!(
        platform_of("tool-1.0-x86_64.pkg.tar.zst"),
        Some(GitPlatform::Linux)
    );
    // Inside a longer word it is no platform.
    assert_eq!(platform_of("winrar-machine-tool.tar.gz"), None);
    assert_eq!(platform_of("sha256sums"), None);

    assert_eq!(
        architecture_of("tool-linux-x86_64.tar.gz"),
        Some(GitArchitecture::X86_64)
    );
    assert_eq!(
        architecture_of("tool-linux-amd64"),
        Some(GitArchitecture::X86_64)
    );
    assert_eq!(
        architecture_of("tool_darwin_arm64.zip"),
        Some(GitArchitecture::Aarch64)
    );
    assert_eq!(architecture_of("tool-i686.zip"), Some(GitArchitecture::X86));
    assert_eq!(
        architecture_of("tool-armhf.deb"),
        Some(GitArchitecture::Arm)
    );
    assert_eq!(architecture_of("tool-universal.dmg"), None);
}

#[test]
fn the_options_select_by_platform_strictly_and_by_architecture_where_one_is_named() {
    let linux_x64 = GitReleaseOptions {
        platforms: vec![GitPlatform::Linux],
        architectures: vec![GitArchitecture::X86_64],
        ..GitReleaseOptions::default()
    };
    assert!(selects(&linux_x64, "tool-1.2.0-linux-x86_64.tar.gz"));
    assert!(!selects(&linux_x64, "tool-1.2.0-linux-aarch64.tar.gz"));
    assert!(!selects(&linux_x64, "tool-1.2.0-windows-x86_64.zip"));
    // A file that names no platform is not taken when a platform is asked for.
    assert!(!selects(&linux_x64, "SHA256SUMS"));
    assert!(
        selects(&linux_x64, "tool.AppImage"),
        "names no architecture"
    );

    let mac = GitReleaseOptions {
        platforms: vec![GitPlatform::Macos],
        architectures: vec![GitArchitecture::Aarch64],
        ..GitReleaseOptions::default()
    };
    assert!(
        selects(&mac, "Tool-1.2.0-universal.dmg"),
        "a universal image passes"
    );

    let patterned = GitReleaseOptions {
        asset_patterns: vec!["*.AppImage".to_owned(), "tool-?.?.?-linux-*".to_owned()],
        ..GitReleaseOptions::default()
    };
    assert!(selects(&patterned, "Tool-x86_64.appimage"));
    assert!(selects(&patterned, "tool-1.2.0-linux-x86_64.tar.gz"));
    assert!(!selects(&patterned, "tool-1.2.0-windows-x86_64.zip"));

    assert!(selects(&GitReleaseOptions::default(), "anything at all"));
}

#[test]
fn a_glob_matches_the_whole_name_and_treats_everything_else_literally() {
    assert!(glob_matches("*.tar.gz", "tool.tar.gz"));
    assert!(!glob_matches("*.tar.gz", "tool.tar.gz.sig"));
    assert!(glob_matches("tool (x64)+.zip", "tool (x64)+.zip"));
    assert!(!glob_matches("tool.zip", "toolxzip"));
}

#[test]
fn a_glob_backtracks_over_several_stars_and_single_wildcards() {
    // Audit 1.9.1, INTAKE-10: the matcher that replaced the per-asset regular expression.
    assert!(glob_matches(
        "*-linux-*.tar.gz",
        "tool-1.2.0-linux-x86_64.tar.gz"
    ));
    assert!(glob_matches("tool-?.?.?-*", "tool-1.2.0-linux.zip"));
    assert!(!glob_matches("tool-?.?.?-*", "tool-1.20.0-linux.zip"));
    assert!(glob_matches("**", ""));
    assert!(!glob_matches("?", ""));
    assert!(glob_matches("  *.ZIP  ", "tool.zip"));
    assert!(glob_matches("t\u{fc}?l*", "t\u{fc}\u{fc}l.zip"));
    assert!(!glob_matches("*a*b", "aaab-c"));
    // A line break was never matched by a wildcard, and still is not.
    assert!(!glob_matches("a*b", "a\nb"));
    assert!(!glob_matches("a?b", "a\nb"));
}

#[test]
fn checksum_files_are_recognised_by_name() {
    assert_eq!(checksum_file("SHA256SUMS"), Some(ChecksumFile::List));
    assert_eq!(checksum_file("checksums.txt"), Some(ChecksumFile::List));
    assert_eq!(
        checksum_file("tool_1.2.0_checksums.txt"),
        Some(ChecksumFile::List)
    );
    assert_eq!(
        checksum_file("tool.tar.gz.sha256"),
        Some(ChecksumFile::Single("tool.tar.gz".to_owned()))
    );
    assert_eq!(checksum_file(".sha256"), None);
    assert_eq!(checksum_file("sha"), None);
    assert_eq!(checksum_file("tool.tar.gz"), None);
}

#[test]
fn checksum_lists_are_read_in_gnu_bsd_and_bare_form() {
    let sums = parse_checksums(SUMS);
    assert_eq!(sums.len(), 35, "one line for every file of the release");
    assert_eq!(
        sums.get("glab_1.128.0_linux_amd64.tar.gz")
            .map(String::as_str),
        Some("bc024819e7598b19429a199cd87f640d98b2b67416d8ad796f19e4260b57a7c7")
    );
    let built = parse_checksums(
        "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB *./dist/tool-linux-aarch64.tar.gz\n",
    );
    assert_eq!(
        built.get("tool-linux-aarch64.tar.gz").map(String::as_str),
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        "binary mode and a build directory are both stripped"
    );
    let bsd = parse_checksums(
        "SHA256 (tool.zip) = EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE\n",
    );
    assert_eq!(
        bsd.get("tool.zip").map(String::as_str),
        Some("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
    );
    let bare =
        parse_checksums("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\n");
    assert!(bare.contains_key(""));
    assert!(parse_checksums("not a checksum\nabc  file\n").is_empty());
}
