use std::{fs::File, io::Write, path::PathBuf};

use zip::write::SimpleFileOptions;

use crate::{
    ArchiveKind, ArchiveLimits, ArchiveSet, ExtractRequest, ExtractionError,
    extract_with_passwords, group_archive_sets,
};

fn write_zip(path: &PathBuf, options: SimpleFileOptions) {
    let file = File::create(path).expect("create ZIP");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("folder/file.txt", options)
        .expect("start member");
    zip.write_all(b"safe").expect("write member");
    zip.finish().expect("finish ZIP");
}

fn single(path: PathBuf, kind: ArchiveKind) -> ArchiveSet {
    ArchiveSet {
        kind,
        base: "set".to_owned(),
        volumes: vec![path],
    }
}

#[tokio::test]
async fn extracts_a_safe_zip_through_staging() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let archive = temp.path().join("safe.zip");
    write_zip(&archive, SimpleFileOptions::default());
    let destination = temp.path().join("result");
    let set = single(archive, ArchiveKind::Zip);
    let (report, password) = extract_with_passwords(
        ExtractRequest {
            set: &set,
            destination: destination.clone(),
            limits: ArchiveLimits::default(),
            rar_tool: None,
            merge: false,
            progress: None,
        },
        &[None],
    )
    .await
    .expect("extract ZIP");
    assert_eq!(report.files, 1);
    assert_eq!(password, None);
    assert_eq!(
        std::fs::read(destination.join("folder/file.txt")).ok(),
        Some(b"safe".to_vec())
    );
}

#[tokio::test]
async fn rejects_zip_slip_without_creating_a_destination() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let archive = temp.path().join("escape.zip");
    let file = File::create(&archive).expect("create ZIP");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("../escape.txt", SimpleFileOptions::default())
        .expect("start member");
    zip.write_all(b"escape").expect("write member");
    zip.finish().expect("finish ZIP");
    let destination = temp.path().join("result");
    let set = single(archive, ArchiveKind::Zip);
    let result = extract_with_passwords(
        ExtractRequest {
            set: &set,
            destination: destination.clone(),
            limits: ArchiveLimits::default(),
            rar_tool: None,
            merge: false,
            progress: None,
        },
        &[None],
    )
    .await;
    assert!(result.is_err());
    assert!(!destination.exists());
    assert!(!temp.path().join("escape.txt").exists());
}

#[tokio::test]
async fn encrypted_zip_needs_the_right_password_from_the_candidate_list() {
    let temp = tempfile::dir_in_tempdir();
    let archive = temp.path().join("secret.zip");
    write_zip(
        &archive,
        SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "hunter2"),
    );
    let set = single(archive, ArchiveKind::Zip);
    let failed = extract_with_passwords(
        ExtractRequest {
            set: &set,
            destination: temp.path().join("wrong"),
            limits: ArchiveLimits::default(),
            rar_tool: None,
            merge: false,
            progress: None,
        },
        &[None, Some("nope".to_owned())],
    )
    .await;
    assert!(matches!(
        failed,
        Err(ExtractionError::WrongPassword | ExtractionError::PasswordRequired)
    ));
    assert!(!temp.path().join("wrong").exists());
    let destination = temp.path().join("right");
    let (_, password) = extract_with_passwords(
        ExtractRequest {
            set: &set,
            destination: destination.clone(),
            limits: ArchiveLimits::default(),
            rar_tool: None,
            merge: false,
            progress: None,
        },
        &[None, Some("nope".to_owned()), Some("hunter2".to_owned())],
    )
    .await
    .expect("extract with matching password");
    assert_eq!(password.as_deref(), Some("hunter2"));
    assert_eq!(
        std::fs::read(destination.join("folder/file.txt")).ok(),
        Some(b"safe".to_vec())
    );
}

#[tokio::test]
async fn encrypted_seven_zip_and_split_volumes_extract() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = temp.path().join("src");
    std::fs::create_dir_all(source.join("nested")).expect("source dir");
    std::fs::write(source.join("nested/data.bin"), vec![7_u8; 50_000]).expect("payload");
    let archive = temp.path().join("secret.7z");
    sevenz_rust2::compress_to_path_encrypted(&source, &archive, "pw123".into())
        .expect("compress encrypted 7z");

    let set = single(archive.clone(), ArchiveKind::SevenZip);
    let destination = temp.path().join("plain");
    let (report, password) = extract_with_passwords(
        ExtractRequest {
            set: &set,
            destination: destination.clone(),
            limits: ArchiveLimits::default(),
            rar_tool: None,
            merge: false,
            progress: None,
        },
        &[None, Some("pw123".to_owned())],
    )
    .await
    .expect("extract encrypted 7z");
    assert_eq!(report.files, 1);
    assert_eq!(password.as_deref(), Some("pw123"));
    assert_eq!(
        std::fs::metadata(destination.join("nested/data.bin"))
            .expect("extracted file")
            .len(),
        50_000
    );

    // Split the archive into two numbered volumes and extract through the multi-volume reader.
    let bytes = std::fs::read(&archive).expect("archive bytes");
    let split = bytes.len() / 2;
    let first = temp.path().join("secret.7z.001");
    let second = temp.path().join("secret.7z.002");
    std::fs::write(&first, &bytes[..split]).expect("volume 1");
    std::fs::write(&second, &bytes[split..]).expect("volume 2");
    let sets = group_archive_sets(&[second, first]);
    assert_eq!(sets.len(), 1);
    assert!(sets[0].is_multipart());
    let destination = temp.path().join("from-volumes");
    extract_with_passwords(
        ExtractRequest {
            set: &sets[0],
            destination: destination.clone(),
            limits: ArchiveLimits::default(),
            rar_tool: None,
            merge: false,
            progress: None,
        },
        &[Some("pw123".to_owned())],
    )
    .await
    .expect("extract split 7z");
    assert!(destination.join("nested/data.bin").exists());
}

mod tempfile {
    pub use ::tempfile::tempdir;

    pub fn dir_in_tempdir() -> ::tempfile::TempDir {
        tempdir().expect("temporary directory")
    }
}

#[tokio::test]
async fn zip_extraction_reports_monotonic_progress_ending_at_one_hundred() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let archive = temp.path().join("progress.zip");
    {
        let file = File::create(&archive).expect("create ZIP");
        let mut zip = zip::ZipWriter::new(file);
        for name in ["a.bin", "b.bin", "c.bin"] {
            zip.start_file(name, SimpleFileOptions::default())
                .expect("start member");
            zip.write_all(&[7_u8; 1000]).expect("write member");
        }
        zip.finish().expect("finish ZIP");
    }
    let destination = temp.path().join("out");
    let set = single(archive, ArchiveKind::Zip);
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    extract_with_passwords(
        ExtractRequest {
            set: &set,
            destination,
            limits: ArchiveLimits::default(),
            rar_tool: None,
            merge: false,
            progress: Some(sender),
        },
        &[None],
    )
    .await
    .expect("extract ZIP");
    let mut percents = Vec::new();
    while let Ok(sample) = receiver.try_recv() {
        percents.push(sample.percent.expect("byte-based percent"));
        assert_eq!(sample.total_bytes, Some(3000));
    }
    assert_eq!(percents, vec![33, 66, 100]);
}

fn seed_sfv_package(entries: &[(&str, &[u8])], index: &str) -> ::tempfile::TempDir {
    let temp = tempfile::tempdir().expect("temporary directory");
    for (name, contents) in entries {
        std::fs::write(temp.path().join(name), contents).expect("write payload");
    }
    std::fs::write(temp.path().join("release.sfv"), index).expect("write SFV");
    temp
}

#[test]
fn parses_comments_names_with_spaces_and_ignores_junk_lines() {
    let entries = crate::parse_sfv(concat!(
        "; Generated by WIN-SFV32 v1.1\n",
        "\n",
        "release.part1.rar 1a2B3c4D\n",
        "my holiday clip.mkv deadbeef\n",
        "no-checksum-here\n",
        "short.bin abc\n",
        "toolong.bin 0123456789\n",
        "not hex here zzzzzzzz\n",
    ));
    assert_eq!(
        entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.crc32.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("release.part1.rar", "1a2b3c4d"),
            ("my holiday clip.mkv", "deadbeef"),
        ]
    );
}

#[test]
fn recognises_the_sfv_extension_in_any_casing() {
    assert!(crate::is_sfv(std::path::Path::new("/pkg/release.SFV")));
    assert!(crate::is_sfv(std::path::Path::new("/pkg/release.sfv")));
    assert!(!crate::is_sfv(std::path::Path::new("/pkg/release.sfv.txt")));
    assert!(!crate::is_sfv(std::path::Path::new("/pkg/release.rar")));
}

#[tokio::test]
async fn verifies_matching_checksums_case_insensitively() {
    // CRC32("safe") = 0x1fa4288f, written upper-case to prove the comparison folds case.
    let temp = seed_sfv_package(&[("payload.bin", b"safe")], "payload.bin 1FA4288F\n");
    let report = crate::verify_sfv(
        temp.path().join("release.sfv"),
        temp.path().to_owned(),
        None,
    )
    .await
    .expect("verify SFV");
    assert!(report.is_ok());
    assert_eq!(report.checked, 1);
    assert_eq!(report.skipped, 0);
}

#[tokio::test]
async fn reports_a_mismatch_and_a_missing_file() {
    let temp = seed_sfv_package(
        &[("payload.bin", b"safe")],
        "payload.bin 00000000\ngone.bin deadbeef\n",
    );
    let report = crate::verify_sfv(
        temp.path().join("release.sfv"),
        temp.path().to_owned(),
        None,
    )
    .await
    .expect("verify SFV");
    assert!(!report.is_ok());
    assert_eq!(report.mismatched, vec!["payload.bin".to_owned()]);
    assert_eq!(report.missing, vec!["gone.bin".to_owned()]);
    assert_eq!(report.checked, 1);
}

#[tokio::test]
async fn refuses_entries_that_escape_the_package_directory() {
    let temp = seed_sfv_package(
        &[("payload.bin", b"safe")],
        "../outside.bin deadbeef\n/etc/passwd deadbeef\npayload.bin 1fa4288f\n",
    );
    let report = crate::verify_sfv(
        temp.path().join("release.sfv"),
        temp.path().to_owned(),
        None,
    )
    .await
    .expect("verify SFV");
    assert!(report.is_ok());
    assert_eq!(report.skipped, 2);
    assert_eq!(report.checked, 1);
    assert!(report.missing.is_empty());
}

#[tokio::test]
async fn reads_a_latin1_index_without_failing() {
    let temp = tempfile::tempdir().expect("temporary directory");
    std::fs::write(temp.path().join("payload.bin"), b"safe").expect("write payload");
    // 0xFC is a Latin-1 accented vowel and invalid UTF-8; the lossy decode must keep the
    // checksum line usable instead of failing the whole index.
    let mut index = b"; T\xFCbingen release\npayload.bin 1fa4288f\n".to_vec();
    index.push(b'\n');
    std::fs::write(temp.path().join("release.sfv"), index).expect("write SFV");
    let report = crate::verify_sfv(
        temp.path().join("release.sfv"),
        temp.path().to_owned(),
        None,
    )
    .await
    .expect("verify SFV");
    assert!(report.is_ok());
    assert_eq!(report.checked, 1);
}

#[tokio::test]
async fn reports_byte_progress_while_hashing() {
    let temp = seed_sfv_package(
        &[("a.bin", &[0_u8; 1000]), ("b.bin", &[0_u8; 3000])],
        "a.bin 060b1780\nb.bin da865b0d\n",
    );
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let report = crate::verify_sfv(
        temp.path().join("release.sfv"),
        temp.path().to_owned(),
        Some(&sender),
    )
    .await
    .expect("verify SFV");
    assert_eq!(report.checked, 2);
    let mut samples = Vec::new();
    while let Ok(sample) = receiver.try_recv() {
        assert_eq!(sample.total_bytes, Some(4000));
        samples.push(sample.percent.expect("byte-based percent"));
    }
    assert_eq!(samples.first(), Some(&0));
    assert_eq!(samples.last(), Some(&100));
}
