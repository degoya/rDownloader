//! The commands of `writer/archive_passwords.rs`.

use super::Reply;

/// The commands `Writer::handle_archive_passwords` applies.
// The variant names were the flat enum's; within one area they share a postfix.
#[allow(clippy::enum_variant_names)]
pub(crate) enum ArchivePasswordsCommand {
    /// Records vault references before their values are written (RD-190-04).
    ReserveArchivePasswords {
        references: Vec<String>,
        reply: Reply<()>,
    },
    /// Hands reservations whose values could not be written to the sweep.
    ReleaseArchivePasswords {
        references: Vec<String>,
        reply: Reply<()>,
    },
    /// Points rows at their new references and empties the plain column in one transaction.
    AdoptArchivePasswords {
        table: crate::archive_password::PasswordTable,
        entries: Vec<(String, Option<String>)>,
        reply: Reply<()>,
    },
    /// Drops the sweep rows whose vault entries were removed.
    ForgetArchivePasswords {
        references: Vec<String>,
        reply: Reply<()>,
    },
}
