//! An installed version this build refuses still gets its update (RD-1251-01): after a contract
//! change every installed package is refused, and the repository's newer version is what makes
//! the plugin run again. Also the address rule, kept here for the parent's line ratchet.

use super::*;

/// Leaves the fixture plugin installed at `version` the way an earlier release did, built
/// against a contract this build no longer speaks and signed with the fixture's key.
fn install_refused(root: &std::path::Path, version: &str) {
    let manifest = fixture_manifest(&public_key_base64(&plugin_key()))
        .replace(r#"version = "1.2.3""#, &format!(r#"version = "{version}""#))
        .replace(r#"api_version = "0.11.0""#, r#"api_version = "0.10.0""#);
    let directory = root
        .join("019d0000-0000-7000-8000-00000000abcd")
        .join(version);
    std::fs::create_dir_all(&directory).expect("version dir");
    std::fs::write(directory.join("manifest.toml"), manifest).expect("manifest");
    std::fs::write(directory.join("component.wasm"), EMPTY_COMPONENT).expect("component");
}

#[tokio::test]
async fn a_refused_installed_version_is_offered_its_update_and_replaced_by_it() {
    let fixture = Fixture::new().await;
    install_refused(fixture.installer.root(), "1.2.3");
    assert!(
        fixture
            .installer
            .list_installed()
            .await
            .expect("installed")
            .is_empty()
    );
    let newer = package("1.2.4");
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &newer)],
        Revocations::default(),
    ));
    let service = fixture.service();
    only_ok(&service.refresh_all().await);

    let updates = service.updates().await.expect("updates");
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].installed_version, "1.2.3");
    assert_eq!(updates[0].offer.entry.version, "1.2.4");
    assert!(
        !updates[0].adds_permissions,
        "{:?}",
        updates[0].added_permissions
    );
    let offers = service.offers().await.expect("offers");
    assert_eq!(offers[0].installed_version.as_deref(), Some("1.2.3"));

    fixture
        .installer
        .install_bytes(newer)
        .await
        .expect("install");
    assert!(service.updates().await.expect("updates").is_empty());
    assert!(
        fixture
            .installer
            .list_incompatible()
            .await
            .expect("incompatible")
            .is_empty(),
        "the refused version stayed beside its update"
    );
}

#[tokio::test]
async fn a_refused_version_under_another_key_gets_no_update() {
    let fixture = Fixture::new().await;
    let root = fixture.installer.root().to_path_buf();
    install_refused(&root, "1.2.3");
    let path = root
        .join("019d0000-0000-7000-8000-00000000abcd")
        .join("1.2.3")
        .join("manifest.toml");
    let other = public_key_base64(&SigningKey::from_bytes(&[77; 32]));
    let manifest = std::fs::read_to_string(&path)
        .expect("read")
        .replace(&public_key_base64(&plugin_key()), &other);
    std::fs::write(&path, manifest).expect("write");
    fixture.serve_index(signed_index(
        5,
        &[("1.2.4", &package("1.2.4"))],
        Revocations::default(),
    ));
    let service = fixture.service();
    only_ok(&service.refresh_all().await);
    assert!(service.updates().await.expect("updates").is_empty());
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

/// The plugins an update fetches for the version it installs (RD-1260-02).
#[path = "repository_prepare_tests.rs"]
mod prepare;

/// The newest offer per plugin, and the repository's version of a bundled package (RD-1260-02).
#[path = "repository_select_tests.rs"]
mod select;
