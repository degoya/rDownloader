//! OneDrive and SharePoint, exercised end to end against a mock Microsoft Graph and a mock
//! Microsoft identity platform (RD-106-05).
//!
//! **Both providers are mocks.** They answer at the host boundary, so no socket is opened and
//! neither graph.microsoft.com nor login.microsoftonline.com is contacted. A run against a real
//! Microsoft account is *not* claimed here; there is none in this checkout, and there is no
//! registered application either. What is proven is everything that does not need one.
//!
//! Two of the three OneDrive plugins are driven here as real WebAssembly components, because
//! that is the only place their guest code exists: the crawler's paging walk and the sign-in's
//! two entrances are written against the WIT imports and cannot run natively. The third,
//! `plugins/onedrive/`, keeps its protocol logic outside the component and is covered by
//! `plugins/onedrive/src/native/tests.rs` against the same kind of mock.
//!
//! The cases are the ones the job asks for, and the ones a host reacts to differently:
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A shared folder of files and subfolders | links, with names, sizes and the folder they sat in |
//! | A folder Graph answers a page at a time | every `@odata.nextLink` followed, to the cap |
//! | A long address that turns out to be one file | that one file |
//! | A folder that contains itself | read once |
//! | An empty folder | a refusal with a code, never an empty package |
//! | A next page that leaves Graph | refused before it is fetched |
//! | Denied, not found, throttled, refused token | four different codes |
//! | The browser sign-in | PKCE through to a stored token and its renewal material |
//! | The device sign-in | a code to type, a wait, a stored token, the same renewal |
//! | The application id | a marker in the address and in every exchange, never a value |
//!
//! The cases every cloud drive shares run through `support::storage` from the OneDrive rows of
//! its table; Graph's next links, the share ids and the device entrance stay here.

use std::sync::Arc;

use base64::Engine as _;
use rd_core::{AccountId, FailureKind};
use rd_plugin_host::{PluginManifest, extension::TokenOutcome};

use crate::common::oauth::Stored;
use crate::support::storage::{
    self,
    sign_in::{Case, PLACEHOLDER_ACCESS_TOKEN, PLACEHOLDER_REFRESH_TOKEN, TokenEndpoint},
    table::{ONEDRIVE, sign_in::ONEDRIVE as ONEDRIVE_SIGN_IN},
};

mod mock;

use mock::{MockGraph, crawler, file, page, page_leaving_graph, subfolder};

const PLACEHOLDER_DEVICE_CODE: &str = "redacted-device-code-0000";

/// A folder sharing link, as OneDrive hands them out.
const FOLDER_LINK: &str = "https://1drv.ms/f/s!AkXy_Zabc-DEF";
/// The long personal address, which says nothing about what it points at.
const UNDECIDED_LINK: &str = "https://onedrive.live.com/?id=ABC123%21456&cid=ABC123";

fn crawler_component() -> Vec<u8> {
    storage::crawler::component(&ONEDRIVE.crawler)
}

fn oauth_component() -> Vec<u8> {
    rd_plugin_host::artifact::component(ONEDRIVE_SIGN_IN.component)
}

/// The share id Graph uses for a sharing link, computed independently of the plugin so a
/// matching request is evidence rather than a tautology.
fn share_of(link: &str) -> String {
    format!(
        "u!{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(link.as_bytes())
    )
}

// -- the crawler ----------------------------------------------------------------------------

storage::shared_cases! {
    ONEDRIVE.crawler;
    /// A crawler is asked before it is handed anything, and it answers from the address alone.
    ///
    /// The check that keeps a crawler from costing every other link in the queue a request — and,
    /// here, the one that keeps the two siblings apart: a file link is the resolver's, and the
    /// canonical Graph address the crawler itself hands back is never listed again.
    only_onedrive_folder_links_and_undecided_ones_are_claimed =>
        storage::crawler::claims_only_its_own_addresses;
    /// The ways a crawl can fail reach the person as different codes, and none of them repeats a
    /// word Microsoft wrote. (That a throttle carries Microsoft's own `Retry-After` is proven at the
    /// resolver, in `plugins/onedrive/src/native/tests.rs`; a crawl refusal carries no category.)
    a_refused_token_a_throttle_a_denied_share_and_a_missing_folder_are_told_apart =>
        storage::crawler::refusals_are_told_apart;
    /// A crawler reaches nothing outside the domains its own manifest declares.
    the_crawler_reaches_nothing_its_manifest_did_not_declare =>
        storage::crawler::reaches_nothing_its_manifest_did_not_declare;
}

/// A shared folder comes back as its files, with the names, the sizes and the structure Graph
/// gave — and each as the canonical address the sibling resolver claims, carrying the share
/// it was found through.
#[tokio::test]
async fn a_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![
        (
            "",
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
        .crawl(FOLDER_LINK, None)
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
    // The one thing that crosses between the two packages: an address, and nothing else —
    // the item inside the share it was found through, never the item on its own.
    let share = share_of(FOLDER_LINK);
    assert_eq!(
        links[1].url,
        format!("https://graph.microsoft.com/v1.0/shares/{share}/items/x1")
    );
    assert_eq!(
        host.requests(),
        vec![
            "get ".to_owned(),
            "list  ".to_owned(),
            "list s1 ".to_owned()
        ]
    );
    // The sharing link went to Graph whole, as the share id Microsoft documents.
    assert_eq!(host.authorizations().len(), 3);
}

/// Graph answers a wide folder a page at a time and names the next page in
/// `@odata.nextLink`. Every page is followed, as given, and the walk is not over until Graph
/// stops naming one.
#[tokio::test]
async fn a_folder_graph_answers_a_page_at_a_time_is_followed_to_the_end() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![(
        "",
        "Long",
        vec![
            page(&[file("x1", "a.mkv", 1)], Some("page-1")),
            page(&[file("x2", "b.mkv", 2)], Some("page-2")),
            page(&[file("x3", "c.mkv", 3)], None),
        ],
    )]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect("a listing");

    assert_eq!(links.len(), 3, "{links:?}");
    assert_eq!(
        host.requests(),
        vec![
            "get ".to_owned(),
            "list  ".to_owned(),
            "list  page-1".to_owned(),
            "list  page-2".to_owned(),
        ],
        "the next link has to be followed as given, or the walk repeats page one"
    );
}

/// A next page that leaves Graph is refused before it is fetched. The sandbox would refuse it
/// too, but a refusal with a name beats one without.
#[tokio::test]
async fn a_next_page_that_leaves_graph_is_refused_before_it_is_fetched() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![(
        "",
        "Trap",
        vec![page_leaving_graph(&[file("x1", "a.mkv", 1)])],
    )]);
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect_err("a next link off Graph is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("onedrive_crawler.invalid_response")
    );
    assert!(
        !host.requests().iter().any(|entry| entry.contains("evil")),
        "{:?}",
        host.requests()
    );
}

/// The long personal address does not say what it points at. When it turns out to be one
/// file, that one file is the answer — with the share as its address, because the resolver
/// never sees the pasted link.
#[tokio::test]
async fn an_address_that_does_not_say_and_points_at_a_file_yields_that_file() {
    let bytes = crawler_component();
    let host = MockGraph::single_file();
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(UNDECIDED_LINK, None)
        .await
        .expect("call")
        .expect("one file");
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("release.bin"));
    assert_eq!(links[0].size, Some(1_048_576));
    assert_eq!(links[0].package_hint, None);
    assert_eq!(
        links[0].url,
        format!(
            "https://graph.microsoft.com/v1.0/shares/{}/driveItem",
            share_of(UNDECIDED_LINK)
        )
    );
    assert_eq!(
        host.requests(),
        vec!["get ".to_owned()],
        "one request, no listing"
    );

    // A link that *said* it was a folder and is not one is refused by name instead.
    let refusal = crawler(MockGraph::single_file(), &bytes)
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect_err("a folder link that is a file is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("onedrive_crawler.not_a_folder")
    );
}

/// The account's token never leaves the plugin as a value.
#[tokio::test]
async fn the_access_token_travels_as_a_template_and_never_as_a_value() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![(
        "",
        "Show",
        vec![page(&[file("x1", "a.mkv", 1)], None)],
    )]);
    crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect("a listing");
    storage::crawler::assert_bearer_markers(&host.authorizations(), "onedrive_access_token");
}

/// A folder that contains itself is read once, not for ever.
#[tokio::test]
async fn a_cycle_is_walked_once() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![
        (
            "",
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
                    subfolder("root-id", "Back"),
                    subfolder("inner", "Self"),
                    file("x2", "b.mkv", 2),
                ],
                None,
            )],
        ),
    ]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(FOLDER_LINK, None)
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
#[tokio::test]
async fn an_empty_folder_is_reported_and_not_silently_dropped() {
    let bytes = crawler_component();
    let host = MockGraph::new(vec![("", "Empty", vec![page(&[], None)])]);
    let refusal = crawler(host, &bytes)
        .crawl(FOLDER_LINK, None)
        .await
        .expect("call")
        .expect_err("an empty folder is a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("onedrive_crawler.folder_empty")
    );
}
// -- the sign-in ----------------------------------------------------------------------------

storage::shared_cases! {
    ONEDRIVE_SIGN_IN;
    /// Authorization code with PKCE, from the address the person is sent to all the way to the
    /// stored token — asking for the least the three plugins need, and for the renewal scope.
    the_browser_sign_in_runs_through_to_a_stored_token_with_its_renewal_material =>
        storage::sign_in::the_browser_sign_in_runs_through;
    /// A stolen code without its verifier buys nothing, which is the whole reason PKCE exists.
    a_code_without_its_verifier_is_refused =>
        storage::sign_in::a_code_without_its_verifier_is_refused;
    /// Two flows started in the same moment, for the same account, share nothing.
    two_flows_in_the_same_moment_share_no_secret =>
        storage::sign_in::two_flows_share_nothing;
    /// A refusal ends the flow and an unreachable Microsoft does not — the distinction the renewal
    /// sweep acts on, and the one that would otherwise sign people out whenever a connection drops.
    a_refused_grant_ends_the_flow_and_an_unreachable_microsoft_does_not =>
        storage::sign_in::a_refusal_ends_the_flow_and_an_outage_does_not;
    /// A renewal stores the new token without anybody being asked, and the stored refresh material
    /// leaves the plugin as a template rather than as a value. Microsoft rotates the refresh token
    /// on every renewal, and the rotated one is what gets stored.
    a_renewal_stores_a_new_token_without_a_redirect_or_a_device_code =>
        storage::sign_in::a_renewal_stores_a_new_token_without_a_redirect;
    /// A rate limit is a wait, not a failure, and carries Microsoft's own `Retry-After`.
    a_rate_limit_becomes_a_wait_carrying_retry_after =>
        storage::sign_in::a_rate_limit_becomes_a_wait;
}

/// The device entrance, end to end: a code the person types at microsoft.com/devicelogin, a
/// poll that waits, and a token that lands in the vault with the same refresh material the
/// browser sign-in produces — so the renewal sweep treats the two alike.
#[tokio::test]
async fn the_device_sign_in_runs_through_to_the_same_stored_token() {
    let bytes = oauth_component();
    let server = TokenEndpoint::new(&ONEDRIVE_SIGN_IN, Case::DevicePending);
    let plugin = storage::sign_in::plugin(&ONEDRIVE_SIGN_IN, &bytes, server.clone());
    let account = AccountId::new();

    let prompt = plugin.device_begin(account, None).await.expect("begin");
    assert_eq!(prompt.verification_url, "https://microsoft.com/devicelogin");
    assert_eq!(prompt.user_code.as_deref(), Some("ABCD1234"));
    assert_eq!(prompt.expires_in_seconds, Some(900));
    assert_eq!(prompt.interval_seconds, Some(5));
    // The device code is what the poll is made with and is never part of what is shown.
    let device_code = prompt.flow_state.clone().expect("a device code");
    assert_eq!(device_code, PLACEHOLDER_DEVICE_CODE);
    assert_ne!(Some(device_code.as_str()), prompt.user_code.as_deref());
    assert!(!prompt.verification_url.contains(&device_code));
    // And the address the person is sent to is one the manifest declares, which is the gate
    // the host applies before showing it.
    let manifest: PluginManifest =
        toml::from_str(ONEDRIVE_SIGN_IN.manifest).expect("the oauth manifest");
    assert!(rd_plugin_host::domain_allowed(
        &url::Url::parse(&prompt.verification_url).expect("URL"),
        manifest.domains()
    ));

    // Nobody has confirmed yet. That is a wait, not a failure.
    let waiting = plugin
        .device_poll(account, prompt.flow_state.as_deref())
        .await
        .expect("poll");
    assert_eq!(
        waiting,
        TokenOutcome::Pending {
            retry_after_seconds: 5
        }
    );
    assert!(server.stored().is_empty(), "a wait stores nothing");

    server.set_case(Case::DeviceGranted);
    let granted = plugin
        .device_poll(account, prompt.flow_state.as_deref())
        .await
        .expect("poll");
    assert_eq!(granted, TokenOutcome::Authorized);

    let exchange = server.requests().pop().expect("one request");
    assert_eq!(exchange.url, ONEDRIVE_SIGN_IN.token_endpoint);
    assert_eq!(
        exchange.field("grant_type"),
        Some("urn:ietf:params:oauth:grant-type:device_code")
    );
    assert_eq!(exchange.field("device_code"), Some(device_code.as_str()));
    assert_eq!(exchange.field("client_id"), Some("{{client_id}}"));
    assert!(exchange.field("code").is_none());

    assert_eq!(
        server.stored(),
        vec![Stored {
            account_id: account,
            access_token: PLACEHOLDER_ACCESS_TOKEN.to_owned(),
            refresh_token: Some(PLACEHOLDER_REFRESH_TOKEN.to_owned()),
            expires_in_seconds: Some(3599),
        }]
    );
}

/// A person who declines at the other screen ends the flow, with the code the interface
/// translates as "not granted" and none of the AADSTS prose beside it.
#[tokio::test]
async fn a_device_sign_in_the_person_declined_ends_the_flow() {
    let bytes = oauth_component();
    let server = TokenEndpoint::new(&ONEDRIVE_SIGN_IN, Case::DeviceDeclined);
    let plugin = storage::sign_in::plugin(&ONEDRIVE_SIGN_IN, &bytes, server.clone());
    let account = AccountId::new();

    let prompt = plugin.device_begin(account, None).await.expect("begin");
    let outcome = plugin
        .device_poll(account, prompt.flow_state.as_deref())
        .await
        .expect("poll");
    let TokenOutcome::Failed { message, category } = outcome else {
        panic!("declining should have failed, got {outcome:?}");
    };
    assert_eq!(category, FailureKind::AuthRequired);
    assert!(message.contains("authorization_declined"), "{message}");
    assert!(
        !message.contains("AADSTS") && !message.contains("denied the authorization"),
        "microsoft's prose reached the message: {message}"
    );
    assert!(server.stored().is_empty());
}

// -- the shape of the three packages ---------------------------------------------------------

/// The three siblings reach only Microsoft, and only the part of it each one needs.
///
/// The shape RD-106-04 decided, asserted against the real manifests: one resolver carrying
/// the provider row, one crawler and one sign-in claiming that row's slug, and no capability
/// beyond HTTP to the provider's own hosts.
#[test]
fn the_three_siblings_reach_only_the_part_of_microsoft_each_one_needs() {
    storage::shape(&ONEDRIVE.shape);
}
