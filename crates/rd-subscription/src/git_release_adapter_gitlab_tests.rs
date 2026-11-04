//! The adapter against the GitLab release list recorded from `gitlab-org/cli`
//! (`tests/fixtures/gitlab-releases.json`, RD-1400-05), and the cases that project has none of.

use std::sync::Arc;

use rd_core::{GitForge, GitReleaseOptions};

use super::{FakeForge, adapter, linux_x64, subscription};

const GITLAB: &str = include_str!("../tests/fixtures/gitlab-releases.json");
const SUMS: &str = include_str!("../tests/fixtures/gitlab-checksums.txt");
const PROJECT: &str = "https://gitlab.com/gitlab-org/cli";
const GITLAB_LIST: &str =
    "https://gitlab.com/api/v4/projects/gitlab-org%2Fcli/releases?per_page=10";
const NEWEST_SUMS: &str =
    "https://gitlab.com/gitlab-org/cli/-/releases/v1.128.0/downloads/checksums.txt";

/// An upcoming release and a pre-release tag, which the recorded project has neither of; built
/// in the documented shape, not recorded.
const UPCOMING_AND_BETA: &str = r#"[
  {"tag_name": "v3.0.0", "released_at": "2026-12-01T00:00:00Z", "upcoming_release": true,
   "assets": {"sources": [], "links": [
     {"id": 8, "name": "app-linux-amd64", "url": "https://downloads.example.test/app/3.0.0"}
   ]}},
  {"tag_name": "v3.0.0-beta.1", "released_at": "2026-11-01T00:00:00Z", "upcoming_release": false,
   "assets": {"sources": [], "links": [
     {"id": 7, "name": "app-linux-amd64", "url": "https://downloads.example.test/app/beta"}
   ]}}
]"#;

#[tokio::test]
async fn a_checksum_list_is_fetched_for_the_files_the_forge_states_no_digest_for() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(GITLAB_LIST, 200, Some(GITLAB), &[]);
    forge.answer(NEWEST_SUMS, 200, Some(SUMS), &[]);
    // gitlab.com needs no forge named.
    let outcome = adapter(&forge)
        .poll(&subscription(PROJECT, linux_x64()))
        .await
        .expect("poll");

    let newest = outcome
        .items
        .iter()
        .find(|item| item.title == "glab_1.128.0_linux_amd64.tar.gz")
        .expect("the newest Linux tarball");
    assert_eq!(
        newest.source_id.as_deref(),
        Some("gitlab:release:v1.128.0:asset:13999616")
    );
    assert_eq!(
        newest.url.as_str(),
        "https://gitlab.com/gitlab-org/cli/-/releases/v1.128.0/downloads/glab_1.128.0_linux_amd64.tar.gz"
    );
    assert_eq!(
        newest.attributes.get("sha256").map(String::as_str),
        Some("bc024819e7598b19429a199cd87f640d98b2b67416d8ad796f19e4260b57a7c7"),
        "from the release's own checksums.txt"
    );
    // The older releases' lists are not answered here: their files go on without a checksum.
    let older = outcome
        .items
        .iter()
        .find(|item| item.title == "glab_1.127.0_linux_amd64.tar.gz")
        .expect("the previous Linux tarball");
    assert!(!older.attributes.contains_key("sha256"));
    // One list per release, each asked for once.
    let lists: Vec<String> = forge
        .requests()
        .into_iter()
        .map(|(url, _)| url)
        .filter(|url| url.ends_with("/checksums.txt"))
        .collect();
    assert_eq!(lists.len(), 3);
    assert_eq!(lists[0], NEWEST_SUMS);
}

#[tokio::test]
async fn the_platform_and_architecture_are_read_from_gitlab_link_names() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(GITLAB_LIST, 200, Some(GITLAB), &[]);
    let outcome = adapter(&forge)
        .poll(&subscription(PROJECT, linux_x64()))
        .await
        .expect("poll");
    let wanted: Vec<&str> = outcome
        .items
        .iter()
        .filter(|item| item.refused.is_none())
        .filter(|item| item.attributes.get("release").map(String::as_str) == Some("v1.128.0"))
        .map(|item| item.title.as_str())
        .collect();
    for file in [
        "glab_1.128.0_linux_amd64.deb",
        "glab_1.128.0_linux_amd64.rpm",
        "glab_1.128.0_linux_amd64.tar.gz",
        "glab_1.128.0_linux_amd64.apk",
    ] {
        assert!(wanted.contains(&file), "{file}: {wanted:?}");
    }
    // Another architecture, another platform, the source link and the checksum list are not.
    for word in [
        "_386",
        "_arm64",
        "_armv6",
        "windows",
        "darwin",
        "freebsd",
        "_source",
        "checksums",
    ] {
        assert!(
            !wanted
                .iter()
                .any(|name| name.to_ascii_lowercase().contains(word)),
            "{word}: {wanted:?}"
        );
    }
}

#[tokio::test]
async fn gitlab_source_archives_come_only_when_asked_for() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(GITLAB_LIST, 200, Some(GITLAB), &[]);
    let is_source = |id: Option<&str>| id.is_some_and(|id| id.contains(":source:"));
    let mut options = linux_x64();
    let adapter = adapter(&forge);
    let outcome = adapter
        .poll(&subscription(PROJECT, options.clone()))
        .await
        .expect("poll");
    assert!(
        !outcome
            .items
            .iter()
            .any(|item| is_source(item.source_id.as_deref()))
    );

    options.source_archives = true;
    let outcome = adapter
        .poll(&subscription(PROJECT, options))
        .await
        .expect("poll");
    let sources: Vec<(&str, &str)> = outcome
        .items
        .iter()
        .filter_map(|item| {
            item.source_id
                .as_deref()
                .filter(|id| id.starts_with("gitlab:release:v1.128.0:source:"))
                .map(|id| (id, item.title.as_str()))
        })
        .collect();
    assert_eq!(
        sources,
        [
            ("gitlab:release:v1.128.0:source:zip", "cli-v1.128.0.zip"),
            (
                "gitlab:release:v1.128.0:source:tar.gz",
                "cli-v1.128.0.tar.gz"
            ),
            (
                "gitlab:release:v1.128.0:source:tar.bz2",
                "cli-v1.128.0.tar.bz2"
            ),
            ("gitlab:release:v1.128.0:source:tar", "cli-v1.128.0.tar"),
        ]
    );
    // Asked for by their own switch, never refused by the platform.
    assert!(
        outcome
            .items
            .iter()
            .filter(|item| is_source(item.source_id.as_deref()))
            .all(|item| item.refused.is_none())
    );
}

#[tokio::test]
async fn an_upcoming_release_never_comes_and_a_beta_only_when_pre_releases_are_asked_for() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(GITLAB_LIST, 200, Some(UPCOMING_AND_BETA), &[]);
    let mut options = GitReleaseOptions {
        forge: Some(GitForge::Gitlab),
        ..GitReleaseOptions::default()
    };
    let adapter = adapter(&forge);
    let outcome = adapter
        .poll(&subscription(PROJECT, options.clone()))
        .await
        .expect("poll");
    assert!(outcome.items.is_empty(), "neither is a release yet");

    options.prereleases = true;
    let outcome = adapter
        .poll(&subscription(PROJECT, options))
        .await
        .expect("poll");
    let addresses: Vec<&str> = outcome.items.iter().map(|item| item.url.as_str()).collect();
    assert_eq!(addresses, ["https://downloads.example.test/app/beta"]);
}
