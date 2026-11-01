//! The account the process runs as and a folder's access list, both in SDDL's text form, for
//! the data directory's protection (`rd_files::protect_private_dir`).

use std::io;
use std::path::Path;
use std::ptr;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{BOOL, HSTRING, PWSTR};

use crate::handle::{Handle, Local};

/// The SID of the account this process runs as (`S-1-5-21-…`). Account names are translated per
/// system language, SIDs are not.
///
/// # Errors
///
/// When the process token cannot be read.
pub fn current_user_sid() -> io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: `GetCurrentProcess` returns a pseudo handle that needs no closing; `token` is a
    // live out pointer, and the handle written there is owned by `Handle` below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }?;
    let token = Handle(token);
    let mut needed = 0_u32;
    // SAFETY: a size query without a buffer; it fails by design and writes the size to `needed`.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &raw mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    // `u64`s, so the TOKEN_USER at the start, which holds a pointer, is aligned.
    let mut buffer = vec![0_u64; (needed as usize).div_ceil(size_of::<u64>())];
    let length = u32::try_from(buffer.len() * size_of::<u64>()).map_err(io::Error::other)?;
    // SAFETY: the buffer is writable for `length` bytes, at least the `needed` the size query
    // asked for, and lives until the SID in it has been converted below.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            length,
            &raw mut needed,
        )
    }?;
    // SAFETY: on success the buffer starts with a TOKEN_USER, suitably aligned (see above).
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text = PWSTR::null();
    // SAFETY: `sid` points into `buffer`, which is alive; the string written to `text` is
    // `LocalAlloc` memory, owned and freed by `Local`.
    unsafe { ConvertSidToStringSidW(sid, &raw mut text) }?;
    Ok(Local(text.0.cast()).wide_string())
}

/// The access list (DACL) of `path` in SDDL: `D:PAI(A;OICI;FA;;;SY)…`, or `D:NO_ACCESS_CONTROL`
/// for a folder without one.
///
/// # Errors
///
/// When the access list cannot be read.
pub fn dacl_sddl(path: &Path) -> io::Result<String> {
    let name = HSTRING::from(path);
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `name` is a NUL-terminated string that outlives the call; `dacl` and `descriptor`
    // are live out pointers. The DACL points into the descriptor, which is `LocalAlloc` memory
    // owned and freed by `Local`.
    unsafe {
        GetNamedSecurityInfoW(
            &name,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&raw mut dacl),
            None,
            &raw mut descriptor,
        )
    }
    .ok()?;
    let descriptor = Local(descriptor.0);
    let mut text = PWSTR::null();
    // SAFETY: the descriptor is the one the call above returned, still allocated; the string
    // written to `text` is `LocalAlloc` memory, owned and freed by `Local`.
    unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            PSECURITY_DESCRIPTOR(descriptor.0),
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &raw mut text,
            None,
        )
    }?;
    Ok(Local(text.0.cast()).wide_string())
}

/// Replaces the access list of `path` with the DACL of `sddl` (`D:P(A;OICI;FA;;;SY)…`) and cuts
/// inheritance from above; Windows hands its inheritable entries down to everything below.
///
/// # Errors
///
/// When `sddl` holds no access list, or the list cannot be set.
pub fn set_protected_dacl(path: &Path, sddl: &str) -> io::Result<()> {
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the SDDL is a NUL-terminated string that outlives the call; the descriptor written
    // to `descriptor` is `LocalAlloc` memory, owned and freed by `Local`.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &HSTRING::from(sddl),
            SDDL_REVISION_1,
            &raw mut descriptor,
            None,
        )
    }?;
    let descriptor = Local(descriptor.0);
    let mut present = BOOL(0);
    let mut defaulted = BOOL(0);
    let mut dacl: *mut ACL = ptr::null_mut();
    // SAFETY: the descriptor is the one converted above, still allocated; the three out pointers
    // are live locals. The DACL written to `dacl` points into the descriptor.
    unsafe {
        GetSecurityDescriptorDacl(
            PSECURITY_DESCRIPTOR(descriptor.0),
            &raw mut present,
            &raw mut dacl,
            &raw mut defaulted,
        )
    }?;
    if !present.as_bool() || dacl.is_null() {
        return Err(io::Error::other("the SDDL holds no access list"));
    }
    let name = HSTRING::from(path);
    // SAFETY: `name` is a NUL-terminated string and `dacl` points into `descriptor`; both
    // outlive the call, which copies the list.
    unsafe {
        SetNamedSecurityInfoW(
            &name,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(dacl.cast_const()),
            None,
        )
    }
    .ok()?;
    Ok(())
}
