//! Google Drive, exercised end to end against a mock Drive API and a mock Google
//! authorization server (RD-106-04).
//!
//! **Both providers are mocks.** They answer at the host boundary, so no socket is opened and
//! neither googleapis.com nor accounts.google.com is contacted. A run against a real Google
//! account is *not* claimed here; there is none in this checkout, and there is no registered
//! OAuth client either. What is proven is everything that does not need one.
//!
//! Two of the three Google Drive plugins are driven here as real WebAssembly components,
//! because that is the only place their guest code exists: the crawler's paging walk and the
//! sign-in's exchange are written against the WIT imports and cannot run natively. The third,
//! `plugins/google-drive/`, keeps its protocol logic outside the component and is covered by
//! `plugins/google-drive/src/native/tests.rs` against the same kind of mock.
//!
//! The cases are the ones the job asks for, and the ones a host reacts to differently:
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A folder of files and subfolders | links, with names, sizes and the folder they sat in |
//! | A folder Drive answers a page at a time | every page followed, to the cap |
//! | A Workspace document inside a folder | the export name, extension already on it |
//! | A folder that contains itself | read once |
//! | An empty folder | a refusal with a code, never an empty package |
//! | Quota, virus scan, rate limit, refused token | four different codes |
//! | The sign-in | PKCE through to a stored token and its renewal material |
//! | The device entrance | refused with a stable code; the manifest offers it to nobody |
//! | The OAuth client | a marker in the address and in both exchanges, never a value |
//!
//! What every cloud drive shares -- claims, refusals, the sandbox, the sign-in and the shape of
//! the three packages -- runs through `support::storage` from the Google Drive rows of its
//! table; what is Drive's own, the page tokens, the Workspace exports and the shortcuts that
//! make a cycle, stays here with the mock in `mock.rs`.

use std::sync::Arc;

use crate::support::storage::{
    self,
    table::{GOOGLE_DRIVE, sign_in::GOOGLE_DRIVE as GOOGLE_SIGN_IN},
};

mod mock;

use mock::{MockDrive, crawler, document, file, page, subfolder};

const ROOT: &str = "redacted-folder-id-0001";

fn crawler_component() -> Vec<u8> {
    storage::crawler::component(&GOOGLE_DRIVE.crawler)
}

fn folder_address(id: &str) -> String {
    format!("https://drive.google.com/drive/folders/{id}")
}

// -- the crawler ----------------------------------------------------------------------------

storage::shared_cases! {
    GOOGLE_DRIVE.crawler;
    /// A crawler is asked before it is handed anything, and it answers from the address alone.
    ///
    /// The check that keeps a crawler from costing every other link in the queue a request — and,
    /// here, the one that keeps the two siblings apart: a file address is the resolver's.
    only_google_drive_folder_addresses_are_claimed =>
        storage::crawler::claims_only_its_own_addresses;
    /// The three ways a crawl can fail reach the person as three different codes, and none of them
    /// repeats a word Google wrote.
    a_refused_token_a_rate_limit_and_a_missing_folder_are_told_apart =>
        storage::crawler::refusals_are_told_apart;
    /// A crawler reaches nothing outside the domains its own manifest declares.
    the_crawler_reaches_nothing_its_manifest_did_not_declare =>
        storage::crawler::reaches_nothing_its_manifest_did_not_declare;
}

/// A folder comes back as its files, with the names, the sizes and the structure Drive gave —
/// and each as the canonical address the sibling resolver claims.
#[tokio::test]
async fn a_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![
        (
            ROOT,
            "Show",
            vec![page(
                &[file("x0", "readme.txt", 12), subfolder("s1", "Season 1")],
                None,
            )],
        ),
        (
            "s1",
            "Season 1",
            vec![page(
                &[file("x1", "e01.mkv", 1024), file("x2", "e02.mkv", 2048)],
                None,
            )],
        ),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&folder_address(ROOT), None)
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
    assert_eq!(links[1].url, "https://drive.google.com/file/d/x1/view");
    assert_eq!(
        host.requests(),
        vec![
            format!("get {ROOT}"),
            format!("list {ROOT} "),
            "list s1 ".to_owned(),
        ]
    );
}

/// Drive answers a wide folder a page at a time. Every page is followed, and the walk is not
/// over until Drive stops handing out a `nextPageToken`.
///
/// The limit the Premiumize crawler did not need: `folder/list` there answers whole.
#[tokio::test]
async fn a_folder_drive_answers_a_page_at_a_time_is_followed_to_the_end() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![(
        ROOT,
        "Long",
        vec![
            page(&[file("x1", "a.mkv", 1)], Some("page-1")),
            page(&[file("x2", "b.mkv", 2)], Some("page-2")),
            page(&[file("x3", "c.mkv", 3)], None),
        ],
    )]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&folder_address(ROOT), None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(
        host.requests(),
        vec![
            format!("get {ROOT}"),
            format!("list {ROOT} "),
            format!("list {ROOT} page-1"),
            format!("list {ROOT} page-2"),
        ],
        "the page token has to be carried into the next request, or the walk repeats page one"
    );
}

/// A Workspace document in a folder carries the name and the extension it will actually arrive
/// under, so it is visible in the LinkGrabber before anything is queued.
#[tokio::test]
async fn a_workspace_document_carries_its_export_name_before_anything_is_queued() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![(
        ROOT,
        "Reports",
        vec![page(&[document("x1", "Quarterly numbers")], None)],
    )]);
    let links = crawler(host, &bytes)
        .crawl(&folder_address(ROOT), None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].file_name.as_deref(),
        Some("Quarterly numbers.xlsx")
    );
    // No size: an export's bytes do not exist until the export runs, so stating one would be
    // inventing it.
    assert_eq!(links[0].size, None);
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_template_and_never_as_a_value() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![(
        ROOT,
        "Show",
        vec![page(&[file("x1", "a.mkv", 1)], None)],
    )]);
    crawler(Arc::clone(&host), &bytes)
        .crawl(&folder_address(ROOT), None)
        .await
        .expect("call")
        .expect("a listing");
    storage::crawler::assert_bearer_markers(&host.authorizations(), "google_drive_access_token");
}

/// A folder that contains itself is read once, not for ever.
///
/// Drive shortcuts make this reachable: a shortcut can point at an ancestor. The failure it
/// prevents cannot be spotted by watching — every request the walk makes is reasonable alone.
#[tokio::test]
async fn a_cycle_is_walked_once() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![
        (
            ROOT,
            "Loop",
            vec![page(
                &[subfolder("inner", "Inner"), file("x1", "a.mkv", 1)],
                None,
            )],
        ),
        (
            "inner",
            "Inner",
            vec![page(
                &[
                    subfolder(ROOT, "Back"),
                    subfolder("inner", "Self"),
                    file("x2", "b.mkv", 2),
                ],
                None,
            )],
        ),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&folder_address(ROOT), None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 2, "{links:?}");
    assert_eq!(
        host.requests()
            .iter()
            .filter(|entry| entry.starts_with("list "))
            .count(),
        2,
        "each folder is listed exactly once"
    );
}

/// An empty folder says so, with a code the interface can translate.
///
/// Returning an empty list would create a package with nothing in it and nothing to explain
/// why — the defect ADR 0001 was written for.
#[tokio::test]
async fn an_empty_folder_is_reported_and_not_silently_dropped() {
    let bytes = crawler_component();
    let host = MockDrive::new(vec![(ROOT, "Empty", vec![page(&[], None)])]);
    let refusal = crawler(host, &bytes)
        .crawl(&folder_address(ROOT), None)
        .await
        .expect("call")
        .expect_err("an empty folder is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("google_drive_crawler.folder_empty")
    );
}

/// A folder Drive says is gone is a refusal, not an empty folder.
#[tokio::test]
async fn a_folder_that_is_not_there_is_reported_with_its_own_code() {
    let bytes = crawler_component();
    let refusal = crawler(MockDrive::new(Vec::new()), &bytes)
        .crawl(&folder_address("redacted-folder-id-0009"), None)
        .await
        .expect("call")
        .expect_err("a missing folder is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("google_drive_crawler.folder_unreachable")
    );
}

// -- the sign-in ----------------------------------------------------------------------------

storage::shared_cases! {
    GOOGLE_SIGN_IN;
    /// Authorization code with PKCE, from the address the person is sent to all the way to the
    /// stored token — and with the two parameters without which Google issues no refresh material
    /// at all.
    the_sign_in_runs_through_to_a_stored_token_with_its_renewal_material =>
        storage::sign_in::the_browser_sign_in_runs_through;
    /// A stolen code without its verifier buys nothing, which is the whole reason PKCE exists.
    a_code_without_its_verifier_is_refused =>
        storage::sign_in::a_code_without_its_verifier_is_refused;
    /// Two flows started in the same moment, for the same account, share nothing.
    two_flows_in_the_same_moment_share_no_secret =>
        storage::sign_in::two_flows_share_nothing;
    /// A refusal ends the flow and an unreachable Google does not — the distinction the renewal
    /// sweep acts on, and the one that would otherwise sign people out whenever a connection drops.
    a_refused_grant_ends_the_flow_and_an_unreachable_google_does_not =>
        storage::sign_in::a_refusal_ends_the_flow_and_an_outage_does_not;
    /// A renewal stores the new token without anybody being asked, and the stored refresh material
    /// leaves the plugin as a template rather than as a value.
    ///
    /// This is what a Drive account lives on: Google's access tokens last an hour, so without it
    /// every download started more than an hour after a sign-in would ask for a new one.
    a_renewal_stores_a_new_token_without_a_redirect =>
        storage::sign_in::a_renewal_stores_a_new_token_without_a_redirect;
    /// A rate limit is a wait, not a failure, and carries Google's own `Retry-After`.
    a_rate_limit_becomes_a_wait_carrying_retry_after =>
        storage::sign_in::a_rate_limit_becomes_a_wait;
    /// The device entrance is not offered, and being asked anyway is a stable refusal rather than
    /// a trap. Google's device flow exists but does not grant the Drive scopes this needs.
    the_device_entrance_is_refused_with_a_stable_code =>
        storage::sign_in::the_device_entrance_is_refused;
}

// -- the shape of the three packages ---------------------------------------------------------

/// The three siblings reach only Google, and only the part of it each one needs.
///
/// The shape RD-106-04 decided, asserted against the real manifests so RD-106-05 and RD-106-06
/// have something to copy that cannot silently drift: one resolver carrying the provider row,
/// one crawler and one sign-in claiming that row's slug, and no capability beyond HTTP to the
/// provider's own hosts.
#[test]
fn the_three_siblings_reach_only_the_part_of_google_each_one_needs() {
    storage::shape(&GOOGLE_DRIVE.shape);
}
