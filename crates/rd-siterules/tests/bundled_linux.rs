//! The bundled Linux rules of RD-1240-01 (Fedora, Arch Linux, Linux Mint), against recorded
//! answers.
//!
//! Every page here was fetched from the live site on 2026-10-10 with `curl` and a current Chrome
//! user agent, no account; the three folder listings are whole, their line ends made Unix ones.
//! Fedora's own `dl.fedoraproject.org` answered with a bot check that day, so the rule reads the
//! kernel.org mirror. Nothing in this file touches the network, so a site that goes down breaks
//! the self-test (RD-110-09) rather than the build.

mod recorded;

use recorded::{Recorded, example, run};

const FEDORA: &str = include_str!("fixtures/examples/fedora-44-server.html");
const ARCH: &str = include_str!("fixtures/examples/arch-iso-latest.html");
const MINT: &str = include_str!("fixtures/examples/mint-22.3.html");

const FEDORA_PAGE: &str = "https://mirrors.edge.kernel.org/fedora/releases/44/Server/x86_64/iso/";
const ARCH_PAGE: &str = "https://geo.mirror.pkgbuild.com/iso/latest/";
const MINT_PAGE: &str = "https://pub.linuxmint.io/stable/22.3/";

#[tokio::test]
async fn a_fedora_image_folder_becomes_one_package_of_its_images() {
    let rule = example("fedora-releases");
    let crawl = run(
        &rule,
        &Recorded::default().page(FEDORA_PAGE, FEDORA),
        FEDORA_PAGE,
    )
    .await
    .expect("crawled");
    assert_eq!(
        crawl.package_name.as_deref(),
        Some("Fedora-Server-44-1.7-x86_64")
    );
    assert_eq!(
        crawl.links,
        [
            "https://mirrors.edge.kernel.org/fedora/releases/44/Server/x86_64/iso/Fedora-Server-dvd-x86_64-44-1.7.iso",
            "https://mirrors.edge.kernel.org/fedora/releases/44/Server/x86_64/iso/Fedora-Server-netinst-x86_64-44-1.7.iso",
        ],
        "the images only: no manifest, no checksum file"
    );
}

#[tokio::test]
async fn the_arch_folder_yields_the_dated_image_once() {
    let rule = example("archlinux-iso");
    let crawl = run(&rule, &Recorded::default().page(ARCH_PAGE, ARCH), ARCH_PAGE)
        .await
        .expect("crawled");
    assert_eq!(crawl.package_name.as_deref(), Some("archlinux-2026.10.01"));
    assert_eq!(
        crawl.links,
        ["https://geo.mirror.pkgbuild.com/iso/latest/archlinux-2026.10.01-x86_64.iso"],
        "not the same image again as archlinux-x86_64.iso, no signature, no bootstrap archive"
    );
}

#[tokio::test]
async fn a_mint_release_folder_becomes_one_package_of_its_editions() {
    let rule = example("linuxmint-releases");
    let crawl = run(&rule, &Recorded::default().page(MINT_PAGE, MINT), MINT_PAGE)
        .await
        .expect("crawled");
    assert_eq!(crawl.package_name.as_deref(), Some("linuxmint-22.3"));
    assert_eq!(
        crawl.links,
        [
            "https://pub.linuxmint.io/stable/22.3/linuxmint-22.3-cinnamon-64bit.iso",
            "https://pub.linuxmint.io/stable/22.3/linuxmint-22.3-mate-64bit.iso",
            "https://pub.linuxmint.io/stable/22.3/linuxmint-22.3-xfce-64bit.iso",
        ]
    );
}

/// Each rule claims the folders it is built for and leaves the levels above them alone.
#[test]
fn the_linux_rules_claim_their_folders_only() {
    let claims =
        |id: &str, address: &str| example(id).claims(&url::Url::parse(address).expect("url"));
    assert!(claims(
        "fedora-releases",
        "https://mirrors.edge.kernel.org/fedora/releases/44/Workstation/x86_64/iso/"
    ));
    assert!(!claims(
        "fedora-releases",
        "https://mirrors.edge.kernel.org/fedora/releases/44/"
    ));
    assert!(claims(
        "archlinux-iso",
        "https://geo.mirror.pkgbuild.com/iso/2026.10.01/"
    ));
    assert!(!claims(
        "archlinux-iso",
        "https://geo.mirror.pkgbuild.com/iso/latest/arch/"
    ));
    assert!(claims(
        "linuxmint-releases",
        "https://pub.linuxmint.io/stable/22/"
    ));
    assert!(!claims(
        "linuxmint-releases",
        "https://pub.linuxmint.io/stable/"
    ));
}
