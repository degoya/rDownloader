//! The example rules the app brings (RD-1230-03), against recorded answers.
//!
//! Every page here was fetched from the live site on 2026-10-09 with `curl` and a current Chrome
//! user agent, no account, and then cut down: `blender-release-4.2.html` keeps the folder's
//! header and the builds of 4.2.9 (the cut is marked), `ubuntu-24.04.html` keeps the title and the
//! image links (the cuts are marked), the Debian and Tears of Steel listings are whole. Nothing
//! in this file touches the network, so a site that goes down breaks the self-test (RD-110-09)
//! rather than the build.

mod recorded;

use rd_siterules::{Executor, SystemClock};
use recorded::{PublicDns, Recorded, example, run};

const DEBIAN: &str = include_str!("fixtures/examples/debian-iso-cd.html");
const UBUNTU: &str = include_str!("fixtures/examples/ubuntu-24.04.html");
const BLENDER: &str = include_str!("fixtures/examples/blender-release-4.2.html");
const TEARS_OF_STEEL: &str = include_str!("fixtures/examples/blender-movies-tos.html");

const DEBIAN_PAGE: &str = "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/";
const UBUNTU_PAGE: &str = "https://releases.ubuntu.com/24.04/";
const BLENDER_PAGE: &str = "https://download.blender.org/release/Blender4.2/";
const MOVIES_PAGE: &str = "https://download.blender.org/demo/movies/ToS/";

#[tokio::test]
async fn the_debian_folder_becomes_one_package_of_its_images() {
    let rule = example("debian-cd");
    let crawl = run(
        &rule,
        &Recorded::default().page(DEBIAN_PAGE, DEBIAN),
        DEBIAN_PAGE,
    )
    .await
    .expect("crawled");
    assert_eq!(crawl.package_name.as_deref(), Some("debian-13.7.0-amd64"));
    assert_eq!(
        crawl.links,
        [
            "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/debian-13.7.0-amd64-netinst.iso",
            "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/debian-edu-13.7.0-amd64-netinst.iso",
            "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/debian-mac-13.7.0-amd64-netinst.iso",
        ],
        "every image once, the checksum files left out"
    );
}

#[tokio::test]
async fn an_ubuntu_release_folder_becomes_one_package_named_after_the_release() {
    let rule = example("ubuntu-releases");
    let crawl = run(
        &rule,
        &Recorded::default().page(UBUNTU_PAGE, UBUNTU),
        UBUNTU_PAGE,
    )
    .await
    .expect("crawled");
    assert_eq!(crawl.package_name.as_deref(), Some("Ubuntu 24.04.5.1"));
    assert_eq!(crawl.links.len(), 5, "{:?}", crawl.links);
    assert!(
        crawl
            .links
            .iter()
            .all(|link| link.starts_with(UBUNTU_PAGE) && link.ends_with(".iso"))
    );
}

#[tokio::test]
async fn a_blender_release_folder_lists_its_builds_and_resolves_the_chosen_one() {
    let rule = example("blender-releases");
    let network = Recorded::default().page(BLENDER_PAGE, BLENDER);
    let crawl = run(&rule, &network, BLENDER_PAGE).await.expect("listed");
    assert!(crawl.links.is_empty(), "the first stage resolves nothing");
    assert_eq!(crawl.package_name.as_deref(), Some("Blender4.2"));
    let list = crawl.pick.clone().expect("a list to choose from");
    let labels: Vec<&str> = list
        .entries
        .iter()
        .filter_map(|entry| entry.label.as_deref())
        .collect();
    assert_eq!(
        labels,
        [
            "blender-4.2.9-linux-x64",
            "blender-4.2.9-macos-arm64",
            "blender-4.2.9-macos-x64",
            "blender-4.2.9-windows-x64",
            "blender-4.2.9-windows-x64",
            "blender-4.2.9-windows-x64",
        ],
        "the builds only: no add-on bundle, no checksum file"
    );
    let first = &list.entries[0].attributes;
    assert_eq!(first.get("version").map(String::as_str), Some("4.2.9"));
    assert_eq!(first.get("platform").map(String::as_str), Some("linux"));
    assert_eq!(first.get("format").map(String::as_str), Some("tar.xz"));

    let clock = SystemClock::new();
    let group = Executor::new(&network, &PublicDns, &clock)
        .resolve(&rule, &crawl.address, &list, 0)
        .await
        .expect("resolved");
    assert_eq!(group.name.as_deref(), Some("blender-4.2.9-linux-x64"));
    let links: Vec<&str> = group.links.iter().map(|link| link.url.as_str()).collect();
    assert_eq!(
        links,
        ["https://download.blender.org/release/Blender4.2/blender-4.2.9-linux-x64.tar.xz"]
    );
}

#[tokio::test]
async fn an_open_movie_folder_lists_its_versions_by_resolution_and_format() {
    let rule = example("blender-open-movies");
    let network = Recorded::default().page(MOVIES_PAGE, TEARS_OF_STEEL);
    let crawl = run(&rule, &network, MOVIES_PAGE).await.expect("listed");
    assert_eq!(crawl.package_name.as_deref(), Some("ToS"));
    let list = crawl.pick.clone().expect("a list to choose from");
    assert_eq!(
        list.entries.len(),
        6,
        "the video files, not the sound tracks"
    );
    let chosen = list
        .entries
        .iter()
        .position(|entry| {
            entry.attributes.get("resolution").map(String::as_str) == Some("1080p")
                && entry.attributes.get("format").map(String::as_str) == Some("webm")
        })
        .expect("the 1080p WebM is listed");

    let clock = SystemClock::new();
    let group = Executor::new(&network, &PublicDns, &clock)
        .resolve(&rule, &crawl.address, &list, chosen)
        .await
        .expect("resolved");
    assert_eq!(group.name.as_deref(), Some("tears_of_steel_1080p.webm"));
    assert_eq!(
        group.links.first().map(|link| link.url.as_str()),
        Some("https://download.blender.org/demo/movies/ToS/tears_of_steel_1080p.webm.zip")
    );
}

/// Every example's probe is an address it claims, and a page it does not claim is left to the
/// selection's next candidate.
#[test]
fn every_example_claims_its_probe_and_not_its_neighbours() {
    for rule in rd_siterules::examples() {
        let probe = url::Url::parse(&rule.probe).expect("probe");
        assert!(
            rule.claims(&probe),
            "{} does not claim {}",
            rule.id,
            rule.probe
        );
    }
    let blender = example("blender-releases");
    let other = url::Url::parse("https://download.blender.org/demo/movies/ToS/").expect("url");
    assert!(!blender.claims(&other));
}
