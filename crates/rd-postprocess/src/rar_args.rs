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
//!
//! 7-Zip has a parser of its own as well (`SplitCommandLine`, `CommandLineParser.cpp`), and until
//! the 2026-09-28 security review it still got Rust's quoting: a password `x" -spf -w"` arrived as
//! the switches `-px\`, `-spf` and `-w\`, all in front of `--`. Its arguments now take the same
//! `raw_arg` route through [`seven_zip_quote`], and a password with a `"` - which that parser
//! cannot receive at all - is refused before the tool starts.

use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

use crate::{ExtractionError, RarToolKind};

/// What the tool is asked to do with the archive.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RarAction<'a> {
    /// Unpack the whole set into this (existing) directory.
    Extract { staging: &'a Path },
    /// Unpack into this (existing) directory, pausing before every volume after the first until
    /// the caller answers on stdin (`-vp`, direct unpack, RD-1100-07). `unrar` only.
    Follow { staging: &'a Path },
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
        RarAction::Extract { .. } | RarAction::Follow { .. } => "x",
        RarAction::Test => "t",
    };
    args.push(command.into());
    let switches: &[&str] = match (kind, action) {
        (RarToolKind::Unrar, RarAction::Extract { .. }) => &["-o-", "-y", "-idc"],
        (RarToolKind::Unrar, RarAction::Test) => &["-y", "-idc"],
        // No `-y`: the volume question has to reach the caller, which answers it once that
        // volume is on disk. `-o-` already settles the only other question an unpack asks, and
        // `-idp` keeps the percentage out of the text the question is read from.
        (RarToolKind::Unrar, RarAction::Follow { .. }) => &["-vp", "-o-", "-idc", "-idp"],
        // 7-Zip cannot pause between volumes; `direct` refuses it before anything starts.
        (RarToolKind::SevenZip, RarAction::Extract { .. } | RarAction::Follow { .. }) => {
            &["-y", "-bsp1", "-bso0"]
        }
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
    if let RarAction::Extract { staging } | RarAction::Follow { staging } = action {
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
        RarAction::Extract { .. } | RarAction::Follow { .. } => {
            rd_files::long_path(first_volume).into_os_string()
        }
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
    /// Under Windows every argument goes onto the command line pre-quoted for the tool's own
    /// parser, through `raw_arg` ([`windows_raw_args`](Self::windows_raw_args)): Rust's quoting
    /// follows the C runtime's rules, which neither `unrar` nor 7-Zip does. Outside Windows no
    /// command line is re-parsed, and the list is handed over as is.
    ///
    /// # Errors
    ///
    /// Under Windows, what [`windows_raw_args`](Self::windows_raw_args) refuses.
    pub(crate) fn apply_to(
        &self,
        kind: RarToolKind,
        command: &mut tokio::process::Command,
    ) -> Result<(), ExtractionError> {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            for raw in self.windows_raw_args(kind)? {
                command.raw_arg(OsString::from_wide(&raw));
            }
        }
        #[cfg(not(windows))]
        {
            let _ = kind;
            command.args(&self.args);
        }
        Ok(())
    }

    /// Every argument in UTF-16, written for `kind`'s Windows command-line parser.
    ///
    /// Pure, so the tests check it on every platform against ports of both parsers.
    ///
    /// # Errors
    ///
    /// [`ExtractionError::PasswordHasQuote`] for a 7-Zip password with a `"`, which 7-Zip's
    /// parser cannot receive (see [`seven_zip_quote`]). An argument with a NUL character, which a
    /// Windows command line would cut short, or any other argument with a `"` for 7-Zip - neither
    /// can come from a Windows path - is [`ExtractionError::Other`].
    #[cfg(any(windows, test))]
    pub(crate) fn windows_raw_args(
        &self,
        kind: RarToolKind,
    ) -> Result<Vec<Vec<u16>>, ExtractionError> {
        let mut raw = Vec::with_capacity(self.args.len());
        for (index, arg) in self.args.iter().enumerate() {
            let units = wide(arg);
            if units.contains(&0) {
                return Err(ExtractionError::Other(anyhow::anyhow!(
                    "RAR tool argument contains a NUL character"
                )));
            }
            raw.push(match kind {
                RarToolKind::Unrar => unrar_quote(&units),
                RarToolKind::SevenZip => match seven_zip_quote(&units) {
                    Some(quoted) => quoted,
                    None if Some(index) == self.password_at => {
                        return Err(ExtractionError::PasswordHasQuote);
                    }
                    None => {
                        return Err(ExtractionError::Other(anyhow::anyhow!(
                            "7-Zip argument contains a quote"
                        )));
                    }
                },
            });
        }
        Ok(raw)
    }
}

#[cfg(windows)]
fn wide(arg: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    arg.encode_wide().collect()
}

/// Outside Windows only the tests build a Windows command line; every argument there is UTF-8.
#[cfg(all(test, not(windows)))]
fn wide(arg: &OsStr) -> Vec<u16> {
    arg.to_string_lossy().encode_utf16().collect()
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

/// One argument, in UTF-16, written so that 7-Zip's Windows command-line parser reads it back
/// exactly - or `None` for an argument with a `"`, which that parser cannot receive at all.
///
/// `SplitCommandLine` (`CPP/Common/CommandLineParser.cpp`) knows two rules: space and tab
/// separate arguments outside quotes, and every `"` toggles quoting and is dropped. There is no
/// escape for a literal `"` - neither `\"` nor `""` - and a backslash is always literal, also in
/// front of a quote. So the argument is wrapped in one pair of quotes and otherwise left alone:
/// spaces and tabs inside stay in it, and a trailing backslash stays a backslash rather than
/// escaping the closing quote as it would for the C runtime. A quote-free argument needs nothing
/// more, whatever else it carries.
#[cfg(any(windows, test))]
pub(crate) fn seven_zip_quote(arg: &[u16]) -> Option<Vec<u16>> {
    const QUOTE: u16 = b'"' as u16;
    if arg.contains(&QUOTE) {
        return None;
    }
    let mut out = Vec::with_capacity(arg.len() + 2);
    out.push(QUOTE);
    out.extend_from_slice(arg);
    out.push(QUOTE);
    Some(out)
}
