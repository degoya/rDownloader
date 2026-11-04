//! The owners give back what a call handed out (RD-1420-05): the last `Handle` of a named kernel
//! object takes the object with it, an invalid handle and null memory are left alone, and `Local`
//! reads the string a conversion function wrote into `LocalAlloc` memory.

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW};
use windows::Win32::Security::PSID;
use windows::Win32::System::Threading::{CreateEventW, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE};
use windows::core::{HRESULT, HSTRING, PWSTR};

use crate::handle::{Handle, Local};

/// An event name of its own per test and process, so tests running side by side share none.
fn event_name(test: &str) -> HSTRING {
    HSTRING::from(format!("Local\\rd-win32-{test}-{}", std::process::id()))
}

/// Opens the named event, owned by a `Handle`.
fn open_event(name: &HSTRING) -> windows::core::Result<Handle> {
    // SAFETY: `name` is a NUL-terminated string that outlives the call; the handle returned is
    // handed to `Handle`, which closes it.
    unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, name) }.map(Handle)
}

#[test]
fn the_last_handle_dropped_takes_the_named_object_with_it() {
    let name = event_name("close");
    // SAFETY: default security and a NUL-terminated name that outlives the call; the handle
    // returned is handed to `Handle`, which closes it.
    let event =
        Handle(unsafe { CreateEventW(None, true, false, &name) }.expect("create the event"));
    let second = open_event(&name).expect("the event exists while a handle holds it");
    drop(second);
    assert!(
        open_event(&name).is_ok(),
        "dropping one handle left the other open"
    );
    drop(event);
    let Err(error) = open_event(&name) else {
        panic!("the event outlived its last handle");
    };
    assert_eq!(error.code(), HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0));
}

#[test]
fn an_invalid_handle_and_null_memory_are_left_alone() {
    // Null, and -1: `INVALID_HANDLE_VALUE`, which is also the process's own pseudo handle.
    drop(Handle(HANDLE::default()));
    drop(Handle(INVALID_HANDLE_VALUE));
    let empty = Local(std::ptr::null_mut());
    assert_eq!(empty.wide_string(), "");
    drop(empty);
}

#[test]
fn local_memory_carries_the_string_a_conversion_wrote() {
    let mut sid = PSID::default();
    // SAFETY: the string outlives the call; the SID written to `sid` is `LocalAlloc` memory,
    // handed to `Local` below, which frees it.
    unsafe { ConvertStringSidToSidW(&HSTRING::from("S-1-5-18"), &raw mut sid) }
        .expect("a well-known SID");
    let sid = Local(sid.0);
    let mut text = PWSTR::null();
    // SAFETY: `sid` holds the SID converted above and stays allocated over the call; the string
    // written to `text` is `LocalAlloc` memory, handed to `Local`, which frees it.
    unsafe { ConvertSidToStringSidW(PSID(sid.0), &raw mut text) }.expect("the SID as text");
    assert_eq!(Local(text.0.cast()).wide_string(), "S-1-5-18");
}
