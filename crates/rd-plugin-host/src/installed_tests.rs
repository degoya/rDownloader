use std::{
    io::{Cursor, Write},
    path::Path,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};

use super::*;
use crate::{PluginVerifier, package_digest, public_key_base64, tests::fixture_manifest};

const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";

fn signed_archive(signing: &SigningKey, locales: &[(String, Vec<u8>)]) -> (Vec<u8>, Vec<u8>) {
    signed_archive_named(signing, "Fixture", "1.2.3", locales)
}

/// The same fixture under a chosen name and version, so one plugin id can be installed
/// twice — including under two different names, which is the case the ordering must hold
/// for.
fn signed_archive_named(
    signing: &SigningKey,
    name: &str,
    version: &str,
    locales: &[(String, Vec<u8>)],
) -> (Vec<u8>, Vec<u8>) {
    let manifest = fixture_manifest(&public_key_base64(signing))
        .replace("name = \"Fixture\"", &format!("name = \"{name}\""))
        .replace("version = \"1.2.3\"", &format!("version = \"{version}\""))
        .into_bytes();
    let signature = STANDARD.encode(
        signing
            .sign(&package_digest(&manifest, EMPTY_COMPONENT, locales))
            .to_bytes(),
    );
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (member, content) in [
        ("manifest.toml".to_owned(), manifest.clone()),
        ("component.wasm".to_owned(), EMPTY_COMPONENT.to_vec()),
        ("signature.ed25519".to_owned(), signature.into_bytes()),
    ] {
        writer.start_file(&member, options).expect("archive member");
        writer.write_all(&content).expect("archive content");
    }
    for (language, bytes) in locales {
        writer
            .start_file(format!("locales/{language}.json"), options)
            .expect("locale member");
        writer.write_all(bytes).expect("locale content");
    }
    (writer.finish().expect("archive").into_inner(), manifest)
}

fn installer_with(signing: &SigningKey, root: &Path) -> PluginInstaller {
    let verifier = PluginVerifier::new(false);
    verifier
        .trust_key("fixture-v1".to_owned(), signing.verifying_key())
        .expect("trust key");
    PluginInstaller::new(root.to_owned(), verifier)
}

#[tokio::test]
async fn installed_signature_is_retained_and_reverified() {
    let directory = tempfile::tempdir().expect("tempdir");
    let signing = SigningKey::from_bytes(&[9_u8; 32]);
    let (archive, _) = signed_archive(&signing, &[]);
    let installer = installer_with(&signing, directory.path());

    let installed = installer.install_bytes(archive).await.expect("install");
    assert!(installed.path.join("signature.ed25519").is_file());
    let loaded = installer.load_verified().await.expect("reload");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].manifest.message_slug(), "fixture");
}

#[tokio::test]
async fn locale_files_survive_install_and_reverification() {
    let directory = tempfile::tempdir().expect("tempdir");
    let signing = SigningKey::from_bytes(&[11_u8; 32]);
    let locales = vec![
        (
            "en".to_owned(),
            br#"{"name":"Fixture","codes":{"fixture.oops":"Oops"}}"#.to_vec(),
        ),
        (
            "de".to_owned(),
            br#"{"name":"Fixture DE","codes":{"fixture.oops":"Hoppla"}}"#.to_vec(),
        ),
    ];
    let (archive, _) = signed_archive(&signing, &locales);
    let installer = installer_with(&signing, directory.path());

    let installed = installer.install_bytes(archive).await.expect("install");
    assert!(installed.path.join("locales").join("de.json").is_file());
    let loaded = installer.load_verified().await.expect("reload");
    assert_eq!(
        loaded.len(),
        1,
        "locale files must not break re-verification"
    );

    let bundle = installer
        .locale_bundle("de".to_owned())
        .await
        .expect("bundle");
    assert_eq!(bundle["server"]["codes"]["fixture.oops"], "Hoppla");
    assert_eq!(bundle["providers"]["fixture"]["name"], "Fixture DE");
    // Untranslated languages fall back to the manifest's own strings.
    let english = installer
        .locale_bundle("fr".to_owned())
        .await
        .expect("bundle");
    assert_eq!(english["providers"]["fixture"]["name"], "Fixture");
    assert_eq!(
        english["providers"]["fixture"]["description"],
        "A fixture resolver"
    );
}

#[tokio::test]
async fn revoking_a_key_skips_the_plugin_instead_of_failing_startup() {
    let directory = tempfile::tempdir().expect("tempdir");
    let signing = SigningKey::from_bytes(&[13_u8; 32]);
    let (archive, _) = signed_archive(&signing, &[]);
    let installer = installer_with(&signing, directory.path());
    installer.install_bytes(archive).await.expect("install");
    assert_eq!(installer.load_verified().await.expect("reload").len(), 1);

    assert!(
        installer
            .verifier()
            .revoke_key("fixture-v1")
            .expect("revoke")
    );
    let loaded = installer
        .load_verified()
        .await
        .expect("startup must still succeed");
    assert!(loaded.is_empty(), "revoked plugin must be skipped");
}

/// Withdrawing one published version must take that version out and nothing else — that
/// is the whole reason content revocation exists next to key revocation.
#[tokio::test]
async fn a_revoked_digest_drops_one_version_and_leaves_the_key_trusted() {
    let directory = tempfile::tempdir().expect("tempdir");
    let signing = SigningKey::from_bytes(&[19_u8; 32]);
    let installer = installer_with(&signing, directory.path());
    let (withdrawn, withdrawn_manifest) = signed_archive_named(&signing, "Fixture", "2.0.0", &[]);
    let (kept, _) = signed_archive_named(&signing, "Fixture", "1.0.0", &[]);
    installer.install_bytes(withdrawn).await.expect("install");
    installer.install_bytes(kept).await.expect("install");
    assert_eq!(installer.load_verified().await.expect("reload").len(), 2);

    let digest = package_digest(&withdrawn_manifest, EMPTY_COMPONENT, &[]);
    assert!(
        installer
            .verifier()
            .revoke_package_digest(digest)
            .expect("revoke the digest"),
        "the first withdrawal of a digest has to report that it is new"
    );
    assert!(
        !installer
            .verifier()
            .revoke_package_digest(digest)
            .expect("revoke the digest again"),
        "repeating a withdrawal must be distinguishable from making one"
    );
    assert!(
        installer
            .verifier()
            .is_package_revoked(&digest)
            .expect("read")
    );

    let loaded = installer
        .load_verified()
        .await
        .expect("startup must still succeed");
    assert_eq!(loaded.len(), 1, "the withdrawn version must be skipped");
    assert_eq!(loaded[0].manifest.version, "1.0.0");
    assert!(
        installer.verifier().is_trusted("fixture-v1").expect("read"),
        "withdrawing one package must not revoke its author's key"
    );
}

/// The withdrawn set is process state, so the only thing between a restart and a package
/// that was withdrawn yesterday is the seeding. This covers that seam end to end: the hex
/// digest a caller stored goes back in, the package stays out of the next load, and taking
/// the withdrawal back lets it load again — at the next load, not in mid-session.
#[tokio::test]
async fn a_seeded_digest_keeps_its_package_out_until_the_withdrawal_is_taken_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let signing = SigningKey::from_bytes(&[29_u8; 32]);
    let installer = installer_with(&signing, directory.path());
    let (archive, manifest_bytes) = signed_archive(&signing, &[]);
    let installed = installer.install_bytes(archive).await.expect("install");

    // What the API would store: the digest of the version the user is looking at.
    let digest = installer
        .installed_package_digest(
            &installed.manifest.id.to_string(),
            &installed.manifest.version,
        )
        .await
        .expect("read the installed digest")
        .expect("the version is installed");
    assert_eq!(
        digest,
        package_digest(&manifest_bytes, EMPTY_COMPONENT, &[])
    );
    let stored = crate::format_package_digest(&digest);

    // A restart: a fresh verifier holds nothing, and only the seeding keeps it from
    // loading a package that was withdrawn before this process existed.
    let restarted = installer_with(&signing, directory.path());
    assert_eq!(restarted.load_verified().await.expect("reload").len(), 1);
    restarted
        .verifier()
        .set_revoked_package_digests([
            crate::parse_package_digest(&stored).expect("the stored form parses back")
        ])
        .expect("seed the withdrawn set");
    assert!(
        restarted.load_verified().await.expect("reload").is_empty(),
        "a seeded digest has to survive the restart it was seeded for"
    );
    assert_eq!(
        restarted
            .verifier()
            .revoked_package_digests()
            .expect("list"),
        vec![stored]
    );

    assert!(
        restarted
            .verifier()
            .unrevoke_package_digest(&digest)
            .expect("take the withdrawal back")
    );
    assert_eq!(restarted.load_verified().await.expect("reload").len(), 1);
}

/// An id or version from a request must not be able to leave the plugin root, and a
/// version that is not installed has to be a plain "nothing here" rather than an error.
#[tokio::test]
async fn a_digest_is_only_read_for_a_version_inside_the_plugin_root() {
    let directory = tempfile::tempdir().expect("tempdir");
    let signing = SigningKey::from_bytes(&[31_u8; 32]);
    let installer = installer_with(&signing, directory.path());
    assert!(
        installer
            .installed_package_digest("019d0000-0000-7000-8000-00000000abcd", "9.9.9")
            .await
            .expect("a missing version is not an error")
            .is_none()
    );
    assert!(
        installer
            .installed_package_digest("..", "1.2.3")
            .await
            .is_err(),
        "a traversal segment must be refused rather than resolved"
    );
}

/// A manifest edited after installation has to stop counting everywhere, not only where a
/// component is compiled. The provider rows and the locale bundle are what such an edit is
/// worth tampering for — request domains, cookie scope, secret slots, the strings shown for
/// an account — and they used to be read straight off disk with no signature check.
#[tokio::test]
async fn a_tampered_manifest_stops_reaching_the_locale_bundle() {
    let directory = tempfile::tempdir().expect("tempdir");
    let signing = SigningKey::from_bytes(&[23_u8; 32]);
    let installer = installer_with(&signing, directory.path());
    let (archive, _) = signed_archive(&signing, &[]);
    let installed = installer.install_bytes(archive).await.expect("install");
    let bundle = installer
        .locale_bundle("en".to_owned())
        .await
        .expect("bundle");
    assert_eq!(bundle["providers"]["fixture"]["name"], "Fixture");

    let manifest_path = installed.path.join("manifest.toml");
    let manifest = std::fs::read_to_string(&manifest_path).expect("read the manifest");
    std::fs::write(
        &manifest_path,
        manifest.replace("A fixture resolver", "Anything the attacker likes"),
    )
    .expect("tamper with the manifest");

    // The same package is already skipped by `load_verified`; this is the other path.
    assert!(installer.load_verified().await.expect("reload").is_empty());
    let bundle = installer
        .locale_bundle("en".to_owned())
        .await
        .expect("bundle");
    assert!(
        bundle["providers"].get("fixture").is_none(),
        "a manifest that no longer verifies must not reach the interface bundle"
    );
}

/// Consumers deduplicate by id and keep the first entry, so the versions of one plugin
/// have to be adjacent and newest first even when the plugin was renamed in between.
#[tokio::test]
async fn a_renamed_plugin_still_yields_its_newest_version_first() {
    let directory = tempfile::tempdir().expect("tempdir");
    let signing = SigningKey::from_bytes(&[21_u8; 32]);
    let installer = installer_with(&signing, directory.path());
    // The newer version sorts last by name, which is exactly what used to hide it.
    for (name, version) in [("Zulu Storage", "2.0.0"), ("Alpha Storage", "1.0.0")] {
        let (archive, _) = signed_archive_named(&signing, name, version, &[]);
        installer.install_bytes(archive).await.expect("install");
    }

    let loaded = installer.load_verified().await.expect("reload");
    assert_eq!(loaded.len(), 2);
    assert_eq!(
        loaded[0].manifest.version, "2.0.0",
        "the newest version must come first whatever it is called"
    );
    assert_eq!(loaded[1].manifest.version, "1.0.0");
}

/// The version choice of RD-140-02 as the adapters see it: the default version first, a
/// staged one never among the versions an adapter picks from, and a withdrawn version
/// neither the default nor under test.
#[tokio::test]
async fn the_version_choice_decides_the_order_the_adapters_pick_from() {
    let directory = tempfile::tempdir().expect("tempdir");
    let signing = SigningKey::from_bytes(&[23_u8; 32]);
    let installer = installer_with(&signing, directory.path());
    let mut manifests = std::collections::HashMap::new();
    for version in ["1.0.0", "2.0.0", "3.0.0"] {
        let (archive, manifest) = signed_archive_named(&signing, "Fixture", version, &[]);
        installer.install_bytes(archive).await.expect("install");
        manifests.insert(version, manifest);
    }
    let id = installer.load_verified().await.expect("load")[0]
        .manifest
        .id
        .to_string();
    let order = |loaded: Vec<(VerifiedPackage, VersionRole)>| -> Vec<(String, VersionRole)> {
        loaded
            .into_iter()
            .map(|(package, role)| (package.manifest.version, role))
            .collect()
    };

    // Rolled back to 1.0.0, 3.0.0 under test.
    installer.set_version_choices(
        [(
            id.clone(),
            crate::VersionChoice {
                active: Some("1.0.0".to_owned()),
                staged: Some("3.0.0".to_owned()),
            },
        )]
        .into(),
    );
    assert_eq!(
        order(installer.load_verified_with_roles().await.expect("load")),
        vec![
            ("1.0.0".to_owned(), VersionRole::Default),
            ("2.0.0".to_owned(), VersionRole::Retained),
            ("3.0.0".to_owned(), VersionRole::Staged),
        ]
    );
    let registry = crate::PluginTypeRegistry::load(&installer)
        .await
        .expect("registry");
    let offered: Vec<&str> = registry
        .of_type(&crate::PluginType::Resolver)
        .map(|package| package.manifest.version.as_str())
        .collect();
    assert_eq!(
        offered,
        ["1.0.0", "2.0.0"],
        "the version under test is reached through a pin, never through `of_type`"
    );

    // Withdrawing the chosen version: it does not load, so the newest version that is not
    // under test takes over, and the choice needs no repair to stay safe.
    installer
        .verifier()
        .revoke_package_digest(package_digest(&manifests["1.0.0"], EMPTY_COMPONENT, &[]))
        .expect("withdraw");
    assert_eq!(
        order(installer.load_verified_with_roles().await.expect("load")),
        vec![
            ("2.0.0".to_owned(), VersionRole::Default),
            ("3.0.0".to_owned(), VersionRole::Staged),
        ]
    );

    // And withdrawing the staged one leaves nothing under test.
    installer
        .verifier()
        .revoke_package_digest(package_digest(&manifests["3.0.0"], EMPTY_COMPONENT, &[]))
        .expect("withdraw");
    assert_eq!(
        order(installer.load_verified_with_roles().await.expect("load")),
        vec![("2.0.0".to_owned(), VersionRole::Default)]
    );
}

/// The health check before a choice: a withdrawn version fails it like the next start would.
#[tokio::test]
async fn a_withdrawn_version_does_not_pass_the_check_before_activation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let signing = SigningKey::from_bytes(&[24_u8; 32]);
    let installer = installer_with(&signing, directory.path());
    let (archive, manifest) = signed_archive_named(&signing, "Fixture", "1.0.0", &[]);
    installer.install_bytes(archive).await.expect("install");
    let id = installer.load_verified().await.expect("load")[0]
        .manifest
        .id
        .to_string();

    assert!(
        installer
            .verify_installed_version(&id, "1.0.0")
            .await
            .expect("check")
            .is_some()
    );
    assert!(
        installer
            .verify_installed_version(&id, "9.9.9")
            .await
            .expect("a missing version is not an error")
            .is_none()
    );
    installer
        .verifier()
        .revoke_package_digest(package_digest(&manifest, EMPTY_COMPONENT, &[]))
        .expect("withdraw");
    assert!(
        installer
            .verify_installed_version(&id, "1.0.0")
            .await
            .is_err()
    );
}

/// `plugin.before_version_promoted` (RD-170-07, recovery matrix): an update stopped after
/// its package was written under the staging name and before the rename that makes it a
/// version.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn an_update_stopped_before_its_rename_is_never_loaded_and_the_start_removes_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let signing = SigningKey::from_bytes(&[23_u8; 32]);
    let installer = installer_with(&signing, directory.path());
    let (current, _) = signed_archive_named(&signing, "Fixture", "1.2.3", &[]);
    installer.install_bytes(current).await.expect("install");

    let (update, _) = signed_archive_named(&signing, "Fixture", "1.2.4", &[]);
    {
        let guard = rd_core::failpoint::FailpointGuard::once("plugin.before_version_promoted");
        assert!(installer.install_bytes(update.clone()).await.is_err());
        assert!(guard.fired(), "the crash point was never reached");
    }
    assert_eq!(install_stagings(directory.path()).expect("list").len(), 1);
    let versions = |manifests: Vec<PluginManifest>| -> Vec<String> {
        manifests
            .into_iter()
            .map(|manifest| manifest.version)
            .collect()
    };
    // The installed version stays the only one, before and after the restart.
    assert_eq!(
        versions(installer.list_installed().await.expect("list")),
        ["1.2.3"]
    );

    let restarted = installer_with(&signing, directory.path());
    assert_eq!(restarted.load_verified().await.expect("load").len(), 1);
    assert_eq!(restarted.sweep_install_staging().await, 1);
    assert!(install_stagings(directory.path()).expect("list").is_empty());
    // The next update pass installs it as if nothing had happened.
    restarted.install_bytes(update).await.expect("update");
    let mut installed = versions(restarted.list_installed().await.expect("list"));
    installed.sort();
    assert_eq!(installed, ["1.2.3", "1.2.4"]);
}
