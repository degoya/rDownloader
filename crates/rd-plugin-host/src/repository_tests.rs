//! Tests for [`super`]: refresh, cache and offline rule, replay, withdrawals, downloads that do
//! not match their index, and updates — against a real database and an in-memory fetcher.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use chrono::{Duration, Utc};
use rd_sign::SigningKey;
use tempfile::TempDir;

use super::*;
use crate::{
    PluginVerifier, format_package_digest,
    index::{PLUGIN_INDEX_SCHEMA_VERSION, Revocations, RevokedKey, describe_package},
    key_fingerprint, package_plugin, public_key_base64,
    tests::fixture_manifest,
};

const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";
const REPOSITORY_KEY_ID: &str = "test-repository";
const PACKAGE_BASE: &str = "https://github.com/degoya/rDownloader/releases/latest/download/";

/// Serves whatever the test put in; everything else is "offline".
#[derive(Default)]
struct MapFetcher(Mutex<HashMap<String, Vec<u8>>>);

impl MapFetcher {
    fn serve(&self, url: &str, bytes: Vec<u8>) {
        self.0.lock().expect("lock").insert(url.to_owned(), bytes);
    }

    fn go_offline(&self) {
        self.0.lock().expect("lock").clear();
    }
}

#[async_trait::async_trait]
impl Fetcher for MapFetcher {
    async fn fetch(&self, url: &url::Url, limit: u64) -> anyhow::Result<Vec<u8>> {
        let bytes = self
            .0
            .lock()
            .expect("lock")
            .get(url.as_str())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("offline: {url}"))?;
        anyhow::ensure!(bytes.len() as u64 <= limit, "larger than {limit}");
        Ok(bytes)
    }
}

fn repository_key() -> SigningKey {
    SigningKey::from_bytes(&[31; 32])
}

fn plugin_key() -> SigningKey {
    SigningKey::from_bytes(&[32; 32])
}

fn official_trust() -> TrustStore {
    let trust = TrustStore::new();
    trust
        .trust(
            REPOSITORY_KEY_ID.to_owned(),
            repository_key().verifying_key(),
        )
        .expect("trust");
    trust
}

/// A signed fixture package of `version`.
fn package(version: &str) -> Vec<u8> {
    let manifest = fixture_manifest(&public_key_base64(&plugin_key()))
        .replace(r#"version = "1.2.3""#, &format!(r#"version = "{version}""#));
    package_plugin(
        manifest.as_bytes(),
        EMPTY_COMPONENT,
        &[],
        Some(&plugin_key()),
    )
    .expect("package")
}

fn file_name(version: &str) -> String {
    format!("fixture-{version}.rdplug")
}

fn signed_index(sequence: u64, packages: &[(&str, &[u8])], revoked: Revocations) -> Vec<u8> {
    let index = PluginIndex {
        schema_version: PLUGIN_INDEX_SCHEMA_VERSION,
        sequence,
        issued_at: Utc::now() - Duration::minutes(1),
        not_after: Utc::now() + Duration::days(30),
        packages: packages
            .iter()
            .map(|(version, bytes)| {
                describe_package(
                    bytes,
                    file_name(version),
                    Some(format!("Notes for {version}")),
                )
                .expect("describe")
            })
            .collect(),
        revoked,
    };
    index::sign(REPOSITORY_KEY_ID, &repository_key(), &index).expect("sign")
}

struct Fixture {
    _directory: TempDir,
    root: PathBuf,
    database: Database,
    installer: PluginInstaller,
    fetcher: Arc<MapFetcher>,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_mode(false).await
    }

    /// `development_mode` is the verifier's: whether it takes unsigned packages.
    async fn with_mode(development_mode: bool) -> Self {
        let directory = TempDir::new().expect("temp");
        let database = Database::open(directory.path().join("db.sqlite"))
            .await
            .expect("database");
        let verifier = PluginVerifier::new(development_mode);
        verifier
            .trust_key("fixture-v1".to_owned(), plugin_key().verifying_key())
            .expect("trust");
        let installer = PluginInstaller::new(directory.path().join("plugins"), verifier);
        Self {
            root: directory.path().join("plugin-repositories"),
            _directory: directory,
            database,
            installer,
            fetcher: Arc::new(MapFetcher::default()),
        }
    }

    /// A service as a fresh start builds it: nothing in memory, everything read back.
    fn service(&self) -> PluginRepositoryService {
        PluginRepositoryService::with_fetcher(
            self.database.clone(),
            self.root.clone(),
            self.installer.clone(),
            self.fetcher.clone(),
            Some(official_trust()),
        )
    }

    fn serve_index(&self, bytes: Vec<u8>) {
        self.fetcher.serve(OFFICIAL_INDEX_URL, bytes);
    }

    fn serve_package(&self, version: &str, bytes: Vec<u8>) {
        self.fetcher
            .serve(&format!("{PACKAGE_BASE}{}", file_name(version)), bytes);
    }

    async fn official(&self) -> PluginRepository {
        self.database
            .plugin_repository(OFFICIAL_REPOSITORY_ID)
            .await
            .expect("read")
            .expect("official")
    }
}

fn only_ok(outcomes: &[(String, Result<(), RepositoryError>)]) {
    for (id, outcome) in outcomes {
        assert!(outcome.is_ok(), "{id}: {outcome:?}");
    }
}

#[tokio::test]
async fn a_refresh_adopts_the_index_and_an_offline_start_uses_only_the_verified_cache() {
    let fixture = Fixture::new().await;
    let bytes = package("1.2.4");
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &bytes)],
        Revocations::default(),
    ));
    let service = fixture.service();
    only_ok(&service.refresh_all().await);
    assert_eq!(fixture.official().await.sequence, Some(5));
    assert_eq!(service.offers().await.expect("offers").len(), 1);

    // Offline, a new start still offers what the cache holds, because it verifies.
    fixture.fetcher.go_offline();
    let restarted = fixture.service();
    restarted.load().await;
    let offers = restarted.offers().await.expect("offers");
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].entry.version, "1.2.4");
    assert_eq!(offers[0].compatibility, PackageCompatibility::Compatible);
    let failed = restarted.refresh_all().await;
    assert_eq!(
        failed[0].1.as_ref().expect_err("offline").code(),
        "plugin_repository.download_failed"
    );
    // A failed refresh keeps the cache and the floor.
    assert_eq!(restarted.offers().await.expect("offers").len(), 1);
    assert_eq!(fixture.official().await.sequence, Some(5));
}

#[tokio::test]
async fn a_tampered_cache_is_deleted_and_offers_nothing() {
    let fixture = Fixture::new().await;
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &package("1.2.4"))],
        Revocations::default(),
    ));
    only_ok(&fixture.service().refresh_all().await);
    let cache = fixture.root.join("index-official.json");
    let text = std::fs::read_to_string(&cache).expect("cache");
    std::fs::write(&cache, text.replacen("1.2.4", "9.9.9", 1)).expect("tamper");

    let restarted = fixture.service();
    restarted.load().await;
    assert!(restarted.offers().await.expect("offers").is_empty());
    assert!(!cache.exists(), "a cache that does not verify was kept");
}

#[tokio::test]
async fn a_replayed_or_expired_index_is_refused_and_the_same_one_again_is_not() {
    let fixture = Fixture::new().await;
    let older = signed_index(4, &[], Revocations::default());
    let newer = signed_index(6, &[], Revocations::default());
    let service = fixture.service();
    fixture.serve_index(newer);
    only_ok(&service.refresh_all().await);
    // Unchanged is the ordinary case, not a replay.
    only_ok(&service.refresh_all().await);

    fixture.serve_index(older);
    let outcome = service.refresh_all().await;
    assert_eq!(
        outcome[0].1.as_ref().expect_err("replay").code(),
        "plugin_index.stale"
    );
    let row = fixture.official().await;
    assert_eq!(row.sequence, Some(6));
    assert_eq!(row.last_error.as_deref(), Some("plugin_index.stale"));

    let expired = PluginIndex {
        schema_version: PLUGIN_INDEX_SCHEMA_VERSION,
        sequence: 9,
        issued_at: Utc::now() - Duration::days(40),
        not_after: Utc::now() - Duration::days(1),
        packages: Vec::new(),
        revoked: Revocations::default(),
    };
    fixture.serve_index(index::sign(REPOSITORY_KEY_ID, &repository_key(), &expired).expect("sign"));
    let outcome = service.refresh_all().await;
    assert_eq!(
        outcome[0].1.as_ref().expect_err("expired").code(),
        "plugin_index.stale"
    );
    assert_eq!(fixture.official().await.sequence, Some(6));
}

#[tokio::test]
async fn an_index_signed_by_another_key_is_refused() {
    let fixture = Fixture::new().await;
    let impostor = SigningKey::from_bytes(&[3; 32]);
    let index = PluginIndex {
        schema_version: PLUGIN_INDEX_SCHEMA_VERSION,
        sequence: 5,
        issued_at: Utc::now() - Duration::minutes(1),
        not_after: Utc::now() + Duration::days(3),
        packages: Vec::new(),
        revoked: Revocations::default(),
    };
    fixture.serve_index(index::sign(REPOSITORY_KEY_ID, &impostor, &index).expect("sign"));
    let outcome = fixture.service().refresh_all().await;
    assert_eq!(
        outcome[0].1.as_ref().expect_err("impostor").code(),
        "plugin_index.bad_signature"
    );
    assert_eq!(fixture.official().await.sequence, None);
}

#[tokio::test]
async fn a_package_that_does_not_match_its_index_is_refused() {
    let fixture = Fixture::new().await;
    let genuine = package("1.2.4");
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &genuine)],
        Revocations::default(),
    ));
    let service = fixture.service();
    only_ok(&service.refresh_all().await);
    let id = "019d0000-0000-7000-8000-00000000abcd";

    // Same size, other bytes: a different version's package served under this name.
    let mut other = package("1.2.5");
    other.resize(genuine.len(), 0);
    fixture.serve_package("1.2.4", other);
    let error = service
        .download(OFFICIAL_REPOSITORY_ID, id, "1.2.4")
        .await
        .expect_err("mismatch");
    assert_eq!(error.code(), "plugin_repository.digest_mismatch");

    fixture.serve_package("1.2.4", genuine.clone());
    let (offer, bytes) = service
        .download(OFFICIAL_REPOSITORY_ID, id, "1.2.4")
        .await
        .expect("download");
    assert_eq!(bytes, genuine);
    assert_eq!(
        offer.entry.release_notes.as_deref(),
        Some("Notes for 1.2.4")
    );
    let error = service
        .download(OFFICIAL_REPOSITORY_ID, id, "7.0.0")
        .await
        .expect_err("not offered");
    assert_eq!(error.code(), "plugin_repository.not_offered");
}

#[tokio::test]
async fn an_update_is_newer_than_every_installed_version_and_from_the_same_key() {
    let fixture = Fixture::new().await;
    fixture
        .installer
        .install_bytes(package("1.2.3"))
        .await
        .expect("install");
    let newer = package("1.2.4");
    fixture.serve_index(signed_index(
        5,
        &[("1.2.3", &package("1.2.3")), ("1.2.4", &newer)],
        Revocations::default(),
    ));
    let service = fixture.service();
    only_ok(&service.refresh_all().await);
    let updates = service.updates().await.expect("updates");
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].installed_version, "1.2.3");
    assert_eq!(updates[0].offer.entry.version, "1.2.4");
    assert_eq!(updates[0].policy, UpdatePolicy::Manual);

    struct Everything;
    #[async_trait::async_trait]
    impl UpdatePolicySource for Everything {
        async fn policy(&self, _plugin_id: &str) -> UpdatePolicy {
            UpdatePolicy::Automatic
        }
    }
    service.set_update_policy(Arc::new(Everything));
    assert_eq!(
        service.updates().await.expect("updates")[0].policy,
        UpdatePolicy::Automatic
    );

    // Disabling the repository takes its offers away and leaves the installed version alone.
    fixture
        .database
        .update_plugin_repository(OFFICIAL_REPOSITORY_ID.to_owned(), Some(false), None)
        .await
        .expect("disable");
    assert!(service.updates().await.expect("updates").is_empty());
    assert!(service.offers().await.expect("offers").is_empty());
    assert!(
        service.refresh_all().await.is_empty(),
        "a disabled repository was refreshed"
    );
    assert_eq!(
        fixture
            .installer
            .list_installed()
            .await
            .expect("installed")
            .len(),
        1
    );
}

#[tokio::test]
async fn the_official_index_withdraws_digests_and_keys() {
    let fixture = Fixture::new().await;
    let installed = package("1.2.3");
    fixture
        .installer
        .install_bytes(installed.clone())
        .await
        .expect("install");
    let digest = crate::preview::archive_digest(&installed).expect("digest");
    let leaked = SigningKey::from_bytes(&[44; 32]).verifying_key();
    fixture
        .installer
        .verifier()
        .trust_key("leaked-v1".to_owned(), leaked)
        .expect("trust");
    let revoked = Revocations {
        package_digests: vec![format_package_digest(&digest)],
        keys: vec![RevokedKey {
            key_id: "leaked-v1".to_owned(),
            fingerprint: key_fingerprint(&leaked),
        }],
    };
    fixture.serve_index(signed_index(5, &[], revoked));
    only_ok(&fixture.service().refresh_all().await);

    let verifier = fixture.installer.verifier();
    assert!(verifier.is_package_revoked(&digest).expect("revoked"));
    assert!(
        verifier
            .is_key_withdrawn(&key_fingerprint(&leaked))
            .expect("withdrawn")
    );
    assert!(!verifier.is_trusted("leaked-v1").expect("trusted"));
    let rows = fixture
        .database
        .list_plugin_digest_revocations()
        .await
        .expect("rows");
    assert_eq!(rows.len(), 1);
    // Named after the installed plugin, so its card can show the withdrawal.
    assert_eq!(rows[0].version.as_deref(), Some("1.2.3"));
    assert_eq!(rows[0].plugin_name.as_deref(), Some("Fixture"));
    assert_eq!(
        fixture
            .database
            .list_plugin_withdrawn_keys()
            .await
            .expect("keys")
            .len(),
        1
    );
}

#[tokio::test]
async fn a_third_party_repository_needs_its_key_and_withdraws_only_what_it_delivered() {
    let fixture = Fixture::new().await;
    let installed = package("1.2.3");
    fixture
        .installer
        .install_bytes(installed.clone())
        .await
        .expect("install");
    let digest = crate::preview::archive_digest(&installed).expect("digest");
    let url = "https://plugins.example.test/index.json";
    let community = SigningKey::from_bytes(&[55; 32]);
    let index = PluginIndex {
        schema_version: PLUGIN_INDEX_SCHEMA_VERSION,
        sequence: 3,
        issued_at: Utc::now() - Duration::minutes(1),
        not_after: Utc::now() + Duration::days(3),
        packages: Vec::new(),
        revoked: Revocations {
            // The bundled package, which this repository never delivered.
            package_digests: vec![format_package_digest(&digest)],
            keys: vec![RevokedKey {
                key_id: "fixture-v1".to_owned(),
                fingerprint: key_fingerprint(&plugin_key().verifying_key()),
            }],
        },
    };
    fixture.fetcher.serve(
        url,
        index::sign("community-v1", &community, &index).expect("sign"),
    );
    let service = fixture.service();

    let wrong = public_key_base64(&SigningKey::from_bytes(&[56; 32]));
    let error = service.probe(url, &wrong).await.expect_err("wrong key");
    assert_eq!(error.code(), "plugin_repository.key_does_not_sign");
    assert_eq!(
        service
            .probe(
                "http://plugins.example.test/index.json",
                &public_key_base64(&community)
            )
            .await
            .expect_err("plain http")
            .code(),
        "plugin_repository.url_invalid"
    );

    let probe = service
        .probe(url, &public_key_base64(&community))
        .await
        .expect("probe");
    assert_eq!(probe.key_id, "community-v1");
    assert_eq!(
        probe.fingerprint,
        key_fingerprint(&community.verifying_key())
    );
    let added = service
        .add("Community".to_owned(), probe)
        .await
        .expect("add");
    assert_eq!(added.kind, "third_party");
    let again = service
        .probe(url, &public_key_base64(&community))
        .await
        .expect("probe");
    assert_eq!(
        service
            .add("Twice".to_owned(), again)
            .await
            .expect_err("twice")
            .code(),
        "plugin_repository.already_added"
    );

    let verifier = fixture.installer.verifier();
    assert!(!verifier.is_package_revoked(&digest).expect("revoked"));
    assert!(verifier.is_trusted("fixture-v1").expect("trusted"));
    assert!(
        fixture
            .database
            .list_plugin_digest_revocations()
            .await
            .expect("rows")
            .is_empty()
    );
    assert!(service.remove(&added.id).await.expect("remove"));
    assert!(
        !service
            .remove(OFFICIAL_REPOSITORY_ID)
            .await
            .expect("official")
    );
}

#[tokio::test]
async fn the_refresh_interval_is_bounded_and_defaults() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    assert_eq!(service.refresh_hours().await, DEFAULT_REFRESH_HOURS);
    service.set_refresh_hours(6).await.expect("set");
    assert_eq!(service.refresh_hours().await, 6);
    fixture
        .database
        .set_setting(
            SETTINGS_KEY.to_owned(),
            serde_json::json!({ "refresh_hours": 0 }),
        )
        .await
        .expect("set");
    assert_eq!(service.refresh_hours().await, DEFAULT_REFRESH_HOURS);
}

#[test]
fn a_repository_address_is_plain_https() {
    assert!(parse_https("https://plugins.example.test/index.json").is_ok());
    for bad in [
        "http://plugins.example.test/index.json",
        "https://user:pw@plugins.example.test/index.json",
        "https://plugins.example.test/index.json#x",
        "file:///etc/passwd",
        "not a url",
    ] {
        assert!(parse_https(bad).is_err(), "{bad}");
    }
}

#[test]
fn only_the_official_repository_withdraws_everything() {
    let mut row = PluginRepository {
        id: OFFICIAL_REPOSITORY_ID.to_owned(),
        kind: "official".to_owned(),
        name: "rDownloader".to_owned(),
        url: None,
        key_id: None,
        public_key: None,
        fingerprint: None,
        enabled: true,
        sequence: None,
        issued_at: None,
        last_checked_at: None,
        last_success_at: None,
        last_error: None,
        created_at: String::new(),
    };
    assert_eq!(withdrawal_scope(&row), WithdrawalScope::Everything);
    row.kind = "third_party".to_owned();
    assert_eq!(withdrawal_scope(&row), WithdrawalScope::DeliveredOnly);
}

/// The malicious-repository cases of the plugin-trust review (`docs/security/plugin-trust.md`).
#[path = "repository_trust_tests.rs"]
mod trust;
