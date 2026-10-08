//! Rules that yield several packages (RD-1170-02), and the proof that every rule written
//! before them still answers the same.
//!
//! The two services here were measured on 2026-10-07: `warez-cx-api.json` is the answer of
//! `https://api.warez.cx/start/d/9IMDqgvdVQQ6`, cut down to the fields a rule reads (the item's
//! name and title, and per release its name, quality and hoster lists) with the answer's own
//! escaping kept -- every slash written `\/`, which is why the rules decode what they find.
//! `hide-cx-container.json` is the answer of `https://api.hide.cx/containers/65f714b9-…`
//! without the uploader's account id. The answer per link of that container was measured
//! for one link (`{"url":"https://ddownload.com/3m1mkmygtzj6"}`); the other eleven are
//! *derived* the same way from the container's `host` and `file`.
//!
//! `warez-cx-links.txt` and `hide-cx-links.txt` are what the owner's own rules -- one package,
//! every link, as they ran on the measurement day -- produced from those answers. They are the
//! golden output: the one-package rules below are copied from the owner's, and they must keep
//! producing exactly this after the format grew `groups`.
//!
//! The rules under test come from `resources/site-rules-payload.json`, the unsigned payload the
//! signed release file is made from (sequence 9: warez.cx offers the choice since 1.19).

mod recorded;

use rd_siterules::{GroupMirrors, Rule};
use recorded::{Recorded, release_pack, run, shipped};

const PAYLOAD: &str = include_str!("../resources/site-rules-payload.json");
const WAREZ_API: &str = include_str!("fixtures/warez-cx-api.json");
const WAREZ_LINKS: &str = include_str!("fixtures/warez-cx-links.txt");
const HIDE_CONTAINER: &str = include_str!("fixtures/hide-cx-container.json");
const HIDE_LINKS: &str = include_str!("fixtures/hide-cx-links.txt");

const WAREZ_PROBE: &str = "https://warez.cx/detail/9IMDqgvdVQQ6/The-Beginning-After-the-End";
const WAREZ_API_URL: &str = "https://api.warez.cx/start/d/9IMDqgvdVQQ6";
const HIDE_PROBE: &str = "https://hide.cx/container/65f714b9-b07c-4ea8-97d7-298f2809efcd";
const HIDE_API_URL: &str = "https://api.hide.cx/containers/65f714b9-b07c-4ea8-97d7-298f2809efcd";

const SHOW: &str = "The Beginning After the End";
const S01_720P: &str =
    "The.Beginning.After.the.End.2025.S01.German.Subbed.ANiME.720p.AMZN.WEB.H264-WAREZCX";

/// The payload's rules, parsed as the signing command parses them.
fn payload() -> rd_siterules::RulePack {
    serde_json::from_str(PAYLOAD).expect("the payload parses")
}

fn payload_rule(id: &str) -> Rule {
    payload()
        .rules
        .into_iter()
        .find(|rule| rule.id == id)
        .unwrap_or_else(|| panic!("the payload carries the rule {id:?}"))
}

fn golden(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

/// The owner's own warez.cx rule of 2026-10-07: one package, every direct link.
fn warez_one_package() -> Rule {
    serde_json::from_value(serde_json::json!({
        "id": "warez-cx", "name": "warez.cx", "group": "board", "version": 1,
        "match": { "hosts": ["warez.cx", "*.warez.cx"],
                   "paths": ["^/(?:v2/)?detail/[0-9A-Za-z]+(?:/[^?]*)?$"] },
        "steps": [
            { "kind": "regex", "from": "url", "pattern": "/detail/([0-9A-Za-z]+)", "into": "uid" },
            { "kind": "fetch", "url": "https://api.warez.cx/start/d/${uid}", "into": "entry" },
            { "kind": "regex", "from": "entry", "pattern": "\"links\":\\{([^}]*)\\}",
              "into": "blocks", "all": true },
            { "kind": "regex", "from": "blocks", "pattern": "(\"https?:[^\"]+\")",
              "into": "quoted", "all": true },
            { "kind": "decode", "encoding": "js-string", "from": "quoted", "into": "links" }
        ],
        "package": { "from": "regex", "pattern": "\"title\":\"([^\"]+)\"", "source": "entry" },
        "probe": WAREZ_PROBE,
        "checked": "2026-10-07"
    }))
    .expect("the owner's rule")
}

/// The container answer and one answer per link, as hide.cx gives them.
fn hide_cx_answers() -> Recorded {
    let container: serde_json::Value =
        serde_json::from_str(HIDE_CONTAINER).expect("the container answer");
    let mut fetcher = Recorded::default().page(HIDE_API_URL, HIDE_CONTAINER);
    for link in container["links"].as_array().expect("links") {
        let id = link["id"].as_str().expect("id");
        let address = format!(
            "https://{}/{}",
            link["host"].as_str().expect("host"),
            link["file"].as_str().expect("file")
        );
        fetcher = fetcher.page(
            &format!("{HIDE_API_URL}/links/{id}"),
            &serde_json::json!({ "url": address }).to_string(),
        );
    }
    fetcher
}

/// Four attributes of a pick entry, in the order the test reads them.
type Attributes<'a> = (
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
);
#[tokio::test]
async fn the_owner_s_one_package_warez_rule_answers_as_it_did() {
    let rule = warez_one_package();
    rule.validate().expect("valid");
    let fetcher = Recorded::default().page(WAREZ_API_URL, WAREZ_API);
    let crawl = run(&rule, &fetcher, WAREZ_PROBE).await.expect("crawled");
    assert_eq!(crawl.links, golden(WAREZ_LINKS));
    assert_eq!(crawl.links.len(), 54);
    assert_eq!(crawl.package_name.as_deref(), Some(SHOW));
    assert!(crawl.groups.is_empty());
    assert!(!crawl.mirrors);
}

#[tokio::test]
async fn the_hide_cx_rule_answers_as_the_owner_s_did() {
    // The shipped rule is the owner's own, unchanged.
    let rule = payload_rule("hide-cx");
    let owner: Rule = serde_json::from_value(serde_json::json!({
        "id": "hide-cx", "name": "hide.cx", "group": "paste", "version": 1,
        "match": { "hosts": ["hide.cx", "*.hide.cx"], "paths": ["^/container/[0-9A-Za-z-]+/?$"] },
        "steps": [
            { "kind": "regex", "from": "url", "pattern": "/container/([0-9A-Za-z-]+)", "into": "cid" },
            { "kind": "fetch", "url": "https://api.hide.cx/containers/${cid}", "into": "container" },
            { "kind": "regex", "from": "container",
              "pattern": "\\{\\s*\"id\"\\s*:\\s*\"([^\"]+)\"\\s*,\\s*\"host\"", "into": "lid", "all": true },
            { "kind": "fetch-json", "url": "https://api.hide.cx/containers/${cid}/links/${lid}",
              "path": "/url", "into": "links" }
        ],
        "package": { "from": "regex", "pattern": "\"name\"\\s*:\\s*\"([^\"]+)\"", "source": "container" },
        "probe": HIDE_PROBE,
        "checked": "2026-10-07"
    }))
    .expect("the owner's rule");
    assert_eq!(rule, owner);

    let crawl = run(&rule, &hide_cx_answers(), HIDE_PROBE)
        .await
        .expect("crawled");
    assert_eq!(crawl.links, golden(HIDE_LINKS));
    assert_eq!(crawl.links.len(), 12);
    assert_eq!(crawl.package_name.as_deref(), Some(S01_720P));
    assert_eq!(
        crawl.pages_fetched, 13,
        "the container and one answer per link"
    );
    assert!(crawl.groups.is_empty());
}

/// The acceptance case of RD-1170-02: four releases, four packages, each named after its
/// release, and each file at ddownload and at rapidgator one mirror group.
///
/// Since RD-1190-17 the shipped rule lists the releases first (`groups.pick`), and each one
/// picked becomes the same package it was before; the rule without `pick` -- what a person's
/// own copy of the earlier rule is -- still yields all four in one run.
#[tokio::test]
async fn the_warez_rule_yields_a_package_per_release_with_its_hosters_as_mirrors() {
    let rule = payload_rule("warez-cx");
    let groups = rule.groups.as_ref().expect("the shipped rule has groups");
    assert_eq!(groups.mirrors, Some(GroupMirrors::ByHost));
    assert!(groups.pick.is_some(), "the shipped rule offers the choice");
    let fetcher = Recorded::default().page(WAREZ_API_URL, WAREZ_API);
    let listed = run(&rule, &fetcher, WAREZ_PROBE).await.expect("listed");
    assert!(
        listed.groups.is_empty() && listed.links.is_empty(),
        "nothing resolved yet"
    );
    let list = listed.pick.clone().expect("a list to choose from");
    let seasons: Vec<Attributes<'_>> = list
        .entries
        .iter()
        .map(|entry| {
            let value = |name: &str| entry.attributes.get(name).map(String::as_str);
            (
                value("season"),
                value("resolution"),
                value("language"),
                value("episode"),
            )
        })
        .collect();
    assert_eq!(
        seasons,
        [
            (Some("1"), Some("720p"), Some("German"), None),
            (Some("1"), Some("1080p"), Some("German"), None),
            (Some("2"), Some("720p"), Some("German"), None),
            (Some("2"), Some("1080p"), Some("German"), None),
        ],
        "four season packs"
    );
    let clock = rd_siterules::SystemClock::new();
    let executor = rd_siterules::Executor::new(&fetcher, &recorded::PublicDns, &clock);
    let mut picked = Vec::new();
    for index in 0..list.entries.len() {
        let group = executor
            .resolve(&rule, &listed.address, &list, index)
            .await
            .expect("resolved");
        picked.push(group);
    }
    assert_eq!(list.entries[0].label.as_deref(), Some(S01_720P));

    let mut whole = rule.clone();
    if let Some(groups) = whole.groups.as_mut() {
        groups.pick = None;
    }
    let crawl = run(&whole, &fetcher, WAREZ_PROBE).await.expect("crawled");
    assert_eq!(
        picked, crawl.groups,
        "a picked release is the package it always was"
    );

    let names: Vec<&str> = crawl
        .groups
        .iter()
        .map(|group| group.name.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        [
            S01_720P,
            "The.Beginning.After.the.End.2025.S01.German.Subbed.ANiME.1080p.AMZN.WEB.H264-WAREZCX",
            "The.Beginning.After.the.End.2025.S02.German.Subbed.ANiME.720p.AMZN.WEB.H264-WAREZCX",
            "The.Beginning.After.the.End.2025.S02.German.Subbed.ANiME.1080p.AMZN.WEB.H264-WAREZCX",
        ]
    );
    let sizes: Vec<usize> = crawl.groups.iter().map(|group| group.links.len()).collect();
    assert_eq!(sizes, [24, 26, 2, 2]);
    // Every link has exactly one copy at the other hoster: two members per mirror set.
    for group in &crawl.groups {
        let half = u32::try_from(group.links.len() / 2).expect("small");
        for set in 1..=half {
            let members: Vec<&str> = group
                .links
                .iter()
                .filter(|link| link.mirror == Some(set))
                .map(|link| link.url.as_str())
                .collect();
            assert_eq!(
                members.len(),
                2,
                "set {set} of {:?}: {members:?}",
                group.name
            );
            assert!(
                members[0].contains("ddownload.com") != members[1].contains("ddownload.com"),
                "one copy per hoster: {members:?}"
            );
        }
    }
    // The first episode of season 1 in 720p, at both hosters.
    let first = &crawl.groups[0].links;
    assert_eq!(first[0].url, "https://ddownload.com/tz91pe4u16o4");
    assert_eq!(first[0].mirror, Some(1));
    assert_eq!(
        first[12].url,
        "https://rapidgator.net/file/47194bdb9826ec9bdde9e5462fcdea2f"
    );
    assert_eq!(first[12].mirror, Some(1));
    // Nothing lost against the one-package form: the same 54 links, in the same order.
    assert_eq!(crawl.links, golden(WAREZ_LINKS));
    assert_eq!(crawl.package_name.as_deref(), Some(SHOW));
    assert_eq!(crawl.pages_fetched, 1);
}

/// What the next signature will carry: every rule valid, unique and probed, the eight that
/// are already signed unchanged, the three of 2026-10-07 added after them (serienjunkies.org
/// with RD-1170-03).
#[test]
fn the_payload_is_what_was_signed() {
    let pack = payload();
    assert_eq!(pack.format_version, rd_siterules::FORMAT_VERSION);
    pack.validate()
        .expect("the payload validates as `sign` validates it");
    let signed = release_pack();
    assert_eq!(
        pack.sequence, signed.sequence,
        "the payload is what was signed"
    );
    assert_eq!(
        pack.rules, signed.rules,
        "the signed rules are the payload's"
    );
    for rule in &signed.rules {
        assert_eq!(&shipped(&rule.id), rule);
    }
    let ids: Vec<&str> = pack.rules.iter().map(|rule| rule.id.as_str()).collect();
    assert!(ids.ends_with(&["hide-cx", "warez-cx", "serienjunkies"]));
    for rule in &pack.rules {
        if ids[..8].contains(&rule.id.as_str()) {
            continue;
        }
        assert_eq!(rule.checked.to_string(), "2026-10-07", "{}", rule.id);
        let probe = url::Url::parse(&rule.probe).expect("probe");
        assert!(rule.claims(&probe), "{} claims its own probe", rule.id);
    }
}

/// Every rule of the payload writes back exactly what was read, so the signature made from
/// it covers the rules as written.
#[test]
fn every_payload_rule_writes_back_what_was_read() {
    let document: serde_json::Value = serde_json::from_str(PAYLOAD).expect("the payload");
    for (written, rule) in document["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .zip(payload().rules)
    {
        assert_eq!(
            &serde_json::to_value(&rule).expect("encode"),
            written,
            "{}",
            rule.id
        );
    }
}
