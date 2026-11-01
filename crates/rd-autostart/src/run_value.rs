//! The command line of a Windows `Run` entry (RD-1400-07): the capture agent itself, quoted,
//! with its arguments, and nothing between Windows and it.
//!
//! The capture agent is a GUI-subsystem program and opens no console window, so it is started
//! directly; the service is a console program, so its entry starts the agent beside it, which
//! starts the service without a window ([`super::launch::start_beside`]). Pure and cfg-free, so
//! the exact value is tested on every host.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::Target;
use super::launch::{AUTOSTART_FLAG, START_SERVICE_COMMAND};

/// The `Run` value for `target`, whose program is `executable`.
pub(super) fn render(target: Target, executable: &Path) -> Result<String> {
    Ok(match target {
        Target::Capture => format!(
            "{} {} {AUTOSTART_FLAG}",
            quoted(executable)?,
            target.argument()
        ),
        Target::Server => format!(
            "{} {START_SERVICE_COMMAND}",
            quoted(&capture_beside(executable))?
        ),
    })
}

/// The capture agent in the folder of `executable`: the program the service's entry starts.
///
/// Split at the last separator by hand rather than with `Path::with_file_name`, which knows no
/// `\` on the hosts that test this.
pub(super) fn capture_beside(executable: &Path) -> PathBuf {
    let text = executable.to_string_lossy();
    let folder = text.rfind(['\\', '/']).map_or("", |end| &text[..=end]);
    PathBuf::from(format!("{folder}{}.exe", Target::Capture.slug()))
}

/// `path` in double quotes, the program token of a command line.
///
/// Windows splits the program off a command line at the closing quote, without any escape: a
/// quote inside the path would end it early and turn the rest into arguments of their own, and a
/// line break has no business in a registry command at all. Neither can occur in a Windows path,
/// so either is refused rather than escaped — a renderer that trusts its input is the one that
/// stops being safe when somebody changes where the program lives. A space, `%` or `'` is
/// ordinary inside the quotes; the value is a `REG_SZ`, so `%` is never expanded.
fn quoted(path: &Path) -> Result<String> {
    Ok(format!("\"{}\"", windows_command_path(path)?))
}

/// The text of a path in a Windows command line, refused when it carries a quote or a control
/// character.
pub(super) fn windows_command_path(path: &Path) -> Result<&str> {
    let value = path.to_str().context("autostart path is not Unicode")?;
    if value.contains(['"', '\n', '\r', '\0']) {
        bail!("autostart path cannot be represented in a Windows command line");
    }
    Ok(value)
}
