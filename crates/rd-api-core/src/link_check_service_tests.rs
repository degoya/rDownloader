use std::collections::BTreeMap;

use rd_core::{LinkCheckResult, LinkStatus};

use super::{
    Rerouted, direct_message, known_for, manifest_plausible, provider_message, rerouted_document,
    unresolvable_message,
};

/// Serialises the tests that write the process-wide provider registry, the way
/// `providers_handlers` does for the same reason.
static REGISTRY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A hoster row for `host`, the way an installed plugin's manifest contributes one.
///
/// Nothing is compiled into the registry since RD-101-13, so "this host is supported"
/// only exists in a test that installs it.
fn hoster_row(host: &str) -> rd_provider_registry::DynamicProvider {
    rd_provider_registry::DynamicProvider {
        plugin_id: format!("plugin-{host}"),
        spec: rd_provider_registry::ProviderSpec {
            slug: host.replace('.', "-"),
            display_name: host.to_owned(),
            kind: rd_provider_registry::ProviderKind::Hoster,
            credentials: rd_provider_registry::CredentialKind::NoneRequired,
            username_required: false,
            transfer_auth: rd_provider_registry::TransferAuth::None,
            secrets: Vec::new(),
            request_domains: vec![host.to_owned()],
            cookie_scope: None,
            match_hosts: vec![host.to_owned()],
            host_aliases: Vec::new(),
            source: rd_provider_registry::ProviderSource::Plugin,
            plugin_id: Some(format!("plugin-{host}")),
            plugin_version: Some("1.0.0".to_owned()),
        },
    }
}

fn unresolvable_at(address: &str) -> LinkCheckResult {
    LinkCheckResult {
        url: address.parse().expect("URL"),
        status: LinkStatus::Unresolvable,
        file_name: None,
        size: None,
        media: None,
    }
}

/// The addresses behind the four cases the owner's live check of 1.1 ended on (RD-120-18).
///
/// Every one of them reached `collector.check_not_a_file`, and in none of them was the
/// address the cause: the site rule had resolved and handed over an address with no
/// resolver. `downmagaz.net` and `avxhm.se` are release pages, so they stand here through
/// the two addresses they hand over to. What those addresses *are* differs -- ADR 0019
/// found `icerbox.com` to be an unsupported hoster and `nfile.cc` and `dwp.la` to be
/// affiliate cloakers -- and one message covers all of them precisely because it claims
/// nothing beyond the missing resolver, which is what this list pins down.
const REPORTED_HOSTS: [&str; 5] = [
    "https://controlc.com/1a2b3c4d",
    "https://nfile.cc/abcdef123456",
    "https://dwp.la/abcdef123456",
    "https://icerbox.com/abcdef123456",
    "https://vipergirls.to/threads/1234567-a-release",
];

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn a_link_without_declared_attributes_is_asked_exactly_as_before() {
    // The criterion for a manually pasted link: nothing behind it, nothing added.
    let media = r#"{"title":"Some clip","duration_seconds":42}"#;
    assert_eq!(
        known_for(Some(media), &BTreeMap::new()).as_deref(),
        Some(media)
    );
    assert_eq!(known_for(None, &BTreeMap::new()), None);
}

#[test]
fn the_indexer_attributes_join_the_media_metadata_as_a_sibling() {
    let media = r#"{"title":"Some clip","duration_seconds":42}"#;
    let known = known_for(
        Some(media),
        &map(&[("imdb", "tt0111161"), ("imdbscore", "9.3")]),
    )
    .expect("known built");
    let value: serde_json::Value = serde_json::from_str(&known).expect("valid json");
    // The media fields stay where an enricher built before this change looks for them.
    assert_eq!(value["title"], "Some clip");
    assert_eq!(value["duration_seconds"], 42);
    assert_eq!(value["indexer"]["imdb"], "tt0111161");
    assert_eq!(value["indexer"]["imdbscore"], "9.3");
}

#[test]
fn attributes_reach_a_plugin_even_without_media_metadata() {
    let known = known_for(None, &map(&[("imdb", "tt0111161")])).expect("known built");
    let value: serde_json::Value = serde_json::from_str(&known).expect("valid json");
    assert_eq!(value["indexer"]["imdb"], "tt0111161");
}

#[test]
fn nothing_the_attribute_gate_discards_reaches_a_plugin() {
    // Exactly the values `rd_subscription::attributes` refuses: a credential by name, a
    // credential inside a value, a cover address that is not an absolute http(s) URL, and
    // a real password written where the specification wants its flag.
    let known = known_for(
        Some(r#"{"title":"Release"}"#),
        &map(&[
            ("apikey", "deadbeefcafe"),
            ("passkey", "0123456789abcdef"),
            ("rsstoken", "sekrit-token"),
            (
                "nfo",
                "https://indexer.example/nfo?apikey=deadbeefcafe&id=7",
            ),
            ("coverurl", "data:image/png;base64,AAAA"),
            ("password", "hunter2"),
            ("imdbscore", "9.3"),
        ]),
    )
    .expect("known built");
    for secret in [
        "deadbeefcafe",
        "0123456789abcdef",
        "sekrit-token",
        "hunter2",
        "data:image",
    ] {
        assert!(!known.contains(secret), "{secret} leaked: {known}");
    }
    let value: serde_json::Value = serde_json::from_str(&known).expect("valid json");
    // The names whose value was a credential are gone entirely, not merely emptied.
    for name in ["apikey", "passkey", "rsstoken"] {
        assert!(value["indexer"].get(name).is_none(), "{name} kept");
    }
    // A real password becomes the specification's flag, never the secret.
    assert_eq!(value["indexer"]["password"], "1");
    // What is safe still arrives, or the gate would be a wall.
    assert_eq!(value["indexer"]["imdbscore"], "9.3");
}

fn probed(status: LinkStatus) -> LinkCheckResult {
    LinkCheckResult {
        url: "https://1fichier.com/?8x6wertoi51r8vptrojn"
            .parse()
            .expect("URL"),
        status,
        file_name: None,
        size: None,
        media: None,
    }
}

/// The defect RD-109-43 was raised for: one sentence for three different situations.
///
/// Two links of two hosters stood in the LinkGrabber with "Check result missing" while
/// the plugins had in fact answered — `Unknown` — and the download worked without any
/// account. Nothing in the row said which of the three had happened, so nothing could be
/// done about it.
#[test]
fn a_missing_answer_and_an_unclear_one_are_two_different_messages() {
    let missing = provider_message(None, true).expect("a message");
    let unclear = provider_message(Some(&probed(LinkStatus::Unknown)), true).expect("one");
    assert_eq!(missing.code.as_deref(), Some("collector.check_no_result"));
    assert_eq!(unclear.code.as_deref(), Some("collector.check_unknown"));
    assert_ne!(missing.code, unclear.code);
    assert_ne!(missing.text, unclear.text);
}

/// Without an account for the link's own hoster, the message says so.
///
/// This is the situation the report described: the check needs an account, the download
/// does not, and the row claimed neither.
#[test]
fn an_unclear_answer_without_an_account_names_the_account() {
    let message = provider_message(Some(&probed(LinkStatus::Unknown)), false).expect("a message");
    assert_eq!(
        message.code.as_deref(),
        Some("collector.check_unknown_no_account")
    );
    assert!(
        message.text.contains("account"),
        "the English fallback has to name it too: {}",
        message.text
    );
}

/// A conclusive answer leaves nothing behind, whichever way it went.
#[test]
fn a_conclusive_answer_carries_no_message() {
    for status in [LinkStatus::Online, LinkStatus::Offline] {
        assert!(
            provider_message(Some(&probed(status)), true).is_none(),
            "{status:?}"
        );
        assert!(
            provider_message(Some(&probed(status)), false).is_none(),
            "{status:?}"
        );
        assert!(
            direct_message(Some(&probed(status))).is_none(),
            "{status:?}"
        );
    }
}

/// A direct probe that timed out is not a missing HTTP client.
///
/// `probe_direct` answers `Unknown` for a timeout, a refused connection or a proxy in the
/// way, and all of those used to be stored as "No HTTP client available" — the one thing
/// they were not.
#[test]
fn an_unclear_probe_is_not_reported_as_a_missing_client() {
    let no_client = direct_message(None).expect("a message");
    let unclear = direct_message(Some(&probed(LinkStatus::Unknown))).expect("a message");
    assert_eq!(no_client.code.as_deref(), Some("collector.check_no_client"));
    assert_eq!(
        unclear.code.as_deref(),
        Some("collector.check_inconclusive")
    );
    assert_ne!(no_client.text, unclear.text);
}

/// Every code this module hands out is distinct and shaped like the ones the REST layer
/// uses, so none of them can silently collide in the catalogue.
#[test]
fn the_check_codes_are_distinct() {
    let mut codes: Vec<String> = [
        provider_message(None, true),
        provider_message(Some(&probed(LinkStatus::Unknown)), true),
        provider_message(Some(&probed(LinkStatus::Unknown)), false),
        direct_message(None),
        direct_message(Some(&probed(LinkStatus::Unknown))),
        // Both halves of the unresolvable verdict, asked directly so the list stays
        // independent of what happens to be installed while it runs.
        Some(unresolvable_message(true)),
        Some(unresolvable_message(false)),
    ]
    .into_iter()
    .map(|message| message.expect("a message").code.expect("a code"))
    .collect();
    codes.sort();
    let total = codes.len();
    codes.dedup();
    assert_eq!(codes.len(), total, "two situations share one code");
    for code in &codes {
        assert!(code.starts_with("collector.check_"), "{code}");
    }
}

/// A page that is not a file says so, and says it differently from a check that reached
/// no conclusion -- the row it produces cannot be queued, so it has to carry its reason.
///
/// The hoster whose plugin *is* installed is the case `collector.check_not_a_file` was
/// meant for, and it keeps the code: the host is supported and this particular address
/// still serves a page (RD-120-18).
#[test]
fn a_page_on_a_supported_host_is_still_not_a_file() {
    let _guard = REGISTRY_LOCK.lock().expect("lock");
    rd_provider_registry::replace_dynamic(vec![hoster_row("1fichier.com")]);
    let not_a_file = direct_message(Some(&probed(LinkStatus::Unresolvable))).expect("a message");
    let unclear = direct_message(Some(&probed(LinkStatus::Unknown))).expect("a message");
    assert_eq!(
        not_a_file.code.as_deref(),
        Some("collector.check_not_a_file")
    );
    assert_ne!(not_a_file.code, unclear.code);
    rd_provider_registry::replace_dynamic(Vec::new());
}

/// The defect RD-120-18 was raised for: the installation, not the address.
///
/// Each of the four reported cases is a host no installed plugin registers. The message
/// has to name that, because the address is not what the reader can do anything about.
#[test]
fn a_host_without_a_resolver_says_so_rather_than_blaming_the_address() {
    let _guard = REGISTRY_LOCK.lock().expect("lock");
    rd_provider_registry::replace_dynamic(Vec::new());
    for address in REPORTED_HOSTS {
        let message = direct_message(Some(&unresolvable_at(address))).expect("a message");
        assert_eq!(
            message.code.as_deref(),
            Some("collector.check_no_resolver"),
            "{address}"
        );
        // The host travels as the candidate's own address, so the English fallback must
        // not try to spell it out -- that would be the assembled sentence the job forbids.
        assert!(
            !message.text.contains(
                url::Url::parse(address)
                    .expect("URL")
                    .host_str()
                    .expect("host")
            ),
            "{address}: {}",
            message.text
        );
    }
}

/// Installing the plugin is the only thing that changes the verdict, and it changes it.
///
/// Same address, same response, two messages -- which is the whole point: the answer is a
/// property of the installation.
#[test]
fn installing_the_hoster_turns_the_message_back_into_not_a_file() {
    let _guard = REGISTRY_LOCK.lock().expect("lock");
    let address = "https://nfile.cc/abcdef123456";
    rd_provider_registry::replace_dynamic(Vec::new());
    assert_eq!(
        direct_message(Some(&unresolvable_at(address)))
            .expect("a message")
            .code
            .as_deref(),
        Some("collector.check_no_resolver")
    );
    rd_provider_registry::replace_dynamic(vec![hoster_row("nfile.cc")]);
    assert_eq!(
        direct_message(Some(&unresolvable_at(address)))
            .expect("a message")
            .code
            .as_deref(),
        Some("collector.check_not_a_file")
    );
    rd_provider_registry::replace_dynamic(Vec::new());
}

/// `controlc.com` as it actually answered on 2026-09-22, replayed.
///
/// The headers are the recorded ones: HTTP 200, `text/html; charset=UTF-8`, no
/// `Content-Disposition`. That is what made `looks_downloadable` refuse the response and
/// what produced the reported message. Running the real probe against them proves the
/// whole path, not just the last decision: probe, verdict, message.
#[tokio::test]
async fn the_recorded_controlc_response_reports_the_missing_resolver() {
    let page = axum::Router::new().route(
        "/1a2b3c4d",
        axum::routing::any(|| async {
            (
                [
                    (axum::http::header::CONTENT_TYPE, "text/html; charset=UTF-8"),
                    (axum::http::header::CACHE_CONTROL, "private, no-store"),
                ],
                RECORDED_CONTROLC_BODY,
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, page).await;
    });
    let probed = super::probe_direct(
        &reqwest::Client::new(),
        &[],
        format!("http://{address}/1a2b3c4d").parse().expect("URL"),
    )
    .await;
    assert_eq!(probed.status, LinkStatus::Unresolvable);
    // The probe never reads the provider table; only the message does. Taking the lock
    // here, after the last await, keeps it from being held across one.
    let _guard = REGISTRY_LOCK.lock().expect("lock");
    rd_provider_registry::replace_dynamic(Vec::new());
    assert_eq!(
        direct_message(Some(&probed))
            .expect("a message")
            .code
            .as_deref(),
        Some("collector.check_no_resolver")
    );
}

/// The head of the recorded body, kept short: the judgement is made on the headers, and
/// the bytes are here only so the response is the real one rather than an empty stub.
const RECORDED_CONTROLC_BODY: &str = concat!(
    "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"UTF-8\">\n",
    "<title>ControlC Pastebin - The easiest way to host your text</title>\n",
    "</head>\n<body>\n<div id=\"paste\"></div>\n</body>\n</html>\n"
);

/// An indexer link is XML a few kilobytes long, which is not downloadable content and
/// must not be: it is imported. Once it has been re-routed, the verdict is withdrawn.
#[test]
fn a_reroutered_document_keeps_its_place_in_the_list() {
    let page = rerouted_document(&Rerouted::No, Some(probed(LinkStatus::Unresolvable)));
    assert_eq!(page.expect("a result").status, LinkStatus::Unresolvable);
    let nzb = rerouted_document(&Rerouted::Container, Some(probed(LinkStatus::Unresolvable)));
    assert_eq!(nzb.expect("a result").status, LinkStatus::Online);
    let gone = rerouted_document(&Rerouted::Container, Some(probed(LinkStatus::Offline)));
    assert_eq!(gone.expect("a result").status, LinkStatus::Offline);
    let unread = rerouted_document(
        &Rerouted::Torrent(None),
        Some(probed(LinkStatus::Unresolvable)),
    )
    .expect("a result");
    assert_eq!(unread.status, LinkStatus::Online);
    assert_eq!(unread.file_name, None);
}

/// RD-120-68: a re-routed torrent is named after its `info.name`, never after the token
/// its address ends in.
#[test]
fn a_rerouted_torrent_is_named_after_its_info_name() {
    let mut bytes = b"d8:announce31:http://tracker.example/announce4:infod".to_vec();
    bytes.extend_from_slice(b"6:lengthi2048e4:name22:2026.09.16 Weekly Pack");
    bytes.extend_from_slice(b"12:piece lengthi16384e6:pieces20:");
    bytes.extend_from_slice(&[1_u8; 20]);
    bytes.extend_from_slice(b"ee");
    let torrent = rd_torrent::parse_torrent(&bytes).expect("torrent");
    let named = rerouted_document(
        &Rerouted::Torrent(Some(Box::new(torrent))),
        Some(probed(LinkStatus::Unresolvable)),
    )
    .expect("a result");
    assert_eq!(named.status, LinkStatus::Online);
    assert_eq!(named.file_name.as_deref(), Some("2026.09.16 Weekly Pack"));
    assert_eq!(named.size.map(rd_core::ByteCount::get), Some(2_048));
}

#[test]
fn a_declared_manifest_type_is_always_worth_reading() {
    for kind in [
        "application/vnd.apple.mpegurl",
        "application/x-mpegurl; charset=utf-8",
        "application/dash+xml",
    ] {
        assert!(manifest_plausible(Some(kind), None), "{kind}");
    }
}

#[test]
fn a_small_text_response_is_worth_reading() {
    // The case this exists for: a signed CDN address with no extension.
    assert!(manifest_plausible(
        Some("text/plain; charset=utf-8"),
        Some(4_096)
    ));
}

#[test]
fn a_large_or_binary_response_is_not() {
    // Reading a slice of every video in the queue is exactly what the gate prevents.
    assert!(!manifest_plausible(Some("video/mp4"), Some(4_096)));
    assert!(!manifest_plausible(
        Some("text/plain"),
        Some(64 * 1024 * 1024)
    ));
    assert!(!manifest_plausible(Some("application/zip"), None));
    assert!(!manifest_plausible(Some("text/html"), Some(2_048)));
}
