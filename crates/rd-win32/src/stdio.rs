//! The process's own standard handles, kept from the programs it starts (RD-1410-02).

use std::io;

use windows::Win32::Foundation::{HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation};
use windows::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};

/// Clears the inherit flag of this process's standard input, output and error handles, so a
/// program it starts no longer gets them.
///
/// `std::process::Command` creates a child with `bInheritHandles` set: the child receives every
/// inheritable handle of its parent, not only the three it is given as its standard handles. The
/// standard handles a caller such as PowerShell passes in are inheritable pipe ends, so a
/// long-running child kept the caller's pipe open after this process had ended, and
/// `rdownloader-capture start-service | Out-Host` never returned. The handles stay usable in this
/// process; a missing one is skipped.
///
/// # Errors
///
/// The last handle Windows refused to change; the others are still tried.
pub fn keep_standard_handles_from_children() -> io::Result<()> {
    let mut result = Ok(());
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: no pointers; the handle is borrowed from this process's table and not closed.
        let handle = unsafe { GetStdHandle(which) };
        let Ok(handle) = handle else {
            continue;
        };
        // SAFETY: `handle` is one of this process's open standard handles; only its inherit flag
        // changes, and nothing is closed or written through it.
        let changed =
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) };
        if let Err(error) = changed {
            result = Err(io::Error::from(error));
        }
    }
    result
}
