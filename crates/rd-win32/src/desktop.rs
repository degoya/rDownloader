//! What the desktop shows, for the capture agent's game mode (RD-1240-19).

use std::io;

use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::Shell::SHQueryUserNotificationState;

use crate::handle::Handle;

/// `SHQueryUserNotificationState`'s answer as its number: `QUNS_BUSY` (2),
/// `QUNS_RUNNING_D3D_FULL_SCREEN` (3) and `QUNS_PRESENTATION_MODE` (4) say a program fills the
/// screen. What the numbers mean is the caller's to decide.
///
/// # Errors
///
/// When the shell cannot answer.
pub fn user_notification_state() -> io::Result<i32> {
    // SAFETY: no pointers are passed; the wrapper writes the answer into a local of its own.
    let state = unsafe { SHQueryUserNotificationState() }?;
    Ok(state.0)
}

/// The executable file name of every running process (`explorer.exe`), as a ToolHelp snapshot
/// lists them; the pseudo processes `[System Process]` and `System` included.
///
/// # Errors
///
/// When the snapshot cannot be taken or read.
pub fn process_names() -> io::Result<Vec<String>> {
    // SAFETY: no pointers; the handle returned is owned by `Handle` and closed when it drops.
    let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }?);
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).map_err(io::Error::other)?,
        ..PROCESSENTRY32W::default()
    };
    let mut names = Vec::new();
    // SAFETY: the snapshot handle is open, and `entry` is a live, writable PROCESSENTRY32W whose
    // `dwSize` says how large it is, as the call requires.
    let mut more = unsafe { Process32FirstW(snapshot.0, &raw mut entry) }.is_ok();
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(entry.szExeFile.len());
        names.push(String::from_utf16_lossy(&entry.szExeFile[..length]));
        // SAFETY: as for `Process32FirstW`; the call ends with an error after the last process.
        more = unsafe { Process32NextW(snapshot.0, &raw mut entry) }.is_ok();
    }
    Ok(names)
}
