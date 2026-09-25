//! Tests for [`super`]: which source is asked when, what a refusal means, and the
//! rules a crawler's answer has to keep (RD-104-03, RD-107-05, RD-108-07, RD-110-06).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::AccountId;
use rd_plugin_host::extension::{CrawlRefusal, CrawledLink as HostLink};

use rd_siterules::{Catalogue, Crawl, RunError};

use super::{
    CrawlOutcome, CrawledLink, Crawler, CrawlerPlugin, FolderCrawlers, keep_newest_version, order,
    sanitize, share_login, split_crawled_address,
};
use crate::siterules::{
    SiteRules,
    tests::{FakeRunner, crawl as rule_crawl, crawl_with_mirrors as rule_crawl_mirrored, rule},
};

/// What one stand-in crawler answers, and the record of who was asked.
struct Fake {
    claims_everything: bool,
    answer: Answer,
    asked: Arc<Mutex<Vec<String>>>,
    name: String,
}

#[derive(Clone, Copy)]
enum Answer {
    /// One file, at this address.
    Found(&'static str),
    /// "I claimed it and it is not mine" — the refusal the selection falls through.
    NotMine,
    /// A real refusal about the folder itself.
    Refused(&'static str),
}

#[async_trait]
impl CrawlerPlugin for Fake {
    async fn claims_address(&self, _url: &str) -> Result<bool> {
        Ok(self.claims_everything)
    }

    async fn crawl_address(
        &self,
        _url: &str,
        _account: Option<AccountId>,
    ) -> Result<Result<Vec<HostLink>, CrawlRefusal>> {
        self.asked.lock().expect("asked").push(self.name.clone());
        Ok(match self.answer {
            Answer::Found(url) => Ok(vec![HostLink {
                url: url.to_owned(),
                file_name: Some("a.bin".to_owned()),
                size: Some(7),
                package_hint: None,
                mirror_hint: None,
            }]),
            Answer::NotMine => Err(CrawlRefusal {
                code: Some("fake.not_mine".to_owned()),
                message: "not mine".to_owned(),
                not_mine: true,
            }),
            Answer::Refused(code) => Err(CrawlRefusal {
                code: Some(code.to_owned()),
                message: "the folder is empty".to_owned(),
                not_mine: false,
            }),
        })
    }
}

fn crawler(name: &str, generic: bool, answer: Answer, asked: &Arc<Mutex<Vec<String>>>) -> Crawler {
    Crawler {
        id: format!("plugin.{name}"),
        name: name.to_owned(),
        claims: Vec::new(),
        generic,
        plugin: Box::new(Fake {
            claims_everything: true,
            answer,
            asked: Arc::clone(asked),
            name: name.to_owned(),
        }),
    }
}

fn crawlers(plugins: Vec<Crawler>) -> FolderCrawlers {
    let mut plugins = plugins;
    order(&mut plugins);
    FolderCrawlers {
        plugins,
        rules: None,
    }
}

/// The same selection with one rule in it, claiming every path on the test host.
fn with_rule(
    plugins: Vec<Crawler>,
    answer: Result<Crawl, RunError>,
    asked: &Arc<Mutex<Vec<String>>>,
) -> FolderCrawlers {
    let rules = SiteRules::new(
        Catalogue::new(vec![rule("the-rule", "cloud.example.org")]),
        Arc::new(FakeRunner::new(vec![("the-rule", answer)], asked)),
    );
    crawlers(plugins).with_rules(Arc::new(rules))
}

fn address() -> url::Url {
    "https://cloud.example.org/s/abc123".parse().expect("url")
}

/// RD-107-05, host gap 3: a crawler that claimed an address and then found it was not
/// its own hands the link on instead of ending it.
#[tokio::test]
async fn a_crawler_that_disclaims_an_address_passes_it_on() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let crawlers = crawlers(vec![
        crawler("a-wrong", false, Answer::NotMine, &asked),
        crawler(
            "b-right",
            false,
            Answer::Found("https://cloud.example.org/f/a.bin"),
            &asked,
        ),
    ]);
    let outcome = crawlers.expand(&address(), &HashMap::new()).await;
    assert!(
        matches!(outcome, CrawlOutcome::Links(ref links) if links.len() == 1),
        "the second crawler's files, not the first one's refusal: {outcome:?}"
    );
    assert_eq!(
        *asked.lock().expect("asked"),
        vec!["a-wrong".to_owned(), "b-right".to_owned()],
        "both were asked, in order"
    );
}

/// And when nobody claims it after all, the address stays exactly what it was — not a
/// refusal somebody would have to read.
#[tokio::test]
async fn an_address_every_crawler_disclaims_is_simply_not_claimed() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let crawlers = crawlers(vec![
        crawler("a", false, Answer::NotMine, &asked),
        crawler("b", true, Answer::NotMine, &asked),
    ]);
    assert_eq!(
        crawlers.expand(&address(), &HashMap::new()).await,
        CrawlOutcome::NotClaimed
    );
}

/// A refusal about the folder itself still ends the search: "this folder is empty" is an
/// answer, and handing the address to the next crawler would produce a second one.
#[tokio::test]
async fn a_refusal_about_the_folder_is_still_the_answer() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let crawlers = crawlers(vec![
        crawler(
            "a",
            false,
            Answer::Refused("nextcloud_crawler.empty"),
            &asked,
        ),
        crawler(
            "b",
            false,
            Answer::Found("https://cloud.example.org/f/a.bin"),
            &asked,
        ),
    ]);
    let outcome = crawlers.expand(&address(), &HashMap::new()).await;
    assert!(
        matches!(
            outcome,
            CrawlOutcome::Refused { ref code, .. } if code == "nextcloud_crawler.empty"
        ),
        "{outcome:?}"
    );
    assert_eq!(*asked.lock().expect("asked"), vec!["a".to_owned()]);
}

/// A generic crawler is asked after every crawler that names a service, whatever order
/// the installer read them in.
#[tokio::test]
async fn a_generic_crawler_is_asked_last() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let crawlers = crawlers(vec![
        crawler("a-generic", true, Answer::NotMine, &asked),
        crawler("z-specific", false, Answer::NotMine, &asked),
    ]);
    let _ = crawlers.expand(&address(), &HashMap::new()).await;
    assert_eq!(
        *asked.lock().expect("asked"),
        vec!["z-specific".to_owned(), "a-generic".to_owned()],
        "the specific crawler goes first even though its name sorts later"
    );
}

/// Installing 1.2.4 leaves 1.2.3 on disk, and both verify. Keeping both asked one
/// crawler about the same address twice and expanded the folder into the review list
/// twice, which reads as a duplicate the person then has to clean up by hand.
#[test]
fn only_the_newest_installed_version_of_a_crawler_runs() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut plugins = vec![
        crawler("newest", false, Answer::NotMine, &asked),
        crawler("older", false, Answer::NotMine, &asked),
    ];
    // Two installed versions of one plugin; `load_verified` yields the newest first.
    for plugin in &mut plugins {
        plugin.id = "019d0000-0000-7000-8000-00000000abcd".to_owned();
    }
    keep_newest_version(&mut plugins);
    assert_eq!(plugins.len(), 1, "one plugin id, one crawler");
    assert_eq!(
        plugins[0].name, "newest",
        "the newest version is the one kept"
    );
}

fn found(url: &str) -> CrawledLink {
    let (address, login) = split_crawled_address(url).expect("a usable address");
    CrawledLink {
        url: address,
        file_name: None,
        size: None,
        package_hint: None,
        mirror: None,
        login,
    }
}

/// A crawler may say which user its files are fetched as. It may not put a password in
/// the address: an address is written to a database column, answered over REST and
/// printed in log lines, so one credential there leaks in three places at once.
#[test]
fn a_login_is_lifted_out_of_an_address_and_a_password_in_one_is_refused() {
    let (url, login) =
        split_crawled_address("https://anonymous@cloud.example.org/public.php/dav/x.bin")
            .expect("a usable address");
    assert_eq!(login.as_deref(), Some("anonymous"));
    assert_eq!(
        url.as_str(),
        "https://cloud.example.org/public.php/dav/x.bin",
        "the address kept afterwards carries no userinfo at all"
    );

    assert!(
        split_crawled_address("https://anonymous:s3cret@cloud.example.org/x.bin").is_none(),
        "an address carrying a password is dropped, not cleaned up"
    );
    assert!(split_crawled_address("https://%41%3a@cloud.example.org/x.bin").is_none());
    assert!(split_crawled_address("ftp://anonymous@cloud.example.org/x.bin").is_none());
    assert_eq!(
        split_crawled_address("https://cloud.example.org/x.bin")
            .expect("a usable address")
            .1,
        None,
    );
}

/// The credential a protected share needs is scoped to the files that share found, and
/// to nothing else on that host.
#[test]
fn the_login_of_a_share_is_scoped_to_the_files_it_found() {
    let links = vec![
        found("https://anonymous@cloud.example.org/public.php/dav/files/QxT7/a.bin"),
        found("https://anonymous@cloud.example.org/public.php/dav/files/QxT7/Season%201/b.bin"),
    ];
    let login = share_login(&links).expect("one login");
    assert_eq!(login.username, "anonymous");
    assert_eq!(login.scope.host, "cloud.example.org");
    assert_eq!(
        login.scope.path_prefix.as_deref(),
        Some("/public.php/dav/files/QxT7"),
        "the longest folder every file sits under, not the host"
    );
    assert!(!login.scope.include_subdomains);

    // A public share names no login and mints no credential.
    assert_eq!(
        share_login(&[found(
            "https://cloud.example.org/public.php/dav/files/QxT7/a.bin"
        )]),
        None
    );
    // Two hosts, two logins, or a prefix that has shrunk to nothing: no profile at all.
    assert_eq!(
        share_login(&[
            found("https://anonymous@cloud.example.org/d/a.bin"),
            found("https://anonymous@other.example.org/d/b.bin"),
        ]),
        None
    );
    assert_eq!(
        share_login(&[
            found("https://anonymous@cloud.example.org/d/a.bin"),
            found("https://someone@cloud.example.org/d/b.bin"),
        ]),
        None
    );
    assert_eq!(
        share_login(&[
            found("https://anonymous@cloud.example.org/a.bin"),
            found("https://anonymous@cloud.example.org/b.bin"),
        ]),
        None,
        "a credential is never scoped to a whole host on a crawler's say-so"
    );
}

#[test]
fn a_name_a_stranger_chose_cannot_carry_a_path_out_of_its_folder() {
    assert_eq!(sanitize(Some("Season 1")).as_deref(), Some("Season 1"));
    assert_eq!(sanitize(Some("a/../b")).as_deref(), Some("a/b"));
    assert_eq!(sanitize(Some("..")), None);
    assert_eq!(sanitize(Some("a\\b")).as_deref(), Some("ab"));
    assert_eq!(sanitize(Some("  ")), None);
    assert_eq!(sanitize(Some("x\u{0}y")).as_deref(), Some("xy"));
    assert_eq!(sanitize(None), None);
}

/// RD-110-06, the order itself, with all three kinds of source offering themselves at once:
/// a crawler plugin that names a service, a rule, and a generic crawler. A reordering would
/// fail here rather than somewhere a person notices it.
#[tokio::test]
async fn the_order_is_plugin_then_rule_then_generic() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let crawlers = with_rule(
        vec![
            crawler("a-generic", true, Answer::NotMine, &asked),
            crawler("z-specific", false, Answer::NotMine, &asked),
        ],
        Err(RunError::NotClaimed(
            "https://cloud.example.org/s/abc123".to_owned(),
        )),
        &asked,
    );

    // And the other half of the criterion: an address no source claims stays exactly what it
    // was, with the same outcome it had before there were any rules.
    assert_eq!(
        crawlers.expand(&address(), &HashMap::new()).await,
        CrawlOutcome::NotClaimed
    );
    assert_eq!(
        *asked.lock().expect("asked"),
        vec![
            "z-specific".to_owned(),
            "the-rule".to_owned(),
            "a-generic".to_owned()
        ],
        "the specific plugin first, then the rule, then the generic crawler"
    );
}

/// A rule that found the files answers, and the generic crawlers are never reached. The
/// package name it read travels as the `package_hint` of every link, which is what
/// `rd_collector::grouping` names the package from.
#[tokio::test]
async fn a_rule_answers_before_the_generic_crawlers_and_names_the_package() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let crawlers = with_rule(
        vec![crawler(
            "a-generic",
            true,
            Answer::Found("https://cloud.example.org/f/wrong.bin"),
            &asked,
        )],
        Ok(rule_crawl(
            &[
                "https://host.example.org/a.bin",
                "https://host.example.org/b.bin",
            ],
            Some("Show S01"),
        )),
        &asked,
    );

    let outcome = crawlers.expand(&address(), &HashMap::new()).await;
    let CrawlOutcome::Links(links) = outcome else {
        panic!("the rule's links, not the generic crawler's: {outcome:?}");
    };
    assert_eq!(links.len(), 2);
    assert!(
        links
            .iter()
            .all(|link| link.package_hint.as_deref() == Some("Show S01")),
        "every link carries the rule's package name: {links:?}"
    );
    assert_eq!(*asked.lock().expect("asked"), vec!["the-rule".to_owned()]);
    assert!(
        links.iter().all(|link| link.mirror.is_none()),
        "a rule that did not say so declares no mirrors"
    );
}

/// Source 1 of a mirror group: the rule says its page is one release (RD-110-18).
///
/// Every link the run produced then carries the same key, and the key names the rule *and*
/// the address it read — two such pages crawled into one package have to stay two groups.
/// The rule format has no per-link metadata, so it names neither quality nor language.
#[tokio::test]
async fn a_rule_that_says_its_page_is_one_release_marks_its_links_as_mirrors() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let crawlers = with_rule(
        Vec::new(),
        Ok(rule_crawl_mirrored(
            &[
                "https://one.example.org/a.bin",
                "https://two.example.org/b.bin",
                "https://three.example.org/c.bin",
            ],
            Some("Show S01E01"),
            true,
        )),
        &asked,
    );

    let outcome = crawlers.expand(&address(), &HashMap::new()).await;
    let CrawlOutcome::Links(links) = outcome else {
        panic!("the rule's links: {outcome:?}");
    };
    assert_eq!(links.len(), 3);
    let first = links[0].mirror.as_ref().expect("a mirror key");
    assert!(
        first.group.contains("the-rule") && first.group.contains("board.example.org"),
        "the key names the rule and the page: {}",
        first.group
    );
    assert!(
        links
            .iter()
            .all(|link| link.mirror.as_ref().map(|hint| &hint.group) == Some(&first.group)),
        "one group for the whole page: {links:?}"
    );
    assert!(
        links.iter().all(|link| link
            .mirror
            .as_ref()
            .is_some_and(|hint| hint.quality.is_none() && hint.language.is_none())),
        "the rule format names neither quality nor language"
    );
}

/// A rule whose `match` does not claim the address says nothing about it, so the generic
/// crawler behind it still gets its turn.
#[tokio::test]
async fn an_address_no_rule_claims_reaches_the_generic_crawlers() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let crawlers = with_rule(
        vec![crawler(
            "a-generic",
            true,
            Answer::Found("https://cloud.example.org/f/a.bin"),
            &asked,
        )],
        Err(RunError::NotClaimed(
            "https://cloud.example.org/s/abc123".to_owned(),
        )),
        &asked,
    );

    let outcome = crawlers.expand(&address(), &HashMap::new()).await;
    assert!(
        matches!(outcome, CrawlOutcome::Links(ref links) if links.len() == 1),
        "{outcome:?}"
    );
    assert_eq!(
        *asked.lock().expect("asked"),
        vec!["the-rule".to_owned(), "a-generic".to_owned()]
    );
}

/// "The page is gone" is a statement about this page, not about whose page it is: it ends
/// the search with its own stable code instead of letting the next source produce a second
/// answer about the same address.
#[tokio::test]
async fn a_rule_that_says_the_page_is_dead_ends_the_search() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let crawlers = with_rule(
        vec![crawler(
            "a-generic",
            true,
            Answer::Found("https://cloud.example.org/f/a.bin"),
            &asked,
        )],
        Err(RunError::PageDead {
            url: "https://cloud.example.org/s/abc123".to_owned(),
            reason: "404".to_owned(),
        }),
        &asked,
    );

    let outcome = crawlers.expand(&address(), &HashMap::new()).await;
    assert!(
        matches!(
            outcome,
            CrawlOutcome::Refused { ref code, .. } if code == "site_rules.page_dead"
        ),
        "{outcome:?}"
    );
    assert_eq!(
        *asked.lock().expect("asked"),
        vec!["the-rule".to_owned()],
        "the generic crawler was not asked about a page that is gone"
    );
}

/// The caller skips the whole pass when the selection is empty, so a service that has rules
/// but no crawler plugin must not read as empty.
#[tokio::test]
async fn a_selection_with_rules_and_no_plugins_is_not_empty() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let crawlers = with_rule(
        Vec::new(),
        Err(RunError::NotClaimed("x".to_owned())),
        &asked,
    );
    assert!(!crawlers.is_empty());
    assert!(!crawlers.has_plugins());
    assert!(FolderCrawlers::none().is_empty());
}
