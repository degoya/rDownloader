use super::*;

#[derive(clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: UpdateCommand,
}

/// `--schema-change` reaches the signed manifest; left out, the manifest does not say, which
/// every installation reads as a change.
#[tokio::test]
async fn the_schema_change_flag_reaches_the_signed_manifest() {
    use clap::Parser as _;
    use sha2::Digest as _;
    let directory =
        std::env::temp_dir().join(format!("rd-pack-schema-change-{}", uuid::Uuid::now_v7()));
    let assets = directory.join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    let archive = b"the archive";
    std::fs::write(assets.join("rdownloader-linux-x86_64.tar.gz"), archive).expect("archive");
    let sums = directory.join("SHA256SUMS");
    std::fs::write(
        &sums,
        format!(
            "{}  ./rdownloader-linux-x86_64.tar.gz\n",
            hex::encode(sha2::Sha256::digest(archive))
        ),
    )
    .expect("sums");
    let key = rd_plugin_host::generate_signing_key();
    let key_file = directory.join("update.key");
    std::fs::write(&key_file, &key.private_pem).expect("key");
    let trust = rd_sign::TrustStore::new();
    trust
        .trust(
            "test-update-key".to_owned(),
            key.signing_key.verifying_key(),
        )
        .expect("trust");
    let path = |path: &Path| path.display().to_string();
    for (flag, expected) in [
        (Some("false"), Some(false)),
        (Some("true"), Some(true)),
        (None, None),
    ] {
        let out = directory.join(format!("out-{}", flag.unwrap_or("absent")));
        let mut argv = vec![
            "rd-pack".to_owned(),
            "manifest".to_owned(),
            "build".to_owned(),
            "--version".to_owned(),
            "v1.8.0".to_owned(),
            "--checksums".to_owned(),
            path(&sums),
            "--assets".to_owned(),
            path(&assets),
            "--base-url".to_owned(),
            "https://example.test/releases/v1.8.0/".to_owned(),
            "--out".to_owned(),
            path(&out),
            "--key".to_owned(),
            path(&key_file),
            "--key-id".to_owned(),
            "test-update-key".to_owned(),
        ];
        if let Some(flag) = flag {
            argv.extend(["--schema-change".to_owned(), flag.to_owned()]);
        }
        run(Cli::try_parse_from(argv).expect("arguments").command)
            .await
            .expect("build");
        let bytes = std::fs::read(out.join(Channel::Stable.file_name())).expect("manifest");
        let verified = manifest::verify_with(&bytes, &trust, Channel::Stable, None, Utc::now())
            .expect("verify");
        assert_eq!(verified.schema_change, expected, "{flag:?}");
        assert_eq!(verified.changes_schema(), expected.unwrap_or(true));
    }
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_release_files_are_classified_by_name() {
    assert_eq!(
        classify("rdownloader-linux-x86_64.tar.gz"),
        Some(("linux", "x86_64", "archive"))
    );
    assert_eq!(
        classify("rdownloader-macos-aarch64.tar.gz"),
        Some(("macos", "aarch64", "archive"))
    );
    assert_eq!(
        classify("rdownloader-windows-x86_64.zip"),
        Some(("windows", "x86_64", "archive"))
    );
    assert_eq!(
        classify("rdownloader-1.8.0-x86_64.msi"),
        Some(("windows", "x86_64", "msi"))
    );
    assert_eq!(
        classify("rdownloader_1.8.0_amd64.deb"),
        Some(("linux", "x86_64", "deb"))
    );
    assert_eq!(
        classify("rdownloader_1.8.0_arm64.deb"),
        Some(("linux", "aarch64", "deb"))
    );
    assert_eq!(
        classify("rdownloader-1.8.0-1.aarch64.rpm"),
        Some(("linux", "aarch64", "rpm"))
    );
    // The names the installers are published under (RD-180-05).
    for (name, expected) in [
        (
            "rdownloader-windows-x86_64.msi",
            ("windows", "x86_64", "msi"),
        ),
        ("rdownloader-linux-x86_64.deb", ("linux", "x86_64", "deb")),
        ("rdownloader-linux-aarch64.rpm", ("linux", "aarch64", "rpm")),
    ] {
        assert_eq!(classify(name), Some(expected), "{name}");
    }
    for other in [
        "rdownloader-chrome.zip",
        "rdownloader-firefox.zip",
        "rdownloader-capture-1.8.0-x86_64.msi",
        "rdownloader-plugin-index.json",
        "SHA256SUMS",
        "rdownloader.spdx.json",
        "ddownload-1.0.0.rdplug",
        "rdownloader-1.8.0.msi",
        "rdownloader-windows-x86_64-de.msi",
        "rdownloader-windows-x86_64-fr.msi",
    ] {
        assert_eq!(classify(other), None, "{other}");
    }
}

#[test]
fn both_checksum_spellings_are_read() {
    let hash = "a".repeat(64);
    let text = format!(
        "{hash}  ./rdownloader-linux-x86_64.tar.gz\n{hash} *rdownloader-windows-x86_64.zip\nnot a line\n"
    );
    let parsed = parse_checksums(&text);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].1, "rdownloader-linux-x86_64.tar.gz");
    assert_eq!(parsed[1].1, "rdownloader-windows-x86_64.zip");
}

const CHANGELOG: &str = "# Changelog\n\n## [Unreleased]\n\n## [1.8.0] - 2026-10-10\n\n### Added\n\n\
- **Update check (RD-180-01).** Text.\n\n## [1.7.0] - 2026-09-30\n";

const RELEASE_NOTES: &str = "# Release notes\n\n## 1.8.0\n\n- rDownloader tells you when a new version \
is out.\n- A **bold** word and a [link](https://example.test) stay readable.\n\n## 1.7.0\n\n\
Maintenance release: internal changes only, no change in behaviour.\n";

/// The manifest carries the version's points for users from `RELEASE-NOTES.md`, never the
/// CHANGELOG's headlines, and the anchor of the CHANGELOG heading for the full changes
/// (RD-1150-02); a section the rules refuse stops the build.
#[tokio::test]
async fn the_notes_come_from_the_release_notes_and_the_anchor_from_the_changelog() {
    use clap::Parser as _;
    use sha2::Digest as _;
    let directory =
        std::env::temp_dir().join(format!("rd-pack-release-notes-{}", uuid::Uuid::now_v7()));
    let assets = directory.join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    let archive = b"the archive";
    std::fs::write(assets.join("rdownloader-linux-x86_64.tar.gz"), archive).expect("archive");
    let sums = directory.join("SHA256SUMS");
    std::fs::write(
        &sums,
        format!(
            "{}  ./rdownloader-linux-x86_64.tar.gz\n",
            hex::encode(sha2::Sha256::digest(archive))
        ),
    )
    .expect("sums");
    let key = rd_plugin_host::generate_signing_key();
    let key_file = directory.join("update.key");
    std::fs::write(&key_file, &key.private_pem).expect("key");
    let trust = rd_sign::TrustStore::new();
    trust
        .trust(
            "test-update-key".to_owned(),
            key.signing_key.verifying_key(),
        )
        .expect("trust");
    let changelog = directory.join("CHANGELOG.md");
    std::fs::write(&changelog, CHANGELOG).expect("changelog");
    let path = |path: &Path| path.display().to_string();
    // The notes as `<name>.md`, the manifest into `<name>/`.
    let build = |name: &str, notes: &str| {
        let file = directory.join(format!("{name}.md"));
        let out = directory.join(name);
        std::fs::write(&file, notes).expect("notes");
        Cli::try_parse_from([
            "rd-pack".to_owned(),
            "manifest".to_owned(),
            "build".to_owned(),
            "--version".to_owned(),
            "v1.8.0".to_owned(),
            "--checksums".to_owned(),
            path(&sums),
            "--assets".to_owned(),
            path(&assets),
            "--base-url".to_owned(),
            "https://example.test/releases/v1.8.0/".to_owned(),
            "--changelog".to_owned(),
            path(&changelog),
            "--release-notes".to_owned(),
            path(&file),
            "--out".to_owned(),
            path(&out),
            "--key".to_owned(),
            path(&key_file),
            "--key-id".to_owned(),
            "test-update-key".to_owned(),
        ])
        .expect("arguments")
        .command
    };
    run(build("accepted", RELEASE_NOTES)).await.expect("build");
    let bytes = std::fs::read(directory.join("accepted").join(Channel::Stable.file_name()))
        .expect("manifest");
    let verified =
        manifest::verify_with(&bytes, &trust, Channel::Stable, None, Utc::now()).expect("verify");
    assert_eq!(
        verified.notes,
        "- rDownloader tells you when a new version is out.\n- A bold word and a link stay readable."
    );
    assert_eq!(
        verified.changelog_anchor.as_deref(),
        Some("180---2026-10-10")
    );

    let error = run(build(
        "refused",
        "## 1.8.0\n\n- Narrower visibility in every crate (RD-1120-12, CR-9).\n",
    ))
    .await
    .expect_err("a job number in the notes");
    assert!(error.to_string().contains("names a job"), "{error}");
    assert!(
        !directory
            .join("refused")
            .join(Channel::Stable.file_name())
            .exists()
    );
    let _ = std::fs::remove_dir_all(&directory);
}
