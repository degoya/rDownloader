//! The in-process extraction boundaries that had no test of their own (security review
//! 2026-09-28, `docs/security/archive-extraction.md`): a ZIP symlink member, and the member-count
//! and declared-size limits, which must refuse a member before it is written - `validate_tree`
//! enforces the same limits afterwards, so only a file that is not there proves the early check.

use std::{fs::File, io::Write, path::Path};

use zip::write::SimpleFileOptions;

use crate::{
    ArchiveKind, ArchiveLimits, ArchiveSet, ExtractRequest, ExtractionError,
    extract_with_passwords, sevenz_format::extract_seven_zip, zip_format::extract_zip,
};

/// A ZIP with one member per `(name, size)`, each filled with that many bytes.
fn zip_with(path: &Path, members: &[(&str, usize)]) {
    let mut zip = zip::ZipWriter::new(File::create(path).expect("create ZIP"));
    for (name, size) in members {
        zip.start_file(*name, SimpleFileOptions::default())
            .expect("start member");
        zip.write_all(&vec![b'x'; *size]).expect("write member");
    }
    zip.finish().expect("finish ZIP");
}

fn limits(max_files: u64, max_uncompressed_bytes: u64) -> ArchiveLimits {
    ArchiveLimits {
        max_files,
        max_uncompressed_bytes,
    }
}

/// T-SLIP's sibling: a ZIP member that is a symbolic link to a path outside the package. The
/// link is refused before anything is created, so neither the link nor the destination exists.
#[tokio::test]
async fn a_zip_symlink_member_is_refused_before_anything_is_written() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let archive = temp.path().join("link.zip");
    let mut zip = zip::ZipWriter::new(File::create(&archive).expect("create ZIP"));
    zip.add_symlink("innocent.txt", "/etc/passwd", SimpleFileOptions::default())
        .expect("symlink member");
    zip.finish().expect("finish ZIP");

    let staging = temp.path().join("staging");
    std::fs::create_dir_all(&staging).expect("staging");
    let refused = extract_zip(&archive, &staging, ArchiveLimits::default(), None, None);
    assert!(
        matches!(&refused, Err(ExtractionError::Unsupported(reason)) if reason.contains("symbolic")),
        "{refused:?}"
    );
    assert!(std::fs::symlink_metadata(staging.join("innocent.txt")).is_err());

    // Through the whole flow: the attempt's staging is removed and no destination appears.
    let destination = temp.path().join("result");
    let set = ArchiveSet {
        kind: ArchiveKind::Zip,
        base: "link".to_owned(),
        volumes: vec![archive],
    };
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
}

/// One member more than `max_files` is refused before the extra member is written.
#[test]
fn a_zip_over_the_member_count_limit_stops_before_the_extra_member() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let archive = temp.path().join("many.zip");
    zip_with(
        &archive,
        &[("one.txt", 1), ("two.txt", 1), ("three.txt", 1)],
    );
    let staging = temp.path().join("staging");
    std::fs::create_dir_all(&staging).expect("staging");

    let error = extract_zip(&archive, &staging, limits(2, u64::MAX), None, None)
        .expect_err("three members over a limit of two");
    assert!(error.to_string().contains("file-count limit"), "{error}");
    assert!(
        staging.join("two.txt").exists(),
        "members up to the limit are written"
    );
    assert!(
        !staging.join("three.txt").exists(),
        "the member past the limit was written"
    );
}

/// A member whose declared size passes `max_uncompressed_bytes` is refused before a byte of it
/// is written.
#[test]
fn a_zip_over_the_size_limit_stops_before_the_member_is_written() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let archive = temp.path().join("large.zip");
    zip_with(&archive, &[("small.bin", 10), ("large.bin", 5_000)]);
    let staging = temp.path().join("staging");
    std::fs::create_dir_all(&staging).expect("staging");

    let error = extract_zip(&archive, &staging, limits(100, 1_000), None, None)
        .expect_err("5 010 bytes over a limit of 1 000");
    assert!(
        error.to_string().contains("uncompressed-size limit"),
        "{error}"
    );
    assert!(staging.join("small.bin").exists());
    assert!(
        !staging.join("large.bin").exists(),
        "the oversized member was written"
    );
}

/// The same two limits in the 7z reader.
#[test]
fn a_seven_zip_archive_over_either_limit_stops_before_the_member_is_written() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = temp.path().join("source");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(source.join("large.bin"), vec![7_u8; 5_000]).expect("payload");
    let archive = temp.path().join("large.7z");
    sevenz_rust2::compress_to_path(&source, &archive).expect("compress 7z");

    for (name, bounds, reason) in [
        ("size", limits(100, 1_000), "uncompressed-size limit"),
        ("count", limits(0, u64::MAX), "file-count limit"),
    ] {
        let staging = temp.path().join(name);
        std::fs::create_dir_all(&staging).expect("staging");
        let error = extract_seven_zip(
            File::open(&archive).expect("archive"),
            &staging,
            bounds,
            None,
            None,
        )
        .expect_err("over the limit");
        assert!(error.to_string().contains(reason), "{name}: {error}");
        assert!(
            !staging.join("large.bin").exists(),
            "{name}: the member past the limit was written"
        );
    }
}
