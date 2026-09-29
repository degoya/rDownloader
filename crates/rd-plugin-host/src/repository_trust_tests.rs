//! Malicious-repository fixtures (RD-140-01, RD-170-06): each test names its threat in
//! `docs/security/plugin-trust.md`. The parent module holds the cases of a broken repository.

use super::*;
use crate::{
    VerifyError,
    index::{IndexPackage, Publisher},
    preview::archive_digest,
};

const FIXTURE_ID: &str = "019d0000-0000-7000-8000-00000000abcd";
const COMMUNITY_URL: &str = "https://plugins.example.test/index.json";
const COMMUNITY_BASE: &str = "https://plugins.example.test/";

struct AllAutomatic;

#[async_trait::async_trait]
impl UpdatePolicySource for AllAutomatic {
    async fn policy(&self, _plugin_id: &str) -> UpdatePolicy {
        UpdatePolicy::Automatic
    }
}

/// The third-party repository's own index key.
fn community_key() -> SigningKey {
    SigningKey::from_bytes(&[55; 32])
}

/// The key the third party signs its *plugins* with, which the person trusted for a plugin of
/// the third party's own.
fn community_plugin_key() -> SigningKey {
    SigningKey::from_bytes(&[57; 32])
}

/// The fixture plugin at `version`, signed by `key` under `key_id`, with `edit` applied to its
/// manifest.
fn package_with(
    key: &SigningKey,
    key_id: &str,
    version: &str,
    edit: impl FnOnce(String) -> String,
) -> Vec<u8> {
    let manifest = edit(
        fixture_manifest(&public_key_base64(key))
            .replace(r#"version = "1.2.3""#, &format!(r#"version = "{version}""#))
            .replace(
                r#"key_id = "fixture-v1""#,
                &format!(r#"key_id = "{key_id}""#),
            ),
    );
    package_plugin(manifest.as_bytes(), EMPTY_COMPONENT, &[], Some(key)).expect("package")
}

fn entry(bytes: &[u8], version: &str) -> IndexPackage {
    describe_package(bytes, file_name(version), None).expect("describe")
}

fn unsigned_index(sequence: u64, packages: Vec<IndexPackage>, revoked: Revocations) -> PluginIndex {
    PluginIndex {
        schema_version: PLUGIN_INDEX_SCHEMA_VERSION,
        sequence,
        issued_at: Utc::now() - Duration::minutes(1),
        not_after: Utc::now() + Duration::days(30),
        packages,
        revoked,
    }
}

fn community_index(sequence: u64, packages: Vec<IndexPackage>, revoked: Revocations) -> Vec<u8> {
    index::sign(
        "community-v1",
        &community_key(),
        &unsigned_index(sequence, packages, revoked),
    )
    .expect("sign")
}

/// Approves the community repository's key and adds it, as the settings dialog does.
async fn add_community(
    fixture: &Fixture,
    service: &PluginRepositoryService,
    index: Vec<u8>,
) -> PluginRepository {
    fixture.fetcher.serve(COMMUNITY_URL, index);
    let probe = service
        .probe(COMMUNITY_URL, &public_key_base64(&community_key()))
        .await
        .expect("probe");
    service
        .add("Community".to_owned(), probe)
        .await
        .expect("add")
}

fn serve_community_package(fixture: &Fixture, version: &str, bytes: Vec<u8>) {
    fixture
        .fetcher
        .serve(&format!("{COMMUNITY_BASE}{}", file_name(version)), bytes);
}

async fn install_fixture(fixture: &Fixture) {
    fixture
        .installer
        .install_bytes(package("1.2.3"))
        .await
        .expect("install");
}

async fn installed_count(fixture: &Fixture) -> usize {
    let installed = fixture.installer.list_installed().await;
    installed.expect("installed").len()
}

fn outcome_of<'a>(
    outcomes: &'a [(String, Result<(), RepositoryError>)],
    id: &str,
) -> &'a Result<(), RepositoryError> {
    &outcomes
        .iter()
        .find(|(repository, _)| repository == id)
        .expect("an outcome for the repository")
        .1
}

/// T-PUB: a third party whose plugin key the person trusts for one plugin lists a package it
/// signed itself under the fingerprint of the key an installed plugin is signed with. The
/// update check reads the entry, so the claim makes it an "update" — installed automatically
/// where the policy says so. The download holds the entry to the package's signature.
#[tokio::test]
async fn an_entry_that_lies_about_its_publisher_is_refused_before_it_can_install() {
    let fixture = Fixture::new().await;
    install_fixture(&fixture).await;
    fixture
        .installer
        .verifier()
        .trust_key(
            "community-plugins-v1".to_owned(),
            community_plugin_key().verifying_key(),
        )
        .expect("trust");
    let hostile = package_with(
        &community_plugin_key(),
        "community-plugins-v1",
        "1.2.4",
        |manifest| manifest,
    );
    let mut lying = entry(&hostile, "1.2.4");
    lying.publisher = Publisher {
        key_id: "fixture-v1".to_owned(),
        fingerprint: key_fingerprint(&plugin_key().verifying_key()),
        author: "Fixture Author".to_owned(),
    };
    let service = fixture.service();
    service.set_update_policy(Arc::new(AllAutomatic));
    let repository = add_community(
        &fixture,
        &service,
        community_index(1, vec![lying], Revocations::default()),
    )
    .await;
    serve_community_package(&fixture, "1.2.4", hostile.clone());

    let updates = service.updates().await.expect("updates");
    assert_eq!(updates.len(), 1, "the claim reaches the update list");
    assert_eq!(updates[0].policy, UpdatePolicy::Automatic);
    let error = service
        .download(&repository.id, FIXTURE_ID, "1.2.4")
        .await
        .expect_err("a lying entry");
    assert_eq!(error.code(), "plugin_repository.package_mismatch");
    assert!(error.to_string().contains("signing key"), "{error}");
    // Installed straight away the package would have gone in, because its key is trusted:
    // exactly why the entry has to be held to it before anything installs.
    assert!(fixture.installer.verifier().verify_bytes(&hostile).is_ok());
    assert_eq!(installed_count(&fixture).await, 1);
}

/// T-PUB, honest variant: a newer version under another key is a different publisher's plugin
/// that shares an id. It is offered with its own preview, never as an update.
#[tokio::test]
async fn a_newer_version_under_another_key_is_offered_but_never_an_update() {
    let fixture = Fixture::new().await;
    install_fixture(&fixture).await;
    let other = package_with(
        &community_plugin_key(),
        "community-plugins-v1",
        "9.0.0",
        |manifest| manifest,
    );
    let service = fixture.service();
    service.set_update_policy(Arc::new(AllAutomatic));
    add_community(
        &fixture,
        &service,
        community_index(1, vec![entry(&other, "9.0.0")], Revocations::default()),
    )
    .await;
    assert!(service.updates().await.expect("updates").is_empty());
    assert_eq!(service.offers().await.expect("offers").len(), 1);
}

/// T-PERM: an entry that hides a permission its package asks for is refused; a truthful one
/// that widens the permissions is marked, and the automatic update leaves it for a click.
#[tokio::test]
async fn a_hidden_permission_is_refused_and_a_new_one_is_marked() {
    let fixture = Fixture::new().await;
    install_fixture(&fixture).await;
    let wider = package_with(&plugin_key(), "fixture-v1", "1.2.4", |manifest| {
        manifest.replace(
            r#"domains = ["example.test"]"#,
            r#"domains = ["example.test", "collector.example.net"]"#,
        )
    });
    fixture.serve_package("1.2.4", wider.clone());
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &wider)],
        Revocations::default(),
    ));
    let service = fixture.service();
    service.set_update_policy(Arc::new(AllAutomatic));
    only_ok(&service.refresh_all().await);
    let updates = service.updates().await.expect("updates");
    assert_eq!(updates.len(), 1);
    assert!(updates[0].adds_permissions, "a new domain went unmarked");
    // And named: the one new domain, nothing the installed version already holds (RD-160-09).
    assert_eq!(
        updates[0].added_permissions.http_domains,
        ["collector.example.net"]
    );
    assert!(updates[0].added_permissions.granted.is_empty());
    let automatic = service.automatic_updates().await.expect("automatic");
    assert!(
        automatic.is_empty(),
        "a widening update would install itself"
    );
    service
        .download(OFFICIAL_REPOSITORY_ID, FIXTURE_ID, "1.2.4")
        .await
        .expect("a truthful entry downloads");

    let mut hidden = entry(&wider, "1.2.4");
    hidden.permissions.http_domains = vec!["example.test".to_owned()];
    let index = unsigned_index(6, vec![hidden], Revocations::default());
    fixture.serve_index(index::sign(REPOSITORY_KEY_ID, &repository_key(), &index).expect("sign"));
    only_ok(&service.refresh_all().await);
    let updates = service.updates().await.expect("updates");
    assert!(
        !updates[0].adds_permissions && updates[0].added_permissions.is_empty(),
        "the hidden domain is invisible to the update list, which is why the download checks"
    );
    // The download cache holds the same bytes; it is checked against the new entry anyway.
    let error = service
        .download(OFFICIAL_REPOSITORY_ID, FIXTURE_ID, "1.2.4")
        .await
        .expect_err("a hidden permission");
    assert_eq!(error.code(), "plugin_repository.package_mismatch");
    assert!(error.to_string().contains("permissions"), "{error}");
}

/// T-UNSIGNED: the digest leaves the signature out, so an index entry for a signed package
/// also matches the same package with its signature stripped. A development-mode verifier
/// would install that; a repository never delivers it.
#[tokio::test]
async fn an_unsigned_package_is_refused_from_a_repository_even_in_development_mode() {
    let fixture = Fixture::with_mode(true).await;
    let signed = package("1.2.4");
    let manifest = fixture_manifest(&public_key_base64(&plugin_key()))
        .replace(r#"version = "1.2.3""#, r#"version = "1.2.4""#);
    let unsigned =
        package_plugin(manifest.as_bytes(), EMPTY_COMPONENT, &[], None).expect("unsigned");
    let mut stripped = entry(&signed, "1.2.4");
    assert_eq!(
        stripped.package_digest,
        format_package_digest(&archive_digest(&unsigned).expect("digest"))
    );
    stripped.size = unsigned.len() as u64;
    let index = unsigned_index(5, vec![stripped], Revocations::default());
    fixture.serve_index(index::sign(REPOSITORY_KEY_ID, &repository_key(), &index).expect("sign"));
    fixture.serve_package("1.2.4", unsigned.clone());
    let service = fixture.service();
    only_ok(&service.refresh_all().await);

    assert!(fixture.installer.verifier().verify_bytes(&unsigned).is_ok());
    let error = service
        .download(OFFICIAL_REPOSITORY_ID, FIXTURE_ID, "1.2.4")
        .await
        .expect_err("unsigned");
    assert_eq!(error.code(), "plugin_repository.package_mismatch");
    assert!(error.to_string().contains("unsigned"), "{error}");
}

/// T-REPLAY: the floor lives in the database, not in the cache, so deleting the cache does not
/// reopen the door to an older index; and a cache older than the floor, or expired, is dropped
/// at start rather than offered offline.
#[tokio::test]
async fn a_replay_is_refused_without_the_cache_and_a_stale_cache_is_dropped() {
    let fixture = Fixture::new().await;
    let older = signed_index(4, &[("1.2.4", &package("1.2.4"))], Revocations::default());
    fixture.serve_index(signed_index(6, &[], Revocations::default()));
    only_ok(&fixture.service().refresh_all().await);
    let cache = fixture.root.join("index-official.json");
    std::fs::remove_file(&cache).expect("remove cache");

    fixture.serve_index(older.clone());
    let outcome = fixture.service().refresh_all().await;
    assert_eq!(
        outcome[0].1.as_ref().expect_err("replay").code(),
        "plugin_index.stale"
    );
    assert_eq!(fixture.official().await.sequence, Some(6));

    std::fs::write(&cache, &older).expect("plant an older cache");
    let restarted = fixture.service();
    restarted.load().await;
    assert!(restarted.offers().await.expect("offers").is_empty());
    assert!(!cache.exists(), "a cache below the floor was kept");

    let mut expired = unsigned_index(9, Vec::new(), Revocations::default());
    expired.issued_at = Utc::now() - Duration::days(40);
    expired.not_after = Utc::now() - Duration::days(1);
    std::fs::write(
        &cache,
        index::sign(REPOSITORY_KEY_ID, &repository_key(), &expired).expect("sign"),
    )
    .expect("plant an expired cache");
    fixture.service().load().await;
    assert!(!cache.exists(), "an expired cache was kept");
}

/// T-KEY: after approval, a third-party repository's index is held to the approved key. A new
/// key — under the same id or another — is refused, and what was accepted before stays.
#[tokio::test]
async fn a_third_party_index_under_an_unapproved_key_is_refused() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let repository = add_community(
        &fixture,
        &service,
        community_index(
            1,
            vec![entry(&package("1.2.4"), "1.2.4")],
            Revocations::default(),
        ),
    )
    .await;
    let rotated = SigningKey::from_bytes(&[58; 32]);
    for (key_id, code) in [
        ("community-v1", "plugin_index.bad_signature"),
        ("community-v2", "plugin_index.untrusted"),
    ] {
        let index = unsigned_index(2, Vec::new(), Revocations::default());
        fixture.fetcher.serve(
            COMMUNITY_URL,
            index::sign(key_id, &rotated, &index).expect("sign"),
        );
        let outcomes = service.refresh_all().await;
        let outcome = outcome_of(&outcomes, &repository.id);
        assert_eq!(outcome.as_ref().expect_err(key_id).code(), code);
    }
    let row = fixture
        .database
        .plugin_repository(&repository.id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(row.sequence, Some(1));
    let restarted = fixture.service();
    restarted.load().await;
    assert_eq!(restarted.offers().await.expect("offers").len(), 1);
}

/// T-WITHDRAW: a third-party repository withdraws the version it delivered here — and nothing
/// else: not the version installed from elsewhere, not the plugin key both are signed with.
#[tokio::test]
async fn a_third_party_repository_withdraws_what_it_delivered_and_nothing_else() {
    let fixture = Fixture::new().await;
    install_fixture(&fixture).await;
    let bundled = package("1.2.3");
    let delivered = package("1.2.4");
    let service = fixture.service();
    let repository = add_community(
        &fixture,
        &service,
        community_index(1, vec![entry(&delivered, "1.2.4")], Revocations::default()),
    )
    .await;
    serve_community_package(&fixture, "1.2.4", delivered.clone());
    let (offer, bytes) = service
        .download(&repository.id, FIXTURE_ID, "1.2.4")
        .await
        .expect("download");
    fixture
        .installer
        .install_bytes(bytes)
        .await
        .expect("install");
    service.record_install(&offer).await.expect("record");

    let delivered_digest = archive_digest(&delivered).expect("digest");
    let bundled_digest = archive_digest(&bundled).expect("digest");
    fixture.fetcher.serve(
        COMMUNITY_URL,
        community_index(
            2,
            Vec::new(),
            Revocations {
                package_digests: vec![
                    format_package_digest(&delivered_digest),
                    format_package_digest(&bundled_digest),
                ],
                keys: vec![RevokedKey {
                    key_id: "fixture-v1".to_owned(),
                    fingerprint: key_fingerprint(&plugin_key().verifying_key()),
                }],
            },
        ),
    );
    let outcomes = service.refresh_all().await;
    assert!(outcome_of(&outcomes, &repository.id).is_ok());

    let verifier = fixture.installer.verifier();
    assert!(
        verifier
            .is_package_revoked(&delivered_digest)
            .expect("read")
    );
    assert!(!verifier.is_package_revoked(&bundled_digest).expect("read"));
    assert!(verifier.is_trusted("fixture-v1").expect("read"));
    assert!(
        !verifier
            .is_key_withdrawn(&key_fingerprint(&plugin_key().verifying_key()))
            .expect("read")
    );
}

/// T-AUTO: an automatic update installs under a key that is trusted *now*. The key of the
/// installed version was dropped since; the update is still listed and downloads, and the
/// install — the call the automatic path makes, with no confirmation — refuses it.
#[tokio::test]
async fn an_automatic_update_needs_a_key_that_is_still_trusted() {
    let fixture = Fixture::new().await;
    install_fixture(&fixture).await;
    let newer = package("1.2.4");
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &newer)],
        Revocations::default(),
    ));
    fixture.serve_package("1.2.4", newer);
    let service = fixture.service();
    service.set_update_policy(Arc::new(AllAutomatic));
    only_ok(&service.refresh_all().await);
    let verifier = fixture.installer.verifier();
    assert!(verifier.revoke_key("fixture-v1").expect("revoke"));

    let updates = service.automatic_updates().await.expect("automatic");
    assert_eq!(updates.len(), 1, "the automatic path picks it up");
    let (_, bytes) = service
        .download(OFFICIAL_REPOSITORY_ID, FIXTURE_ID, "1.2.4")
        .await
        .expect("download");
    let error = fixture
        .installer
        .install_bytes(bytes)
        .await
        .err()
        .expect("an untrusted key");
    assert!(
        matches!(error, VerifyError::UntrustedKey { .. }),
        "{error:?}"
    );
    assert_eq!(installed_count(&fixture).await, 1);
}

/// T-SIZE: the service asks the fetcher for at most the index ceiling and at most the size the
/// entry declares; the production fetcher cuts a body off at that limit while it streams.
#[tokio::test]
async fn an_oversize_index_or_package_is_cut_off() {
    let fixture = Fixture::new().await;
    fixture.serve_index(vec![b' '; index::MAX_INDEX_BYTES + 1]);
    let service = fixture.service();
    let outcome = service.refresh_all().await;
    assert_eq!(
        outcome[0].1.as_ref().expect_err("oversize index").code(),
        "plugin_repository.download_failed"
    );

    let genuine = package("1.2.4");
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &genuine)],
        Revocations::default(),
    ));
    only_ok(&service.refresh_all().await);
    let mut padded = genuine;
    padded.push(0);
    fixture.serve_package("1.2.4", padded);
    let error = service
        .download(OFFICIAL_REPOSITORY_ID, FIXTURE_ID, "1.2.4")
        .await
        .expect_err("oversize package");
    assert_eq!(error.code(), "plugin_repository.download_failed");
}
