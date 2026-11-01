//! Owners that give back what a call handed out: a kernel handle, and memory from `LocalAlloc`.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::core::PWSTR;

/// A kernel handle, closed when dropped.
pub(crate) struct Handle(pub(crate) HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the handle came from a successful call that handed it to this owner, and
            // nothing else closes it.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// Memory a call allocated with `LocalAlloc` for the caller to free, freed when dropped.
pub(crate) struct Local(pub(crate) *mut c_void);

impl Local {
    /// The memory as the NUL-terminated wide string a conversion function wrote there.
    pub(crate) fn wide_string(&self) -> String {
        if self.0.is_null() {
            return String::new();
        }
        // SAFETY: the pointer is non-null and points to the NUL-terminated string the call
        // wrote; it stays allocated while `self` lives.
        unsafe { PWSTR(self.0.cast()).to_hstring() }.to_string_lossy()
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the memory came from `LocalAlloc` inside a successful call, which leaves
            // freeing it to the caller; nothing else frees it.
            let _ = unsafe { LocalFree(Some(HLOCAL(self.0))) };
        }
    }
}
