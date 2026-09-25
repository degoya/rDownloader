//! Every entry point that hands a plugin an address refuses one carrying a marker (RD-120-66).
//!
//! `foreign_address_wire.rs` proves the leak and its end on the wire for the plugins that had
//! one. This file walks the host's doors instead, one test each, with a real component of the
//! plugin type behind it and the application's own host in front of the wire: the address is
//! refused (or not claimed) with the stable code, and nothing arrives anywhere.
//!
//! The intake parser's `normalize` is left out on purpose: the bundled parsers answer it without
//! a request and have no credential, so no test of it could fail. Its door answers "no rewrite"
//! for a marked address and the resolver behind it refuses the address with the code.

#[path = "support/notifier_wire.rs"]
mod support;

use std::sync::Arc;

use rd_plugin_api::{ClientIdentity, ResolveRequest, Resolver};
use rd_plugin_host::{
    ComponentResolver, PluginManifest, TransferBackend, TransferState, TransferTarget,
    artifact::component,
    extension::{
        CacheAnswer, CacheKind, CacheQuery, FolderCrawler, RemoteJobPlugin, RemoteJobSource,
        StreamTransformProvider,
    },
};
use support::{Service, account, register_providers, service_over, wire};
use tokio_util::sync::CancellationToken;

const CODE: &str = "plugin.address_carries_marker";

fn manifest(plugin: &str) -> PluginManifest {
    let path = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("dir"))
        .join("../../plugins")
        .join(plugin)
        .join("manifest.toml");
    toml::from_str(&std::fs::read_to_string(path).expect("manifest")).expect("parse manifest")
}

fn anonymous() -> ClientIdentity {
    ClientIdentity {
        account_id: None,
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

async fn setup() -> (support::Wire, Service, tempfile::TempDir) {
    register_providers();
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let parts = service_over(directory.path(), &wire).await;
    (wire, parts, directory)
}

fn nothing_arrived(wire: &support::Wire) {
    let arrived = wire.arrived.lock().expect("arrived").clone();
    assert!(arrived.is_empty(), "{arrived:?}");
}

#[tokio::test]
async fn resolve_and_claims_of_an_installed_resolver() {
    let (wire, parts, _directory) = setup().await;
    let resolver = ComponentResolver::new(
        manifest("box"),
        &component("rd-plugin-box"),
        parts.service.host(),
    )
    .expect("compile");
    let marked = "https://app.box.com/s/abc?x=%7B%7Bsecret:box_access_token%7D%7D";

    assert!(!resolver.guest_claims(marked).await.expect("claims"));
    let failure = resolver
        .resolve(ResolveRequest {
            url: marked.parse().expect("url"),
            client: anonymous(),
        })
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some(CODE));
    nothing_arrived(&wire);
}

#[tokio::test]
async fn a_link_check_answers_unknown_for_a_marked_address() {
    let (wire, parts, _directory) = setup().await;
    let id = account(&parts, "linksnappy", "owner", "pw").await;
    let marked: url::Url = "https://rapidgator.net/file/a?x={{username}}"
        .parse()
        .expect("url");

    let results = parts
        .service
        .check(id, vec![marked.clone()])
        .await
        .expect("check");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].url, marked);
    assert_eq!(results[0].status, rd_core::LinkStatus::Unknown);
    nothing_arrived(&wire);
}

#[tokio::test]
async fn claims_and_crawl_of_a_folder_crawler() {
    let (wire, parts, _directory) = setup().await;
    let id = account(&parts, "box", "owner", "token").await;
    let crawler = FolderCrawler::new(
        manifest("box-crawler"),
        &component("rd-plugin-box-crawler"),
        Some(parts.service.host()),
    )
    .expect("compile");
    let marked = "https://app.box.com/s/abc?x={{secret:box_access_token}}";

    assert!(!crawler.claims(marked).await.expect("claims"));
    let error = crawler.crawl(marked, Some(id)).await.expect_err("refused");
    let failure = error.downcast_ref::<rd_core::Failure>().expect("a failure");
    assert_eq!(failure.code.as_deref(), Some(CODE));
    nothing_arrived(&wire);
}

#[tokio::test]
async fn claims_and_resolve_of_a_stream_transform() {
    let (wire, parts, _directory) = setup().await;
    let provider = StreamTransformProvider::new(
        manifest("example-stream-transform"),
        &component("rd-plugin-example-stream-transform"),
        Some(parts.service.host()),
    )
    .expect("compile");
    let marked = "https://transform.example.invalid/v?x={{secret}}";

    assert!(!provider.claims(marked).await.expect("claims"));
    let answer = provider
        .resolve(&ResolveRequest {
            url: marked.parse().expect("url"),
            client: anonymous(),
        })
        .await
        .expect("no trap");
    let Err(failure) = answer else {
        panic!("a marked address is refused");
    };
    assert_eq!(failure.code.as_deref(), Some(CODE));
    nothing_arrived(&wire);
}

#[tokio::test]
async fn claims_and_identify_of_a_remote_job_address_and_magnet() {
    let (wire, parts, _directory) = setup().await;
    let plugin = RemoteJobPlugin::new(
        manifest("torbox-jobs"),
        &component("rd-plugin-torbox-jobs"),
        Some(parts.service.host()),
    )
    .expect("compile");
    for source in [
        RemoteJobSource::Address("https://evil.example/f?x={{secret:torbox_api_key}}".to_owned()),
        RemoteJobSource::Magnet(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567\
             &tr=http://t.example/a%3Fk%3D%7B%7Bsecret:torbox_api_key%7D%7D"
                .to_owned(),
        ),
    ] {
        assert!(!plugin.claims(&source).await.expect("claims"), "{source:?}");
        let refusal = plugin
            .identify(&source)
            .await
            .expect("no trap")
            .expect_err("refused");
        assert_eq!(refusal.code.as_deref(), Some(CODE), "{source:?}");
    }
    nothing_arrived(&wire);
}

/// RD-130-11: a marked source in a cache check never reaches the guest, and answers
/// `unknown` at its own position -- a batch of nothing but marked sources makes no call at all.
#[tokio::test]
async fn check_cached_of_a_remote_job_answers_unknown_for_a_marked_source() {
    let (wire, parts, _directory) = setup().await;
    // No account is read: nothing reaches the guest, so no credential could be expanded.
    let id = rd_core::AccountId::new();
    let plugin = RemoteJobPlugin::new(
        manifest("torbox-jobs"),
        &component("rd-plugin-torbox-jobs"),
        Some(parts.service.host()),
    )
    .expect("compile");
    let queries = [
        CacheQuery {
            source: RemoteJobSource::Address(
                "https://rapidgator.net/file/a?x={{secret:torbox_api_key}}".to_owned(),
            ),
            kind: CacheKind::Hoster,
        },
        CacheQuery {
            source: RemoteJobSource::Magnet(
                "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567\
                 &tr=http://t.example/a%3Fk%3D%7B%7Bsecret:torbox_api_key%7D%7D"
                    .to_owned(),
            ),
            kind: CacheKind::Torrent,
        },
    ];
    let answers = plugin
        .check_cached(id, &queries)
        .await
        .expect("no trap")
        .expect("no refusal");
    assert_eq!(
        answers,
        vec![CacheAnswer::unknown(), CacheAnswer::unknown()]
    );
    nothing_arrived(&wire);
}

#[tokio::test]
async fn probe_of_a_transfer_backend() {
    let backend = TransferBackend::new(
        manifest("example-transfer"),
        &component("rd-plugin-example-transfer"),
    )
    .expect("compile");
    let directory = tempfile::tempdir().expect("tempdir");
    let part = rd_files::PartFile::open(directory.path().join("download.part"), None)
        .await
        .expect("part file");
    let state = TransferState::new(
        TransferTarget {
            part,
            committed: 0,
            total: None,
        },
        CancellationToken::new(),
        rd_limits::ScopedLimiter::unlimited(),
        backend.manifest().capabilities.net_stream.clone(),
        Arc::new(rd_http::tls_client_config(&[]).expect("tls")),
        Arc::new(|_, _| {}),
    );

    let failure = backend
        .probe(
            state,
            "example+tcp://127.0.0.1:9/{{secret}}".to_owned(),
            None,
        )
        .await
        .expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some(CODE));
}
