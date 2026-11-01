//! The service's wake lock and standby (RD-050-13).

use std::io;
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::System::Power::{
    ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetSuspendState,
    SetThreadExecutionState,
};

/// Keeps the machine (and with `display` the screen) awake for as long as it lives.
///
/// `SetThreadExecutionState` is tracked per thread and cleared when that thread ends, and a
/// worker thread of the async runtime is no owner for it. So each hold runs a thread of its own
/// that sets the flags, sleeps until the hold is dropped, and clears them again. The service
/// being killed ends the thread, and Windows forgets the flags with it.
#[derive(Debug)]
pub struct ExecutionStateHold {
    /// Dropped to wake the thread; it never carries a message.
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl ExecutionStateHold {
    /// Starts holding.
    ///
    /// # Errors
    ///
    /// When the thread cannot be started or Windows refuses the flags.
    pub fn start(display: bool) -> io::Result<Self> {
        let flags = if display {
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
        } else {
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED
        };
        let (stop, stopped) = mpsc::channel::<()>();
        let (ready, set) = mpsc::sync_channel::<bool>(1);
        let thread = std::thread::Builder::new()
            .name("rd-wake-lock".to_owned())
            .spawn(move || {
                // SAFETY: no pointers; the flags belong to this thread until it clears them.
                let previous = unsafe { SetThreadExecutionState(flags) };
                let held = previous.0 != 0;
                let _ = ready.send(held);
                if held {
                    // Returns once the hold drops its sender.
                    let _ = stopped.recv();
                    // SAFETY: as above; `ES_CONTINUOUS` alone clears what this thread set.
                    let _ = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                }
            })?;
        if set.recv().unwrap_or(false) {
            Ok(Self {
                stop: Some(stop),
                thread: Some(thread),
            })
        } else {
            let _ = thread.join();
            Err(io::Error::other("Windows refused the wake lock"))
        }
    }
}

impl Drop for ExecutionStateHold {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Puts the machine to sleep (suspend to RAM, wake events allowed).
///
/// # Errors
///
/// When Windows refuses, for instance because the account may not shut the machine down.
pub fn suspend() -> io::Result<()> {
    // SAFETY: no pointers; plain flags.
    if unsafe { SetSuspendState(false, false, false) } {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
