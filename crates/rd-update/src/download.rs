//! The verified download: an artifact streamed to disk and kept only if it is exactly the file
//! the manifest describes (RD-180-01). The updater of RD-180-02 installs from what this returns
//! and from nothing else.
//!
//! The bytes go to `<name>.partial` beside the target and are hashed on the way; the size is
//! held to the manifest's at every chunk, so a server that sends more is cut off, not read to
//! the end. Only a file whose size and SHA-256 both match is renamed to its final name; anything
//! else is deleted, so a half or a forged download is never found lying where an installer
//! would look.
//!
//! A verified file stays where it is until the update that installs it is cleaned up, so the
//! interface can download in the background and the install use what is already there
//! ([`verified_file`], owner 2026-10-01): the file is read and hashed again before it is used.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{Fetcher, manifest::Artifact, manifest::UpdateError};

/// Downloads `artifact` into `directory` and returns the verified file's path.
pub async fn download_verified(
    fetcher: &dyn Fetcher,
    artifact: &Artifact,
    directory: &Path,
) -> Result<PathBuf, UpdateError> {
    download_verified_with(fetcher, artifact, directory, &|_| {}).await
}

/// [`download_verified`], telling `received` how many bytes are written after every chunk.
pub async fn download_verified_with(
    fetcher: &dyn Fetcher,
    artifact: &Artifact,
    directory: &Path,
    received: &(dyn Fn(u64) + Send + Sync),
) -> Result<PathBuf, UpdateError> {
    let url = https_url(artifact)?;
    let target = directory.join(file_name(&url));
    let partial = target.with_file_name(format!(
        "{}.partial",
        target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|error| UpdateError::Other(error.into()))?;
    let outcome = stream_to(fetcher, &url, artifact, &partial, received).await;
    match outcome {
        Ok(()) => {
            tokio::fs::rename(&partial, &target)
                .await
                .map_err(|error| UpdateError::Other(error.into()))?;
            Ok(target)
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&partial).await;
            Err(error)
        }
    }
}

/// The file an earlier [`download_verified`] of `artifact` left in `directory`, if it is still
/// exactly the one the manifest describes: its size and SHA-256 are read again.
pub async fn verified_file(artifact: &Artifact, directory: &Path) -> Option<PathBuf> {
    let target = directory.join(file_name(&https_url(artifact).ok()?));
    let mut file = tokio::fs::File::open(&target).await.ok()?;
    if file.metadata().await.ok()?.len() != artifact.size {
        return None;
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    (hex::encode(hasher.finalize()) == artifact.sha256).then_some(target)
}

fn https_url(artifact: &Artifact) -> Result<url::Url, UpdateError> {
    let url = url::Url::parse(&artifact.url)
        .map_err(|error| UpdateError::Invalid(format!("artifact URL: {error}")))?;
    if url.scheme() != "https" {
        return Err(UpdateError::Invalid(format!("{url} is not https")));
    }
    Ok(url)
}

async fn stream_to(
    fetcher: &dyn Fetcher,
    url: &url::Url,
    artifact: &Artifact,
    partial: &Path,
    received: &(dyn Fn(u64) + Send + Sync),
) -> Result<(), UpdateError> {
    let mut download = fetcher
        .open(url)
        .await
        .map_err(|error| UpdateError::Download(format!("{error:#}")))?;
    if let Some(length) = download.length
        && length != artifact.size
    {
        return Err(UpdateError::SizeMismatch {
            expected: artifact.size,
            saw: length,
        });
    }
    let mut file = tokio::fs::File::create(partial)
        .await
        .map_err(|error| UpdateError::Other(error.into()))?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    while let Some(chunk) = download.chunks.next().await {
        let chunk = chunk.map_err(|error| UpdateError::Download(format!("{error:#}")))?;
        written = written.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if written > artifact.size {
            return Err(UpdateError::SizeMismatch {
                expected: artifact.size,
                saw: written,
            });
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| UpdateError::Other(error.into()))?;
        received(written);
    }
    file.sync_all()
        .await
        .map_err(|error| UpdateError::Other(error.into()))?;
    drop(file);
    if written != artifact.size {
        return Err(UpdateError::SizeMismatch {
            expected: artifact.size,
            saw: written,
        });
    }
    if hex::encode(hasher.finalize()) != artifact.sha256 {
        return Err(UpdateError::DigestMismatch);
    }
    Ok(())
}

/// The URL's last segment, reduced to characters that are safe in a file name everywhere.
fn file_name(url: &url::Url) -> String {
    let last = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    let cleaned: String = last
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
        .collect();
    let cleaned = cleaned.trim_start_matches('.');
    if cleaned.is_empty() {
        "rdownloader-update.bin".to_owned()
    } else {
        cleaned.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryFetcher;

    const URL: &str = "https://github.com/degoya/rDownloader/releases/download/v1.8.0/rdownloader-linux-x86_64.tar.gz";

    fn artifact_for(body: &[u8]) -> Artifact {
        Artifact {
            platform: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            kind: "archive".to_owned(),
            url: URL.to_owned(),
            sha256: hex::encode(Sha256::digest(body)),
            size: body.len() as u64,
        }
    }

    fn body() -> Vec<u8> {
        (0..200u8).collect()
    }

    #[tokio::test]
    async fn a_matching_download_is_kept_under_its_name() {
        let directory = tempfile::tempdir().expect("tempdir");
        let fetcher = MemoryFetcher::new();
        fetcher.serve(URL, body());
        let path = download_verified(&fetcher, &artifact_for(&body()), directory.path())
            .await
            .expect("download");
        assert_eq!(
            path,
            directory.path().join("rdownloader-linux-x86_64.tar.gz")
        );
        assert_eq!(std::fs::read(&path).expect("read"), body());
    }

    #[tokio::test]
    async fn a_kept_download_is_found_again_only_while_it_is_the_signed_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        let fetcher = MemoryFetcher::new();
        fetcher.serve(URL, body());
        let artifact = artifact_for(&body());
        assert_eq!(verified_file(&artifact, directory.path()).await, None);
        let seen = std::sync::Mutex::new(Vec::new());
        let path = download_verified_with(&fetcher, &artifact, directory.path(), &|bytes| {
            seen.lock().expect("seen").push(bytes);
        })
        .await
        .expect("download");
        assert_eq!(seen.lock().expect("seen").last(), Some(&artifact.size));
        assert_eq!(
            verified_file(&artifact, directory.path()).await,
            Some(path.clone())
        );
        // Changed on disk since: not the signed file any more.
        let mut changed = body();
        changed[3] ^= 0xff;
        std::fs::write(&path, changed).expect("change");
        assert_eq!(verified_file(&artifact, directory.path()).await, None);
    }

    #[tokio::test]
    async fn a_download_with_the_wrong_hash_is_refused_and_removed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let fetcher = MemoryFetcher::new();
        let mut forged = body();
        forged[17] ^= 0xff;
        fetcher.serve(URL, forged);
        let error = download_verified(&fetcher, &artifact_for(&body()), directory.path())
            .await
            .expect_err("bad hash");
        assert_eq!(error.code(), "update.digest_mismatch");
        assert_eq!(
            std::fs::read_dir(directory.path()).expect("list").count(),
            0
        );
    }

    #[tokio::test]
    async fn a_download_of_the_wrong_size_is_refused_and_removed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let fetcher = MemoryFetcher::new();
        let mut longer = body();
        longer.push(0);
        fetcher.serve(URL, longer);
        let error = download_verified(&fetcher, &artifact_for(&body()), directory.path())
            .await
            .expect_err("too long");
        assert_eq!(error.code(), "update.size_mismatch");
        assert_eq!(
            std::fs::read_dir(directory.path()).expect("list").count(),
            0
        );
    }

    #[tokio::test]
    async fn an_unreachable_artifact_is_a_download_failure() {
        let directory = tempfile::tempdir().expect("tempdir");
        let error = download_verified(
            &MemoryFetcher::new(),
            &artifact_for(&body()),
            directory.path(),
        )
        .await
        .expect_err("404");
        assert_eq!(error.code(), "update.download_failed");
    }

    #[test]
    fn the_file_name_cannot_leave_the_directory() {
        let url = url::Url::parse("https://example.test/a/..%2F..%2Fetc%2Fpasswd").expect("url");
        assert!(!file_name(&url).contains('/'));
        let url = url::Url::parse("https://example.test/").expect("url");
        assert_eq!(file_name(&url), "rdownloader-update.bin");
    }
}
