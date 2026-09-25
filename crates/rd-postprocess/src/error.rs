use std::fmt;

/// Extraction outcome classification used to drive password retries and the user-facing message.
///
/// The split between "the password is wrong", "the data is damaged" and "the tool cannot tell"
/// is not cosmetic: it decides whether trying another password can possibly help, and it is what
/// the user reads. RD-107-11 measured what `unrar` and `7z` actually report; see
/// `docs/postprocessing.md` for the exit-code table these variants are derived from.
#[derive(Debug)]
pub enum ExtractionError {
    /// The archive is encrypted and no password was supplied.
    PasswordRequired,
    /// The tool said the password is wrong and is in a position to know
    /// (`unrar` exit 11, or 7-Zip refusing an encrypted header).
    WrongPassword,
    /// The tool failed a checksum inside an encrypted member and cannot say which of the two it
    /// was: RAR4 without header encryption carries no password check value, and 7-Zip reports
    /// both cases as exit 2. Another password may still succeed, so this keeps the retry going.
    PasswordOrDataDamaged,
    /// The payload is damaged and the password is not in question
    /// (`unrar` exit 3 on a format that verified the password, a truncated archive).
    DataDamaged,
    /// The configured tool kind and the configured executable contradict each other, so the tool
    /// rejected the command line (`unrar`/7-Zip exit 7). Retrying passwords cannot help.
    ToolMismatch(String),
    /// `unrar` older than 6.10 rejected `-op`, the switch the destination travels in since
    /// RD-120-56. The bundled `unrar` is 7.23; this is a configured or `PATH` one. Retrying
    /// passwords cannot help, and the old positional form cannot come back as a fallback: it is
    /// exactly what failed under Windows for a path with a space.
    ToolTooOld(String),
    /// The archive layout is not supported by the available tools.
    Unsupported(String),
    /// The tool could not create or write the file it was extracting (`unrar` exit 5, 6 and 9).
    ///
    /// Nothing to do with the archive: the destination is what refused. On Windows that is
    /// almost always the 260-character path limit, which an extracted tree crosses easily
    /// (RD-108-30); elsewhere it is permissions or a full disk. Naming it apart matters
    /// because the raw tool text reads like a broken archive, and the user goes looking for
    /// the wrong thing.
    CannotWrite(String),
    /// Any other failure (I/O, limits, validation).
    Other(anyhow::Error),
}

impl fmt::Display for ExtractionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PasswordRequired => f.write_str("archive requires a password"),
            Self::WrongPassword => f.write_str("archive password is wrong"),
            Self::PasswordOrDataDamaged => {
                f.write_str("archive password is wrong or the data is damaged")
            }
            Self::DataDamaged => f.write_str("archive data is damaged"),
            Self::ToolMismatch(detail) => write!(f, "RAR tool mismatch: {detail}"),
            Self::ToolTooOld(detail) => {
                write!(
                    f,
                    "the RAR tool is too old, unrar 6.10 or newer is needed: {detail}"
                )
            }
            Self::Unsupported(reason) => write!(f, "unsupported archive: {reason}"),
            Self::CannotWrite(detail) => {
                write!(f, "the extracted file could not be written: {detail}")
            }
            Self::Other(error) => write!(f, "{error:#}"),
        }
    }
}

impl std::error::Error for ExtractionError {}

impl From<anyhow::Error> for ExtractionError {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

impl From<std::io::Error> for ExtractionError {
    fn from(error: std::io::Error) -> Self {
        Self::Other(error.into())
    }
}

impl ExtractionError {
    /// Whether trying another password could succeed.
    #[must_use]
    pub const fn is_password_problem(&self) -> bool {
        matches!(
            self,
            Self::PasswordRequired | Self::WrongPassword | Self::PasswordOrDataDamaged
        )
    }

    /// Stable identifier the frontend translates; never a free-text message.
    ///
    /// These values are part of the interface: they are stored in the `code` field of the
    /// post-processing step and looked up in `web/src/locales/*/server.json` under `codes`,
    /// beside every other step code. Do not rename one without the catalogues.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::PasswordRequired => "extract.password_required",
            Self::WrongPassword => "extract.wrong_password",
            Self::PasswordOrDataDamaged => "extract.password_or_data_damaged",
            Self::DataDamaged => "extract.data_damaged",
            Self::ToolMismatch(_) => "extract.tool_mismatch",
            Self::ToolTooOld(_) => "extract.tool_too_old",
            Self::Unsupported(_) => "extract.unsupported",
            Self::CannotWrite(_) => "extract.cannot_write",
            Self::Other(_) => "extract.failed",
        }
    }

    /// The tool's own words, where the code alone does not tell a reader what to do.
    ///
    /// Travels as the `detail` parameter of the step code, which is what the catalogues
    /// interpolate; the four verdicts about a password or damaged data say everything in the
    /// code itself and carry no detail (RD-108-08).
    #[must_use]
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::PasswordRequired
            | Self::WrongPassword
            | Self::PasswordOrDataDamaged
            | Self::DataDamaged => None,
            Self::ToolMismatch(detail)
            | Self::ToolTooOld(detail)
            | Self::Unsupported(detail)
            | Self::CannotWrite(detail) => Some(detail.clone()),
            Self::Other(error) => Some(format!("{error:#}")),
        }
    }

    /// The English step text that travels beside [`code`](Self::code) as the fallback.
    ///
    /// Plain prose, with no code in front of it: the code has a field of its own since
    /// RD-107-04, and prefixing the text was the workaround from the time it did not
    /// (RD-108-08). The text is what a build that does not know the code yet still shows.
    #[must_use]
    pub fn message(&self) -> String {
        self.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::ExtractionError;

    #[test]
    fn damaged_data_stops_the_password_retry_but_ambiguity_does_not() {
        assert!(!ExtractionError::DataDamaged.is_password_problem());
        assert!(!ExtractionError::ToolMismatch("x".to_owned()).is_password_problem());
        assert!(ExtractionError::PasswordOrDataDamaged.is_password_problem());
        assert!(ExtractionError::WrongPassword.is_password_problem());
        assert!(ExtractionError::PasswordRequired.is_password_problem());
    }

    /// One wire format: the code is a field, the message is text (RD-108-08).
    #[test]
    fn every_variant_names_itself_in_the_code_and_never_in_the_message() {
        for error in [
            ExtractionError::PasswordRequired,
            ExtractionError::WrongPassword,
            ExtractionError::PasswordOrDataDamaged,
            ExtractionError::DataDamaged,
            ExtractionError::ToolMismatch("detail".to_owned()),
            ExtractionError::ToolTooOld("detail".to_owned()),
            ExtractionError::Unsupported("reason".to_owned()),
            ExtractionError::Other(anyhow::anyhow!("boom")),
        ] {
            assert!(error.code().starts_with("extract."), "{}", error.code());
            assert!(!error.message().contains("extract."), "{error}");
        }
    }

    /// The detail is what the code cannot say: the tool's own words, and only where there are any.
    #[test]
    fn only_the_variants_with_something_to_add_carry_a_detail() {
        assert_eq!(ExtractionError::DataDamaged.detail(), None);
        assert_eq!(ExtractionError::WrongPassword.detail(), None);
        assert_eq!(
            ExtractionError::ToolMismatch("unrar vs 7z".to_owned()).detail(),
            Some("unrar vs 7z".to_owned())
        );
        assert_eq!(
            ExtractionError::Unsupported("solid RAR5".to_owned()).detail(),
            Some("solid RAR5".to_owned())
        );
        assert_eq!(
            ExtractionError::Other(anyhow::anyhow!("boom")).detail(),
            Some("boom".to_owned())
        );
    }
}
