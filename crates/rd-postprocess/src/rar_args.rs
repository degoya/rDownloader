//! The command lines of the external RAR tools, built apart from the process that runs them.
//!
//! RD-120-56: until then the destination went to `unrar` as a positional argument with a
//! trailing separator. Under Windows a path with a space is quoted by Rust's `Command`, which
//! doubles the backslashes in front of the closing quote - `"D:\a b\.rd-xabc\\"` - and `unrar`
//! does not read its command line with the C runtime's rules but with its own parser
//! (`GetCmdParam`, `strfn.cpp`), which takes every backslash literally. The destination arrived
//! as `\\?\D:\a b\.rd-xabc\\`, and a verbatim path does not fold `\\` into one separator, so
//! every file failed with exit 9. A password with a `"` broke the same way.
//!
//! Two things fix it, and both are pure so the tests can check them on every platform: the
//! destination travels as `-op<staging>` (unrar 6.10 and later add the separator themselves, so
//! no argument ends in one), and under Windows every `unrar` argument is written onto the command
//! line in the form `unrar`'s own parser reads back unchanged ([`unrar_quote`]).

use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

use crate::RarToolKind;

/// What the tool is asked to do with the archive.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RarAction<'a> {
    /// Unpack the whole set into this (existing) directory.
    Extract { staging: &'a Path },
    /// Read every volume and check the stored checksums, writing nothing.
    Test,
}

/// One tool invocation's argument list, and which entry carries the password.
#[derive(Debug)]
pub(crate) struct RarArguments {
    pub(crate) args: Vec<OsString>,
    password_at: Option<usize>,
}

/// Builds the argument list for `kind`.
///
/// Every argument is non-empty - `unrar`'s parser has no way to receive an empty one, see
/// [`unrar_quote`] - and none ends in a path separator.
pub(crate) fn rar_arguments(
    kind: RarToolKind,
    action: RarAction<'_>,
    first_volume: &Path,
    password: Option<&str>,
) -> RarArguments {
    let mut args: Vec<OsString> = Vec::with_capacity(8);
    let command = match action {
        RarAction::Extract { .. } => "x",
        RarAction::Test => "t",
    };
    args.push(command.into());
    let switches: &[&str] = match (kind, action) {
        (RarToolKind::Unrar, RarAction::Extract { .. }) => &["-o-", "-y", "-idc"],
        (RarToolKind::Unrar, RarAction::Test) => &["-y", "-idc"],
        (RarToolKind::SevenZip, RarAction::Extract { .. }) => &["-y", "-bsp1", "-bso0"],
        (RarToolKind::SevenZip, RarAction::Test) => &["-y", "-bso0"],
    };
    args.extend(switches.iter().map(OsString::from));
    let password_at = password.map(|_| args.len());
    args.push(match (kind, password) {
        (_, Some(secret)) => format!("-p{secret}").into(),
        // unrar: "do not ask". 7-Zip: an empty password, so it never waits on the console.
        (RarToolKind::Unrar, None) => "-p-".into(),
        (RarToolKind::SevenZip, None) => "-p".into(),
    });
    if let RarAction::Extract { staging } = action {
        // `-op` for unrar, `-o` for 7-Zip: the destination is a switch, so it needs no trailing
        // separator to be read as a directory, and it stays in front of `--`.
        let prefix = match kind {
            RarToolKind::Unrar => "-op",
            RarToolKind::SevenZip => "-o",
        };
        args.push(joined(prefix, staging.as_os_str()));
    }
    args.push("--".into());
    args.push(match action {
        // The long form for the archive as well: its path can be as long as the destination's.
        RarAction::Extract { .. } => rd_files::long_path(first_volume).into_os_string(),
        RarAction::Test => first_volume.as_os_str().to_owned(),
    });
    RarArguments { args, password_at }
}

fn joined(prefix: &str, value: &OsStr) -> OsString {
    let mut out = OsString::from(prefix);
    out.push(value);
    out
}

impl RarArguments {
    /// The arguments as one line for the log, the password replaced by `-p***`.
    ///
    /// Only whether a password was passed is visible: `-p-` (unrar) or `-p` (7-Zip) for none,
    /// `-p***` for one. Its length is not.
    pub(crate) fn redacted(&self) -> String {
        self.args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                if Some(index) == self.password_at {
                    "-p***".to_owned()
                } else {
                    arg.to_string_lossy().into_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Hands the arguments to `command`.
    ///
    /// Under Windows `unrar` gets each argument pre-quoted for its own parser through `raw_arg`;
    /// Rust's quoting follows the C runtime's rules, which `unrar` does not. Everything else - 7-Zip,
    /// and every tool outside Windows, where no command line is re-parsed - takes the list as is.
    ///
    /// # Errors
    ///
    /// An argument with a NUL character, which a Windows command line would cut short. `Command`
    /// refuses one itself for arguments it quotes, but not for a raw one.
    pub(crate) fn apply_to(
        &self,
        kind: RarToolKind,
        command: &mut tokio::process::Command,
    ) -> std::io::Result<()> {
        #[cfg(windows)]
        if kind == RarToolKind::Unrar {
            use std::os::windows::ffi::{OsStrExt, OsStringExt};
            for arg in &self.args {
                let units: Vec<u16> = arg.encode_wide().collect();
                if units.contains(&0) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "RAR tool argument contains a NUL character",
                    ));
                }
                command.raw_arg(OsString::from_wide(&unrar_quote(&units)));
            }
            return Ok(());
        }
        #[cfg(not(windows))]
        let _ = kind;
        command.args(&self.args);
        Ok(())
    }
}

/// One argument, in UTF-16, written so that `unrar`'s Windows command-line parser reads it back
/// exactly.
///
/// `GetCmdParam` (`strfn.cpp`, unrar 7.x) knows three rules: space and tab separate arguments
/// outside quotes, a `"` toggles quoting, and two adjacent `"` are one literal `"` - in or out of
/// quotes, and checked *before* a `"` toggles anything. A backslash is always literal. So every
/// `"` of the argument is doubled, and quoting is switched on right before the first space or
/// tab, never earlier: a quote opened directly in front of a literal `"` would be read as the
/// first half of a doubled pair. The closing quote is always followed by the separating space.
///
/// An empty argument cannot be expressed at all (`""` reads as one `"`); [`rar_arguments`] never
/// builds one.
#[cfg(any(windows, test))]
pub(crate) fn unrar_quote(arg: &[u16]) -> Vec<u16> {
    const QUOTE: u16 = b'"' as u16;
    let mut out = Vec::with_capacity(arg.len() + 4);
    let mut quoted = false;
    for &unit in arg {
        if unit == QUOTE {
            out.extend([QUOTE, QUOTE]);
            continue;
        }
        if !quoted && (unit == u16::from(b' ') || unit == u16::from(b'\t')) {
            out.push(QUOTE);
            quoted = true;
        }
        out.push(unit);
    }
    if quoted {
        out.push(QUOTE);
    }
    out
}
