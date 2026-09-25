//! Turning a non-zero exit of `unrar` / `7z` into a verdict.
//!
//! RD-107-11 measured both tools instead of guessing (the table lives in
//! `docs/postprocessing.md`). The exit code is the primary signal and replaces the substring
//! `password` that used to decide this — that substring made every checksum error and every
//! command-line error look like a wrong password. But the exit code alone is not enough either:
//! 7-Zip reports a wrong password and a damaged encrypted member both as exit 2, and `unrar` on a
//! RAR4 volume without header encryption reports both as exit 3 with the same sentence, because
//! that format carries no password check value. Those cases get their own verdict rather than a
//! confident wrong one.

use crate::{ExtractionError, RarToolKind};

/// `unrar`: the password was checked and rejected.
const UNRAR_BAD_PASSWORD: i32 = 11;
/// `unrar`: a CRC or checksum failed, or the archive ended early.
const UNRAR_CHECKSUM: i32 = 3;
/// 7-Zip: fatal error. Everything encryption-related lands here.
const SEVEN_ZIP_FATAL: i32 = 2;
/// Both tools: the command line was rejected. That is what a kind/executable swap looks like.
const COMMAND_LINE_ERROR: i32 = 7;
/// `unrar`: write error, open error and create error - the destination, not the archive.
const UNRAR_WRITE_ERRORS: [i32; 3] = [5, 6, 9];
/// `unrar` before 6.10 does not know `-op` and names it: `ERROR: Unknown option: op<path>`.
const UNRAR_NO_OP_SWITCH: &str = "unknown option: op";
/// 7-Zip says it in words rather than in an exit code of its own.
const SEVEN_ZIP_CANNOT_WRITE: &str = "can not open output file";

/// `unrar`, RAR4 without header encryption: it cannot tell the two apart and says so.
const UNRAR_AMBIGUOUS: &str = "corrupt file or wrong password";
/// 7-Zip could not decrypt the header, so the password itself is what failed.
const SEVEN_ZIP_HEADER: &str = "cannot open encrypted archive";
/// 7-Zip failed inside an encrypted member: "Data Error in encrypted file" / "CRC Failed in
/// encrypted file". Either a wrong password or damaged data.
const SEVEN_ZIP_MEMBER: &str = "in encrypted file";

/// Classifies a failed run. `output` is the tool's stderr and stdout, already lowercased.
pub(crate) fn classify_failure(
    kind: RarToolKind,
    code: Option<i32>,
    output: &str,
    password: Option<&str>,
) -> ExtractionError {
    if code == Some(COMMAND_LINE_ERROR) {
        if kind == RarToolKind::Unrar && output.contains(UNRAR_NO_OP_SWITCH) {
            return ExtractionError::ToolTooOld(first_line(output));
        }
        return ExtractionError::ToolMismatch(first_line(output));
    }
    if let Some(code) = code
        && kind == RarToolKind::Unrar
        && UNRAR_WRITE_ERRORS.contains(&code)
    {
        return ExtractionError::CannotWrite(first_line(output));
    }
    match (kind, code) {
        (RarToolKind::Unrar, Some(UNRAR_BAD_PASSWORD)) => certain(password),
        (RarToolKind::Unrar, Some(UNRAR_CHECKSUM)) => {
            if output.contains(UNRAR_AMBIGUOUS) {
                ambiguous(password)
            } else {
                ExtractionError::DataDamaged
            }
        }
        (RarToolKind::SevenZip, Some(SEVEN_ZIP_FATAL)) => {
            if output.contains(SEVEN_ZIP_HEADER) {
                certain(password)
            } else if output.contains(SEVEN_ZIP_MEMBER) || output.contains("wrong password") {
                ambiguous(password)
            } else if output.contains(SEVEN_ZIP_CANNOT_WRITE) {
                ExtractionError::CannotWrite(first_line(output))
            } else if output.contains("crc failed") || output.contains("data error") {
                ExtractionError::DataDamaged
            } else {
                failed(code, output)
            }
        }
        _ => failed(code, output),
    }
}

/// The tool is in a position to know the password was wrong.
fn certain(password: Option<&str>) -> ExtractionError {
    if password.is_none() {
        ExtractionError::PasswordRequired
    } else {
        ExtractionError::WrongPassword
    }
}

/// The tool cannot separate a wrong password from damaged data.
fn ambiguous(password: Option<&str>) -> ExtractionError {
    if password.is_none() {
        ExtractionError::PasswordRequired
    } else {
        ExtractionError::PasswordOrDataDamaged
    }
}

fn failed(code: Option<i32>, output: &str) -> ExtractionError {
    ExtractionError::Other(anyhow::anyhow!(
        "RAR tool failed (exit {}): {}",
        code.map_or_else(|| "signal".to_owned(), |value| value.to_string()),
        first_line(output)
    ))
}

fn first_line(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod write_error_tests {
    use super::classify_failure;
    use crate::{ExtractionError, RarToolKind};

    /// RD-108-30: the field failure. The archive and the password were both fine; the
    /// destination path was 275 characters long, and Windows stops at 260.
    #[test]
    fn a_create_error_is_about_the_destination_not_the_archive() {
        let error = classify_failure(
            RarToolKind::Unrar,
            Some(9),
            "cannot create d:\\downloads\\tv\\release\\release\\release.mkv\n",
            Some("secret"),
        );
        assert!(
            matches!(error, ExtractionError::CannotWrite(_)),
            "{error:?} must not read as a damaged archive or a wrong password"
        );
        assert_eq!(error.code(), "extract.cannot_write");
        assert!(!error.is_password_problem(), "another password cannot help");
    }

    #[test]
    fn seven_zip_says_it_in_words() {
        let error = classify_failure(
            RarToolKind::SevenZip,
            Some(2),
            "error: can not open output file d:\\x\\y.mkv\n",
            None,
        );
        assert_eq!(error.code(), "extract.cannot_write");
    }
}

#[cfg(test)]
mod tests {
    use super::classify_failure;
    use crate::{ExtractionError, RarToolKind};

    // Every fixture below is output this job actually observed from unrar 7.12 / 6.24 and
    // 7zz 25.01; the exit codes are the ones those runs returned.

    #[test]
    fn unrar_exit_eleven_is_a_definite_password_verdict() {
        assert!(matches!(
            classify_failure(
                RarToolKind::Unrar,
                Some(11),
                "incorrect password for payload/note.txt",
                Some("nope")
            ),
            ExtractionError::WrongPassword
        ));
        assert!(matches!(
            classify_failure(
                RarToolKind::Unrar,
                Some(11),
                "incorrect password for payload/note.txt",
                None
            ),
            ExtractionError::PasswordRequired
        ));
    }

    #[test]
    fn unrar_checksum_error_is_damaged_data_not_a_wrong_password() {
        // RD-107-11: the old rule called this a wrong password because of "password" further up
        // the merged output; unrar 7.12 exits 3 here with the password already verified.
        assert!(matches!(
            classify_failure(
                RarToolKind::Unrar,
                Some(3),
                "payload/data.bin     - checksum error",
                Some("s3cret")
            ),
            ExtractionError::DataDamaged
        ));
        assert!(matches!(
            classify_failure(
                RarToolKind::Unrar,
                Some(3),
                "unexpected end of archive\npayload/data.bin     - checksum error",
                Some("s3cret")
            ),
            ExtractionError::DataDamaged
        ));
    }

    #[test]
    fn unrar_rar4_cannot_tell_and_the_verdict_says_so() {
        let observed = "checksum error in the encrypted file payload/data.bin. \
                        corrupt file or wrong password.";
        assert!(matches!(
            classify_failure(RarToolKind::Unrar, Some(3), observed, Some("s3cret")),
            ExtractionError::PasswordOrDataDamaged
        ));
        assert!(matches!(
            classify_failure(RarToolKind::Unrar, Some(3), observed, None),
            ExtractionError::PasswordRequired
        ));
    }

    #[test]
    fn seven_zip_separates_the_header_refusal_from_a_member_failure() {
        assert!(matches!(
            classify_failure(
                RarToolKind::SevenZip,
                Some(2),
                "error: hdr.7z\ncannot open encrypted archive. wrong password?",
                Some("nope")
            ),
            ExtractionError::WrongPassword
        ));
        assert!(matches!(
            classify_failure(
                RarToolKind::SevenZip,
                Some(2),
                "error: crc failed in encrypted file. wrong password? : payload/data.bin",
                Some("s3cret")
            ),
            ExtractionError::PasswordOrDataDamaged
        ));
        assert!(matches!(
            classify_failure(
                RarToolKind::SevenZip,
                Some(2),
                "error: data error in encrypted file. wrong password? : payload/note.txt",
                None
            ),
            ExtractionError::PasswordRequired
        ));
        assert!(matches!(
            classify_failure(
                RarToolKind::SevenZip,
                Some(2),
                "error: crc failed : payload/data.bin",
                Some("s3cret")
            ),
            ExtractionError::DataDamaged
        ));
    }

    #[test]
    fn a_rejected_command_line_is_a_tool_mismatch_for_both_tools() {
        assert!(matches!(
            classify_failure(
                RarToolKind::Unrar,
                Some(7),
                "\nerror: unknown option: bsp1",
                Some("s3cret")
            ),
            ExtractionError::ToolMismatch(_)
        ));
        let mismatch = classify_failure(
            RarToolKind::SevenZip,
            Some(7),
            "command line error:\nincorrect wildcard type marker",
            Some("s3cret"),
        );
        assert!(!mismatch.is_password_problem());
        assert_eq!(mismatch.code(), "extract.tool_mismatch");
    }

    /// RD-120-56: the destination travels as `-op` now, which unrar learnt in 6.10. An older one
    /// is refused with its own code rather than blamed on the settings.
    #[test]
    fn an_unrar_without_the_op_switch_is_too_old_not_a_mismatch() {
        let error = classify_failure(
            RarToolKind::Unrar,
            Some(7),
            "\nerror: unknown option: op/downloads/a b/.rd-xabc",
            Some("s3cret"),
        );
        assert!(matches!(error, ExtractionError::ToolTooOld(_)), "{error}");
        assert!(!error.is_password_problem());
        assert_eq!(error.code(), "extract.tool_too_old");
        assert_eq!(
            error.detail().as_deref(),
            Some("error: unknown option: op/downloads/a b/.rd-xabc")
        );
    }

    #[test]
    fn anything_else_stays_a_plain_failure_with_the_exit_code() {
        let error = classify_failure(
            RarToolKind::Unrar,
            Some(10),
            "\ncannot open trunc.rar\nno such file or directory",
            Some("s3cret"),
        );
        assert!(!error.is_password_problem());
        assert!(error.to_string().contains("exit 10"), "{error}");
    }
}
