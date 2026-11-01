//! Windows adapter. Standby and the wake lock are calls in this process through `rd-win32`,
//! shutdown is the system's `shutdown.exe`; battery and metered state would need WinRT, so they
//! are reported as unavailable rather than guessed.

use async_trait::async_trait;

use crate::adapter::{Inhibition, PowerAdapter, PowerCapabilities, PowerState, run};

#[derive(Debug)]
pub(crate) struct WindowsAdapter;

#[async_trait]
impl PowerAdapter for WindowsAdapter {
    fn capabilities(&self) -> PowerCapabilities {
        PowerCapabilities {
            standby: true,
            shutdown: true,
            battery: false,
            metered: false,
            inhibit_standby: true,
            inhibit_display: true,
        }
    }

    async fn state(&self) -> PowerState {
        PowerState::default()
    }

    /// `SetSuspendState` in this process: `shutdown /h` hibernates, and `rundll32` cannot pass
    /// the call its arguments. The call returns after the wake-up, so it blocks a thread of its
    /// own rather than a worker of the runtime.
    async fn standby(&self) -> anyhow::Result<()> {
        Ok(tokio::task::spawn_blocking(rd_win32::suspend).await??)
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        run("shutdown.exe", &["/s", "/t", "0"]).await.map(|_| ())
    }

    /// Holds `SetThreadExecutionState` on a thread of its own for as long as the inhibition
    /// lives ([`rd_win32::ExecutionStateHold`]); the flag is per thread, and a worker of the async
    /// runtime is no owner for it. In this process, not in a PowerShell child (RD-1400-07): the
    /// flag ends with the service, killed or not.
    async fn inhibit(&self, display: bool) -> anyhow::Result<Inhibition> {
        Ok(Inhibition::holding(rd_win32::ExecutionStateHold::start(
            display,
        )?))
    }
}
