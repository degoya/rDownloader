//! The git-release adapter against a fake forge answering sanitized documents in the shape
//! GitHub and GitLab document (`tests/fixtures/`). No request leaves the process.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rd_core::{FilterReason, GitForge, GitReleaseOptions, Subscription};
use url::Url;

use super::{ApiFetcher, ApiResponse, GitReleaseAdapter, rate_limit_pause};
use crate::{
    adapter::{RateLimited, SourceAdapter},
    indexer::SecretResolver,
};

const GITHUB: &str = include_str!("../tests/fixtures/github-releases.json");
const GITLAB: &str = include_str!("../tests/fixtures/gitlab-releases.json");
const SUMS: &str = include_str!("../tests/fixtures/github-sha256sums.txt");
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

const GITHUB_LIST: &str = "https://api.github.com/repos/example/tool/releases?per_page=10";
const GITLAB_LIST: &str =
    "https://gitlab.example.test/api/v4/projects/group%2Fsub%2Fapp/releases?per_page=10";

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
    forge.answer(
        "https://github.com/example/tool/releases/download/v1.2.0/SHA256SUMS",
        200,
        Some(SUMS),
        &[],
    );
    let outcome = adapter(&forge)
        .poll(&subscription(
            "https://github.com/example/tool",
            linux_x64(),
        ))
        .await
        .expect("poll");

    assert_eq!(outcome.etag.as_deref(), Some("W/\"list-1\""));
    // The draft and the pre-release are not releases; v1.2.0 has five finished files, v1.1.0 one.
    let names: Vec<&str> = outcome
        .items
        .iter()
        .map(|item| item.title.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "tool-1.2.0-linux-x86_64.tar.gz",
            "tool-1.2.0-linux-aarch64.tar.gz",
            "tool-1.2.0-windows-x86_64.zip",
            "Tool-1.2.0-universal.dmg",
            "SHA256SUMS",
            "tool-1.1.0-linux-x86_64.tar.gz",
        ]
    );
    let wanted: Vec<&str> = outcome
        .items
        .iter()
        .filter(|item| item.refused.is_none())
        .map(|item| item.title.as_str())
        .collect();
    assert_eq!(
        wanted,
        [
            "tool-1.2.0-linux-x86_64.tar.gz",
            "tool-1.1.0-linux-x86_64.tar.gz"
        ]
    );
    assert_eq!(outcome.items[1].refused, Some(FilterReason::AssetNotWanted));

    let first = &outcome.items[0];
    assert_eq!(
        first.source_id.as_deref(),
        Some("github:release:3001:asset:9101")
    );
    assert_eq!(
        first.url.as_str(),
        "https://github.com/example/tool/releases/download/v1.2.0/tool-1.2.0-linux-x86_64.tar.gz"
    );
    assert_eq!(
        first.attributes.get("release").map(String::as_str),
        Some("v1.2.0")
    );
    assert_eq!(
        first.attributes.get("size").map(String::as_str),
        Some("4000000")
    );
    // The digest GitHub states, without a request for the list.
    assert_eq!(
        first.attributes.get("sha256").map(String::as_str),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert!(first.published_at.is_some());
    // v1.1.0 states no digest and carries no list.
    assert!(!outcome.items[5].attributes.contains_key("sha256"));
}

#[tokio::test]
async fn a_checksum_list_is_fetched_only_for_files_the_forge_states_no_digest_for() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(GITHUB_LIST, 200, Some(GITHUB), &[]);
    forge.answer(
        "https://github.com/example/tool/releases/download/v1.2.0/SHA256SUMS",
        200,
        Some(SUMS),
        &[],
    );
    let everything = GitReleaseOptions::default();
    let outcome = adapter(&forge)
        .poll(&subscription("https://github.com/example/tool", everything))
        .await
        .expect("poll");
    let aarch64 = outcome
        .items
        .iter()
        .find(|item| item.title == "tool-1.2.0-linux-aarch64.tar.gz")
        .expect("aarch64 file");
    assert_eq!(
        aarch64.attributes.get("sha256").map(String::as_str),
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
    );
    let lists = forge
        .requests()
        .iter()
        .filter(|(url, _)| url.ends_with("SHA256SUMS"))
        .count();
    assert_eq!(lists, 1);
}

#[tokio::test]
async fn an_unchanged_list_costs_one_conditional_request_and_nothing_else() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(GITHUB_LIST, 304, None, &[("etag", "W/\"list-1\"")]);
    let mut polled = subscription("https://github.com/example/tool", linux_x64());
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
        .poll(&subscription(
            "https://github.com/example/tool",
            linux_x64(),
        ))
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
    let mut private = subscription("https://github.com/example/tool", linux_x64());
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
        "https://github.com/example/tool",
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
    let file = &outcome.items[0];
    assert_eq!(
        file.url.as_str(),
        "https://api.github.com/repos/example/tool/releases/assets/9101"
    );

    // Handed over as the short-lived address the API redirects to, which needs no token.
    let signed: Url = "https://release-assets.example.test/9101?sig=abc"
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
    let public: Url = "https://github.com/example/tool/releases/download/v1.2.0/x"
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
async fn gitlab_pre_releases_and_source_archives_come_only_when_asked_for() {
    let forge = Arc::new(FakeForge::default());
    forge.answer(GITLAB_LIST, 200, Some(GITLAB), &[]);
    forge.answer(
        "https://downloads.example.test/app/2.1.0/app-linux-amd64.sha256",
        200,
        Some("9999999999999999999999999999999999999999999999999999999999999999\n"),
        &[],
    );
    let project = "https://gitlab.example.test/group/sub/app";
    let mut options = GitReleaseOptions {
        forge: Some(GitForge::Gitlab),
        platforms: vec![rd_core::GitPlatform::Linux],
        ..GitReleaseOptions::default()
    };
    let adapter = adapter(&forge);
    let outcome = adapter
        .poll(&subscription(project, options.clone()))
        .await
        .expect("poll");
    let wanted: Vec<&str> = outcome
        .items
        .iter()
        .filter(|item| item.refused.is_none())
        .map(|item| item.title.as_str())
        .collect();
    // The upcoming release is a draft and the beta a pre-release: only v2.1.0's Linux file.
    assert_eq!(wanted, ["app-2.1.0-linux-amd64"]);
    let linux = &outcome.items[0];
    assert_eq!(
        linux.source_id.as_deref(),
        Some("gitlab:release:v2.1.0:asset:501")
    );
    assert_eq!(
        linux.attributes.get("sha256").map(String::as_str),
        Some("9999999999999999999999999999999999999999999999999999999999999999"),
        "the file's own .sha256 next to it"
    );

    options.prereleases = true;
    options.source_archives = true;
    let outcome = adapter
        .poll(&subscription(project, options))
        .await
        .expect("poll");
    let titles: Vec<&str> = outcome
        .items
        .iter()
        .filter(|item| item.refused.is_none())
        .map(|item| item.title.as_str())
        .collect();
    assert_eq!(
        titles,
        [
            "app-2.2.0-beta.1-linux-amd64",
            "app-v2.2.0-beta.1.zip",
            "app-v2.2.0-beta.1.tar.gz",
            "app-2.1.0-linux-amd64",
            "app-v2.1.0.zip",
            "app-v2.1.0.tar.gz",
        ]
    );
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
