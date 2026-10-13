//! Dropbox, exercised end to end against a mock Dropbox API and a mock Dropbox authorization
//! server (RD-106-06).
//!
//! **Both providers are mocks.** They answer at the host boundary, so no socket is opened and
//! neither dropboxapi.com nor dropbox.com is contacted. A run against a real Dropbox account
//! is *not* claimed here; there is none in this checkout, and there is no registered app
//! either. What is proven is everything that does not need one.
//!
//! Two of the three Dropbox plugins are driven here as real WebAssembly components, because
//! that is the only place their guest code exists: the crawler's cursor walk and the sign-in's
//! exchange are written against the WIT imports and cannot run natively. The third,
//! `plugins/dropbox/`, keeps its protocol logic outside the component and is covered by
//! `plugins/dropbox/src/native/tests.rs` against the same kind of mock.
//!
//! The cases are the ones the job asks for, and the ones a host reacts to differently:
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A folder of files and subfolders | links, with names, sizes and the folder they sat in |
//! | A folder Dropbox answers a page at a time | every cursor followed, out of the walk's state |
//! | A shared folder link with a password | listed through `shared_link`, files carry it on |
//! | The same link without its password | one code, `link_access_denied` |
//! | An empty folder | a refusal with a code, never an empty package |
//! | Refused token, rate limit, missing folder | three different codes |
//! | The sign-in | PKCE through to a stored token and its renewal material |
//! | The device entrance | refused with a stable code; the manifest offers it to nobody |
//! | The app key | a marker in the address and in both exchanges, never a value |
//!
//! What every cloud drive shares -- claims, refusals, the sandbox, the sign-in and the shape of
//! the three packages -- runs through `support::storage` from the Dropbox rows of its table;
//! what is Dropbox's own, the cursor walk and the shared link's password, stays here with the
//! mock in `mock.rs`.

use std::sync::Arc;

use crate::support::storage::{
    self,
    table::{DROPBOX, sign_in::DROPBOX as DROPBOX_SIGN_IN},
};

mod mock;

use mock::{Folder, MockDropbox, crawler, file, page, paper, subfolder};

const FOLDER_PAGE_1: &str = include_str!("../fixtures/dropbox/folder_page_1.json");
const FOLDER_PAGE_2: &str = include_str!("../fixtures/dropbox/folder_page_2.json");
const FOLDER_PAGE_3: &str = include_str!("../fixtures/dropbox/folder_page_3.json");

/// The shared folder link every shared-link case uses, as the API takes it.
const SHARED_LINK: &str =
    "https://www.dropbox.com/scl/fo/redacted-link-id/redacted-hash?rlkey=redacted-rlkey";

fn crawler_component() -> Vec<u8> {
    storage::crawler::component(&DROPBOX.crawler)
}

// -- the crawler ----------------------------------------------------------------------------

storage::shared_cases! {
    DROPBOX.crawler;
    /// A crawler is asked before it is handed anything, and it answers from the address alone.
    ///
    /// The check that keeps a crawler from costing every other link in the queue a request — and,
    /// here, the one that keeps the two siblings apart: a file address is the resolver's.
    only_dropbox_folder_addresses_are_claimed =>
        storage::crawler::claims_only_its_own_addresses;
    /// The three ways a crawl can fail reach the person as three different codes, and none of them
    /// repeats a word Dropbox wrote.
    a_refused_token_a_rate_limit_and_a_missing_folder_are_told_apart =>
        storage::crawler::refusals_are_told_apart;
    /// A crawler reaches nothing outside the domains its own manifest declares.
    the_crawler_reaches_nothing_its_manifest_did_not_declare =>
        storage::crawler::reaches_nothing_its_manifest_did_not_declare;
}

/// A folder comes back as its files, with the names, the sizes and the structure Dropbox gave —
/// and each as the canonical address the sibling resolver claims.
#[tokio::test]
async fn a_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = crawler_component();
    let host = MockDropbox::new(vec![
        Folder {
            path: "/Show",
            name: "Show",
            pages: vec![page(
                "show",
                0,
                &[
                    file("readme.txt", 12),
                    subfolder("Season 1"),
                    paper("Notes"),
                ],
                false,
            )],
        },
        Folder {
            path: "/Show/Season 1",
            name: "Season 1",
            pages: vec![page(
                "season",
                0,
                &[file("e01.mkv", 1024), file("e02.mkv", 2048)],
                false,
            )],
        },
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl("https://www.dropbox.com/home/Show", None)
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
    // The one thing that crosses between the two packages: an address, and nothing else.
    assert_eq!(
        links[1].url,
        "https://www.dropbox.com/home/Show/Season%201?preview=e01.mkv"
    );
    assert_eq!(
        host.requests(),
        vec![
            "metadata /Show".to_owned(),
            "list /Show".to_owned(),
            "list /Show/Season 1".to_owned(),
        ]
    );
}

/// Dropbox answers a wide folder a page at a time. Every cursor is followed, out of the walk's
/// own state, and the walk is not over until Dropbox says `has_more` is false.
///
/// The pages are the sanitised fixtures, cursors included: a walk that forgot the cursor would
/// be answered page one again, and one that invented a cursor would be answered `reset`.
#[tokio::test]
async fn a_folder_dropbox_answers_a_page_at_a_time_is_followed_by_its_cursor() {
    let bytes = crawler_component();
    let host = MockDropbox::new(vec![Folder {
        path: "/Long",
        name: "Long",
        pages: vec![
            FOLDER_PAGE_1.to_owned(),
            FOLDER_PAGE_2.to_owned(),
            FOLDER_PAGE_3.to_owned(),
        ],
    }]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl("https://www.dropbox.com/home/Long", None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(links[2].file_name.as_deref(), Some("c.mkv"));
    assert_eq!(
        host.requests(),
        vec![
            "metadata /Long".to_owned(),
            "list /Long".to_owned(),
            "continue AAE_redacted-cursor-0001".to_owned(),
            "continue AAE_redacted-cursor-0002".to_owned(),
        ],
        "each cursor has to be carried into the next request, or the walk repeats page one"
    );
    // And the deleted entry on page one was dropped, not handed back.
    assert!(
        links
            .iter()
            .all(|link| link.file_name.as_deref() != Some("old.mkv"))
    );
}

/// A password-protected shared folder link is listed through the official `shared_link`
/// argument with its password, and every file it yields carries the password on — so a
/// crawled file resolves the same way its folder was listed.
#[tokio::test]
async fn a_shared_folder_link_is_listed_with_its_password_and_its_files_carry_it() {
    let bytes = crawler_component();
    let host = MockDropbox::password_protected(
        vec![
            Folder {
                path: "",
                name: "Shared",
                pages: vec![page(
                    "root",
                    0,
                    &[file("a.bin", 1), subfolder("Sub")],
                    false,
                )],
            },
            Folder {
                path: "/Sub",
                name: "Sub",
                pages: vec![page("sub", 0, &[file("b.bin", 2)], false)],
            },
        ],
        "hunter2",
    );
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("{SHARED_LINK}&dl=0&link_password=hunter2"), None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(links[0].package_hint.as_deref(), Some("Shared"));
    assert_eq!(links[1].package_hint.as_deref(), Some("Shared/Sub"));
    assert_eq!(
        links[1].url,
        "https://www.dropbox.com/scl/fo/redacted-link-id/redacted-hash/Sub?rlkey=redacted-rlkey&preview=b.bin&link_password=hunter2"
    );
    // The listing went through `shared_link`, with the path relative to the link's root.
    let bodies = host.bodies();
    assert_eq!(bodies[0]["url"], SHARED_LINK);
    assert_eq!(bodies[0]["link_password"], "hunter2");
    assert_eq!(bodies[1]["path"], "");
    assert_eq!(bodies[1]["shared_link"]["url"], SHARED_LINK);
    assert_eq!(bodies[1]["shared_link"]["password"], "hunter2");
    assert_eq!(bodies[2]["path"], "/Sub");
}

/// The same link without its password is one refusal with its own code — the one thing a
/// person can fix from the address — and nothing is guessed.
#[tokio::test]
async fn a_password_protected_link_without_its_password_is_refused_by_name() {
    let bytes = crawler_component();
    let host = MockDropbox::password_protected(
        vec![Folder {
            path: "",
            name: "Shared",
            pages: vec![page("root", 0, &[file("a.bin", 1)], false)],
        }],
        "hunter2",
    );
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("{SHARED_LINK}&dl=0"), None)
        .await
        .expect("call")
        .expect_err("a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("dropbox_crawler.link_access_denied")
    );
    assert_eq!(host.requests().len(), 1, "refused once, not retried");
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_template_and_never_as_a_value() {
    let bytes = crawler_component();
    let host = MockDropbox::new(vec![Folder {
        path: "/Show",
        name: "Show",
        pages: vec![page("show", 0, &[file("a.mkv", 1)], false)],
    }]);
    crawler(Arc::clone(&host), &bytes)
        .crawl("https://www.dropbox.com/home/Show", None)
        .await
        .expect("call")
        .expect("a listing");
    storage::crawler::assert_bearer_markers(&host.authorizations(), "dropbox_access_token");
}

/// An empty folder says so, with a code the interface can translate.
///
/// Returning an empty list would create a package with nothing in it and nothing to explain
/// why — the defect ADR 0001 was written for. A folder holding only a Paper document is empty
/// in the same sense: nothing in it can be downloaded.
#[tokio::test]
async fn an_empty_folder_is_reported_and_not_silently_dropped() {
    let bytes = crawler_component();
    let host = MockDropbox::new(vec![Folder {
        path: "/Empty",
        name: "Empty",
        pages: vec![page("empty", 0, &[paper("Notes")], false)],
    }]);
    let refusal = crawler(host, &bytes)
        .crawl("https://www.dropbox.com/home/Empty", None)
        .await
        .expect("call")
        .expect_err("an empty folder is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("dropbox_crawler.folder_empty")
    );
}

// -- the sign-in ----------------------------------------------------------------------------

storage::shared_cases! {
    DROPBOX_SIGN_IN;
    /// Authorization code with PKCE, from the address the person is sent to all the way to the
    /// stored token — and with the one parameter without which Dropbox issues no refresh material.
    the_sign_in_runs_through_to_a_stored_token_with_its_renewal_material =>
        storage::sign_in::the_browser_sign_in_runs_through;
    /// A stolen code without its verifier buys nothing, which is the whole reason PKCE exists.
    a_code_without_its_verifier_is_refused =>
        storage::sign_in::a_code_without_its_verifier_is_refused;
    /// Two flows started in the same moment, for the same account, share nothing.
    two_flows_in_the_same_moment_share_no_secret => storage::sign_in::two_flows_share_nothing;
    /// A refusal ends the flow and an unreachable Dropbox does not — the distinction the renewal
    /// sweep acts on, and the one that would otherwise sign people out whenever a connection drops.
    a_refused_grant_ends_the_flow_and_an_unreachable_dropbox_does_not =>
        storage::sign_in::a_refusal_ends_the_flow_and_an_outage_does_not;
    /// A renewal stores the new token without anybody being asked, and the stored refresh material
    /// leaves the plugin as a template rather than as a value.
    ///
    /// This is what a Dropbox account lives on: its access tokens last four hours, so without it
    /// every download started later than that after a sign-in would ask for a new one.
    a_renewal_stores_a_new_token_without_a_redirect =>
        storage::sign_in::a_renewal_stores_a_new_token_without_a_redirect;
    /// A rate limit is a wait, not a failure, and carries Dropbox's own `Retry-After`.
    a_rate_limit_becomes_a_wait_carrying_retry_after =>
        storage::sign_in::a_rate_limit_becomes_a_wait;
    /// The device entrance is not offered, and being asked anyway is a stable refusal rather than
    /// a trap. Dropbox has no device flow at all.
    the_device_entrance_is_refused_with_a_stable_code =>
        storage::sign_in::the_device_entrance_is_refused;
}

// -- the shape of the three packages ---------------------------------------------------------

/// The three siblings reach only Dropbox, and only the part of it each one needs.
///
/// The shape RD-106-04 decided, asserted against the real manifests: one resolver carrying the
/// provider row, one crawler and one sign-in claiming that row's slug, and no capability beyond
/// HTTP to the provider's own hosts.
#[test]
fn the_three_siblings_reach_only_the_part_of_dropbox_each_one_needs() {
    storage::shape(&DROPBOX.shape);
}
