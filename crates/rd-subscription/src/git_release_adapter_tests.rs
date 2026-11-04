//! The git-release adapter against a fake forge answering the release lists recorded from
//! GitHub and GitLab (`tests/fixtures/`, how they were recorded: `git_release_tests.rs`). No
//! request leaves the process.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rd_core::{FilterReason, GitReleaseOptions, Subscription};
use url::Url;

use super::{ApiFetcher, ApiResponse, GitReleaseAdapter, rate_limit_pause};
use crate::{
    adapter::{RateLimited, SourceAdapter},
    indexer::SecretResolver,
};

const GITHUB: &str = include_str!("../tests/fixtures/github-releases.json");
const TOKEN: &str = "github_pat_example_never_shown";

/// One recorded request: its address and its headers.
type RecordedRequest = (String, Vec<(String, String)>);

/// Answers by address and records every request with its headers.
#[derive(Default)]
struct FakeForge {
    answers: Mutex<BTreeMap<String, ApiResponse>>,
    located: Mutex<Option<Url>>,
    requests: Mutex<Vec<RecordedRequest>>,
}

impl FakeForge {
    fn answer(&self, url: &str, status: u16, body: Option<&str>, headers: &[(&str, &str)]) {
        self.answers.lock().expect("answers").insert(
            url.to_owned(),
            ApiResponse {
                status,
                headers: headers
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
                body: body.map(str::to_owned),
            },
        );
    }

    fn requests(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.requests.lock().expect("requests").clone()
    }
}

#[async_trait]
impl ApiFetcher for FakeForge {
    async fn get(
        &self,
        url: &Url,
        headers: &[(String, String)],
        _limit: usize,
    ) -> anyhow::Result<ApiResponse> {
        self.requests
            .lock()
            .expect("requests")
            .push((url.to_string(), headers.to_vec()));
        Ok(self
            .answers
            .lock()
            .expect("answers")
            .get(url.as_str())
            .cloned()
            .unwrap_or(ApiResponse {
                status: 404,
                ..ApiResponse::default()
            }))
    }

    async fn locate(&self, url: &Url, headers: &[(String, String)]) -> anyhow::Result<Url> {
        self.requests
            .lock()
            .expect("requests")
            .push((url.to_string(), headers.to_vec()));
        self.located
            .lock()
            .expect("located")
            .clone()
            .ok_or_else(|| anyhow::anyhow!("HTTP 404"))
    }
}

struct Vault;

#[async_trait]
impl SecretResolver for Vault {
    async fn resolve(&self, _reference: &str) -> anyhow::Result<String> {
        Ok(TOKEN.to_owned())
    }
}

const REPOSITORY: &str = "https://github.com/neovim/neovim";
const GITHUB_LIST: &str = "https://api.github.com/repos/neovim/neovim/releases?per_page=10";

fn subscription(url: &str, options: GitReleaseOptions) -> Subscription {
    Subscription {
        git_release: options,
        ..crate::test_support::subscription("Releases", rd_core::SubscriptionKind::GitRelease, url)
    }
}

fn linux_x64() -> GitReleaseOptions {
    GitReleaseOptions {
        platforms: vec![rd_core::GitPlatform::Linux],
        architectures: vec![rd_core::GitArchitecture::X86_64],
        ..GitReleaseOptions::default()
    }
}

fn adapter(forge: &Arc<FakeForge>) -> GitReleaseAdapter {
    GitReleaseAdapter::new(Arc::clone(forge) as Arc<dyn ApiFetcher>, Arc::new(Vault))
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[tokio::test]
async fn every_file_of_a_published_release_is_an_item_and_the_unwanted_ones_say_why() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(GITHUB_LIST, 200, Some(GITHUB), &[("etag", "W/\"list-1\"")]);
    let outcome = adapter(&forge)
        .poll(&subscription(REPOSITORY, linux_x64()))
        .await
        .expect("poll");

    assert_eq!(outcome.etag.as_deref(), Some("W/\"list-1\""));
    // The nightly is a pre-release; v0.12.6, `stable` and v0.12.5 have twelve files each.
    assert_eq!(outcome.items.len(), 36);
    let names: Vec<&str> = outcome.items[..12]
        .iter()
        .map(|item| item.title.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "nvim-linux-arm64.appimage",
            "nvim-linux-arm64.appimage.zsync",
            "nvim-linux-arm64.tar.gz",
            "nvim-linux-x86_64.appimage",
            "nvim-linux-x86_64.appimage.zsync",
            "nvim-linux-x86_64.tar.gz",
            "nvim-macos-arm64.tar.gz",
            "nvim-macos-x86_64.tar.gz",
            "nvim-win-arm64.msi",
            "nvim-win-arm64.zip",
            "nvim-win64.msi",
            "nvim-win64.zip",
        ]
    );
    assert_eq!(outcome.items[0].refused, Some(FilterReason::AssetNotWanted));
    let wanted: Vec<(&str, &str)> = outcome
        .items
        .iter()
        .filter(|item| item.refused.is_none())
        .map(|item| {
            (
                item.attributes.get("release").map_or("", String::as_str),
                item.title.as_str(),
            )
        })
        .collect();
    let mut expected = Vec::new();
    for release in ["v0.12.6", "stable", "v0.12.5"] {
        for name in [
            "nvim-linux-x86_64.appimage",
            "nvim-linux-x86_64.appimage.zsync",
            "nvim-linux-x86_64.tar.gz",
        ] {
            expected.push((release, name));
        }
    }
    assert_eq!(wanted, expected);

    let tarball = outcome
        .items
        .iter()
        .find(|item| item.title == "nvim-linux-x86_64.tar.gz")
        .expect("the newest Linux tarball");
    assert_eq!(
        tarball.source_id.as_deref(),
        Some("github:release:408064517:asset:625399155")
    );
    assert_eq!(
        tarball.url.as_str(),
        "https://github.com/neovim/neovim/releases/download/v0.12.6/nvim-linux-x86_64.tar.gz"
    );
    assert_eq!(
        tarball.attributes.get("release").map(String::as_str),
        Some("v0.12.6")
    );
    assert_eq!(
        tarball.attributes.get("size").map(String::as_str),
        Some("11404582")
    );
    // The digest GitHub states, without a request for a list.
    assert_eq!(
        tarball.attributes.get("sha256").map(String::as_str),
        Some("474430d53e6264f6d6dd18db42d6dc9df3a1b56ca9e88a325bbf860e1a811d87")
    );
    assert!(tarball.published_at.is_some());
}

#[tokio::test]
async fn a_digest_the_forge_states_spares_every_further_request() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(GITHUB_LIST, 200, Some(GITHUB), &[]);
    let everything = GitReleaseOptions::default();
    let outcome = adapter(&forge)
        .poll(&subscription(REPOSITORY, everything))
        .await
        .expect("poll");
    assert!(
        outcome
            .items
            .iter()
            .all(|item| item.attributes.contains_key("sha256"))
    );
    assert_eq!(forge.requests().len(), 1, "the list and nothing else");
}

#[tokio::test]
async fn an_unchanged_list_costs_one_conditional_request_and_nothing_else() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(GITHUB_LIST, 304, None, &[("etag", "W/\"list-1\"")]);
    let mut polled = subscription(REPOSITORY, linux_x64());
    polled.etag = Some("W/\"list-1\"".to_owned());
    let outcome = adapter(&forge).poll(&polled).await.expect("poll");

    assert!(outcome.not_modified);
    assert!(outcome.items.is_empty());
    let requests = forge.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        header(&requests[0].1, "if-none-match"),
        Some("W/\"list-1\"")
    );
    assert_eq!(
        header(&requests[0].1, "accept"),
        Some("application/vnd.github+json")
    );
    assert_eq!(
        header(&requests[0].1, "authorization"),
        None,
        "no token stored"
    );
}

#[tokio::test]
async fn a_rate_limit_pauses_until_the_named_time_instead_of_failing() {
    let reset = Utc::now() + Duration::minutes(40);
    let forge = Arc::new(FakeForge::default());
    forge.answer(
        GITHUB_LIST,
        403,
        Some("{\"message\":\"API rate limit exceeded\"}"),
        &[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", &reset.timestamp().to_string()),
        ],
    );
    let error = adapter(&forge)
        .poll(&subscription(REPOSITORY, linux_x64()))
        .await
        .expect_err("refused");
    let limited = error.downcast_ref::<RateLimited>().expect("a rate limit");
    assert_eq!(limited.until.timestamp(), reset.timestamp());
}

#[tokio::test]
async fn a_forbidden_repository_is_a_failure_and_not_a_rate_limit() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(
        GITHUB_LIST,
        403,
        Some("{}"),
        &[("x-ratelimit-remaining", "4999")],
    );
    let mut private = subscription(REPOSITORY, linux_x64());
    private.secret_ref = Some("vault://subscription/token".to_owned());
    let error = adapter(&forge).poll(&private).await.expect_err("refused");
    assert!(error.downcast_ref::<RateLimited>().is_none());
    let message = error.to_string();
    assert!(message.contains("403"), "{message}");
    assert!(
        !message.contains(TOKEN),
        "the token reached an error: {message}"
    );
}

#[test]
fn the_pause_reads_retry_after_and_the_budget_reset_and_stays_bounded() {
    let now = DateTime::from_timestamp(1_790_000_000, 0).expect("now");
    let headers = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    };
    // GitLab's refusal: 429 with Retry-After.
    assert_eq!(
        rate_limit_pause(429, &headers(&[("retry-after", "120")]), now),
        Some(now + Duration::seconds(120))
    );
    // A refusal that names no time waits a minute rather than retrying at once.
    assert_eq!(
        rate_limit_pause(429, &headers(&[]), now),
        Some(now + Duration::seconds(60))
    );
    // A success that spent the last request waits for the reset.
    assert_eq!(
        rate_limit_pause(
            200,
            &headers(&[
                ("ratelimit-remaining", "0"),
                ("ratelimit-reset", "1790001800")
            ]),
            now
        ),
        Some(now + Duration::seconds(1_800))
    );
    // A stranger's number cannot park the subscription for good, nor overflow the clock.
    assert_eq!(
        rate_limit_pause(
            429,
            &headers(&[("retry-after", "9223372036854775807")]),
            now
        ),
        Some(now + Duration::hours(24))
    );
    assert_eq!(
        rate_limit_pause(200, &headers(&[("x-ratelimit-remaining", "12")]), now),
        None
    );
}

#[tokio::test]
async fn a_private_repository_is_read_with_the_token_and_its_files_resolved_when_handed_over() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(GITHUB_LIST, 200, Some(GITHUB), &[]);
    let mut private = subscription(
        REPOSITORY,
        GitReleaseOptions {
            asset_patterns: vec!["*linux-x86_64*".to_owned()],
            ..GitReleaseOptions::default()
        },
    );
    private.secret_ref = Some("vault://subscription/token".to_owned());
    let adapter = adapter(&forge);
    let outcome = adapter.poll(&private).await.expect("poll");

    let requests = forge.requests();
    assert_eq!(
        header(&requests[0].1, "authorization").map(str::to_owned),
        Some(format!("Bearer {TOKEN}"))
    );
    // Archived under the API address, the only one a private repository answers.
    let file = outcome
        .items
        .iter()
        .find(|item| item.refused.is_none())
        .expect("a wanted file");
    assert_eq!(file.title, "nvim-linux-x86_64.appimage");
    assert_eq!(
        file.url.as_str(),
        "https://api.github.com/repos/neovim/neovim/releases/assets/625399159"
    );

    // Handed over as the short-lived address the API redirects to, which needs no token.
    let signed: Url = "https://release-assets.example.test/625399159?sig=abc"
        .parse()
        .expect("url");
    *forge.located.lock().expect("located") = Some(signed.clone());
    assert_eq!(
        adapter
            .download_address(&private, &file.url)
            .await
            .expect("located"),
        signed
    );
    let (_, headers) = forge.requests().pop().expect("the locate request");
    assert_eq!(header(&headers, "accept"), Some("application/octet-stream"));

    // An API that does not redirect would hand the file's description, not the file.
    *forge.located.lock().expect("located") = Some(file.url.clone());
    assert!(adapter.download_address(&private, &file.url).await.is_err());

    // A public file is handed over as it was archived, without a request.
    let public: Url = "https://github.com/neovim/neovim/releases/download/v0.12.6/x"
        .parse()
        .expect("url");
    let before = forge.requests().len();
    assert_eq!(
        adapter
            .download_address(&private, &public)
            .await
            .expect("as is"),
        public
    );
    assert_eq!(forge.requests().len(), before);
}

#[tokio::test]
async fn an_address_that_names_no_repository_fails_before_any_request() {
    let forge = Arc::new(FakeForge::default());
    let error = adapter(&forge)
        .poll(&subscription(
            "https://git.example.test/team/tool",
            linux_x64(),
        ))
        .await
        .expect_err("unknown forge");
    assert!(error.to_string().contains("git.example.test"), "{error}");
    assert!(forge.requests().is_empty());
}

#[path = "git_release_adapter_gitlab_tests.rs"]
mod gitlab;
