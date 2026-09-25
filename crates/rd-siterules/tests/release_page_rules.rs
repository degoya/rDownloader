//! The rules RD-110-11 added, against recorded answers.
//!
//! Every answer here was fetched from the live service on 2026-09-21 with a current Chrome
//! user agent, no account and nothing solved, and then sanitised: scripts, stylesheets and
//! comments removed, unrelated markup cut out and every cut marked. Nothing in this file
//! touches the network, so a service that goes down breaks the self-test (RD-110-09) rather
//! than the build.
//!
//! One of them walks a gateway page on the service's own host before the address it produces
//! -- `avaxhome` -- and ends at a redirect, which is recorded as a status and a `location`
//! rather than as a body.
//!
//! Two of the seven, `libgen` and `satdl`, are gone: the owner's check of 2026-09-22
//! (RD-120-17) found libgen.bz no longer resolving its links, and satdl.com was withdrawn.
//! Their rules, fixtures and cases left with them.

mod recorded;

use recorded::{Recorded, run, shipped};
use url::Url;

const GETCOMICS_RELEASE: &str = include_str!("fixtures/getcomics-release.html");
const SCENERLS_RELEASE: &str = include_str!("fixtures/scenerls-release.html");
const AVAXHOME_EBOOK: &str = include_str!("fixtures/avaxhome-ebook.html");
const CGPERSIA_POST: &str = include_str!("fixtures/cgpersia-post.html");
const VIPERGIRLS_THREAD: &str = include_str!("fixtures/vipergirls-thread.html");

const GETCOMICS_PROBE: &str = "https://getcomics.org/dc/absolute-green-arrow-5-2026/";
const SCENERLS_PROBE: &str = "https://scene-rls.com/enigma-1982-webrip-x264-ion10/";
const AVAXHOME_PROBE: &str = "https://avxhm.se/ebooks/3032034086E.html";
const CGPERSIA_PROBE: &str = "https://cgpersia.com/2026/09/udemy-3ds-max-and-3d-space-204968.html";
const VIPERGIRLS_PROBE: &str =
    "https://vipergirls.to/threads/6629687-Adult-Magazines-Mix-Collection";

#[tokio::test]
async fn a_getcomics_post_becomes_a_package_of_hoster_links() {
    let fetcher = Recorded::default().page(GETCOMICS_PROBE, GETCOMICS_RELEASE);
    let crawl = run(&shipped("getcomics"), &fetcher, GETCOMICS_PROBE)
        .await
        .expect("crawled");
    assert_eq!(crawl.links.len(), 6);
    assert!(
        crawl
            .links
            .iter()
            .any(|link| link.starts_with("https://datanodes.to/")),
        "{:?}",
        crawl.links
    );
    assert!(
        crawl
            .links
            .iter()
            .any(|link| link.starts_with("https://vikingfile.com/")),
        "{:?}",
        crawl.links
    );
    assert_eq!(
        crawl.package_name.as_deref(),
        Some("Absolute Green Arrow #5 (2026)")
    );
    assert_eq!(crawl.pages_fetched, 1);
}

#[tokio::test]
async fn the_getcomics_rule_leaves_the_pages_own_reading_links_out_of_the_package() {
    // The recorded answer carries the navigation, the author profile and the reader help
    // pages; none of them sits in an `aio-button-center` block, so none of them survives.
    // The one address inside such a block that is not a file -- the theme's READ ONLINE
    // button -- does survive the rule and is refused later by the crawl verdict
    // (`collector.crawl_not_a_file`, RD-110-07). That division is deliberate: there is no
    // filter step, and a container cannot tell those two buttons apart.
    assert!(GETCOMICS_RELEASE.contains("https://www.7-zip.org/download.html"));
    let fetcher = Recorded::default().page(GETCOMICS_PROBE, GETCOMICS_RELEASE);
    let crawl = run(&shipped("getcomics"), &fetcher, GETCOMICS_PROBE)
        .await
        .expect("crawled");
    assert!(
        !crawl.links.iter().any(|link| link.contains("7-zip.org")
            || link.contains("getcomics.info")
            || link.contains("/tag/")),
        "{:?}",
        crawl.links
    );
    assert_eq!(
        crawl
            .links
            .iter()
            .filter(|link| link.contains("readcomicsonline.ru"))
            .count(),
        1,
        "the read-online button is the one the verdict has to refuse"
    );
}

#[tokio::test]
async fn an_address_on_the_forwarding_getcomics_domain_is_rewritten() {
    // Measured on 2026-09-21: getcomics.info answers every path with a 301 to getcomics.org.
    let rule = shipped("getcomics");
    assert_eq!(rule.dead, ["getcomics.info"]);
    let fetcher = Recorded::default().page(GETCOMICS_PROBE, GETCOMICS_RELEASE);
    let old = GETCOMICS_PROBE.replace("getcomics.org", "getcomics.info");
    let crawl = run(&rule, &fetcher, &old).await.expect("crawled");
    assert_eq!(crawl.address.as_str(), GETCOMICS_PROBE);
    assert_eq!(crawl.pages_fetched, 1, "the rewrite saves the redirect hop");
}

#[tokio::test]
async fn a_scene_rls_release_page_becomes_a_package_of_hoster_links() {
    let fetcher = Recorded::default().page(SCENERLS_PROBE, SCENERLS_RELEASE);
    let crawl = run(&shipped("scene-rls"), &fetcher, SCENERLS_PROBE)
        .await
        .expect("crawled");
    assert_eq!(
        crawl.links,
        [
            "http://nitroflare.com/view/AE54EED43E933D8/Enigma.1982.WEBRip.x264-ION10.mp4",
            "https://rapidgator.net/file/78e6db8ceac7a58a00c11a6c3be7b4e2/Enigma.1982.WEBRip.x264-ION10.mp4.html",
            "https://4downfiles.org/azq2hggv6iel",
            "https://uploadev.org/bdk05b441ikw",
        ]
    );
    assert_eq!(
        crawl.package_name.as_deref(),
        Some("Enigma 1982 WEBRip x264-ION10")
    );
}

#[tokio::test]
async fn the_scene_rls_container_keeps_the_screenshot_and_the_imdb_link_out() {
    // RD-110-10 recorded that scene-rls mixes its own addresses into the links block and
    // could therefore not be described. Re-measured on 2026-09-21 over eight release pages,
    // four per domain: it does not. The screenshot, the IMDB and the TVDB address sit in the
    // description paragraph; the centred `h2` holds the hosters alone. What the finding did
    // catch is one page of the `.com` half whose `h2` also carries the site's own NFO
    // viewer -- that address survives the rule and is refused by the crawl verdict, the same
    // way the getcomics read-online button is.
    assert!(SCENERLS_RELEASE.contains("https://www.imdb.com/title/tt0083891/"));
    assert!(SCENERLS_RELEASE.contains("https://i.imgaa.com/"));
    let fetcher = Recorded::default().page(SCENERLS_PROBE, SCENERLS_RELEASE);
    let crawl = run(&shipped("scene-rls"), &fetcher, SCENERLS_PROBE)
        .await
        .expect("crawled");
    assert!(
        !crawl
            .links
            .iter()
            .any(|link| link.contains("imdb.com") || link.contains("imgaa.com")),
        "{:?}",
        crawl.links
    );
}

#[tokio::test]
async fn the_scene_rls_rule_claims_both_of_its_domains_and_not_its_own_pages() {
    let rule = shipped("scene-rls");
    for claimed in [
        "https://scene-rls.com/enigma-1982-webrip-x264-ion10/",
        "https://scene-rls.net/999-what-happened-next-s02e03-1080p-all4-web-dl-aac2-0-h264-rawr/",
    ] {
        assert!(
            rule.claims(&Url::parse(claimed).expect("address")),
            "{claimed}"
        );
    }
    for unclaimed in [
        "https://scene-rls.com/contact/",
        "https://scene-rls.com/category/movies/",
        "https://scene-rls.net/",
    ] {
        assert!(
            !rule.claims(&Url::parse(unclaimed).expect("address")),
            "{unclaimed}"
        );
    }
}

#[tokio::test]
async fn an_avaxhome_page_hands_its_forwarder_over_to_the_hoster() {
    // Measured on 2026-09-21: the download block holds one `/go/<token>` address on the
    // site's own host, and that address answers 302 to the hoster. The token carries the
    // requester's address, so it is read from the page rather than built.
    let gate = "https://avxhm.se/go/g6NuaWTZIDE0ZmIyNjFjMmY1OTQ4NjNhNzJlYWM1NGM3MTlkYzdmoWkAomlwtTo6ZmZmZjo5My4xMjcuMjUyLjEyNw:1x8m0U:0vxe7UF3FOffByljZGxqB6AXO_o/";
    let target = "https://icerbox.com/lWBye5en/3032034086.epub";
    let fetcher = Recorded::default()
        .page(AVAXHOME_PROBE, AVAXHOME_EBOOK)
        .redirect(gate, target);
    let crawl = run(&shipped("avaxhome"), &fetcher, AVAXHOME_PROBE)
        .await
        .expect("crawled");
    assert_eq!(crawl.links, [target]);
    assert_eq!(
        crawl.package_name.as_deref(),
        Some("Peripheral Nerve Surgery - A Compendium")
    );
    assert_eq!(crawl.pages_fetched, 2, "the page and the forwarder");
}

#[tokio::test]
async fn the_avaxhome_container_keeps_the_advertising_links_out() {
    // The recorded answer carries three paid links in the sidebar; the download block does
    // not, so none of them reaches the package.
    assert!(AVAXHOME_EBOOK.contains("https://koalanames.com"));
    let fetcher = Recorded::default().page(AVAXHOME_PROBE, AVAXHOME_EBOOK);
    let error = run(&shipped("avaxhome"), &fetcher, AVAXHOME_PROBE)
        .await
        .expect_err("the forwarder is not recorded here");
    assert_eq!(
        error.code(),
        "site_rules.page_dead",
        "only the forwarder was asked for, never an advertisement"
    );
}

#[tokio::test]
async fn an_address_on_a_parked_avaxhome_domain_is_rewritten_to_the_living_one() {
    // Measured on 2026-09-21: avxhm.in forwards to avxhm.se, avxhm.is and avh.world answer
    // with a parking redirect, avaxhome.bz serves a lander and avaxhome.ws was sold and now
    // forwards to a betting site. None of them is the service any more.
    let rule = shipped("avaxhome");
    assert_eq!(
        rule.dead,
        [
            "avxhm.in",
            "avxhm.is",
            "avaxhome.bz",
            "avaxhome.ws",
            "avh.world",
        ]
    );
    let fetcher = Recorded::default().page(AVAXHOME_PROBE, AVAXHOME_EBOOK);
    for dead in &rule.dead {
        let old = format!("https://{dead}/ebooks/3032034086E.html");
        let error = run(&rule, &fetcher, &old)
            .await
            .expect_err("the forwarder is not recorded");
        assert_ne!(error.code(), "site_rules.not_claimed", "{old}");
        assert_eq!(
            error.code(),
            "site_rules.page_dead",
            "{old} reached the living host and stopped at the forwarder"
        );
    }
}

#[tokio::test]
async fn a_cgpersia_post_becomes_a_package_of_every_part_at_every_hoster() {
    // The page keeps its addresses as running text inside `pre` blocks, one block per
    // hoster; nothing else on the page is in one.
    let fetcher = Recorded::default().page(CGPERSIA_PROBE, CGPERSIA_POST);
    let crawl = run(&shipped("cgpersia"), &fetcher, CGPERSIA_PROBE)
        .await
        .expect("crawled");
    assert_eq!(crawl.links.len(), 15);
    assert_eq!(
        crawl.links.iter().filter(|l| l.contains("rg.to")).count(),
        5
    );
    assert_eq!(
        crawl
            .links
            .iter()
            .filter(|l| l.contains("alfafile.net"))
            .count(),
        5
    );
    assert_eq!(
        crawl
            .links
            .iter()
            .filter(|l| l.contains("nitroflare.com"))
            .count(),
        5
    );
    assert!(
        !crawl.links.iter().any(|link| link.contains("cgpersia.com")),
        "{:?}",
        crawl.links
    );
    assert_eq!(
        crawl.package_name.as_deref(),
        Some("Udemy &#8211; 3ds max and 3D Space")
    );
}

#[tokio::test]
async fn the_cgpersia_rule_carries_no_mirror_group() {
    // Five parts, each at three hosters. The format states one group per page, so a page
    // that holds several different files leaves the field out rather than claiming a group
    // that would fold five files into one.
    assert!(!shipped("cgpersia").mirrors);
    assert!(shipped("getcomics").mirrors, "one comic, five hosters");
}

#[tokio::test]
async fn a_vipergirls_thread_becomes_a_package_of_the_hoster_links_in_its_posts() {
    // Group `adult`: the board is one, and the rule says so rather than hiding it.
    let rule = shipped("vipergirls");
    assert_eq!(rule.group, "adult");
    let fetcher = Recorded::default().page(VIPERGIRLS_PROBE, VIPERGIRLS_THREAD);
    let crawl = run(&rule, &fetcher, VIPERGIRLS_PROBE)
        .await
        .expect("crawled");
    assert_eq!(crawl.links.len(), 9, "three posts, three hosters each");
    for host in ["rapidgator.net", "katfile.com", "filefox.cc"] {
        assert!(
            crawl.links.iter().any(|link| link.contains(host)),
            "{host} in {:?}",
            crawl.links
        );
    }
    // The board's own name does appear inside a rapidgator address, as that hoster's
    // referrer tag; what must not appear is a link whose *host* is the board.
    assert!(
        !crawl
            .links
            .iter()
            .any(|link| link.starts_with("https://vipergirls.to")),
        "{:?}",
        crawl.links
    );
    assert_eq!(
        crawl.package_name.as_deref(),
        Some("Adult Magazines Mix Collection")
    );
}

#[tokio::test]
async fn the_vipergirls_rule_claims_a_thread_with_its_session_identifier() {
    // Every address the board hands out carries `?s=<session>`; a rule that did not claim
    // it would leave the commonest form of its own address unclaimed.
    let rule = shipped("vipergirls");
    for claimed in [
        "https://vipergirls.to/threads/6629687-Adult-Magazines-Mix-Collection",
        "https://vipergirls.to/threads/6629687-Adult-Magazines-Mix-Collection?s=9cd69c2e",
        "https://vipergirls.to/threads/6629687-Adult-Magazines-Mix-Collection/page2",
        "https://viper.to/threads/6629687-Adult-Magazines-Mix-Collection",
    ] {
        assert!(
            rule.claims(&Url::parse(claimed).expect("address")),
            "{claimed}"
        );
    }
    for unclaimed in [
        "https://vipergirls.to/forums/227-Magazine-Publications",
        "https://vipergirls.to/member.php?u=401078",
        "https://vipergirls.to/",
    ] {
        assert!(
            !rule.claims(&Url::parse(unclaimed).expect("address")),
            "{unclaimed}"
        );
    }
}

#[tokio::test]
async fn every_rule_of_this_wave_refuses_a_page_whose_structure_changed() {
    // One shape for all of them: the service answers, the block the rule reads is gone.
    let empty = "<html><head><title>nothing</title></head><body><p>gone</p></body></html>";
    for (id, probe) in [
        ("getcomics", GETCOMICS_PROBE),
        ("scene-rls", SCENERLS_PROBE),
        ("avaxhome", AVAXHOME_PROBE),
        ("cgpersia", CGPERSIA_PROBE),
        ("vipergirls", VIPERGIRLS_PROBE),
    ] {
        let fetcher = Recorded::default().page(probe, empty);
        let error = run(&shipped(id), &fetcher, probe)
            .await
            .expect_err("an empty page refuses");
        assert_eq!(error.code(), "site_rules.structure", "{id}");
        assert!(!error.not_mine(), "{id}");
    }
}
