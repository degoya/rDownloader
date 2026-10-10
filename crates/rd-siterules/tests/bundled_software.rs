//! The bundled free-software rules of RD-1240-01 (LibreOffice, VLC), against recorded answers.
//!
//! Every page here was fetched from the live site on 2026-10-10 with `curl` and a current Chrome
//! user agent, no account. `libreoffice-26.8.1-win-x86_64.html` keeps the folder's header, the
//! installer, three of its help packs and the SDK (the cuts are marked); the VLC listing is
//! whole, its line ends made Unix ones. Nothing in this file touches the network, so a site that
//! goes down breaks the self-test (RD-110-09) rather than the build.

mod recorded;

use rd_siterules::{Executor, SystemClock};
use recorded::{PublicDns, Recorded, example, run};

const LIBREOFFICE: &str = include_str!("fixtures/examples/libreoffice-26.8.1-win-x86_64.html");
const VLC: &str = include_str!("fixtures/examples/vlc-last-win64.html");

const LIBREOFFICE_PAGE: &str =
    "https://download.documentfoundation.org/libreoffice/stable/26.8.1/win/x86_64/";
const VLC_PAGE: &str = "https://download.videolan.org/pub/videolan/vlc/last/win64/";

#[tokio::test]
async fn a_libreoffice_folder_lists_the_installer_and_its_packs_to_choose_from() {
    let rule = example("libreoffice-stable");
    let network = Recorded::default().page(LIBREOFFICE_PAGE, LIBREOFFICE);
    let crawl = run(&rule, &network, LIBREOFFICE_PAGE)
        .await
        .expect("listed");
    assert!(crawl.links.is_empty(), "the first stage resolves nothing");
    assert_eq!(crawl.package_name.as_deref(), Some("LibreOffice_26.8.1"));
    let list = crawl.pick.clone().expect("a list to choose from");
    let labels: Vec<&str> = list
        .entries
        .iter()
        .filter_map(|entry| entry.label.as_deref())
        .collect();
    assert_eq!(
        labels,
        [
            "LibreOffice_26.8.1_Win_x86-64",
            "LibreOffice_26.8.1_Win_x86-64_helppack_de",
            "LibreOffice_26.8.1_Win_x86-64_helppack_en-US",
            "LibreOffice_26.8.1_Win_x86-64_helppack_fr",
            "LibreOffice_26.8.1_Win_x86-64_sdk",
        ],
        "no signature, no mirror list"
    );
    let installer = &list.entries[0].attributes;
    assert_eq!(installer.get("version").map(String::as_str), Some("26.8.1"));
    assert_eq!(installer.get("format").map(String::as_str), Some("msi"));
    assert_eq!(installer.get("part"), None, "the installer is no pack");
    let german = &list.entries[1].attributes;
    assert_eq!(german.get("part").map(String::as_str), Some("helppack"));
    assert_eq!(german.get("language").map(String::as_str), Some("de"));
    assert_eq!(
        list.entries[4].attributes.get("part").map(String::as_str),
        Some("sdk")
    );

    let clock = SystemClock::new();
    let group = Executor::new(&network, &PublicDns, &clock)
        .resolve(&rule, &crawl.address, &list, 0)
        .await
        .expect("resolved");
    assert_eq!(group.name.as_deref(), Some("LibreOffice_26.8.1_Win_x86-64"));
    let links: Vec<&str> = group.links.iter().map(|link| link.url.as_str()).collect();
    assert_eq!(
        links,
        [
            "https://download.documentfoundation.org/libreoffice/stable/26.8.1/win/x86_64/LibreOffice_26.8.1_Win_x86-64.msi"
        ]
    );
}

#[tokio::test]
async fn a_vlc_folder_lists_one_build_per_format() {
    let rule = example("vlc-releases");
    let network = Recorded::default().page(VLC_PAGE, VLC);
    let crawl = run(&rule, &network, VLC_PAGE).await.expect("listed");
    assert!(crawl.links.is_empty(), "the first stage resolves nothing");
    assert_eq!(crawl.package_name.as_deref(), Some("vlc-3.0.24"));
    let list = crawl.pick.clone().expect("a list to choose from");
    let formats: Vec<&str> = list
        .entries
        .iter()
        .filter_map(|entry| entry.attributes.get("format").map(String::as_str))
        .collect();
    assert_eq!(
        formats,
        ["7z", "exe", "msi", "zip"],
        "no debug symbols, no checksum, no signature"
    );
    assert!(list.entries.iter().all(|entry| {
        entry.attributes.get("platform").map(String::as_str) == Some("win64")
            && entry.attributes.get("version").map(String::as_str) == Some("3.0.24")
    }));
    let installer = list
        .entries
        .iter()
        .position(|entry| entry.attributes.get("format").map(String::as_str) == Some("exe"))
        .expect("the installer is listed");

    let clock = SystemClock::new();
    let group = Executor::new(&network, &PublicDns, &clock)
        .resolve(&rule, &crawl.address, &list, installer)
        .await
        .expect("resolved");
    assert_eq!(group.name.as_deref(), Some("vlc-3.0.24-win64"));
    assert_eq!(
        group.links.first().map(|link| link.url.as_str()),
        Some("https://download.videolan.org/pub/videolan/vlc/last/win64/vlc-3.0.24-win64.exe")
    );
}

/// The VLC rule claims the same folders on the mirror redirector, and neither rule claims the
/// level above a platform folder.
#[test]
fn the_software_rules_claim_platform_folders_only() {
    let claims =
        |id: &str, address: &str| example(id).claims(&url::Url::parse(address).expect("url"));
    assert!(claims(
        "vlc-releases",
        "https://get.videolan.org/vlc/3.0.24/macosx/"
    ));
    assert!(!claims(
        "vlc-releases",
        "https://download.videolan.org/pub/videolan/vlc/3.0.24/"
    ));
    assert!(claims(
        "libreoffice-stable",
        "https://download.documentfoundation.org/libreoffice/stable/26.2.6/deb/x86_64/"
    ));
    assert!(!claims(
        "libreoffice-stable",
        "https://download.documentfoundation.org/libreoffice/stable/26.8.1/"
    ));
}
