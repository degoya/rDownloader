//! macOS adapter: `pmset` for sleep and the battery, System Events for the shutdown.

use async_trait::async_trait;

use crate::adapter::{Inhibition, PowerAdapter, PowerCapabilities, PowerState, run};

#[derive(Debug)]
pub(crate) struct MacosAdapter;

#[async_trait]
impl PowerAdapter for MacosAdapter {
    fn capabilities(&self) -> PowerCapabilities {
        PowerCapabilities {
            standby: true,
            shutdown: true,
            battery: true,
            // macOS has no per-connection metered flag comparable to Windows or NM.
            metered: false,
            inhibit_standby: true,
            inhibit_display: true,
        }
    }

    async fn state(&self) -> PowerState {
        let on_battery = run("pmset", &["-g", "batt"])
            .await
            .ok()
            .map(|output| output.contains("Battery Power"));
        PowerState {
            on_battery,
            metered: None,
        }
    }

    async fn standby(&self) -> anyhow::Result<()> {
        run("pmset", &["sleepnow"]).await.map(|_| ())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        run(
            "osascript",
            &["-e", "tell application \"System Events\" to shut down"],
        )
        .await
        .map(|_| ())
    }

    /// `caffeinate` holds the assertion for as long as it runs.
    ///
    /// `-w` on our own pid is belt and braces: the child is killed on drop, and it also exits
    /// by itself if this process disappears without killing it.
    async fn inhibit(&self, display: bool) -> anyhow::Result<Inhibition> {
        let pid = std::process::id().to_string();
        let mut args = vec!["-i"];
        if display {
            args.push("-d");
        }
        args.extend(["-w", &pid]);
        let child = tokio::process::Command::new("caffeinate")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        Ok(Inhibition::holding(child))
    }
}
