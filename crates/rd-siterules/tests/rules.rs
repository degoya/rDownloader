//! The shipped rules against recorded answers (RD-110-10).
//!
//! Every answer here was fetched from the live service on 2026-09-21 and then sanitised:
//! scripts, styles and comments removed, unrelated markup cut out and the cut marked. Nothing
//! in this file touches the network, so a service that goes down breaks the self-test
//! (RD-110-09) rather than the build.
//!
//! The one exception is `scnlog-dmca.html`, which is *derived* rather than recorded: no page
//! carrying the notice was reachable on the measurement day, so the download block of the
//! recorded release page was replaced by the sentence JDownloader's `ScnlogEu` matches. It is
//! labelled as derived in the job file too.
//!
//! `controlc-paste.html` came later, with RD-110-12, and was recorded the same way.
//!
//! The seven services RD-110-11 added are in `release_page_rules.rs`, with their own nine
//! recorded answers; both files share the harness in `recorded/mod.rs`.

mod recorded;

use recorded::{Recorded, release_pack, run, shipped};
use url::Url;

const SCNLOG_RELEASE: &str = include_str!("fixtures/scnlog-release.html");
const SCNLOG_CATEGORY: &str = include_str!("fixtures/scnlog-category.html");
const SCNLOG_DMCA: &str = include_str!("fixtures/scnlog-dmca.html");
const DOWNMAGAZ_RELEASE: &str = include_str!("fixtures/downmagaz-release.html");
const CONTROLC_PASTE: &str = include_str!("fixtures/controlc-paste.html");

const SCNLOG_PROBE: &str =
    "https://scnlog.me/foreign/eyeshield-21-e071-multi-1080p-web-x264-amb3r/";
const DOWNMAGAZ_PROBE: &str =
    "https://downmagaz.net/business_magazine_economics/484259-the-economist-usa-09192026.html";
const CONTROLC_PROBE: &str = "https://controlc.com/cb6c58e7";

/// The pack is what the owner measured on 2026-09-22 (RD-120-17), and nothing else.
///
/// `libgen` and `satdl` were taken out in that pass: libgen.bz no longer resolves its links,
/// and satdl.com was withdrawn. The eight that remain were each opened on the live service
/// that day, which is the date every one of them now carries.
///
/// This case is also the re-signing gate. The list and the date come from
/// `resources/site-rules-payload.json`, the pack under test is the *signed*
/// `resources/site-rules.json` every release carries as an artifact (RD-130-07), and the two
/// only agree once somebody has run `rdownloader site-rules sign` over the payload. An edit
/// that never got a signature fails here rather than in front of a person.
///
/// Since RD-130-07 the rules sit in five groups: `comics` and `magazines` were merged into
/// `ebooks`, `graphics` stays its own, and every group name in the file is one the settings page
/// has a label for.
#[test]
fn the_shipped_pack_carries_the_rules_that_were_measured() {
    let pack = release_pack();
    let ids: Vec<&str> = pack.rules.iter().map(|rule| rule.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "scnlog",
            "downmagaz",
            "paste-generic",
            "getcomics",
            "scene-rls",
            "avaxhome",
            "cgpersia",
            "vipergirls",
        ]
    );
    for rule in &pack.rules {
        assert!(
            ["board", "paste", "ebooks", "graphics", "adult"].contains(&rule.group.as_str()),
            "{} sits in the group {:?}, which is not one of the five",
            rule.id,
            rule.group
        );
        rule.validate().expect("a shipped rule validates");
        // RD-110-09 reads this date, and a rule without one may not ship.
        assert_eq!(
            rule.checked.to_string(),
            "2026-09-22",
            "{} carries its measurement date",
            rule.id
        );
        let probe = Url::parse(&rule.probe).expect("probe");
        assert!(rule.claims(&probe), "{} claims its own probe", rule.id);
    }
}

#[tokio::test]
async fn a_scnlog_release_page_becomes_a_package_of_hoster_links() {
    let fetcher = Recorded::default().page(SCNLOG_PROBE, SCNLOG_RELEASE);
    let crawl = run(&shipped("scnlog"), &fetcher, SCNLOG_PROBE)
        .await
        .expect("crawled");
    assert_eq!(
        crawl.links,
        [
            "https://nitroflare.com/view/3919AC1DC9A1C66/Eyeshield.21.E071.MULTi.1080p.WEB.x264-AMB3R.mkv",
            "https://ddownload.com/ez7v6qhlp3q6/Eyeshield.21.E071.MULTi.1080p.WEB.x264-AMB3R.mkv",
            "https://multiup.io/download/431704e65c9da93a4c3f22a74c437d3f/Eyeshield.21.E071.MULTi.1080p.WEB.x264-AMB3R.mkv",
        ]
    );
    assert_eq!(
        crawl.package_name.as_deref(),
        Some("Eyeshield.21.E071.MULTi.1080p.WEB.x264-AMB3R")
    );
    assert_eq!(crawl.pages_fetched, 1);
}

#[tokio::test]
async fn the_scnlog_rule_leaves_the_pages_own_links_out_of_the_package() {
    // The recorded answer really does carry them: the navigation alone is a dozen.
    assert!(
        SCNLOG_RELEASE.matches("href=\"https://scnlog.me/").count() > 10,
        "the recorded answer carries the site's own links"
    );
    let fetcher = Recorded::default().page(SCNLOG_PROBE, SCNLOG_RELEASE);
    let crawl = run(&shipped("scnlog"), &fetcher, SCNLOG_PROBE)
        .await
        .expect("crawled");
    assert!(
        !crawl.links.iter().any(|link| link.contains("scnlog")),
        "no self-link survived: {:?}",
        crawl.links
    );
}

#[tokio::test]
async fn a_downmagaz_release_page_becomes_a_package_of_hoster_links() {
    let fetcher = Recorded::default().page(DOWNMAGAZ_PROBE, DOWNMAGAZ_RELEASE);
    let crawl = run(&shipped("downmagaz"), &fetcher, DOWNMAGAZ_PROBE)
        .await
        .expect("crawled");
    assert_eq!(
        crawl.links,
        ["https://nfile.cc/qK7XDAwq", "https://dwp.la/d/dro"]
    );
    assert_eq!(
        crawl.package_name.as_deref(),
        Some("The Economist USA - 09.19.2026")
    );
    assert!(
        !crawl.links.iter().any(|link| link.contains("downmagaz")),
        "no self-link survived: {:?}",
        crawl.links
    );
}

#[tokio::test]
async fn a_release_that_was_taken_down_is_page_dead_rather_than_an_empty_package() {
    // Measured on 2026-09-21: both services answer a missing release with a real 404.
    let gone = "https://scnlog.me/movies/this-release-does-not-exist-2026-xyz/";
    let fetcher = Recorded::default().status(gone, 404);
    let error = run(&shipped("scnlog"), &fetcher, gone)
        .await
        .expect_err("a missing release refuses");
    assert_eq!(error.code(), "site_rules.page_dead");
    assert!(!error.not_mine(), "a dead page ends the search");
}

#[tokio::test]
async fn a_scnlog_page_whose_links_were_removed_refuses_with_a_code() {
    let fetcher = Recorded::default().page(SCNLOG_PROBE, SCNLOG_DMCA);
    let error = run(&shipped("scnlog"), &fetcher, SCNLOG_PROBE)
        .await
        .expect_err("a page without links refuses");
    assert_eq!(error.code(), "site_rules.structure");
    assert!(!error.not_mine());
}

#[tokio::test]
async fn a_scnlog_page_that_carries_no_download_block_refuses_with_a_code() {
    // A recorded category listing: a real 200 that is not a release page.
    let listing = "https://scnlog.me/music/black-metal/";
    let fetcher = Recorded::default().page(listing, SCNLOG_CATEGORY);
    let error = run(&shipped("scnlog"), &fetcher, listing)
        .await
        .expect_err("a listing produces no package");
    assert_eq!(error.code(), "site_rules.structure");
}

#[tokio::test]
async fn an_address_on_a_dead_scnlog_domain_is_rewritten_to_the_living_one() {
    // Measured on 2026-09-21: neither scnlog.eu nor scnlog.life resolves at all.
    let rule = shipped("scnlog");
    assert_eq!(rule.dead, ["scnlog.eu", "scnlog.life"]);
    // Only the living address is recorded, so a run that reached the dead host would fail.
    let fetcher = Recorded::default().page(SCNLOG_PROBE, SCNLOG_RELEASE);
    for dead in ["scnlog.eu", "scnlog.life"] {
        let old = SCNLOG_PROBE.replace("scnlog.me", dead);
        let crawl = run(&rule, &fetcher, &old).await.expect("crawled");
        assert_eq!(crawl.address.as_str(), SCNLOG_PROBE);
        assert_eq!(crawl.links.len(), 3);
    }
}

#[tokio::test]
async fn an_address_neither_rule_claims_lets_the_selection_keep_looking() {
    let fetcher = Recorded::default();
    let error = run(
        &shipped("scnlog"),
        &fetcher,
        "https://example.org/some/page/",
    )
    .await
    .expect_err("a foreign address is not claimed");
    assert_eq!(error.code(), "site_rules.not_claimed");
    assert!(error.not_mine(), "only not_claimed moves the search on");
}

#[tokio::test]
async fn a_controlc_paste_becomes_a_package_of_hoster_links() {
    // RD-110-12. The recorded paste is running text with addresses in it: an Indonesian
    // instruction, a quality marker, a hoster name before each address. Only the addresses
    // come out, in the order the paste carries them.
    let fetcher = Recorded::default().page(CONTROLC_PROBE, CONTROLC_PASTE);
    let crawl = run(&shipped("paste-generic"), &fetcher, CONTROLC_PROBE)
        .await
        .expect("crawled");
    assert_eq!(
        crawl.links,
        [
            "https://oload.stream/f/fYN9O-yxlOc",
            "https://thevideo.cc/5eiefakwm7vn",
            "https://bdupload.info/cfnnvk6nppnd",
            "https://clicknupload.org/5b2j7ugvrmim",
            "https://filebebo.com/d/wwrwcx6gdsmf",
            "https://rapidgator.net/file/0bd1b2392e95ed5bee7236992f2e78b0/DWish.mkv.html",
            "http://ul.to/yrtz9eal",
        ]
    );
    assert_eq!(crawl.package_name.as_deref(), Some("Death Wish (2018)"));
    assert_eq!(crawl.pages_fetched, 1);
}

#[tokio::test]
async fn the_paste_rule_leaves_the_pages_own_links_out_of_the_package() {
    // What the container keeps out: the service's own pages, its image host and the font
    // service it loads from. The recorded answer really does carry all three.
    for chrome in [
        "https://controlc.com/images/og.jpg",
        "https://fonts.googleapis.com",
        "href=\"/faq\"",
    ] {
        assert!(
            CONTROLC_PASTE.contains(chrome),
            "the recorded answer carries {chrome}"
        );
    }
    let fetcher = Recorded::default().page(CONTROLC_PROBE, CONTROLC_PASTE);
    let crawl = run(&shipped("paste-generic"), &fetcher, CONTROLC_PROBE)
        .await
        .expect("crawled");
    assert!(
        !crawl
            .links
            .iter()
            .any(|link| link.contains("controlc") || link.contains("fonts.google")),
        "no page of the service's own survived: {:?}",
        crawl.links
    );
}

#[tokio::test]
async fn a_paste_address_that_is_not_a_paste_lets_the_selection_keep_looking() {
    // The service's own pages sit on the same host. `^/[a-f0-9]{8}$` does not claim them, so
    // the selection moves on to the generic crawlers instead of the rule refusing for them.
    let fetcher = Recorded::default();
    for page in ["https://controlc.com/", "https://controlc.com/register"] {
        let error = run(&shipped("paste-generic"), &fetcher, page)
            .await
            .expect_err("the rule does not claim the service's own pages");
        assert_eq!(error.code(), "site_rules.not_claimed", "{page}");
        assert!(error.not_mine(), "{page}");
    }
}

#[tokio::test]
async fn the_paste_rule_revives_an_address_on_a_domain_that_only_forwards() {
    // Measured on 2026-09-21: each of these answers 301 to https://controlc.com/<id> with the
    // paste's own identifier, and www.controlc.com answers 522. The rewrite is what makes a
    // year-old bookmark work, and it saves the redirect hop.
    let rule = shipped("paste-generic");
    assert_eq!(
        rule.dead,
        [
            "www.controlc.com",
            "pasted.co",
            "www.pasted.co",
            "tinypaste.com",
            "www.tinypaste.com",
            "tny.cz",
            "www.tny.cz",
            "binbox.io",
            "www.binbox.io",
        ]
    );
    // Only the living address is recorded, so a run that asked a dead host would fail.
    let fetcher = Recorded::default().page(CONTROLC_PROBE, CONTROLC_PASTE);
    for dead in &rule.dead {
        let old = format!("https://{dead}/cb6c58e7");
        let crawl = run(&rule, &fetcher, &old).await.expect("crawled");
        assert_eq!(crawl.address.as_str(), CONTROLC_PROBE, "{old}");
        assert_eq!(crawl.links.len(), 7, "{old}");
        assert_eq!(crawl.pages_fetched, 1, "{old}");
    }
}

#[tokio::test]
async fn a_deleted_paste_is_page_dead_rather_than_an_empty_package() {
    // Measured on 2026-09-21: controlc.com answers a missing paste with a real 404, both for
    // an identifier of the shape the rule claims and for one it does not.
    let gone = "https://controlc.com/0000dead";
    let fetcher = Recorded::default().status(gone, 404);
    let error = run(&shipped("paste-generic"), &fetcher, gone)
        .await
        .expect_err("a missing paste refuses");
    assert_eq!(error.code(), "site_rules.page_dead");
    assert!(!error.not_mine(), "a dead page ends the search");
}
