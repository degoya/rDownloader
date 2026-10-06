use super::*;

fn entry(archive: ArchiveFormat, members: Vec<String>) -> ToolEntry {
    ToolEntry {
        name: "yt-dlp".to_owned(),
        version: "2024.09.07".to_owned(),
        platform: "x86_64-unknown-linux-gnu".to_owned(),
        url: "https://example.invalid/yt-dlp".to_owned(),
        sha256: "0".repeat(64),
        size: 4,
        archive,
        members,
        min_app_version: None,
        max_app_version: None,
    }
}

#[tokio::test]
async fn a_raw_payload_lands_under_the_tool_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    unpack(
        &entry(ArchiveFormat::Raw, Vec::new()),
        b"body".to_vec(),
        directory.path(),
    )
    .await
    .expect("unpack");
    let written = directory.path().join(executable_name("yt-dlp"));
    assert_eq!(
        tokio::fs::read(&written).await.expect("read"),
        b"body".to_vec()
    );
}

/// One tar block for `name`, with the member name written straight into the header.
///
/// `tar::Builder` refuses to write a path containing `..`, which is precisely the path a
/// hostile archive would carry, so the header is assembled here and its checksum
/// recomputed over the patched name.
fn tar_block(name: &str, body: &[u8]) -> Vec<u8> {
    assert!(name.len() < 100, "the test names fit the ustar name field");
    let mut header = tar::Header::new_ustar();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    let mut block = header.as_bytes().to_vec();
    block[..name.len()].copy_from_slice(name.as_bytes());
    // The checksum is computed with its own field read as spaces, so it has to be
    // rewritten after the name it covers changed.
    block[148..156].fill(b' ');
    let sum: u32 = block.iter().map(|byte| u32::from(*byte)).sum();
    block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    block.extend_from_slice(body);
    block.resize(block.len().div_ceil(512) * 512, 0);
    block
}

/// Compresses `bytes` as a single xz stream.
fn xz(bytes: &[u8]) -> Vec<u8> {
    let mut compressed = Vec::new();
    let mut writer =
        lzma_rust2::XzWriter::new(&mut compressed, lzma_rust2::XzOptions::with_preset(1))
            .expect("xz writer");
    std::io::Write::write_all(&mut writer, bytes).expect("compress");
    writer.finish().expect("finish the xz stream");
    compressed
}

/// Builds a `.tar.xz` in memory, so the extraction cases need no committed binary
/// fixture and no network.
fn tar_xz(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tarball = Vec::new();
    for (name, body) in members {
        tarball.extend_from_slice(&tar_block(name, body));
    }
    // The two zero blocks that end a tar.
    tarball.resize(tarball.len() + 1024, 0);
    xz(&tarball)
}

/// The FFmpeg case: a member deep inside the archive lands in the version directory under
/// its base name, and it is executable once it is there.
#[tokio::test]
async fn a_tar_xz_member_lands_flat_under_its_base_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let member = format!(
        "ffmpeg-n9.0.1-linux64-gpl/bin/{}",
        executable_name("ffmpeg")
    );
    let payload = tar_xz(&[
        (member.as_str(), b"ffmpeg-binary"),
        ("ffmpeg-n9.0.1-linux64-gpl/bin/ffprobe", b"ffprobe-binary"),
        ("ffmpeg-n9.0.1-linux64-gpl/README.txt", b"not wanted"),
    ]);
    let mut wanted = entry(ArchiveFormat::TarXz, vec![member.clone()]);
    wanted.name = "ffmpeg".to_owned();
    unpack(&wanted, payload, directory.path())
        .await
        .expect("unpack");

    let written = directory.path().join(executable_name("ffmpeg"));
    assert_eq!(
        tokio::fs::read(&written).await.expect("read"),
        b"ffmpeg-binary".to_vec()
    );
    assert!(!directory.path().join("ffprobe").exists());
    assert!(!directory.path().join("README.txt").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&written)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "the binary has to be executable");
    }
}

/// An archive that delivers the program under another platform's name is refused: the
/// resolver would never find it, so activating it would report an install that is not one.
#[tokio::test]
async fn an_archive_member_under_the_other_platforms_name_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let foreign = if cfg!(windows) {
        "ffmpeg"
    } else {
        "ffmpeg.exe"
    };
    let member = format!("ffmpeg-n9.0.1-linux64-gpl/bin/{foreign}");
    let payload = tar_xz(&[(member.as_str(), b"ffmpeg-binary")]);
    let mut wanted = entry(ArchiveFormat::TarXz, vec![member.clone()]);
    wanted.name = "ffmpeg".to_owned();
    let result = unpack(&wanted, payload, directory.path()).await;
    match result {
        Err(ToolError::DownloadFailed { reason, .. }) => {
            assert!(reason.contains(&executable_name("ffmpeg")), "{reason}");
        }
        other => panic!("expected a refused download, got {other:?}"),
    }
}

/// The same flattening rule as for a ZIP, on the format that actually carries directories.
#[test]
fn a_tar_xz_member_cannot_escape_the_staging_directory() {
    let directory = tempfile::tempdir().expect("tempdir");
    let payload = tar_xz(&[("../../escaped/ffmpeg", b"body")]);
    let written = extract_tar_xz(&payload, &[], directory.path()).expect("extract");
    assert_eq!(written, vec![directory.path().join("ffmpeg")]);
    assert!(!directory.path().join("../../escaped").exists());
}

/// Bytes that are not an xz stream are refused rather than half-unpacked.
#[test]
fn a_corrupt_tar_xz_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    assert!(extract_tar_xz(b"not an xz stream at all", &[], directory.path()).is_err());

    // A valid xz stream whose payload is not a tar fails at the tar layer instead.
    assert!(extract_tar_xz(&xz(b"still not a tar"), &[], directory.path()).is_err());

    // And a truncated stream, which is what a half-delivered body looks like.
    let payload = tar_xz(&[("bin/ffmpeg", b"body")]);
    let truncated = &payload[..payload.len() / 2];
    assert!(extract_tar_xz(truncated, &[], directory.path()).is_err());
}

/// The unpack budget counts what is written, not what a header promises: a member that
/// keeps producing bytes is cut off rather than allowed to fill the disk.
#[test]
fn a_member_larger_than_the_remaining_budget_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let destination = directory.path().join("ffmpeg");
    let mut budget = 8_u64;
    let mut source = std::io::Cursor::new(vec![0_u8; 4096]);
    let result = write_member(&mut source, &destination, "bin/ffmpeg", &mut budget);
    assert!(result.is_err(), "{result:?}");
}

/// Two members that flatten to the same name would overwrite each other, and the budget
/// still has to account for both.
#[test]
fn every_written_member_is_charged_against_one_archive_budget() {
    let directory = tempfile::tempdir().expect("tempdir");
    let mut budget = 10_u64;
    let mut first = std::io::Cursor::new(vec![b'a'; 6]);
    write_member(
        &mut first,
        &directory.path().join("ffmpeg"),
        "bin/ffmpeg",
        &mut budget,
    )
    .expect("first member");
    assert_eq!(budget, 4);
    let mut second = std::io::Cursor::new(vec![b'b'; 6]);
    assert!(
        write_member(
            &mut second,
            &directory.path().join("ffprobe"),
            "bin/ffprobe",
            &mut budget,
        )
        .is_err()
    );
}

/// A member path that tries to climb out of the directory is flattened to its base name,
/// so nothing is ever written above the staging directory.
#[test]
fn an_archive_member_cannot_escape_the_staging_directory() {
    let directory = tempfile::tempdir().expect("tempdir");
    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("../../escaped/yt-dlp", options)
            .expect("start");
        std::io::Write::write_all(&mut writer, b"body").expect("write");
        writer.finish().expect("finish");
    }
    let written = extract_zip(&buffer, &[], directory.path()).expect("extract");
    assert_eq!(written, vec![directory.path().join("yt-dlp")]);
    assert!(!directory.path().join("../../escaped").exists());
}
