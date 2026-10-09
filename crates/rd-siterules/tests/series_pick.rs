//! A two-stage rule (RD-1170-03) on a synthetic series site: a series page's releases listed with
//! their season, episode, resolution, language and hoster, and one chosen release resolved with
//! one captcha.
//!
//! The site is invented (`series.example.com`, RD-1230-03): the project carries no rule and no
//! recorded page of a content site. Its shape is the one such a site has -- a page naming a
//! media id and a captcha site key, a release API answering one object per release (32 of them,
//! 28 episodes of season 1 and 4 season packs), and a links answer behind a solved captcha --
//! and `fixtures/series-rule.json` is the two-stage rule for it.

mod recorded;

use async_trait::async_trait;
use rd_siterules::{
    CaptchaRequest, CaptchaSolver, Executor, Rule, RunError, SystemClock,
    exec::ports::{FetchFailure, FetchRequest, FetchResponse, Fetcher, Method},
};
use recorded::{PublicDns, Recorded, example, run};
use std::sync::Mutex;

const RULE: &str = include_str!("fixtures/series-rule.json");
const PAGE: &str = include_str!("fixtures/series-page.html");
const RELEASES: &str = include_str!("fixtures/series-releases.json");
const DOWNLOADS: &str = include_str!("fixtures/series-downloads.json");

const PROBE: &str = "https://series.example.com/serie/example-open-series/";
const API: &str = "https://series.example.com/api/media/6a96aa7cff33487fe64137e1/releases";
/// S01E07 in 720p, the second release of the list.
const CHOSEN: &str =
    "https://series.example.com/api/releases/6ac64b43b35595d9966c903e/downloads/ddownload";
const DEVICE: &str = "00112233445566778899aabbccddeeff";

fn series() -> Rule {
    serde_json::from_str(RULE).expect("the synthetic series rule parses")
}

fn network() -> Recorded {
    Recorded::default().page(PROBE, PAGE).page(API, RELEASES)
}

#[tokio::test]
async fn the_series_page_lists_its_releases_and_resolves_none() {
    let rule = series();
    rule.validate().expect("valid");
    let crawl = run(&rule, &network(), PROBE).await.expect("listed");
    assert!(crawl.links.is_empty() && crawl.groups.is_empty());
    assert_eq!(crawl.pages_fetched, 2, "the page and the release list");
    assert_eq!(
        crawl.package_name.as_deref(),
        Some("Example Open Series 2026")
    );
    let list = crawl.pick.expect("a list to choose from");
    assert_eq!(list.entries.len(), 32);
    let first = &list.entries[0];
    assert_eq!(
        first.label.as_deref(),
        Some("Example.Open.Series.2026.S01E07.DL.GERMAN.WEBRiP.x264-GRP1"),
        "the trailing dot of the release name is not part of the package name"
    );
    let attribute = |index: usize, name: &str| {
        list.entries[index]
            .attributes
            .get(name)
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(attribute(0, "season"), "1");
    assert_eq!(attribute(0, "episode"), "7");
    assert_eq!(attribute(0, "resolution"), "SD");
    assert_eq!(attribute(0, "language"), "GERMAN/ENGLISH");
    assert_eq!(attribute(0, "hoster"), "ddownload");
    assert_eq!(attribute(1, "resolution"), "720p");
    // Four season packs: a season, no episode.
    let packs = list
        .entries
        .iter()
        .filter(|entry| !entry.attributes.contains_key("episode"))
        .count();
    assert_eq!(packs, 4);
    assert!(
        list.entries
            .iter()
            .all(|entry| entry.attributes.get("season").map(String::as_str) == Some("1"))
    );
}

/// The broker as a person in front of it answers.
#[derive(Default)]
struct Person(Mutex<Vec<CaptchaRequest>>);

#[async_trait]
impl CaptchaSolver for Person {
    async fn solve(&self, request: CaptchaRequest) -> Result<String, String> {
        self.0.lock().expect("asked").push(request);
        Ok("solved-token".to_owned())
    }
}

/// The recorded pages, the links answer of the one chosen release, and every request kept.
struct Site {
    pages: Recorded,
    requests: Mutex<Vec<FetchRequest>>,
}

#[async_trait]
impl Fetcher for Site {
    async fn fetch(&self, request: FetchRequest) -> Result<FetchResponse, FetchFailure> {
        self.requests
            .lock()
            .expect("requests")
            .push(request.clone());
        if request.url.as_str() == CHOSEN {
            return Ok(FetchResponse::ok(DOWNLOADS));
        }
        self.pages.fetch(request).await
    }
}

#[tokio::test]
async fn one_chosen_release_is_resolved_with_one_captcha_on_the_series_page() {
    let rule = series();
    let site = Site {
        pages: network(),
        requests: Mutex::new(Vec::new()),
    };
    let person = Person::default();
    let clock = SystemClock::new();
    let executor = Executor::new(&site, &PublicDns, &clock)
        .with_captcha(&person)
        .with_device_id(DEVICE);
    let crawl = executor
        .run(&rule, &PROBE.parse().expect("url"))
        .await
        .expect("listed");
    let list = crawl.pick.expect("list");
    assert!(
        person.0.lock().expect("asked").is_empty(),
        "listing asks no captcha"
    );

    let group = executor
        .resolve(&rule, &crawl.address, &list, 1)
        .await
        .expect("resolved");
    assert_eq!(
        group.name.as_deref(),
        Some("Example.Open.Series.2026.S01E07.German.DL.720p.WEB.h264-GRP4")
    );
    let links: Vec<&str> = group.links.iter().map(|link| link.url.as_str()).collect();
    assert_eq!(
        links,
        [
            "https://ddownload.com/sj1a2b3c4d5e",
            "https://ddownload.com/sj6f7a8b9c0d"
        ]
    );
    let asked = person.0.lock().expect("asked").clone();
    assert_eq!(asked.len(), 1, "one captcha for one release");
    assert_eq!(asked[0].challenge, "recaptcha-v2");
    assert_eq!(asked[0].sitekey.as_deref(), Some("synthetic-site-key-0000"));
    assert_eq!(
        asked[0].page_url.as_str(),
        PROBE,
        "on the page, not the API"
    );
    assert!(asked[0].invisible);
    let requests = site.requests.lock().expect("requests").clone();
    let post = requests.last().expect("the links request");
    assert_eq!(post.url.as_str(), CHOSEN);
    assert_eq!(post.method, Method::Post);
    assert!(post.json);
    assert_eq!(
        post.form.get("recaptchaToken").map(String::as_str),
        Some("solved-token")
    );
    assert_eq!(post.form.get("fphash").map(String::as_str), Some(DEVICE));

    // Without the token the site answers 403; an entry that meets it reports the page as
    // guarded rather than inventing links.
    let refusing = network().status(CHOSEN, 403);
    let executor = Executor::new(&refusing, &PublicDns, &clock)
        .with_captcha(&person)
        .with_device_id(DEVICE);
    let error = executor
        .resolve(&rule, &crawl.address, &list, 1)
        .await
        .expect_err("refused");
    assert!(
        matches!(error, RunError::Blocked { status: 403, .. }),
        "{error:?}"
    );
}

/// The owner's requirement of 2026-10-07: a rule written before two-stage rules existed reads
/// and runs as before. The examples the app brings include the one-stage shape, and none of
/// those carries a field of the two-stage shape.
#[test]
fn no_one_stage_example_carries_a_field_of_the_two_stage_shape() {
    assert!(example("debian-cd").groups.is_none());
    for rule in rd_siterules::examples() {
        if rule
            .groups
            .as_ref()
            .is_some_and(|groups| groups.pick.is_some())
        {
            continue;
        }
        let text = serde_json::to_string(&rule).expect("encode");
        for field in ["\"pick\"", "\"json\"", "\"page\"", "\"invisible\""] {
            assert!(!text.contains(field), "{} carries {field}", rule.id);
        }
    }
}
