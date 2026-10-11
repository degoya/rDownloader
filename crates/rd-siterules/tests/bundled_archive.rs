//! The bundled Internet Archive rule of RD-1240-01, against recorded answers.
//!
//! Every answer here was fetched from the live site on 2026-10-10 with `curl` and a current
//! Chrome user agent, no account, for the Blender Foundation's "Cosmos Laundromat: First Cycle"
//! (CC BY): the item page and the download folder keep only their titles, the file list keeps
//! every entry but the thumbnails 000045 to 000705 (the cuts are marked). The file list's address
//! answered with a redirect to a storage node, recorded as such. Nothing in this file touches the
//! network, so a site that goes down breaks the self-test (RD-110-09) rather than the build.

mod recorded;

use recorded::{Recorded, example, run};

const DETAILS: &str = include_str!("fixtures/examples/archive-cosmos-details.html");
const FILES: &str = include_str!("fixtures/examples/archive-cosmos-files.xml");
const DOWNLOAD: &str = include_str!("fixtures/examples/archive-cosmos-download.html");

const ITEM_PAGE: &str = "https://archive.org/details/CosmosLaundromatFirstCycle";
const FILES_ADDRESS: &str =
    "https://archive.org/download/CosmosLaundromatFirstCycle/CosmosLaundromatFirstCycle_files.xml";
const FILES_NODE: &str = "https://dn720707.ca.archive.org/0/items/CosmosLaundromatFirstCycle/CosmosLaundromatFirstCycle_files.xml";
const DOWNLOAD_FOLDER: &str = "https://archive.org/download/CosmosLaundromatFirstCycle/";

fn network() -> Recorded {
    Recorded::default()
        .page(ITEM_PAGE, DETAILS)
        .redirect(FILES_ADDRESS, FILES_NODE)
        .page(FILES_NODE, FILES)
        .page(DOWNLOAD_FOLDER, DOWNLOAD)
}

#[tokio::test]
async fn an_item_becomes_one_package_of_its_original_files() {
    let rule = example("archive-org-items");
    let crawl = run(&rule, &network(), ITEM_PAGE).await.expect("crawled");
    assert_eq!(
        crawl.package_name.as_deref(),
        Some("Cosmos Laundromat: First Cycle"),
        "the item's title, without the creator and the site's suffix"
    );
    assert_eq!(
        crawl.pages_fetched, 4,
        "item page, file list and its redirect, folder"
    );
    assert_eq!(
        crawl.links,
        [
            "https://archive.org/download/CosmosLaundromatFirstCycle/Cosmos%20Laundromat%20-%20First%20Cycle%20(1080p).mp4",
            "https://archive.org/download/CosmosLaundromatFirstCycle/CosmosLaundromat-FirstCycle1080p.en.srt",
            "https://archive.org/download/CosmosLaundromatFirstCycle/CosmosLaundromat-FirstCycle1080p.es.srt",
            "https://archive.org/download/CosmosLaundromatFirstCycle/CosmosLaundromat-FirstCycle1080p.fr.srt",
            "https://archive.org/download/CosmosLaundromatFirstCycle/CosmosLaundromat-FirstCycle1080p.it.srt",
            "https://archive.org/download/CosmosLaundromatFirstCycle/CosmosLaundromatFirstCycle_files.xml",
            "https://archive.org/download/CosmosLaundromatFirstCycle/CosmosLaundromatFirstCycle_meta.sqlite",
            "https://archive.org/download/CosmosLaundromatFirstCycle/CosmosLaundromatFirstCycle_meta.xml",
        ],
        "the uploaded files below archive.org/download, not the storage node: no derivative, no \
         thumbnail, no __ia_thumb.jpg"
    );
}

/// The rule claims an item page with or without a trailing slash, on www. too, and nothing
/// else of the site.
#[test]
fn the_archive_rule_claims_item_pages_only() {
    let rule = example("archive-org-items");
    let claims = |address: &str| rule.claims(&url::Url::parse(address).expect("url"));
    assert!(claims(
        "https://archive.org/details/CosmosLaundromatFirstCycle/"
    ));
    assert!(claims("https://www.archive.org/details/ElephantsDream"));
    assert!(!claims(
        "https://archive.org/details/ElephantsDream/ed_hd.avi"
    ));
    assert!(!claims("https://archive.org/download/ElephantsDream/"));
    assert!(!claims("https://archive.org/search?query=blender"));
}
