//! Quiet hours, queue completion actions and the platform power/network context
//! (RD-050-13).
//!
//! The platform-specific parts sit behind [`PowerAdapter`] so the completion state machine
//! can be tested against a fake instead of suspending the machine running the tests.

mod adapter;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod service;
mod settings;
#[cfg(target_os = "windows")]
mod windows;

pub use adapter::{
    Held, Inhibition, PowerAdapter, PowerCapabilities, PowerState, UnsupportedAdapter,
    platform_adapter,
};
pub use service::{PendingAction, PowerService, PowerStatus};
pub use settings::{
    CompletionAction, DEFAULT_COMPLETION_COUNTDOWN, MAX_COMPLETION_COUNTDOWN,
    MIN_COMPLETION_COUNTDOWN, PowerSettings,
};
