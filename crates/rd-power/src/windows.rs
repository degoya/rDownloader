//! Windows adapter. Standby and shutdown go through the shipped system tools; battery and
//! metered state would need WinRT, so they are reported as unavailable rather than guessed.

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

    async fn standby(&self) -> anyhow::Result<()> {
        // `shutdown /h` hibernates; suspend-to-RAM is only reachable through powrprof.
        run("rundll32.exe", &["powrprof.dll,SetSuspendState", "0,1,0"])
            .await
            .map(|_| ())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        run("shutdown.exe", &["/s", "/t", "0"]).await.map(|_| ())
    }

    /// Holds `SetThreadExecutionState` in a helper process.
    ///
    /// The flag is tracked per thread and cleared when that thread ends, so it has to be held
    /// by something that stays alive. A helper is used rather than a foreign call from this
    /// process for two reasons: the workspace denies `unsafe_code`, and a thread inside an
    /// async runtime is a poor owner for thread-affine state. The helper blocks on a pipe this
    /// process owns, so the flag is released when it is killed and equally if the service dies
    /// without killing it.
    async fn inhibit(&self, display: bool) -> anyhow::Result<Inhibition> {
        // ES_CONTINUOUS | ES_SYSTEM_REQUIRED, plus ES_DISPLAY_REQUIRED when the screen is
        // wanted too.
        let flags: u32 = if display {
            0x8000_0000 | 0x0000_0001 | 0x0000_0002
        } else {
            0x8000_0000 | 0x0000_0001
        };
        let script = format!(
            "$api = Add-Type -MemberDefinition '[DllImport(\"kernel32.dll\")] \
             public static extern uint SetThreadExecutionState(uint esFlags);' \
             -Name Power -Namespace RDownloader -PassThru; \
             [void]$api::SetThreadExecutionState({flags}); \
             [void][Console]::In.ReadLine()"
        );
        let child = tokio::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        Ok(Inhibition::holding(child))
    }
}
