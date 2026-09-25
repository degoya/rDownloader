use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
};

use anyhow::{Context, Result, bail};
use sevenz_rust2::Password;

use crate::{
    ArchiveLimits, ExtractionError, ExtractionReport, ProgressSender,
    archive::{enforce_limits, safe_relative, validate_tree},
    progress::{ExtractProgress, report as report_progress},
};

/// Extracts a 7z archive (single file or a `MultiVolumeReader`) into `staging`.
pub(crate) fn extract_seven_zip<R: Read + Seek>(
    reader: R,
    staging: &Path,
    limits: ArchiveLimits,
    password: Option<&str>,
    progress: Option<&ProgressSender>,
) -> Result<ExtractionReport, ExtractionError> {
    let mut report = ExtractionReport::default();
    let secret = password.map_or_else(Password::empty, Password::from);
    let result = sevenz_rust2::decompress_with_extract_fn_and_password(
        reader,
        staging,
        secret,
        // `_destination` is the path the library computed, and it is deliberately ignored: the
        // guard below only means anything if the checked name is also the written one. The two
        // agree today because `sevenz_rust2` derives the destination from the very same
        // `entry.name()` — a library that stops doing so would silently decouple the check from
        // the write, with nothing here failing to compile. The ZIP reader joins the checked name
        // itself for the same reason (`zip_format.rs`).
        |entry, reader, _destination| {
            let result = (|| -> Result<()> {
                let relative = safe_relative(entry.name())?;
                if relative.as_os_str().is_empty() {
                    return Ok(());
                }
                let output = staging.join(relative);
                if entry.is_directory() {
                    std::fs::create_dir_all(&output)?;
                    return Ok(());
                }
                report.files = report.files.saturating_add(1);
                report.uncompressed_bytes = report
                    .uncompressed_bytes
                    .checked_add(entry.size())
                    .context("7z size overflow")?;
                enforce_limits(report, limits)?;
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut target = File::create(&output)?;
                let copied = std::io::copy(
                    &mut reader.take(entry.size().saturating_add(1)),
                    &mut target,
                )?;
                if copied != entry.size() {
                    bail!("7z member size changed during extraction");
                }
                target.flush()?;
                report_progress(
                    progress,
                    ExtractProgress {
                        done_bytes: report.uncompressed_bytes,
                        total_bytes: None,
                        percent: None,
                        current: Some(entry.name().to_owned()),
                    },
                );
                Ok(())
            })();
            result.map(|()| true).map_err(|error| {
                sevenz_rust2::Error::from(std::io::Error::other(error.to_string()))
            })
        },
    );
    match result {
        Ok(()) => {
            validate_tree(staging, limits)?;
            Ok(report)
        }
        Err(error) => Err(map_error(error, password.is_some())),
    }
}

/// `MaybeBadPassword` and a failed checksum are the in-process twins of what `7z` reports as
/// exit 2: an encrypted member that would not come out. Neither says whether the key or the data
/// was wrong, so neither is reported as a certainty any more (RD-107-11).
fn map_error(error: sevenz_rust2::Error, with_password: bool) -> ExtractionError {
    match error {
        sevenz_rust2::Error::PasswordRequired => ExtractionError::PasswordRequired,
        sevenz_rust2::Error::MaybeBadPassword(_) if !with_password => {
            ExtractionError::PasswordRequired
        }
        sevenz_rust2::Error::MaybeBadPassword(_) => ExtractionError::PasswordOrDataDamaged,
        // Without a password the reader would have raised `PasswordRequired` for an encrypted
        // member, so a checksum failure here is plain damage.
        sevenz_rust2::Error::ChecksumVerificationFailed if !with_password => {
            ExtractionError::DataDamaged
        }
        sevenz_rust2::Error::ChecksumVerificationFailed => ExtractionError::PasswordOrDataDamaged,
        other => ExtractionError::Other(anyhow::anyhow!("{other}")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::map_error;
    use crate::{ExtractionError, archive::safe_relative};

    /// The written path is the checked path, not whatever the library handed the callback.
    #[test]
    fn a_member_is_written_under_the_name_that_was_checked() {
        let staging = Path::new("/staging");
        assert!(
            safe_relative("../../etc/passwd").is_err(),
            "an escaping name must never reach a join"
        );
        assert_eq!(
            staging.join(safe_relative(r"sub\dir/./file.bin").expect("an ordinary member")),
            Path::new("/staging/sub/dir/file.bin")
        );
    }

    #[test]
    fn an_encrypted_member_that_failed_is_never_a_certain_wrong_password() {
        // RD-107-11: the in-process reader is in exactly the position `7z` exit 2 is - it cannot
        // separate a wrong key from damaged bytes, so it must not claim it can.
        assert!(matches!(
            map_error(sevenz_rust2::Error::ChecksumVerificationFailed, true),
            ExtractionError::PasswordOrDataDamaged
        ));
        assert!(matches!(
            map_error(
                sevenz_rust2::Error::MaybeBadPassword(std::io::Error::other("bad")),
                true
            ),
            ExtractionError::PasswordOrDataDamaged
        ));
    }

    #[test]
    fn without_a_password_a_checksum_failure_is_plain_damage() {
        assert!(matches!(
            map_error(sevenz_rust2::Error::ChecksumVerificationFailed, false),
            ExtractionError::DataDamaged
        ));
        assert!(matches!(
            map_error(sevenz_rust2::Error::PasswordRequired, false),
            ExtractionError::PasswordRequired
        ));
    }
}
