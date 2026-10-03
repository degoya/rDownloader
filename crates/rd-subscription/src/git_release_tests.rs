//! The pure half of the git-release adapter, against sanitized answers in the shape both
//! forges document (`tests/fixtures/`).

use rd_core::{GitArchitecture, GitForge, GitPlatform, GitReleaseOptions};
use url::Url;

use super::{
    ChecksumFile, Repository, architecture_of, checksum_file, glob_matches, looks_like_prerelease,
    parse_checksums, parse_releases, platform_of, selects,
};

const GITHUB: &str = include_str!("../tests/fixtures/github-releases.json");
const GITLAB: &str = include_str!("../tests/fixtures/gitlab-releases.json");
const SUMS: &str = include_str!("../tests/fixtures/github-sha256sums.txt");

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
fn github_releases_carry_their_flags_digests_and_finished_files_only() {
    let releases = parse_releases(GitForge::Github, GITHUB, "example/tool").expect("parse");
    assert_eq!(releases.len(), 4);
    assert!(releases[0].draft);
    assert!(releases[1].prerelease);
    let stable = &releases[2];
    assert_eq!(stable.id, "3001");
    assert_eq!(stable.tag, "v1.2.0");
    assert!(stable.published_at.is_some());
    // The file still being uploaded (`state: open`) is not a file yet.
    assert_eq!(stable.assets.len(), 5);
    assert_eq!(
        stable.assets[0].sha256.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(
        stable.assets[0].api_url.as_ref().map(Url::as_str),
        Some("https://api.github.com/repos/example/tool/releases/assets/9101")
    );
    let names: Vec<&str> = stable
        .sources
        .iter()
        .map(|source| source.name.as_str())
        .collect();
    assert_eq!(names, ["tool-v1.2.0.tar.gz", "tool-v1.2.0.zip"]);
}

#[test]
fn gitlab_releases_are_named_by_tag_and_an_upcoming_one_is_a_draft() {
    let releases = parse_releases(GitForge::Gitlab, GITLAB, "group/sub/app").expect("parse");
    assert_eq!(releases.len(), 3);
    assert!(
        releases[0].draft,
        "an upcoming release is not published yet"
    );
    assert!(releases[1].prerelease, "the tag names a beta");
    let stable = &releases[2];
    assert_eq!(stable.id, "v2.1.0");
    assert!(!stable.prerelease);
    // The permanent release address when the link has one, its target otherwise.
    assert_eq!(
        stable.assets[0].url.as_str(),
        "https://gitlab.example.test/group/sub/app/-/releases/v2.1.0/downloads/app-linux-amd64"
    );
    assert_eq!(
        stable.assets[1].url.as_str(),
        "https://downloads.example.test/app/2.1.0/app-windows-amd64.exe"
    );
    assert_eq!(stable.sources[0].name, "app-v2.1.0.zip");
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
    assert_eq!(sums.len(), 4);
    assert_eq!(
        sums.get("tool-1.2.0-linux-aarch64.tar.gz")
            .map(String::as_str),
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
