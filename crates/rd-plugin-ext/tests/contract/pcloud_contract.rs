//! pCloud, exercised end to end against a mock pCloud API that has **two installations**, and
//! a mock pCloud authorization server (RD-120-06).
//!
//! **Both providers are mocks.** They answer at the host boundary, so no socket is opened and
//! neither `pcloud.com` nor `pcloud.link` is contacted. A run against a real pCloud account is
//! *not* claimed here; there is none in this checkout, and there is no registered application
//! either. The fixtures beside this file are written to pCloud's **documented** answer shapes
//! rather than recorded from a live account, and every token-shaped value in them is a
//! placeholder — `fixtures_carry_no_credential_material` is what keeps it that way.
//!
//! Two of the three pCloud plugins are driven here as real WebAssembly components, because
//! that is the only place their guest code exists: the crawler's walk and the sign-in's
//! exchange are written against the WIT imports and cannot run natively. The third,
//! `plugins/pcloud/`, keeps its protocol logic outside the component and is covered by
//! `plugins/pcloud/src/native/tests.rs` against the same kind of mock.
//!
//! The case this file exists for above all others is the **region**. The mock holds the
//! account, the files and the link codes in exactly one of pCloud's two installations and
//! answers the other one the way pCloud does — `result: 2094` for a token it will not accept,
//! `result: 7001` for a link code it never issued, both under HTTP 200. A plugin that read
//! either at face value would report a bad credential or a dead link, which is precisely the
//! mistake that is impossible to tell from the real thing in production.
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A folder of files and subfolders | links, with names, sizes and the folder they sat in |
//! | The same folder, addressed at the wrong installation | one correction, then pinned |
//! | A public folder link | one `showpublink`, its whole tree walked under the same limits |
//! | A public link holding one file | that one file, spelled with its `fileid` |
//! | An empty folder | a refusal with a code, never an empty package |
//! | Refused token, rate limit, missing folder | three different codes, no provider prose |
//! | The sign-in | a code through to a stored token, at whichever installation issued it |
//! | The renewal | refused, because pCloud issues nothing to renew with |
//! | The device entrance | refused with a stable code; the manifest offers it to nobody |
//! | The application key and secret | markers in the address and the exchange, never values |
//!
//! The cases every cloud drive shares run through `support::storage` from the pCloud row of its
//! table; the two installations stay here, the crawler's mock in `mock.rs` and the sign-in,
//! which shares no flow with the others, in `sign_in.rs`.

use std::sync::Arc;

use crate::support::storage::{self, table::PCLOUD};

mod mock;
mod sign_in;

use mock::{MockPCloud, SHOWPUBLINK_FILE, crawler};

/// The sentence every fixture's `error` carries. Nothing that reaches a person may contain it.
const PROVIDER_PROSE: &str = "A sentence pCloud wrote";

const US_API: &str = "api.pcloud.com";
const EU_API: &str = "eapi.pcloud.com";

/// The public link every shared case uses, spelled on the European short host.
const PUBLIC_LINK_EU: &str = "https://e.pcloud.link/publink/show?code=XZredacted";
/// The same code on the host pCloud's own `getfilepublink` hands back, which names no region.
const PUBLIC_LINK_UNSPECIFIED: &str = "https://my.pcloud.com/#page=publink&code=XZredacted";

fn crawler_component() -> Vec<u8> {
    storage::crawler::component(&PCLOUD.crawler)
}

// -- the crawler -----------------------------------------------------------------------------

storage::shared_cases! {
    PCLOUD.crawler;
    /// A crawler is asked before it is handed anything, and it answers from the address alone.
    ///
    /// The check that keeps a crawler from costing every other link in the queue a request — and,
    /// here, the one that keeps the two siblings apart. pCloud gives no handle but `fileid`: a
    /// link code is opaque, so an address carrying one is the resolver's and an address without
    /// one is this plugin's, whatever the link turns out to hold.
    only_pcloud_folder_and_link_addresses_are_claimed =>
        storage::crawler::claims_only_its_own_addresses;
    /// A crawler reaches nothing outside the domains its own manifest declares.
    the_crawler_reaches_nothing_its_manifest_did_not_declare =>
        storage::crawler::reaches_nothing_its_manifest_did_not_declare;
}

/// A folder comes back as its files, with the names, the sizes and the structure pCloud gave —
/// and each as the canonical address the sibling resolver claims.
#[tokio::test]
async fn a_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = crawler_component();
    let host = MockPCloud::new(US_API);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl("https://my.pcloud.com/#/filemanager?folder=42", None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("readme.txt"));
    assert_eq!(links[0].size, Some(12));
    // The crawled folder's own name is the package suggestion; a subfolder extends it.
    assert_eq!(links[0].package_hint.as_deref(), Some("Show"));
    assert_eq!(links[1].file_name.as_deref(), Some("e01.mkv"));
    assert_eq!(links[1].package_hint.as_deref(), Some("Show/Season 1"));
    // The one thing that crosses between the two packages: an address, and nothing else — and
    // it carries the installation that answered, so the resolver never pays the correction.
    assert_eq!(
        links[1].url,
        "https://my.pcloud.com/#/filemanager?folder=43&fileid=5002"
    );
    // The row without an identifier was dropped, not guessed at.
    assert!(
        links
            .iter()
            .all(|link| link.file_name.as_deref() != Some("no identifier"))
    );
    assert_eq!(
        host.route(),
        vec![
            (US_API.to_owned(), "listfolder".to_owned()),
            (US_API.to_owned(), "listfolder".to_owned()),
        ],
        "one call per folder and no more: pCloud paginates none of this"
    );
    assert_eq!(host.requests()[0].parameter("folderid"), Some("42"));
    assert_eq!(host.requests()[1].parameter("folderid"), Some("43"));
}

/// **The region.** The address names one installation, the account lives in the other, and
/// pCloud answers the first exactly as it answers a bad token. One correction, and then the
/// installation that answered is pinned for the whole walk.
#[tokio::test]
async fn a_folder_in_the_other_installation_is_found_after_one_correction() {
    let bytes = crawler_component();
    let host = MockPCloud::new(EU_API);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl("https://my.pcloud.com/#/filemanager?folder=42", None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    // Every address handed back names the installation that actually answered, so nothing
    // downstream repeats the correction.
    assert!(
        links
            .iter()
            .all(|link| link.url.starts_with("https://e.pcloud.com/")),
        "{links:?}"
    );
    assert_eq!(
        host.route(),
        vec![
            // The address said the United States, so that is where it started.
            (US_API.to_owned(), "listfolder".to_owned()),
            // 2094 there, corrected once.
            (EU_API.to_owned(), "listfolder".to_owned()),
            // And pinned: the subfolder pays nothing.
            (EU_API.to_owned(), "listfolder".to_owned()),
        ],
        "the correction must happen once and then stop happening"
    );
}

/// A public folder link is read from the **one** tree pCloud answers with, under the same
/// limits an own-drive walk runs under — a tree a stranger shared is no more trustworthy for
/// having arrived in one piece.
#[tokio::test]
async fn a_public_folder_link_is_read_from_the_one_tree_pcloud_answers_with() {
    let bytes = crawler_component();
    let host = MockPCloud::new(EU_API);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(PUBLIC_LINK_EU, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("a.bin"));
    assert_eq!(links[0].package_hint.as_deref(), Some("Shared"));
    assert_eq!(links[1].package_hint.as_deref(), Some("Shared/Sub"));
    assert_eq!(
        links[1].url,
        "https://e.pcloud.link/publink/show?code=XZredacted&fileid=9003"
    );
    assert_eq!(
        host.route(),
        vec![(EU_API.to_owned(), "showpublink".to_owned())],
        "pCloud answers a public folder link with its whole tree, so one call is all of it"
    );
    // A public link needs no credential, and none is sent.
    assert!(
        host.authorizations().is_empty(),
        "a public link needs no credential, so none may be sent"
    );
}

/// The address pCloud's own `getfilepublink` hands back names no installation, so the link
/// code is looked for in one and then the other — and `7001` at the first is what says so.
#[tokio::test]
async fn a_public_link_that_names_no_installation_is_found_in_the_other_one() {
    let bytes = crawler_component();
    let host = MockPCloud::new(EU_API);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(PUBLIC_LINK_UNSPECIFIED, None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(
        host.route(),
        vec![
            (US_API.to_owned(), "showpublink".to_owned()),
            (EU_API.to_owned(), "showpublink".to_owned()),
        ]
    );
    // And the addresses it hands back name the installation that answered.
    assert!(
        links
            .iter()
            .all(|link| link.url.starts_with("https://e.pcloud.link/")),
        "{links:?}"
    );
}

/// A public link holding a single file is still this plugin's — no address could have said
/// otherwise — and comes back as that one file, spelled with its `fileid`.
#[tokio::test]
async fn a_public_link_to_one_file_comes_back_as_that_one_file() {
    let bytes = crawler_component();
    let host = MockPCloud::with_publink(EU_API, SHOWPUBLINK_FILE);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(PUBLIC_LINK_EU, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("single.bin"));
    assert_eq!(links[0].size, Some(33));
    assert_eq!(
        links[0].url,
        "https://e.pcloud.link/publink/show?code=XZredacted&fileid=9100"
    );
    // One file is not a package.
    assert_eq!(links[0].package_hint, None);
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_template_and_never_as_a_value() {
    let bytes = crawler_component();
    let host = MockPCloud::new(US_API);
    crawler(Arc::clone(&host), &bytes)
        .crawl("https://my.pcloud.com/#/filemanager?folder=42", None)
        .await
        .expect("call")
        .expect("a listing");
    storage::crawler::assert_bearer_markers(&host.authorizations(), "pcloud_access_token");
}

/// An empty folder says so, with a code the interface can translate.
///
/// Returning an empty list would create a package with nothing in it and nothing to explain
/// why — the defect ADR 0001 was written for.
#[tokio::test]
async fn an_empty_folder_is_reported_and_not_silently_dropped() {
    let bytes = crawler_component();
    let refusal = crawler(MockPCloud::empty_folder(), &bytes)
        .crawl("https://my.pcloud.com/#/filemanager?folder=44", None)
        .await
        .expect("call")
        .expect_err("an empty folder is a refusal");
    assert_eq!(refusal.code.as_deref(), Some("pcloud_crawler.folder_empty"));
}

/// The ways a crawl can fail reach the person as different codes, none of them repeating a
/// word pCloud wrote — the number travels instead, because a number cannot carry a token.
#[tokio::test]
async fn a_refused_token_a_rate_limit_and_a_missing_folder_are_told_apart() {
    let asked: Vec<usize> = storage::crawler::refusals(&PCLOUD.crawler)
        .await
        .into_iter()
        .map(|(_, requests)| requests)
        .collect();
    // A rate limit and a missing folder are not region questions and are asked once; a
    // refused token and a refused link code are, and are asked exactly twice.
    assert_eq!(asked, vec![2, 1, 1, 2]);
}

// -- the shape of the three packages ---------------------------------------------------------

/// The three siblings reach only pCloud, and only the part of it each one needs.
///
/// The shape RD-106-04 decided, asserted against the real manifests: one resolver carrying the
/// provider row, one crawler and one sign-in claiming that row's slug, and no capability beyond
/// HTTP to the provider's own hosts.
#[test]
fn the_three_siblings_reach_only_the_part_of_pcloud_each_one_needs() {
    let (resolver, crawler, _) = storage::shape(&PCLOUD.shape);
    let provider = resolver.provider.as_ref().expect("the provider row");
    // The account's username field holds this installation's own application key, and it is
    // required: pCloud's token endpoint is a confidential client and there is no PKCE to fall
    // back on, so the secret beside it is a slot of its own.
    assert!(provider.username_required);
    let slots = provider.secret_slots();
    assert_eq!(
        slots
            .iter()
            .map(|slot| slot.reference.as_str())
            .collect::<Vec<_>>(),
        vec!["pcloud_client_secret", "pcloud_access_token"]
    );
    for slot in &slots {
        assert_eq!(
            slot.domains,
            vec![US_API.to_owned(), EU_API.to_owned()],
            "{}",
            slot.reference
        );
    }
    for slot in &slots {
        assert!(
            !slot.domains.iter().any(|host| host.contains('*')),
            "a credential may only ever go to an exact host"
        );
    }
    // Both installations are matched, because a pCloud address names one in its host.
    for host in [
        "my.pcloud.com",
        "e.pcloud.com",
        "u.pcloud.link",
        "e.pcloud.link",
    ] {
        assert!(
            resolver.match_domains.iter().any(|known| known == host),
            "{host}"
        );
    }
    // The crawler may spend the token but never the application secret.
    assert_eq!(
        crawler.capabilities.secrets,
        vec!["pcloud_access_token".to_owned()]
    );
}
