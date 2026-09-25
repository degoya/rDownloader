use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};

use anyhow::Context;
use zip::result::ZipError;

use crate::{
    ArchiveLimits, ExtractionError, ExtractionReport, ProgressSender,
    archive::{enforce_limits, safe_relative, validate_tree},
    progress::{ExtractProgress, percent_of, report as report_progress},
};

/// Extracts a ZIP archive into `staging`; `password` unlocks AES/ZipCrypto members.
pub(crate) fn extract_zip(
    archive: &Path,
    staging: &Path,
    limits: ArchiveLimits,
    password: Option<&str>,
    progress: Option<&ProgressSender>,
) -> Result<ExtractionReport, ExtractionError> {
    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(map_zip_error)?;
    let mut report = ExtractionReport::default();
    let total: u64 = (0..zip.len())
        .filter_map(|index| zip.by_index_raw(index).ok().map(|member| member.size()))
        .sum();
    for index in 0..zip.len() {
        let encrypted = zip.by_index_raw(index).map_err(map_zip_error)?.encrypted();
        if encrypted && password.is_none() {
            return Err(ExtractionError::PasswordRequired);
        }
        let mut member = match password {
            Some(secret) if encrypted => zip
                .by_index_decrypt(index, secret.as_bytes())
                .map_err(map_zip_error)?,
            _ => zip.by_index(index).map_err(map_zip_error)?,
        };
        let relative = safe_relative(member.name())?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        if member
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
        {
            return Err(ExtractionError::Unsupported(
                "ZIP symbolic links are not supported".to_owned(),
            ));
        }
        let output = staging.join(relative);
        if member.is_dir() {
            std::fs::create_dir_all(&output)?;
            continue;
        }
        let declared_size = member.size();
        report.files = report.files.saturating_add(1);
        report.uncompressed_bytes = report
            .uncompressed_bytes
            .checked_add(declared_size)
            .context("ZIP size overflow")?;
        enforce_limits(report, limits)?;
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut target = File::create(output)?;
        let copied = std::io::copy(
            &mut member.by_ref().take(declared_size.saturating_add(1)),
            &mut target,
        )
        .map_err(|error| map_read_error(error, encrypted))?;
        target.flush()?;
        if copied != declared_size {
            if encrypted {
                // A short read of an encrypted member is either a wrong key or damaged data;
                // the ZIP reader cannot say which (RD-107-11).
                return Err(ExtractionError::PasswordOrDataDamaged);
            }
            return Err(ExtractionError::Other(anyhow::anyhow!(
                "ZIP member size changed during extraction"
            )));
        }
        report_progress(
            progress,
            ExtractProgress {
                done_bytes: report.uncompressed_bytes,
                total_bytes: Some(total),
                percent: percent_of(report.uncompressed_bytes, Some(total)),
                current: Some(member.name().to_owned()),
            },
        );
    }
    validate_tree(staging, limits)?;
    Ok(report)
}

fn map_zip_error(error: ZipError) -> ExtractionError {
    match error {
        ZipError::InvalidPassword => ExtractionError::WrongPassword,
        ZipError::UnsupportedArchive(ZipError::PASSWORD_REQUIRED) => {
            ExtractionError::PasswordRequired
        }
        ZipError::UnsupportedArchive(reason) => ExtractionError::Unsupported(reason.to_owned()),
        other => ExtractionError::Other(other.into()),
    }
}

fn map_read_error(error: std::io::Error, encrypted: bool) -> ExtractionError {
    if encrypted
        && matches!(
            error.kind(),
            std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
        )
    {
        // Undecipherable bytes in an encrypted member: a wrong key and a damaged member look
        // exactly alike here, so the verdict says both rather than picking one (RD-107-11).
        return ExtractionError::PasswordOrDataDamaged;
    }
    if !encrypted && error.kind() == std::io::ErrorKind::InvalidData {
        return ExtractionError::DataDamaged;
    }
    ExtractionError::Other(error.into())
}

#[cfg(test)]
mod tests {
    use super::{map_read_error, map_zip_error};
    use crate::ExtractionError;

    #[test]
    fn undecipherable_bytes_in_an_encrypted_member_name_both_causes() {
        // RD-107-11: this used to be reported as a certain wrong password.
        assert!(matches!(
            map_read_error(std::io::Error::from(std::io::ErrorKind::InvalidData), true),
            ExtractionError::PasswordOrDataDamaged
        ));
        assert!(matches!(
            map_read_error(std::io::Error::from(std::io::ErrorKind::InvalidData), false),
            ExtractionError::DataDamaged
        ));
    }

    #[test]
    fn a_rejected_aes_password_stays_a_certainty() {
        // AES ZIP carries a password verification value, so here the reader really does know.
        assert!(matches!(
            map_zip_error(zip::result::ZipError::InvalidPassword),
            ExtractionError::WrongPassword
        ));
    }
}
