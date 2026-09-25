use std::path::PathBuf;

use anyhow::Context;
use rd_files::ArchiveKind;

use crate::{
    ArchiveLimits, ArchiveSet, ExternalRarTool, ExtractionError, ExtractionReport, RarToolKind,
    archive::{create_staging, create_staging_inside, merge_tree, promote},
    multipart::MultiVolumeReader,
    rar::extract_rar_into,
    sevenz_format::extract_seven_zip,
    zip_format::extract_zip,
};

/// One extraction job for a complete archive set.
pub struct ExtractRequest<'a> {
    pub set: &'a ArchiveSet,
    /// Output directory. Must not exist unless `merge` is set.
    pub destination: PathBuf,
    pub limits: ArchiveLimits,
    pub rar_tool: Option<ExternalRarTool>,
    /// Move the extracted tree into an existing `destination`, replacing same-named entries.
    pub merge: bool,
    /// Receives progress samples while a backend extracts.
    pub progress: Option<crate::ProgressSender>,
}

/// Tries every password candidate in order (each attempt uses a fresh staging directory)
/// and promotes the first successful result. Returns the password that worked.
pub async fn extract_with_passwords(
    request: ExtractRequest<'_>,
    candidates: &[Option<String>],
) -> Result<(ExtractionReport, Option<String>), ExtractionError> {
    if request.merge {
        if !request.destination.is_dir() {
            return Err(ExtractionError::Other(anyhow::anyhow!(
                "extraction destination is not a directory"
            )));
        }
    } else if request.destination.exists() {
        return Err(ExtractionError::Other(anyhow::anyhow!(
            "extraction destination already exists"
        )));
    }
    let mut last_password_error = None;
    for candidate in candidates {
        let staging = if request.merge {
            create_staging_inside(&request.destination)?
        } else {
            create_staging(&request.destination)?
        };
        let attempt = extract_once(&request, &staging, candidate.as_deref()).await;
        match attempt {
            Ok(report) => {
                if request.merge {
                    // The long form for the moves as well: an extracted tree that only fits
                    // under `\\?\` cannot be moved into place with a path that does not.
                    merge_tree(&staging, &rd_files::long_path(&request.destination))?;
                    let _ = std::fs::remove_dir_all(&staging);
                } else {
                    promote(&staging, &rd_files::long_path(&request.destination))?;
                }
                return Ok((report, candidate.clone()));
            }
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&staging).await;
                if error.is_password_problem() {
                    // Keep the most specific verdict rather than the last one: the `None`
                    // candidate always answers "a password is required", which would otherwise
                    // bury what the real password attempt found out (RD-107-11).
                    if last_password_error
                        .as_ref()
                        .is_none_or(|previous| verdict_rank(&error) > verdict_rank(previous))
                    {
                        last_password_error = Some(error);
                    }
                    continue;
                }
                return Err(error);
            }
        }
    }
    Err(last_password_error.unwrap_or(ExtractionError::PasswordRequired))
}

/// How much a password verdict tells the user; higher wins when several attempts failed.
const fn verdict_rank(error: &ExtractionError) -> u8 {
    match error {
        ExtractionError::PasswordRequired => 0,
        ExtractionError::PasswordOrDataDamaged => 1,
        _ => 2,
    }
}

async fn extract_once(
    request: &ExtractRequest<'_>,
    staging: &std::path::Path,
    password: Option<&str>,
) -> Result<ExtractionReport, ExtractionError> {
    let limits = request.limits;
    let set = request.set;
    match set.kind {
        ArchiveKind::Rar => {
            let tool = request.rar_tool.as_ref().ok_or_else(|| {
                ExtractionError::Unsupported("no external RAR tool configured".to_owned())
            })?;
            extract_rar_into(
                tool,
                set.first(),
                staging,
                limits,
                password,
                request.progress.as_ref(),
            )
            .await
        }
        ArchiveKind::Zip if set.is_multipart() => {
            let tool = request
                .rar_tool
                .as_ref()
                .filter(|tool| tool.kind == RarToolKind::SevenZip)
                .ok_or_else(|| {
                    ExtractionError::Unsupported(
                        "split ZIP requires the external 7z tool".to_owned(),
                    )
                })?;
            extract_rar_into(
                tool,
                set.first(),
                staging,
                limits,
                password,
                request.progress.as_ref(),
            )
            .await
        }
        ArchiveKind::Zip => {
            let archive = set.first().to_owned();
            let staging = staging.to_owned();
            let password = password.map(str::to_owned);
            let progress = request.progress.clone();
            tokio::task::spawn_blocking(move || {
                extract_zip(
                    &archive,
                    &staging,
                    limits,
                    password.as_deref(),
                    progress.as_ref(),
                )
            })
            .await
            .context("join ZIP extraction")?
        }
        ArchiveKind::SevenZip => {
            let volumes = set.volumes.clone();
            let staging = staging.to_owned();
            let password = password.map(str::to_owned);
            let progress = request.progress.clone();
            tokio::task::spawn_blocking(move || {
                if volumes.len() == 1 {
                    let file = std::fs::File::open(&volumes[0])?;
                    extract_seven_zip(
                        file,
                        &staging,
                        limits,
                        password.as_deref(),
                        progress.as_ref(),
                    )
                } else {
                    let reader = MultiVolumeReader::open(&volumes)?;
                    extract_seven_zip(
                        reader,
                        &staging,
                        limits,
                        password.as_deref(),
                        progress.as_ref(),
                    )
                }
            })
            .await
            .context("join 7z extraction")?
        }
    }
}
