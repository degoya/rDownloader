//! Fetching a manifest entry's bytes and turning them into a staged version directory.
//!
//! Three limits apply before anything is written anywhere the resolver can see it.
//!
//! * **The hash decides.** The bytes are hashed while they stream and compared against the
//!   manifest before the staging directory is promoted. A mismatch removes everything and
//!   reports [`ToolError::HashMismatch`]; there is no "install it anyway".
//! * **A size cap.** The manifest declares a size and the stream is cut off the moment it
//!   exceeds it. Without that, a mirror that answers with an endless body fills the disk of
//!   an installation that only asked for a 30 MB binary.
//! * **A timeout.** The whole transfer, not just the connect, so a stalled body cannot hold a
//!   request open forever.
//!
//! Unpacking adds two more. Every archive member is flattened to its base name, so no member
//! path can be written outside the staging directory, and the bytes an archive expands to are
//! counted against [`MAX_UNPACKED_BYTES`] as they are written rather than as the archive's own
//! headers claim them.

use std::{io::Read as _, path::Path};

use anyhow::Context;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use crate::{
    error::ToolError,
    manifest::{ArchiveFormat, ToolEntry},
};

/// Hard ceiling on one tool download, whatever the manifest declares.
///
/// A full ffmpeg build is around 100 MB; half a gigabyte is generous and still bounded.
pub const MAX_TOOL_BYTES: u64 = 512 * 1024 * 1024;

/// How long one tool download may take in total.
pub const DOWNLOAD_TIMEOUT_SECONDS: u64 = 900;

/// Largest amount this will unpack out of one archive, so a decompression bomb cannot expand
/// into the data directory.
///
/// A budget for the whole archive rather than a per-member cap: a thousand members just under
/// a per-member limit fill a disk exactly as well as one member over it, and an xz stream
/// says nothing trustworthy about how much it expands to until it has been expanded.
pub const MAX_UNPACKED_BYTES: u64 = MAX_TOOL_BYTES;

/// Downloads `entry`, verifies its hash and lays the executables out in `staging`.
///
/// On any failure the caller discards the staging directory; nothing here promotes anything.
pub async fn fetch_into(
    client: &reqwest::Client,
    entry: &ToolEntry,
    staging: &Path,
) -> Result<(), ToolError> {
    let payload = tokio::time::timeout(
        std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECONDS),
        fetch_bytes(client, entry),
    )
    .await
    .map_err(|_| ToolError::DownloadFailed {
        name: entry.name.clone(),
        reason: format!("no answer within {DOWNLOAD_TIMEOUT_SECONDS} seconds"),
    })??;
    unpack(entry, payload, staging).await
}

/// Streams the body, hashing as it goes, and refuses it when the digest does not match.
async fn fetch_bytes(client: &reqwest::Client, entry: &ToolEntry) -> Result<Vec<u8>, ToolError> {
    let limit = entry.size.min(MAX_TOOL_BYTES);
    let response =
        client
            .get(&entry.url)
            .send()
            .await
            .map_err(|error| ToolError::DownloadFailed {
                name: entry.name.clone(),
                reason: error.to_string(),
            })?;
    if !response.status().is_success() {
        return Err(ToolError::DownloadFailed {
            name: entry.name.clone(),
            reason: format!("the server answered {}", response.status()),
        });
    }
    let mut hasher = Sha256::new();
    let mut collected: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| ToolError::DownloadFailed {
            name: entry.name.clone(),
            reason: error.to_string(),
        })?;
        if collected.len() as u64 + chunk.len() as u64 > limit {
            return Err(ToolError::DownloadFailed {
                name: entry.name.clone(),
                reason: format!("the body exceeds the declared {limit} bytes"),
            });
        }
        hasher.update(&chunk);
        collected.extend_from_slice(&chunk);
    }
    if collected.len() as u64 != entry.size {
        return Err(ToolError::DownloadFailed {
            name: entry.name.clone(),
            reason: format!(
                "the body is {} bytes, the manifest declares {}",
                collected.len(),
                entry.size
            ),
        });
    }
    let digest = hex::encode(hasher.finalize());
    if !digest.eq_ignore_ascii_case(&entry.sha256) {
        return Err(ToolError::HashMismatch {
            name: entry.name.clone(),
            version: entry.version.clone(),
        });
    }
    Ok(collected)
}

/// Writes the verified bytes into the staging directory, unpacking when the entry says so.
async fn unpack(entry: &ToolEntry, payload: Vec<u8>, staging: &Path) -> Result<(), ToolError> {
    match entry.archive {
        ArchiveFormat::Raw => {
            let destination = staging.join(executable_name(&entry.name));
            tokio::fs::write(&destination, &payload)
                .await
                .with_context(|| format!("write {}", destination.display()))
                .map_err(ToolError::Other)?;
            make_executable(&destination).await;
            Ok(())
        }
        ArchiveFormat::Zip | ArchiveFormat::TarXz => {
            let format = entry.archive;
            let staging = staging.to_path_buf();
            let members = entry.members.clone();
            let name = entry.name.clone();
            let written = tokio::task::spawn_blocking(move || {
                if format == ArchiveFormat::Zip {
                    extract_zip(&payload, &members, &staging)
                } else {
                    extract_tar_xz(&payload, &members, &staging)
                }
            })
            .await
            .map_err(|error| ToolError::Other(error.into()))?
            .map_err(|error| ToolError::DownloadFailed {
                name: name.clone(),
                reason: error.to_string(),
            })?;
            if written.is_empty() {
                return Err(ToolError::DownloadFailed {
                    name,
                    reason: "the archive held none of the members the manifest named".to_owned(),
                });
            }
            for path in written {
                make_executable(&path).await;
            }
            Ok(())
        }
    }
}

/// Extracts the named members (or every regular file) of a ZIP into one flat directory.
fn extract_zip(
    payload: &[u8],
    members: &[String],
    staging: &Path,
) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(payload)).context("read tool archive")?;
    let mut written = Vec::new();
    let mut budget = MAX_UNPACKED_BYTES;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        if !file.is_file() {
            continue;
        }
        let full_name = file.name().to_owned();
        let Some(base) = wanted_member(&full_name, members)? else {
            continue;
        };
        let destination = staging.join(base);
        write_member(&mut file, &destination, &full_name, &mut budget)?;
        written.push(destination);
    }
    Ok(written)
}

/// Extracts the named members (or every regular file) of an xz-compressed tar into one flat
/// directory.
///
/// Only regular files are taken. A tar can also carry symlinks, hard links, devices and
/// directories, and every one of them is a way to make an extraction write somewhere it was
/// not asked to; none of them is a way to deliver an executable, so none of them is followed.
fn extract_tar_xz(
    payload: &[u8],
    members: &[String],
    staging: &Path,
) -> anyhow::Result<Vec<std::path::PathBuf>> {
    // `false`: a single xz stream. Concatenated streams are legal xz and no tool release
    // uses them, so accepting them would only widen what a release host can append to a
    // build whose hash was published before the bytes were served.
    let reader = lzma_rust2::XzReader::new(std::io::Cursor::new(payload), false);
    let mut archive = tar::Archive::new(reader);
    let mut written = Vec::new();
    let mut budget = MAX_UNPACKED_BYTES;
    for entry in archive.entries().context("read tool archive")? {
        let mut entry = entry.context("read tool archive member")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let full_name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        let Some(base) = wanted_member(&full_name, members)? else {
            continue;
        };
        let destination = staging.join(base);
        write_member(&mut entry, &destination, &full_name, &mut budget)?;
        written.push(destination);
    }
    Ok(written)
}

/// The flat file name a wanted archive member lands under, or `None` when it was not asked
/// for.
///
/// Flattening to the base name is what makes a member path harmless: the resolver looks for
/// `<version>/<name>` and nothing else, so a nested archive layout must not survive into the
/// store — and a name with no separator left in it cannot climb out of the staging directory,
/// whatever the archive claims its entries are called. [`crate::store::validate_segment`]
/// then refuses what is left if it is still not usable as a file name.
fn wanted_member(full_name: &str, members: &[String]) -> anyhow::Result<Option<String>> {
    let Some(base) = full_name
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
    else {
        return Ok(None);
    };
    let wanted = members.is_empty()
        || members
            .iter()
            .any(|member| member == full_name || member == base);
    if !wanted {
        return Ok(None);
    }
    crate::store::validate_segment(base)
        .map_err(|_| anyhow::anyhow!("archive member {full_name} has an unusable name"))?;
    Ok(Some(base.to_owned()))
}

/// Writes one archive member out, spending at most `budget` bytes on it.
///
/// The budget counts bytes as they are written rather than trusting what the archive says a
/// member expands to: a header is a claim, and the whole point of the limit is the case where
/// that claim is a lie.
fn write_member(
    source: &mut impl std::io::Read,
    destination: &Path,
    full_name: &str,
    budget: &mut u64,
) -> anyhow::Result<()> {
    let mut sink = std::fs::File::create(destination)
        .with_context(|| format!("write {}", destination.display()))?;
    let allowed = *budget;
    let mut limited = source.take(allowed.saturating_add(1));
    let copied = std::io::copy(&mut limited, &mut sink)?;
    if copied > allowed {
        anyhow::bail!("archive member {full_name} exceeds the unpack limit");
    }
    *budget = allowed - copied;
    Ok(())
}

/// The file name a raw download lands under, `.exe` included where the platform wants one.
#[must_use]
pub fn executable_name(tool: &str) -> String {
    if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_owned()
    }
}

/// Marks a freshly written file executable on Unix. A no-op elsewhere, where the extension
/// decides.
async fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).await;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
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
        let payload = tar_xz(&[
            ("ffmpeg-n9.0.1-linux64-gpl/bin/ffmpeg", b"ffmpeg-binary"),
            ("ffmpeg-n9.0.1-linux64-gpl/bin/ffprobe", b"ffprobe-binary"),
            ("ffmpeg-n9.0.1-linux64-gpl/README.txt", b"not wanted"),
        ]);
        let mut wanted = entry(
            ArchiveFormat::TarXz,
            vec!["ffmpeg-n9.0.1-linux64-gpl/bin/ffmpeg".to_owned()],
        );
        wanted.name = "ffmpeg".to_owned();
        unpack(&wanted, payload, directory.path())
            .await
            .expect("unpack");

        let written = directory.path().join("ffmpeg");
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
}
