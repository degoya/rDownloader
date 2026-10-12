//! Choosing among the offers (RD-1260-02): the newest one this build runs, the repository's
//! version of a bundled package, and the bundle when the repository cannot be reached.

use super::*;
use crate::BundledPackage;

fn offer(entry: IndexPackage) -> Offer {
    Offer {
        repository_id: OFFICIAL_REPOSITORY_ID.to_owned(),
        repository_name: "Official".to_owned(),
        official: true,
        compatibility: compatibility(&entry),
        entry,
        installed_version: None,
    }
}

fn entry(version: &str) -> IndexPackage {
    describe_package(&package(version), file_name(version), None).expect("describe")
}

/// The bundled fixture plugin at `version`, as the start's sync remembers it.
fn bundled(fixture: &Fixture, version: &str) -> BundledPackage {
    let verified = fixture
        .installer
        .verifier()
        .verify_bytes(&package(version))
        .expect("verify");
    BundledPackage::from_verified(PathBuf::from(file_name(version)), &verified)
}

/// The index lists the last version per contract; this build takes the newest it runs.
#[test]
fn the_newest_offer_this_build_runs_wins() {
    let mut future = entry("1.2.4");
    future.version = "2.0.0".to_owned();
    future.api_version = "99.0.0".to_owned();
    let offers = vec![offer(entry("1.2.3")), offer(future), offer(entry("1.2.4"))];
    let newest = newest_compatible(&offers);
    assert_eq!(newest.len(), 1);
    assert_eq!(
        newest
            .values()
            .next()
            .map(|offer| offer.entry.version.as_str()),
        Some("1.2.4")
    );
}

#[tokio::test]
async fn a_bundled_package_is_replaced_only_by_the_same_plugin_asking_for_nothing_more() {
    let fixture = Fixture::new().await;
    let package = bundled(&fixture, "1.2.3");
    let same = offer(entry("1.2.4"));
    assert_eq!(
        replacement(std::slice::from_ref(&same), &package)
            .map(|offer| offer.entry.version.as_str()),
        Some("1.2.4")
    );

    let older = offer(entry("1.2.2"));
    assert!(replacement(std::slice::from_ref(&older), &package).is_none());

    let mut wider = same.clone();
    wider
        .entry
        .permissions
        .http_domains
        .push("more.example.test".to_owned());
    assert!(replacement(std::slice::from_ref(&wider), &package).is_none());

    let mut foreign = same.clone();
    foreign.entry.publisher.fingerprint = "ef".repeat(32);
    assert!(replacement(std::slice::from_ref(&foreign), &package).is_none());
}

#[tokio::test]
async fn a_reachable_repository_delivers_its_version_of_a_bundled_package() {
    let fixture = Fixture::new().await;
    let newer = package("1.2.4");
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &newer)],
        Revocations::default(),
    ));
    fixture.serve_package("1.2.4", newer);
    let service = fixture.service();
    service.ensure_index().await;
    let (offer, bytes) = service
        .fetch_replacement(&bundled(&fixture, "1.2.3"))
        .await
        .expect("the repository's version");
    assert_eq!(offer.entry.version, "1.2.4");
    assert!(!bytes.is_empty());
}

/// Without a network the install takes the bundle, and the next service of the same wizard run
/// does not wait for the repository again.
#[tokio::test]
async fn an_unreachable_repository_leaves_the_install_to_the_bundle() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    service.ensure_index().await;
    let official = url::Url::parse(OFFICIAL_INDEX_URL).expect("url");
    assert!(!service.host_reachable(&official));
    assert!(
        service
            .fetch_replacement(&bundled(&fixture, "1.2.3"))
            .await
            .is_none()
    );

    // The network is back, but within the pause the repository is not asked.
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &package("1.2.4"))],
        Revocations::default(),
    ));
    service.ensure_index().await;
    assert_eq!(fixture.official().await.sequence, None);
}
