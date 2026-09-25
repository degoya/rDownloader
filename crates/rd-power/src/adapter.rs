//! Platform adapters for standby, shutdown and the network/battery context.
//!
//! Every capability is probed rather than assumed: where a platform cannot do something,
//! the matrix says so and the UI disables it, instead of the service accepting a setting it
//! will silently fail to honour. This mirrors how the torrent interface binding is handled.

use async_trait::async_trait;
use serde::Serialize;
use utoipa::ToSchema;

/// What the machine underneath can actually do.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, ToSchema)]
pub struct PowerCapabilities {
    pub standby: bool,
    pub shutdown: bool,
    /// Whether battery state can be read at all.
    pub battery: bool,
    /// Whether the connection can be told to be metered.
    pub metered: bool,
    /// Whether the machine can be kept awake while work is in flight.
    pub inhibit_standby: bool,
    /// Whether the display can be kept awake as well.
    pub inhibit_display: bool,
}

/// The live power and network context; `None` means "cannot tell on this platform".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, ToSchema)]
pub struct PowerState {
    pub on_battery: Option<bool>,
    pub metered: Option<bool>,
}

/// Holds a platform sleep inhibition for exactly as long as it is alive.
///
/// Releasing is tied to the value being dropped rather than to an explicit call, so a panic
/// or an early return cannot leave a machine permanently awake.
/// Anything whose `Drop` ends an inhibition.
///
/// Blanket-implemented, so an adapter hands over whatever it happens to hold — a helper
/// process on every real platform, a counter in a test — without this module knowing what.
pub trait Held: Send + Sync + std::fmt::Debug {}
impl<T: Send + Sync + std::fmt::Debug> Held for T {}

/// Holds a platform sleep inhibition for exactly as long as it is alive.
///
/// Every real adapter holds a helper process spawned with `kill_on_drop`, whose death releases
/// the inhibition. That also covers the service being killed outright, which a flag set inside
/// this process would not.
#[derive(Debug)]
pub struct Inhibition {
    /// Never read: the value is held, not talked to, and releasing is its `Drop`.
    _held: Box<dyn Held>,
}

impl Inhibition {
    #[must_use]
    pub fn holding(held: impl Held + 'static) -> Self {
        Self {
            _held: Box::new(held),
        }
    }
}

#[async_trait]
pub trait PowerAdapter: Send + Sync + std::fmt::Debug {
    fn capabilities(&self) -> PowerCapabilities;
    async fn state(&self) -> PowerState;
    async fn standby(&self) -> anyhow::Result<()>;
    async fn shutdown(&self) -> anyhow::Result<()>;

    /// Asks the platform to stay awake until the returned value is dropped.
    ///
    /// `display` additionally keeps the screen on, which is a separate wish: a download does
    /// not need a lit screen, but somebody watching progress on a media box does.
    async fn inhibit(&self, display: bool) -> anyhow::Result<Inhibition> {
        let _ = display;
        anyhow::bail!("keeping this machine awake is not supported")
    }
}

/// An adapter that can do nothing; used where a platform is not supported and in tests.
#[derive(Debug, Default)]
pub struct UnsupportedAdapter;

#[async_trait]
impl PowerAdapter for UnsupportedAdapter {
    fn capabilities(&self) -> PowerCapabilities {
        PowerCapabilities::default()
    }

    async fn state(&self) -> PowerState {
        PowerState::default()
    }

    async fn standby(&self) -> anyhow::Result<()> {
        anyhow::bail!("standby is not supported on this platform")
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        anyhow::bail!("shutdown is not supported on this platform")
    }
}

/// How long a platform helper may take before its answer is abandoned.
///
/// Every one of these is a local query or a D-Bus request that answers in milliseconds when
/// the daemon is healthy, so the cap only ever fires on a wedged one.
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Runs a command and turns a non-zero exit into an error carrying its stderr.
///
/// Bounded, because `metered()` runs this on every supervision tick: a NetworkManager whose
/// D-Bus never answers used to make `PowerService::refresh_state` await forever, which stops
/// power and quiet-hours supervision outright with nothing in the log to say why. A timeout
/// is reported as a failed probe, the same as any other error, so a caller that cannot tell
/// reports "cannot tell" rather than hanging.
pub(crate) async fn run(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        // Dropping the timed-out future has to take the wedged child with it; without this
        // every tick would leave another orphan behind.
        .kill_on_drop(true)
        .output();
    let Ok(output) = tokio::time::timeout(COMMAND_TIMEOUT, output).await else {
        let seconds = COMMAND_TIMEOUT.as_secs();
        tracing::warn!(program, seconds, "a platform helper did not answer in time");
        anyhow::bail!("{program} did not answer within {seconds} seconds");
    };
    let output = output?;
    if !output.status.success() {
        anyhow::bail!(
            "{program} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The adapter for the platform this build runs on.
#[must_use]
pub fn platform_adapter() -> std::sync::Arc<dyn PowerAdapter> {
    #[cfg(target_os = "linux")]
    {
        std::sync::Arc::new(crate::linux::LinuxAdapter::probe())
    }
    #[cfg(target_os = "windows")]
    {
        std::sync::Arc::new(crate::windows::WindowsAdapter)
    }
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(crate::macos::MacosAdapter)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        std::sync::Arc::new(UnsupportedAdapter)
    }
}
