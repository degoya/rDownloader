//! What the process sets up before its command runs (RD-1400-07).
//!
//! On Windows the agent is a GUI-subsystem program (`main.rs`), so no console window opens when
//! it starts at login, from Explorer, a file association or an `rdownloader://` address. Started
//! from a terminal it joins that terminal's console, so `--help` and the other commands still
//! print there; started by its autostart entry it writes into its log files, which the wrapper
//! script behind the entry used to redirect its output into.

use std::{
    fs::File,
    io::Write as _,
    sync::{Mutex, OnceLock},
};

use tracing_subscriber::EnvFilter;

/// The error log of an agent its autostart entry started, where [`error_line`] writes too.
static ERROR_LOG: OnceLock<Mutex<File>> = OnceLock::new();

/// Joins the console of the program that started the agent — `cmd`, PowerShell, Windows
/// Terminal — when there is one and neither output stream is redirected; a redirected stream
/// already leads where its caller reads it. Does nothing off Windows, where a terminal's streams
/// are inherited anyway.
///
/// Through `winsafe`'s safe binding: `unsafe` is allowed only in `rd-win32` (owner,
/// 2026-11-01), and a maintained safe crate is preferred where one exists.
pub(crate) fn attach_parent_console() {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        if std::io::stdout().as_raw_handle().is_null()
            && std::io::stderr().as_raw_handle().is_null()
        {
            // Fails when there is no console to join: started at login or from Explorer.
            let _ = winsafe::AttachConsole(winsafe::PidParent::Parent);
        }
    }
}

/// Sets up tracing: into the output log of an agent its autostart entry started (`run
/// --autostart`, after moving into its working folder), otherwise onto standard output.
pub(crate) fn init_logging(autostart: bool) {
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if autostart {
        match rd_autostart::enter_autostart(rd_autostart::Target::Capture) {
            Ok(logs) => {
                let _ = ERROR_LOG.set(Mutex::new(logs.errors));
                tracing_subscriber::fmt()
                    .with_env_filter(filter())
                    .with_ansi(false)
                    .with_writer(Mutex::new(logs.output))
                    .init();
                return;
            }
            Err(error) => {
                tracing_subscriber::fmt().with_env_filter(filter()).init();
                tracing::warn!("autostart logs unavailable: {error:#}");
                return;
            }
        }
    }
    tracing_subscriber::fmt().with_env_filter(filter()).init();
}

/// Writes one line of the agent's account of a failure: onto standard error, and into the error
/// log of an agent its autostart entry started, where nobody reads standard error.
pub(crate) fn error_line(text: &str) {
    eprintln!("{text}");
    if let Some(log) = ERROR_LOG.get()
        && let Ok(mut file) = log.lock()
    {
        let _ = writeln!(file, "{text}");
    }
}
