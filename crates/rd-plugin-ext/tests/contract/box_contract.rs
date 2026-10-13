//! Box, exercised end to end against a mock Box Content API and a mock Box token endpoint
//! (RD-120-05).
//!
//! **Both providers are mocks.** They answer at the host boundary, so no socket is opened and
//! neither api.box.com nor account.box.com is contacted. A run against a real Box account is
//! *not* claimed here; there is none in this checkout, and no registered Box application
//! either. What is proven is everything that does not need one.
//!
//! All three Box plugins are driven here as real WebAssembly components, which is what makes
//! the fourth acceptance criterion — shared and account content use the same contract —
//! something a test can state rather than something a reader has to believe: the crawler's
//! answers are handed to the resolver's own `match-url`, and the resolver's answer for a file
//! found in a folder and for the same file found behind a shared link is built by one code
//! path. `plugins/box/src/native/tests.rs` covers the resolver's protocol logic in more detail
//! against the same kind of mock.
//!
//! The cases are the ones the job asks for, and the ones a host reacts to differently:
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A folder of files and subfolders | links, with names, sizes and the folder they sat in |
//! | A folder Box answers a page at a time | every offset followed, to the cap |
//! | A shared link that turns out to be one file | that one file |
//! | A password-protected shared link | the password in the `boxapi` header and nowhere else |
//! | A folder that contains itself | read once |
//! | An empty folder | a refusal with a code, never an empty package |
//! | Denied, not found, throttled, refused token | four different codes |
//! | Shared and account content | one resolver, one download route, one version pin |
//! | The browser sign-in | a code exchanged for a stored token and its renewal material |
//! | The application's id and secret | markers in the exchange, never values |
//!
//! The cases every cloud drive shares run through `support::storage` from the Box rows of its
//! table; the offsets, the `boxapi` header and the hand-over to the resolver stay here.

use std::sync::Arc;

use rd_core::AccountId;
use rd_plugin_api::{ClientIdentity, ResolveRequest, Resolver};
use rd_plugin_host::SecretFilledByManifest;

use crate::support::storage::{
    self,
    sign_in::TokenEndpoint,
    table::{BOX, sign_in::BOX as BOX_SIGN_IN},
};

mod mock;

use mock::{MockBox, crawler, file_row, folder_row, resolver};

/// A folder in the account's own Box, as its web address is spelled.
const FOLDER: &str = "https://app.box.com/folder/987654321";
/// A shared link, which does not say whether it points at a folder or a file.
const SHARED_LINK: &str = "https://app.box.com/s/abc123def456";
/// The same link with its password, which is the one thing that must not travel anywhere but
/// the `boxapi` header.
const SHARED_LINK_WITH_PASSWORD: &str =
    "https://app.box.com/s/abc123def456?shared_link_password=hunter2";
const PASSWORD: &str = "hunter2";

fn crawler_component() -> Vec<u8> {
    storage::crawler::component(&BOX.crawler)
}

fn resolver_component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-box")
}

// -- the crawler -----------------------------------------------------------------------------

storage::shared_cases! {
    BOX.crawler;
    /// A crawler is asked before it is handed anything, and it answers from the address alone.
    ///
    /// The check that keeps a crawler from costing every other link in the queue a request — and,
    /// here, the one that keeps the two siblings apart: a file link is the resolver's, and the
    /// canonical address the crawler itself hands back is never listed again.
    only_box_folder_addresses_and_shared_links_are_claimed =>
        storage::crawler::claims_only_its_own_addresses;
}

/// A folder comes back as its files, with the names, the sizes and the structure Box gave — and
/// each as the canonical address the sibling resolver claims.
#[tokio::test]
async fn a_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = crawler_component();
    let host = MockBox::new(vec![
        (
            "987654321",
            "Show",
            vec![vec![
                file_row("11", "readme.txt", 12),
                folder_row("22", "Season 1"),
            ]],
        ),
        (
            "22",
            "Season 1",
            vec![vec![
                file_row("33", "e01.mkv", 1024),
                file_row("34", "e02.mkv", 2048),
            ]],
        ),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER, None)
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
    assert_eq!(links[1].url, "https://app.box.com/file/33");
    assert_eq!(
        host.requests(),
        vec![
            "get 987654321".to_owned(),
            "list 987654321 0".to_owned(),
            "list 22 0".to_owned(),
        ]
    );
    // A folder in the account's own Box needs no shared link, so none is sent.
    assert!(host.box_apis().iter().all(Option::is_none));
}

/// Box answers a wide folder a page at a time and says how many rows it holds. Every page is
/// asked for, and the walk is not over until Box stops saying there are more.
#[tokio::test]
async fn a_folder_box_answers_a_page_at_a_time_is_followed_to_the_end() {
    let bytes = crawler_component();
    let host = MockBox::new(vec![(
        "987654321",
        "Long",
        vec![
            vec![file_row("11", "a.mkv", 1)],
            vec![file_row("12", "b.mkv", 2)],
            vec![file_row("13", "c.mkv", 3)],
        ],
    )]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(
        host.requests(),
        vec![
            "get 987654321".to_owned(),
            "list 987654321 0".to_owned(),
            "list 987654321 1".to_owned(),
            "list 987654321 2".to_owned(),
        ]
    );
}

/// A collection that paginates for as long as anybody asks is stopped by the crawler's own cap,
/// not by the sandbox running out of fuel.
#[tokio::test]
async fn a_folder_that_pages_further_than_the_cap_stops_at_the_cap() {
    let bytes = crawler_component();
    let pages: Vec<Vec<String>> = (0..14)
        .map(|index| {
            vec![file_row(
                &format!("{}", 100 + index),
                &format!("f{index}.bin"),
                1,
            )]
        })
        .collect();
    let host = MockBox::endless(vec![("987654321", "Endless", pages)]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER, None)
        .await
        .expect("call")
        .expect("a listing");

    // Ten pages of one row each, and no eleventh request.
    assert_eq!(links.len(), 10, "{links:?}");
    let listings = host
        .requests()
        .into_iter()
        .filter(|entry| entry.starts_with("list "))
        .count();
    assert_eq!(listings, 10);
}

/// A shared link does not say what it points at. When it turns out to be one file, that one
/// file is the answer — addressed through the link it was found behind, because the link is
/// what grants the access.
#[tokio::test]
async fn a_shared_link_that_turns_out_to_be_one_file_yields_that_file() {
    let bytes = crawler_component();
    let host = MockBox::single_file();
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(SHARED_LINK, None)
        .await
        .expect("call")
        .expect("one file");

    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("release.bin"));
    assert_eq!(links[0].size, Some(1_048_576));
    assert_eq!(links[0].package_hint, None);
    assert_eq!(links[0].url, format!("{SHARED_LINK}/file/42"));
    assert_eq!(host.requests(), vec!["shared_items".to_owned()]);
}

/// A shared link's password reaches the `boxapi` header on every request of the crawl, and
/// nothing else anywhere: not the address, not a query parameter, not what is handed back.
#[tokio::test]
async fn a_shared_links_password_travels_in_the_box_api_header_and_in_nothing_else() {
    let bytes = crawler_component();
    let host = MockBox::new(vec![(
        "987654321",
        "Show",
        vec![vec![file_row("11", "e01.mkv", 1024)]],
    )]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(SHARED_LINK_WITH_PASSWORD, None)
        .await
        .expect("call")
        .expect("a listing");

    let expected = format!("shared_link={SHARED_LINK}&shared_link_password={PASSWORD}");
    let sent = host.box_apis();
    assert!(!sent.is_empty());
    for value in &sent {
        assert_eq!(value.as_deref(), Some(expected.as_str()));
    }
    // Every request went to Box's own API and carried the password nowhere but that header.
    for entry in host.requests() {
        assert!(!entry.contains(PASSWORD), "{entry}");
    }
    // The one address it does travel in is the hand-over to the sibling resolver, because
    // nothing else passes between the two packages — the same reluctant exception Dropbox
    // makes (RD-106-06). `rd_core::REDACTED_QUERY` names the parameter, so it is struck out of
    // every log line, and it never reaches the address the bytes come from.
    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].url,
        format!("{SHARED_LINK}/file/11?shared_link_password={PASSWORD}")
    );
    assert!(
        rd_core::is_secret_parameter("shared_link_password"),
        "the parameter a crawled address carries has to be one the core redacts"
    );
    let redacted = rd_core::redact_url(&links[0].url.parse().expect("URL"));
    assert!(!redacted.contains(PASSWORD), "{redacted}");
}

/// A folder that contains itself is read once.
#[tokio::test]
async fn a_folder_that_contains_itself_is_read_once() {
    let bytes = crawler_component();
    let host = MockBox::new(vec![
        (
            "987654321",
            "Show",
            vec![vec![folder_row("22", "Inner"), file_row("11", "a.bin", 1)]],
        ),
        (
            "22",
            "Inner",
            vec![vec![
                folder_row("987654321", "Show"),
                folder_row("22", "Inner"),
                file_row("12", "b.bin", 2),
            ]],
        ),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 2, "{links:?}");
    let listings = host
        .requests()
        .into_iter()
        .filter(|entry| entry.starts_with("list "))
        .count();
    assert_eq!(listings, 2, "{:?}", host.requests());
}

/// An empty folder is a refusal with a code, never a package with nothing in it.
#[tokio::test]
async fn an_empty_folder_is_a_refusal_with_a_code_and_never_an_empty_package() {
    let bytes = crawler_component();
    let host = MockBox::new(vec![("987654321", "Empty", vec![Vec::new()])]);
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER, None)
        .await
        .expect("call")
        .expect_err("an empty folder is not a result");
    assert_eq!(refusal.code.as_deref(), Some("box_crawler.folder_empty"));
}

/// Denied, gone, throttled and a refused token are four different codes, because a person acts
/// differently on each — and nothing Box wrote travels into any of them.
#[tokio::test]
async fn the_ways_box_refuses_a_folder_are_four_different_codes() {
    let refusals = storage::crawler::refusals(&BOX.crawler).await;
    assert_eq!(refusals.len(), 4);
    // A throttled crawl is still this plugin's folder: `not_mine` is what the link intake
    // falls back on when a crawler was wrong to claim an address, and a rate limit says
    // nothing of the sort.
    let (throttled, _) = &refusals[2];
    assert_eq!(throttled.code.as_deref(), Some("box_crawler.rate_limited"));
    assert!(!throttled.not_mine, "{throttled:?}");
}

// -- the two siblings together ----------------------------------------------------------------

/// Shared and account content use the same contract, which is the fourth acceptance criterion.
///
/// Whichever way a file was found, the crawler hands back an address the *resolver* claims, and
/// the resolver turns it into the same thing: the stable API route, pinned to the version Box
/// named, with the shared link — and only the shared link — carried in a `boxapi` header when
/// there is one. Two ways in, one download.
#[tokio::test]
async fn shared_and_account_content_use_the_same_contract() {
    let crawler_bytes = crawler_component();
    let resolver_bytes = resolver_component();

    let folders = || {
        vec![(
            "987654321",
            "Show",
            vec![vec![file_row("42", "release.bin", 1_048_576)]],
        )]
    };
    let from_account = crawler(MockBox::new(folders()), &crawler_bytes)
        .crawl(FOLDER, None)
        .await
        .expect("call")
        .expect("a listing");
    let from_share = crawler(MockBox::new(folders()), &crawler_bytes)
        .crawl(SHARED_LINK_WITH_PASSWORD, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(from_account[0].url, "https://app.box.com/file/42");
    assert_eq!(
        from_share[0].url,
        format!("{SHARED_LINK}/file/42?shared_link_password={PASSWORD}")
    );

    for (link, expected_box_api) in [
        (&from_account[0], None),
        (
            &from_share[0],
            Some(format!(
                "shared_link={SHARED_LINK}&shared_link_password={PASSWORD}"
            )),
        ),
    ] {
        let host = MockBox::new(Vec::new());
        let resolver = resolver(Arc::clone(&host), &resolver_bytes);
        // The sibling's answer is an address this resolver claims. That is the whole of what
        // passes between the two packages.
        assert!(
            resolver.guest_claims(&link.url).await.expect("claims"),
            "{}",
            link.url
        );
        let resolved = resolver
            .resolve(ResolveRequest {
                url: link.url.parse().expect("URL"),
                client: ClientIdentity {
                    account_id: Some(AccountId::new()),
                    proxy_profile_id: None,
                    tls_revision: 7,
                },
            })
            .await
            .expect("resolved");
        // One download route, and one version pin, whichever way the file was found.
        assert_eq!(
            resolved.url.as_str(),
            "https://api.box.com/2.0/files/42/content?version=98765"
        );
        assert_eq!(
            resolved
                .checksum
                .as_ref()
                .map(|checksum| checksum.algorithm),
            Some(rd_core::ChecksumAlgorithm::Sha1)
        );
        let sent = resolved
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case("boxapi"))
            .map(|header| header.value.clone());
        assert_eq!(sent, expected_box_api);
        // The password is never in the address the bytes come from.
        assert!(!resolved.url.as_str().contains(PASSWORD));
        // And the token left the resolver as a marker, exactly as it left the crawler.
        assert_eq!(
            host.authorizations(),
            vec!["Bearer {{secret:box_access_token}}".to_owned()]
        );
    }
}

// -- the sign-in ------------------------------------------------------------------------------

storage::shared_cases! {
    BOX_SIGN_IN;
    /// The browser sign-in, from the address the person is sent to all the way to the stored token —
    /// with neither half of the application's registration anywhere in the plugin.
    the_browser_sign_in_runs_through_to_a_stored_token_with_its_renewal_material =>
        storage::sign_in::the_browser_sign_in_runs_through;
    /// Two flows started in the same moment, for the same account, share nothing.
    two_flows_in_the_same_moment_share_no_state =>
        storage::sign_in::two_flows_share_nothing;
    /// A renewal sends both halves of the registration as markers, stores the new token without
    /// anybody being asked, and never carries a code.
    a_renewal_sends_both_credentials_as_markers_and_never_as_values =>
        storage::sign_in::a_renewal_stores_a_new_token_without_a_redirect;
    /// Each refusal ends the flow with its own code, a provider that cannot be reached ends nothing,
    /// and Box's prose reaches no message.
    the_refusals_box_makes_are_told_apart_and_an_outage_is_not_one_of_them =>
        storage::sign_in::a_refusal_ends_the_flow_and_an_outage_does_not;
    /// A rate limit is a wait, not a failure, and carries Box's own `Retry-After`.
    a_rate_limit_becomes_a_wait_carrying_retry_after =>
        storage::sign_in::a_rate_limit_becomes_a_wait;
    /// Box has no device entrance, and being asked for one is a refusal with a stable code rather
    /// than a trap.
    box_offers_no_device_sign_in_and_says_so =>
        storage::sign_in::the_device_entrance_is_refused;
}

/// An account with no registered application is told so before anybody is sent anywhere.
#[tokio::test]
async fn a_sign_in_without_a_registered_application_reaches_nothing() {
    let bytes = rd_plugin_host::artifact::component(BOX_SIGN_IN.component);
    let server = TokenEndpoint::unregistered(&BOX_SIGN_IN);
    let plugin = storage::sign_in::plugin(&BOX_SIGN_IN, &bytes, server.clone());
    let refusal = plugin
        .begin(AccountId::new(), None)
        .await
        .expect_err("no application")
        .to_string();
    assert!(
        refusal.contains("no registered Box application"),
        "{refusal}"
    );
    assert!(server.requests().is_empty(), "nothing may be asked");
}

// -- the shape of the three packages -----------------------------------------------------------

/// The three siblings reach only Box, and only the part of it each one needs.
///
/// The shape RD-106-04 decided, asserted against the real manifests: one resolver carrying the
/// provider row, one crawler and one sign-in claiming that row's slug, and no capability beyond
/// HTTP to the provider's own hosts.
#[test]
fn the_three_siblings_reach_only_the_part_of_box_each_one_needs() {
    let (resolver, _, _) = storage::shape(&BOX.shape);
    let provider = resolver.provider.as_ref().expect("the provider row");
    // Two slots, because Box's token endpoint needs a client secret on every grant: the one the
    // person fills, and the one the sign-in fills (RD-106-03). Writing the second over the first
    // would destroy the value every later renewal needs.
    let slots = provider.secret_slots();
    assert_eq!(slots.len(), 2, "{slots:?}");
    let person = slots
        .iter()
        .find(|slot| slot.reference == "box_client_secret")
        .expect("the slot the person fills");
    let flow = slots
        .iter()
        .find(|slot| slot.reference == "box_access_token")
        .expect("the slot the sign-in fills");
    assert_eq!(person.filled_by, SecretFilledByManifest::Person);
    assert_eq!(flow.filled_by, SecretFilledByManifest::Flow);
    // The exact host the token may be sent to, and the one the download engine checks the
    // transfer address against before attaching the bearer header.
    assert_eq!(flow.domains, vec!["api.box.com".to_owned()]);
    // The resolver spends the access token; the sign-in holds the client secret. Neither is
    // granted the other's, which is what least privilege means with two credentials in play.
    assert_eq!(
        resolver.capabilities.secrets,
        vec!["box_access_token".to_owned()]
    );
    assert!(!resolver.match_domains.iter().any(|domain| domain == "*"));
}
