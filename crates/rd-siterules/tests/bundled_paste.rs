//! The bundled paste rules of RD-1240-01 (Pastebin, Rentry, sourcehut), against recorded answers.
//!
//! Every answer here was fetched from the live site on 2026-10-10 with `curl` and a current
//! Chrome user agent, no account. The pages keep their titles and what the rules read (the cuts
//! are marked); the raw texts are whole, Pastebin's with Unix line ends. The pastes are public and
//! old: an Ubuntu install script (Pastebin, set to never expire), a page on Manjaro (Rentry) and
//! sourcehut's own note on Docker (paste.sr.ht). Nothing in this file touches the network, so a
//! service that goes down breaks the self-test (RD-110-09) rather than the build.

mod recorded;

use recorded::{Recorded, example, run};

const PASTEBIN: &str = include_str!("fixtures/examples/pastebin-trusty-kickstart.html");
const PASTEBIN_RAW: &str = include_str!("fixtures/examples/pastebin-trusty-kickstart.txt");
const RENTRY: &str = include_str!("fixtures/examples/rentry-manjaro-controversies.html");
const SOURCEHUT: &str = include_str!("fixtures/examples/srht-docker.html");
const SOURCEHUT_RAW: &str = include_str!("fixtures/examples/srht-docker.txt");

const PASTEBIN_PAGE: &str = "https://pastebin.com/eY5ybGfc";
const PASTEBIN_RAW_PAGE: &str = "https://pastebin.com/raw/eY5ybGfc";
const RENTRY_PAGE: &str = "https://rentry.co/manjaro-controversies";
const SOURCEHUT_PAGE: &str =
    "https://paste.sr.ht/~sircmpwn/78cc21e1661d5a9d8038f47e532d286807ac89ad";
const SOURCEHUT_BLOB: &str = "https://paste.sr.ht/blob/a4e72163574e25a0d9722618f8293cb0004454aa";

#[tokio::test]
async fn a_pastebin_paste_yields_the_addresses_in_its_text() {
    let rule = example("pastebin");
    let network = Recorded::default()
        .page(PASTEBIN_PAGE, PASTEBIN)
        .page(PASTEBIN_RAW_PAGE, PASTEBIN_RAW);
    // The raw address is claimed as well, and reads the same two answers.
    for address in [PASTEBIN_PAGE, PASTEBIN_RAW_PAGE] {
        let crawl = run(&rule, &network, address).await.expect("crawled");
        assert_eq!(crawl.package_name.as_deref(), Some("trusty kickstart"));
        assert_eq!(crawl.links, ["http://mirror.yandex.ru/ubuntu/"]);
    }
}

#[tokio::test]
async fn a_rentry_page_yields_the_links_of_its_entry_only() {
    let rule = example("rentry");
    let crawl = run(
        &rule,
        &Recorded::default().page(RENTRY_PAGE, RENTRY),
        RENTRY_PAGE,
    )
    .await
    .expect("crawled");
    assert_eq!(crawl.package_name.as_deref(), Some("Manjaro Controversies"));
    assert_eq!(
        crawl.links,
        [
            "https://en.wikipedia.org/wiki/Manjaro",
            "https://en.wikipedia.org/wiki/Arch_Linux",
            "https://www.archlinux.org/",
            "https://wiki.archlinux.org/index.php/installation_guide",
            "https://en.opensuse.org/Portal:Tumbleweed",
            "https://www.ubuntu.com/download/flavours",
            "https://en.opensuse.org/Portal:Leap",
            "https://manjaro.org/",
            "https://archive.fo/pBN8X",
            "https://reddit.com/comments/adf6cx/_/edgpidc",
            "https://archive.fo/TwuVC",
            "https://forum.manjaro.org/t/stable-update-2019-02-19-kernels-kde-libreoffice-systemd-virtualbox-deepin-qt-firmwares-wine/76420/2",
        ],
        "no heading anchor, no raw export, no warning dialog: the entry's external links"
    );
}

#[tokio::test]
async fn a_sourcehut_paste_yields_the_addresses_of_its_raw_files() {
    let rule = example("sourcehut-paste");
    let network = Recorded::default()
        .page(SOURCEHUT_PAGE, SOURCEHUT)
        .page(SOURCEHUT_BLOB, SOURCEHUT_RAW);
    let crawl = run(&rule, &network, SOURCEHUT_PAGE).await.expect("crawled");
    assert_eq!(crawl.package_name.as_deref(), Some("docker.md"));
    assert_eq!(
        crawl.links,
        [
            "https://man.sr.ht/installation.md",
            "https://imgs.xkcd.com/comics/containers.png",
            "https://xkcd.com/1988/",
        ],
        "the markdown's brackets are not part of a link"
    );
}

/// Each paste rule claims a paste and none of the service's other pages.
#[test]
fn the_paste_rules_claim_pastes_only() {
    let claims =
        |id: &str, address: &str| example(id).claims(&url::Url::parse(address).expect("url"));
    assert!(!claims("pastebin", "https://pastebin.com/archive"));
    assert!(!claims("pastebin", "https://pastebin.com/u/someone"));
    assert!(claims("rentry", "https://rentry.org/manjaro-controversies"));
    assert!(!claims(
        "rentry",
        "https://rentry.co/manjaro-controversies/raw"
    ));
    assert!(!claims("sourcehut-paste", "https://paste.sr.ht/~sircmpwn"));
    assert!(!claims(
        "sourcehut-paste",
        "https://paste.sr.ht/blob/a4e72163574e25a0d9722618f8293cb0004454aa"
    ));
}
