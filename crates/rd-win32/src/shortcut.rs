//! Shell links (`.lnk`) and the folders they lie in, for the capture agent's drop targets on the
//! desktop and in the Send to menu (RD-1260-09).

use std::io;
use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IPersistFile,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_SendTo, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWMINNOACTIVE, SW_SHOWNORMAL};
use windows::core::{HSTRING, IUnknown, Interface};

/// A folder of the shell, by its known-folder id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KnownFolder {
    /// The user's desktop.
    Desktop,
    /// The Send to menu of every file's context menu.
    SendTo,
}

/// Where `folder` lies for the user this process runs as.
///
/// # Errors
///
/// When the shell does not know the folder or it does not exist.
pub fn known_folder(folder: KnownFolder) -> io::Result<PathBuf> {
    let id = match folder {
        KnownFolder::Desktop => FOLDERID_Desktop,
        KnownFolder::SendTo => FOLDERID_SendTo,
    };
    // SAFETY: `id` is a live GUID; the string returned is freed with `CoTaskMemFree` below.
    let path = unsafe { SHGetKnownFolderPath(&raw const id, KF_FLAG_DEFAULT, None) }?;
    // SAFETY: the pointer is the NUL-terminated string the call just returned, not freed yet.
    let text = unsafe { path.to_hstring() };
    // SAFETY: the call leaves freeing the string with `CoTaskMemFree` to the caller; it is not
    // used after this.
    unsafe { CoTaskMemFree(Some(path.0.cast_const().cast())) };
    Ok(PathBuf::from(text.to_os_string()))
}

/// What a shell link holds.
#[derive(Clone, Copy, Debug)]
pub struct Shortcut<'a> {
    /// The program it starts.
    pub target: &'a Path,
    /// The arguments before any files dropped on the link.
    pub arguments: &'a str,
    pub working_directory: &'a Path,
    /// The file whose first icon the link shows.
    pub icon: &'a Path,
    /// The tooltip.
    pub description: &'a str,
    /// The program's window starts minimised and without focus.
    pub minimized: bool,
}

/// Writes `shortcut` to the `.lnk` file `link`, replacing one that is there.
///
/// The shell link is a COM object. It is written on a thread of its own, which joins a
/// single-threaded apartment and leaves it again, so the caller's thread -- an async worker, the
/// tray's event loop -- never has its COM state changed.
///
/// # Errors
///
/// When the link cannot be created or saved.
pub fn write_shortcut(link: &Path, shortcut: &Shortcut<'_>) -> io::Result<()> {
    std::thread::scope(|scope| scope.spawn(|| in_apartment(link, shortcut)).join())
        .unwrap_or_else(|_| Err(io::Error::other("writing the shortcut panicked")))
}

fn in_apartment(link: &Path, shortcut: &Shortcut<'_>) -> io::Result<()> {
    // SAFETY: no pointers; this thread is new and joins an apartment of its own.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok()?;
    let written = write(link, shortcut);
    // SAFETY: balances the successful `CoInitializeEx` above on the same thread; `write` has
    // released every COM object it made by now.
    unsafe { CoUninitialize() };
    written
}

fn write(link: &Path, shortcut: &Shortcut<'_>) -> io::Result<()> {
    // SAFETY: COM is initialised on this thread; `ShellLink` is the system's shell link class,
    // which implements `IShellLinkW`.
    let object: IShellLinkW =
        unsafe { CoCreateInstance(&ShellLink, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }?;
    let show = if shortcut.minimized {
        SW_SHOWMINNOACTIVE
    } else {
        SW_SHOWNORMAL
    };
    // SAFETY: `object` is a live shell link; every string is a NUL-terminated `HSTRING` alive for
    // its call, which copies it.
    unsafe {
        object.SetPath(&HSTRING::from(shortcut.target))?;
        object.SetArguments(&HSTRING::from(shortcut.arguments))?;
        object.SetWorkingDirectory(&HSTRING::from(shortcut.working_directory))?;
        object.SetIconLocation(&HSTRING::from(shortcut.icon), 0)?;
        object.SetDescription(&HSTRING::from(shortcut.description))?;
        object.SetShowCmd(show)?;
    }
    let file: IPersistFile = object.cast()?;
    // SAFETY: `file` is the same live object; the path is a NUL-terminated `HSTRING` alive for
    // the call.
    unsafe { file.Save(&HSTRING::from(link), true) }?;
    Ok(())
}
