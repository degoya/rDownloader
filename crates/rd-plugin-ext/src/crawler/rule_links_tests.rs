//! Tests for [`super`]: a rule with `groups` becomes one package per group with its mirror
//! sets as declared mirror groups, and a rule without them answers as it did.

use rd_siterules::{Crawl, CrawlGroup, GroupLink};

use super::proposals;

fn crawl(groups: Vec<CrawlGroup>) -> Crawl {
    Crawl {
        address: "https://board.example.org/a/b".parse().expect("url"),
        links: groups
            .iter()
            .flat_map(|group| group.links.iter().map(|link| link.url.clone()))
            .collect(),
        package_name: Some("Show".to_owned()),
        pages_fetched: 1,
        mirrors: false,
        groups,
        pick: None,
    }
}

fn link(url: &str, mirror: Option<u32>) -> GroupLink {
    GroupLink {
        url: url.to_owned(),
        mirror,
    }
}

#[test]
fn every_group_is_a_package_and_every_mirror_set_a_group_of_its_own() {
    let found = proposals(
        "board",
        crawl(vec![
            CrawlGroup {
                name: Some("Show.S01.720p".to_owned()),
                links: vec![
                    link("https://one.example/a1", Some(1)),
                    link("https://one.example/a2", Some(2)),
                    link("https://two.example/b1", Some(1)),
                    link("https://two.example/b2", Some(2)),
                ],
            },
            CrawlGroup {
                name: None,
                links: vec![
                    link("https://one.example/c1", Some(1)),
                    link("https://two.example/d1", Some(1)),
                    link("https://three.example/e1", None),
                ],
            },
        ]),
    );
    let packages: Vec<Option<&str>> = found
        .iter()
        .map(|link| link.package_hint.as_deref())
        .collect();
    assert_eq!(
        packages,
        [
            Some("Show.S01.720p"),
            Some("Show.S01.720p"),
            Some("Show.S01.720p"),
            Some("Show.S01.720p"),
            // A group whose source read nothing takes the page's name.
            Some("Show"),
            Some("Show"),
            Some("Show"),
        ]
    );
    let keys: Vec<Option<&str>> = found
        .iter()
        .map(|link| link.mirror_hint.as_ref().map(|hint| hint.group.as_str()))
        .collect();
    let page = "https://board.example.org/a/b";
    let first = format!("board|{page}|0|1");
    let second = format!("board|{page}|0|2");
    let other = format!("board|{page}|1|1");
    assert_eq!(
        keys,
        [
            Some(first.as_str()),
            Some(second.as_str()),
            Some(first.as_str()),
            Some(second.as_str()),
            // Set 1 of the second group is not set 1 of the first: another release.
            Some(other.as_str()),
            Some(other.as_str()),
            None,
        ]
    );
    assert!(
        found
            .iter()
            .filter_map(|link| link.mirror_hint.as_ref())
            .all(|hint| hint.quality.is_none() && hint.language.is_none())
    );
}

#[test]
fn a_rule_without_groups_proposes_what_it_did_before() {
    let mut plain = crawl(Vec::new());
    plain.links = vec![
        "https://one.example/a".to_owned(),
        "https://two.example/b".to_owned(),
    ];
    let found = proposals("board", plain.clone());
    assert!(
        found
            .iter()
            .all(|link| link.package_hint.as_deref() == Some("Show") && link.mirror_hint.is_none())
    );
    plain.mirrors = true;
    let found = proposals("board", plain);
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(
        |link| link.mirror_hint.as_ref().map(|hint| hint.group.as_str())
            == Some("board|https://board.example.org/a/b")
    ));
}
