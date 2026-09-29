//! A full backup from a live database to a sealed archive and back (RD-160-01): every part
//! the manifest names comes out byte for byte, the plugin trust survives the trip, and nothing
//! in the archive is readable without the key.

use rd_backup::{
    BackupKey, BackupSources, LocalFolder, PartKind, archive::extract_archive, create_backup,
    staging_root, sweep_staging,
};
use rd_db::{Database, NewPluginRepository, NewPluginTrustedKey, NewPluginVersionChoice};
use tempfile::TempDir;

const CANARY: &str = "account-password-canary-that-must-never-be-readable";

struct Setup {
    directory: TempDir,
    database: Database,
}

async fn setup() -> Setup {
    let directory = TempDir::new().expect("temp");
    let database = Database::open(directory.path().join("data/rdownloader.sqlite3"))
        .await
        .expect("database");
    database
        .trust_plugin_key(NewPluginTrustedKey {
            key_id: "author-v1".to_owned(),
            public_key: "AAAA".to_owned(),
            fingerprint: "cd".repeat(32),
            plugin_name: Some("Example".to_owned()),
        })
        .await
        .expect("trust");
    database
        .add_plugin_repository(NewPluginRepository {
            id: "community".to_owned(),
            name: "Community".to_owned(),
            url: "https://plugins.example.org/index.json".to_owned(),
            key_id: "community-v1".to_owned(),
            public_key: "BBBB".to_owned(),
            fingerprint: "ab".repeat(32),
        })
        .await
        .expect("repository");
    database
        .save_plugin_version_choice(NewPluginVersionChoice {
            plugin_id: "example".to_owned(),
            active_version: Some("1.0.0".to_owned()),
            previous_version: Some("0.9.0".to_owned()),
            staged_version: None,
            update_policy: "manual".to_owned(),
        })
        .await
        .expect("choice");
    let session = directory.path().join("data/torrent-session");
    std::fs::create_dir_all(session.join("nested")).expect("session");
    std::fs::write(session.join("session.json"), br#"{"torrents":{}}"#).expect("session file");
    std::fs::write(session.join("nested/piece.bin"), [7_u8; 300]).expect("nested");
    std::fs::write(session.join("session.json.rdownloader.tmp"), b"half").expect("tmp");
    let torrents = directory.path().join("data/torrents");
    std::fs::create_dir_all(&torrents).expect("torrents");
    std::fs::write(torrents.join("ubuntu.torrent"), b"d4:infoe").expect("torrent");
    Setup {
        directory,
        database,
    }
}

fn sources(setup: &Setup) -> BackupSources {
    BackupSources {
        settings_bundle: format!(
            r#"{{"format":"rdownloader-settings-bundle","probe":"{CANARY}"}}"#
        )
        .into_bytes(),
        torrent_session: Some(setup.directory.path().join("data/torrent-session")),
        torrent_files: Some(setup.directory.path().join("data/torrents")),
        app_version: "1.6.0-test".to_owned(),
        instance_id: "0a1b2c3d".to_owned(),
    }
}

#[tokio::test]
async fn a_backup_opens_with_the_passphrase_and_every_part_matches_its_manifest() {
    let setup = setup().await;
    let key = BackupKey::derive_new("correct horse battery")
        .await
        .expect("key");
    let destination = LocalFolder::open(&setup.directory.path().join("nas"))
        .await
        .expect("destination");
    let staging = staging_root(&setup.directory.path().join("data"));
    let started = chrono::Utc::now();
    let created = create_backup(
        &setup.database,
        sources(&setup),
        &key,
        &destination,
        &staging,
        "run-1",
        started,
    )
    .await
    .expect("backup");
    sweep_staging(&staging).await.expect("sweep");
    assert!(!staging.exists());

    let archive = std::path::PathBuf::from(&created.location);
    assert!(archive.starts_with(setup.directory.path().join("nas")));
    assert_eq!(
        std::fs::metadata(&archive).expect("archive").len(),
        created.size_bytes
    );

    // Nothing is readable without the key: not the settings bundle, not a table.
    let raw = std::fs::read(&archive).expect("read archive");
    for needle in [
        CANARY.as_bytes(),
        b"community-v1".as_slice(),
        b"SQLite format 3".as_slice(),
    ] {
        assert!(
            !raw.windows(needle.len()).any(|window| window == needle),
            "{} readable in the archive",
            String::from_utf8_lossy(needle)
        );
    }

    // A restore asks for the passphrase again and derives the key from the header's salt.
    let header = rd_backup::stream::read_header(&archive).expect("header");
    let restored_key = BackupKey::derive("correct horse battery", header.salt)
        .await
        .expect("derive");
    let opened = setup.directory.path().join("opened");
    let manifest = extract_archive(&archive, &restored_key, &opened).expect("extract");
    assert_eq!(manifest, created.manifest);
    assert_eq!(manifest.app_version, "1.6.0-test");
    let names: Vec<&str> = manifest
        .parts
        .iter()
        .map(|part| part.name.as_str())
        .collect();
    for expected in [
        "database.sqlite3",
        "settings.json",
        "plugin-trust.json",
        "partial-transfers.json",
        "torrent-session/session.json",
        "torrent-session/nested/piece.bin",
        "torrents/ubuntu.torrent",
    ] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
    assert!(
        !names.iter().any(|name| name.ends_with(".tmp")),
        "an uncommitted engine write was copied"
    );
    assert_eq!(
        manifest.part("torrents/ubuntu.torrent").expect("part").kind,
        PartKind::TorrentFile
    );
    assert!(
        std::fs::read_to_string(opened.join("settings.json"))
            .expect("settings")
            .contains(CANARY)
    );

    // The wrong passphrase opens nothing.
    let wrong = BackupKey::derive("wrong horse battery", header.salt)
        .await
        .expect("derive");
    assert!(extract_archive(&archive, &wrong, &setup.directory.path().join("wrong")).is_err());
}

#[tokio::test]
async fn the_plugin_trust_comes_back_through_the_archive_table_for_table() {
    let setup = setup().await;
    let key = BackupKey::derive_new("correct horse battery")
        .await
        .expect("key");
    let destination = LocalFolder::open(&setup.directory.path().join("nas"))
        .await
        .expect("destination");
    let staging = staging_root(&setup.directory.path().join("data"));
    let created = create_backup(
        &setup.database,
        sources(&setup),
        &key,
        &destination,
        &staging,
        "run-trust",
        chrono::Utc::now(),
    )
    .await
    .expect("backup");
    let opened = setup.directory.path().join("opened");
    extract_archive(std::path::Path::new(&created.location), &key, &opened).expect("extract");

    // The trust part says what the live tables said.
    let part: serde_json::Value = serde_json::from_slice(
        &std::fs::read(opened.join("plugin-trust.json")).expect("trust part"),
    )
    .expect("json");
    let live =
        rd_db::snapshot::read_tables(setup.database.path(), rd_db::snapshot::PLUGIN_TRUST_TABLES)
            .await
            .expect("live tables");
    assert_eq!(part["tables"], serde_json::to_value(&live).expect("json"));

    // And the database copy, opened as a database, answers with the same trust.
    let restored = Database::open(opened.join("database.sqlite3"))
        .await
        .expect("restored database");
    assert_eq!(
        restored.list_plugin_trusted_keys().await.expect("keys"),
        setup
            .database
            .list_plugin_trusted_keys()
            .await
            .expect("keys")
    );
    assert_eq!(
        restored
            .list_plugin_repositories()
            .await
            .expect("repositories"),
        setup
            .database
            .list_plugin_repositories()
            .await
            .expect("repositories")
    );
    assert_eq!(
        restored
            .list_plugin_version_choices()
            .await
            .expect("choices"),
        setup
            .database
            .list_plugin_version_choices()
            .await
            .expect("choices")
    );
}

#[tokio::test]
async fn a_member_the_manifest_does_not_name_is_refused() {
    // The archive is written only by `write_archive`, so a forged one is built here by hand
    // from the same pieces: a manifest naming one part, and a second, unnamed member.
    let directory = TempDir::new().expect("temp");
    let key = BackupKey::derive_new("correct horse battery")
        .await
        .expect("key");
    let staging = directory.path().join("staging");
    std::fs::create_dir_all(&staging).expect("staging");
    std::fs::write(staging.join("settings.json"), b"{}").expect("part");
    let (size, sha256) =
        rd_backup::archive::digest_file(&staging.join("settings.json")).expect("digest");
    let manifest = rd_backup::Manifest::new(
        chrono::Utc::now(),
        "test".to_owned(),
        vec![rd_backup::ManifestPart {
            name: "settings.json".to_owned(),
            kind: PartKind::Settings,
            size,
            sha256,
        }],
    );
    let archive = directory.path().join("forged.rdbackup");
    {
        let file = std::fs::File::create(&archive).expect("create");
        let sealing = rd_backup::stream::SealingWriter::new(file, &key).expect("seal");
        let mut builder = tar::Builder::new(sealing);
        let manifest_bytes = serde_json::to_vec(&manifest).expect("manifest");
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_bytes.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        builder
            .append_data(&mut header, "manifest.json", manifest_bytes.as_slice())
            .expect("manifest");
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o600);
        header.set_cksum();
        builder
            .append_data(&mut header, "settings.json", b"{}".as_slice())
            .expect("part");
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o600);
        header.set_cksum();
        builder
            .append_data(&mut header, "extra.sh", b"evil".as_slice())
            .expect("extra");
        builder.into_inner().expect("tar").finish().expect("finish");
    }
    let opened = directory.path().join("opened");
    let refused = extract_archive(&archive, &key, &opened).expect_err("unnamed member");
    assert!(refused.to_string().contains("extra.sh"), "{refused:#}");
    assert!(!opened.join("extra.sh").exists());
}
