//! Tests for [`super`]: a listed page waits for a choice, only the chosen entries are
//! resolved, one after the other, an unanswered captcha leaves its entry pending, and a stop
//! gives up the entry in flight without delivering anything.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use rd_siterules::{
    Catalogue, Crawl, CrawlGroup, GroupLink, PickEntry, PickList, Rule, RunError, exec::Variables,
};
use url::Url;

use super::{CANCELLED, EntryOutcome, EntryState, PickDelivery, PickError, PickJob};
use crate::siterules::{RuleOutcome, RuleRunner, SiteRules};

const PAGE: &str = "https://series.example/serie/show/";

/// A two-stage rule whose group asks for a captcha, as serienjunkies.org's does.
fn rule() -> Rule {
    serde_json::from_value(serde_json::json!({
        "id": "series",
        "name": "series.example",
        "group": "board",
        "version": 1,
        "match": { "hosts": ["series.example"] },
        "steps": [{ "kind": "fetch" }, { "kind": "regex", "pattern": "(r\\d)", "into": "releases",
                    "all": true }],
        "package": { "from": "title" },
        "groups": {
            "from": "releases",
            "pick": { "attributes": { "season": "(\\d)" } },
            "steps": [
                { "kind": "captcha", "challenge": "recaptcha-v2", "sitekey": "key" },
                { "kind": "regex", "from": "entry", "pattern": "(.+)", "into": "links" }
            ],
            "package": { "from": "variable", "name": "entry" }
        },
        "probe": PAGE,
        "checked": "2026-10-07"
    }))
    .expect("rule")
}

fn listed() -> Crawl {
    let entry = |text: &str| PickEntry {
        text: text.to_owned(),
        label: Some(format!("Show.{text}")),
        attributes: BTreeMap::from([("season".to_owned(), "1".to_owned())]),
    };
    Crawl {
        address: PAGE.parse().expect("url"),
        links: Vec::new(),
        package_name: Some("Show".to_owned()),
        pages_fetched: 2,
        mirrors: false,
        groups: Vec::new(),
        pick: Some(PickList {
            entries: vec![entry("r1"), entry("r2"), entry("r3"), entry("r4")],
            variables: Variables::default(),
        }),
    }
}

/// Lists the page; resolves r1 and r4, leaves r2's captcha unanswered, finds r3 changed --
/// and, when told to, never answers at all, the way a captcha nobody looks at does not.
struct Runner {
    hang: bool,
    asked: Mutex<Vec<usize>>,
}

impl Runner {
    fn new(hang: bool) -> Arc<Self> {
        Arc::new(Self {
            hang,
            asked: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl RuleRunner for Runner {
    async fn run(&self, _rule: &Rule, _address: &Url) -> Result<Crawl, RunError> {
        Ok(listed())
    }

    async fn resolve(
        &self,
        _rule: &Rule,
        _address: &Url,
        list: &PickList,
        index: usize,
    ) -> Result<CrawlGroup, RunError> {
        self.asked.lock().expect("asked").push(index);
        if self.hang {
            std::future::pending::<()>().await;
        }
        match index {
            1 => Err(RunError::CaptchaFailed {
                step: 2,
                reason: "captcha.timeout".to_owned(),
            }),
            2 => Err(RunError::NoLinks),
            _ => Ok(CrawlGroup {
                name: None,
                links: vec![GroupLink {
                    url: format!("https://hoster.example/{}", list.entries[index].text),
                    mirror: None,
                }],
            }),
        }
    }
}

/// The LinkGrabber, as far as these tests need one.
#[derive(Default)]
struct Grabber(Mutex<Vec<(Option<String>, Vec<String>)>>);

#[async_trait]
impl PickDelivery for Grabber {
    async fn deliver(&self, job: &PickJob, group: CrawlGroup) -> Result<u32, String> {
        let links: Vec<String> = group.links.into_iter().map(|link| link.url).collect();
        let count = u32::try_from(links.len()).unwrap_or(u32::MAX);
        self.0
            .lock()
            .expect("delivered")
            .push((job.label.clone(), links));
        Ok(count)
    }
}

fn rules(runner: Arc<Runner>) -> SiteRules {
    let rule = rule();
    rule.validate().expect("valid");
    SiteRules::new(Catalogue::new(vec![rule]), runner)
}

async fn list_page(rules: &SiteRules) -> String {
    let outcome = rules
        .consult(&PAGE.parse().expect("url"))
        .await
        .expect("claimed");
    let RuleOutcome::Listed { rule, page } = outcome else {
        panic!("not listed: {outcome:?}");
    };
    assert_eq!(rule, "series.example");
    assert_eq!(page.entries, 4);
    page.id
}

#[tokio::test]
async fn a_listed_page_waits_and_only_the_chosen_entries_are_resolved_in_turn() {
    let runner = Runner::new(false);
    let rules = rules(Arc::clone(&runner));
    let id = list_page(&rules).await;
    let page = rules.picks().page(&id).expect("kept");
    assert!(
        page.progress
            .iter()
            .all(|entry| entry.state == EntryState::Pending)
    );
    assert!(
        runner.asked.lock().expect("asked").is_empty(),
        "nothing resolved yet"
    );

    let (queued, round) = rules.picks().queue(&id, &[0, 1, 2]).expect("queued");
    assert_eq!(queued.total, 3);
    let round = round.expect("a worker has to start");
    // Asking again while the round runs adds to it rather than starting a second worker.
    let (_, again) = rules.picks().queue(&id, &[3]).expect("queued");
    assert_eq!(again, None);

    let grabber = Grabber::default();
    rules.work_picks(&id, round, &grabber).await;

    assert_eq!(*runner.asked.lock().expect("asked"), [0, 1, 2, 3]);
    let page = rules.picks().page(&id).expect("kept");
    let states: Vec<_> = page.progress.iter().map(|entry| entry.state).collect();
    assert_eq!(
        states,
        [
            EntryState::Done,
            // Nobody answered: back to pending, not failed, and it says why.
            EntryState::Pending,
            EntryState::Failed,
            EntryState::Done
        ]
    );
    assert_eq!(
        page.progress[1].code.as_deref(),
        Some("site_rules.captcha_failed")
    );
    assert_eq!(
        page.progress[2].code.as_deref(),
        Some("site_rules.no_links")
    );
    assert_eq!(page.progress[0].links, 1);
    assert_eq!((page.finished, page.total), (4, 4));
    assert!(!page.running);
    // One package per release, named after it.
    let delivered = grabber.0.lock().expect("delivered").clone();
    assert_eq!(
        delivered,
        [
            (
                Some("Show.r1".to_owned()),
                vec!["https://hoster.example/r1".to_owned()]
            ),
            (
                Some("Show.r4".to_owned()),
                vec!["https://hoster.example/r4".to_owned()]
            ),
        ]
    );

    // The pending entry can be picked again; a done one is left alone.
    let (_, round) = rules.picks().queue(&id, &[0, 1]).expect("queued");
    assert!(round.is_some());
    let page = rules.picks().page(&id).expect("kept");
    assert_eq!(page.progress[0].state, EntryState::Done);
    assert_eq!(page.progress[1].state, EntryState::Queued);
    assert_eq!(page.total, 1);
}

#[tokio::test]
async fn a_stop_gives_up_the_entry_waiting_for_its_captcha_and_delivers_nothing() {
    let runner = Runner::new(true);
    let rules = Arc::new(rules(Arc::clone(&runner)));
    let id = list_page(&rules).await;
    let (_, round) = rules.picks().queue(&id, &[0, 1]).expect("queued");
    let grabber = Arc::new(Grabber::default());
    let worker = {
        let rules = Arc::clone(&rules);
        let grabber = Arc::clone(&grabber);
        let id = id.clone();
        tokio::spawn(async move {
            rules
                .work_picks(&id, round.expect("round"), grabber.as_ref())
                .await;
        })
    };
    // Waits until the first entry is in front of the (silent) broker.
    for _ in 0..200 {
        if !runner.asked.lock().expect("asked").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let page = rules.picks().page(&id).expect("kept");
    assert_eq!(
        page.progress[0].state,
        EntryState::Captcha,
        "waiting for a person"
    );
    assert_eq!(page.progress[1].state, EntryState::Queued);

    let stopped = rules.picks().cancel(&id).expect("stopped");
    assert!(!stopped.running);
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .expect("the worker ends")
        .expect("the worker did not panic");
    let page = rules.picks().page(&id).expect("kept");
    for entry in &page.progress[..2] {
        assert_eq!(entry.state, EntryState::Pending);
        assert_eq!(entry.code.as_deref(), Some(CANCELLED));
    }
    assert!(grabber.0.lock().expect("delivered").is_empty());
    assert_eq!(
        *runner.asked.lock().expect("asked"),
        [0],
        "the second never started"
    );
}

#[tokio::test]
async fn the_board_refuses_what_it_does_not_hold_and_keeps_one_list_per_page() {
    let rules = rules(Runner::new(false));
    let first = list_page(&rules).await;
    assert_eq!(
        rules.picks().queue("nothing", &[0]).map(|_| ()),
        Err(PickError::NotFound)
    );
    assert_eq!(
        rules.picks().queue(&first, &[9]).map(|_| ()),
        Err(PickError::NoEntry(9))
    );
    // The same page listed again replaces the list rather than adding a second one.
    let second = list_page(&rules).await;
    let ids: Vec<String> = rules
        .picks()
        .pages()
        .into_iter()
        .map(|page| page.id)
        .collect();
    assert_eq!(ids, std::slice::from_ref(&second));
    rules.picks().finish(&second, 0, EntryOutcome::Done(3));
    assert!(rules.picks().remove(&second));
    assert!(rules.picks().page(&second).is_none());
    assert!(!rules.picks().remove(&second));
}
