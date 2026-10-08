//! Archives built by hand to break the reader (RD-1190-22): a link member, a member twice, and a
//! member longer than its manifest entry. The archive is written only by `write_archive`, so each
//! forged one is built here from the same pieces, sealed with the right key -- what an attacker
//! who knows the passphrase could hand over.

use rd_backup::{BackupKey, ManifestPart, PartKind, archive::extract_archive};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// One member of a forged archive: its name, its tar type and its bytes.
struct Member<'a> {
    name: &'a str,
    kind: tar::EntryType,
    content: &'a [u8],
}

fn plain<'a>(name: &'a str, content: &'a [u8]) -> Member<'a> {
    Member {
        name,
        kind: tar::EntryType::Regular,
        content,
    }
}

/// Seals a manifest naming `settings.json` with `stated` as its content, followed by `members`.
async fn forged(
    directory: &TempDir,
    stated: &[u8],
    members: &[Member<'_>],
) -> (std::path::PathBuf, BackupKey) {
    let key = BackupKey::derive_new("correct horse battery")
        .await
        .expect("key");
    let manifest = rd_backup::Manifest::new(
        chrono::Utc::now(),
        "test".to_owned(),
        vec![ManifestPart {
            name: "settings.json".to_owned(),
            kind: PartKind::Settings,
            size: stated.len() as u64,
            sha256: hex::encode(Sha256::digest(stated)),
        }],
    );
    let archive = directory.path().join("forged.rdbackup");
    let file = std::fs::File::create(&archive).expect("create");
    let sealing = rd_backup::stream::SealingWriter::new(file, &key).expect("seal");
    let mut builder = tar::Builder::new(sealing);
    let manifest_bytes = serde_json::to_vec(&manifest).expect("manifest");
    append(&mut builder, &plain("manifest.json", &manifest_bytes));
    for member in members {
        append(&mut builder, member);
    }
    builder.into_inner().expect("tar").finish().expect("finish");
    (archive, key)
}

fn append(builder: &mut tar::Builder<impl std::io::Write>, member: &Member<'_>) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(member.kind);
    header.set_size(member.content.len() as u64);
    header.set_mode(0o600);
    if member.kind == tar::EntryType::Symlink {
        header.set_link_name("/etc/passwd").expect("link name");
    }
    header.set_cksum();
    builder
        .append_data(&mut header, member.name, member.content)
        .expect("member");
}

async fn refused(stated: &[u8], members: &[Member<'_>]) -> (String, TempDir) {
    let directory = TempDir::new().expect("temp");
    let (archive, key) = forged(&directory, stated, members).await;
    let opened = directory.path().join("opened");
    let error = extract_archive(&archive, &key, &opened).expect_err("a forged archive");
    (format!("{error:#}"), directory)
}

#[tokio::test]
async fn a_link_member_is_refused_and_never_created() {
    let link = Member {
        name: "settings.json",
        kind: tar::EntryType::Symlink,
        content: b"",
    };
    let (error, directory) = refused(b"{}", &[link]).await;
    assert!(error.contains("not a plain file"), "{error}");
    assert!(
        std::fs::symlink_metadata(directory.path().join("opened/settings.json")).is_err(),
        "the link was created"
    );
}

#[tokio::test]
async fn a_member_twice_is_refused() {
    let (error, _directory) = refused(
        b"{}",
        &[plain("settings.json", b"{}"), plain("settings.json", b"{}")],
    )
    .await;
    assert!(error.contains("appears twice"), "{error}");
}

#[tokio::test]
async fn a_member_longer_than_its_manifest_entry_is_refused() {
    let (error, _directory) = refused(b"{}", &[plain("settings.json", b"{\"forged\":true}")]).await;
    assert!(
        error.contains("does not match its manifest entry"),
        "{error}"
    );
}
