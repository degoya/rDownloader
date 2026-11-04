//! The Windows calls the service and the capture agent make in their own process (RD-1400-07).
//!
//! The workspace denies `unsafe_code`, and for a long time that meant a Windows call was made by a
//! hidden PowerShell child that declared it with `Add-Type … DllImport`: the game mode's look at
//! the desktop, the wake lock, the data directory's access list, the desktop shortcut. A program
//! that starts hidden script interpreters is what malware looks like, and Microsoft Defender's
//! machine-learning heuristic quarantined the capture agent as `Trojan:Win32/Bearfoos.A!ml`.
//!
//! So this crate is the one place `unsafe` is allowed (owner, 2026-11-01). Every function here is
//! a small safe wrapper over the `windows` crate that returns `std::io::Result`; every `unsafe`
//! block carries a `// SAFETY:` comment (`clippy::undocumented_unsafe_blocks` holds it), and
//! `crates/rdownloader/tests/repo_lints/unsafe_only_in_win32.rs` refuses `unsafe` and
//! `allow(unsafe_code)` in every other host crate. What a caller decides from an answer stays
//! in the caller, where it compiles and is tested on every platform.
//!
//! On every other platform the crate is empty: a dependant names it under
//! `[target.'cfg(windows)'.dependencies]` and calls it from code that is Windows-only anyway.

#![allow(unsafe_code)]
#![warn(clippy::undocumented_unsafe_blocks, unreachable_pub)]

#[cfg(windows)]
mod desktop;
#[cfg(windows)]
mod handle;
#[cfg(all(test, windows))]
mod handle_tests;
#[cfg(windows)]
mod power;
#[cfg(windows)]
mod security;
#[cfg(all(test, windows))]
mod security_tests;
#[cfg(windows)]
mod shortcut;
#[cfg(windows)]
mod stdio;

#[cfg(windows)]
pub use desktop::{process_names, user_notification_state};
#[cfg(windows)]
pub use power::{ExecutionStateHold, suspend};
#[cfg(windows)]
pub use security::{current_user_sid, dacl_sddl, set_protected_dacl};
#[cfg(windows)]
pub use shortcut::{KnownFolder, Shortcut, known_folder, write_shortcut};
#[cfg(windows)]
pub use stdio::keep_standard_handles_from_children;
