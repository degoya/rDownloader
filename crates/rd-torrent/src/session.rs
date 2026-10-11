//! Lifecycle of the shared librqbit session.
//!
//! The session used to be a `OnceCell`, which meant every session-level setting — the
//! listen port, the upload limit and later the interface binding, proxy and blocklist —
//! only took effect after a process restart. It is now a slot that can be rebuilt: the
//! settings endpoint calls [`TorrentService::reconfigure`], which applies what the engine
//! can change live and rebuilds only when it must.

use std::{collections::HashSet, num::NonZeroU32, sync::Arc};

use anyhow::{Context, Result};
use librqbit::{ListenerOptions, Session, SessionOptions, SessionPersistenceConfig};
use rd_core::{TorrentEngineCapabilities, TorrentSettings};

use crate::{ServiceInner, TorrentService, kill_switch};

/// Who pauses, in the log line of a torrent that refused.
const REBUILD: &str = "session rebuild";

/// What the embedded librqbit 9 build can and cannot do.
///
/// Everything reported `false` here is rejected by the API with
/// `torrent.capability_unsupported` instead of being accepted and quietly ignored.
pub const CAPABILITIES: TorrentEngineCapabilities = TorrentEngineCapabilities {
    engine: "librqbit",
    engine_version: "9",
    file_selection: true,
    // Emulated on top of include/exclude by opening one tier at a time; librqbit itself
    // has no priority concept.
    file_priorities: true,
    priorities_emulated: true,
    // librqbit 9.0.1 has one fixed piece order and nothing to switch (measured for RD-1240-16):
    // files sorted by name, of each its first and last piece, then the rest front to back
    // (`torrent_state/live/mod.rs`, "TODO: make it configurable"; `file_info.rs`). A per-torrent
    // mode would change nothing, so neither is offered.
    sequential_download: false,
    first_last_piece: false,
    // The tracker list is persisted by rDownloader and applied when the torrent is added
    // or re-added; the engine has no runtime tracker API.
    tracker_edit: true,
    tracker_reannounce: true,
    tracker_scrape: true,
    peer_stats: true,
    piece_stats: true,
    // `bind_device_name` uses SO_BINDTODEVICE / IP_BOUND_IF, neither of which exists on
    // Windows, so the binding is only offered where it actually binds.
    interface_binding: cfg!(any(target_os = "linux", target_os = "macos")),
    ip_blocklist_url: true,
    ip_blocklist_file: false,
    protocol_encryption: false,
    socks5_peer_proxy: true,
    per_class_proxy: false,
    tracker_proxy: false,
    upnp: true,
    natpmp: false,
    pcp: false,
    web_seeds: false,
    // librqbit 9 takes a torrent's own rates only when it is added: a running torrent cannot
    // be given new ones, and a torrent restored from the persisted session comes back without
    // them. Honouring a limit set or changed later would mean deleting and re-adding the
    // torrent, which re-checks every piece, so the option is refused instead (RD-1100-01).
    per_torrent_limits: false,
};

/// The session-level settings, separated from the rest so a change can be classified as
/// "apply live" or "needs a rebuild".
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SessionConfig {
    /// Incoming peer port; `None` picks a random one.
    pub listen_port: Option<u16>,
    pub upload_bps: Option<NonZeroU32>,
    pub download_bps: Option<NonZeroU32>,
    /// Interface every torrent socket is bound to.
    pub bind_interface: Option<String>,
    pub blocklist_url: Option<String>,
    pub listen_mode: rd_core::TorrentListenMode,
    pub peer_limit: Option<usize>,
    /// Assembled SOCKS5 URL for outgoing peer connections.
    pub proxy_url: Option<String>,
    pub upnp: bool,
    pub announce_port: Option<u16>,
    /// Whether the engine uploads to peers at all.
    pub sharing: bool,
}

impl SessionConfig {
    /// Derives the session config from the stored settings.
    pub(crate) fn from_settings(settings: &TorrentSettings) -> Self {
        Self {
            listen_port: settings.torrent_listen_port,
            upload_bps: bps(settings.torrent_upload_limit_bytes_per_second),
            download_bps: bps(settings.torrent_download_limit_bytes_per_second),
            // Only offered where the platform actually binds; elsewhere the setting is
            // ignored rather than silently doing nothing under a working-looking switch.
            bind_interface: CAPABILITIES
                .interface_binding
                .then(|| settings.torrent_bind_interface.clone())
                .flatten(),
            blocklist_url: settings.torrent_ip_blocklist_url.clone(),
            listen_mode: settings.torrent_listen_mode,
            peer_limit: settings
                .torrent_peer_limit
                .and_then(|limit| usize::try_from(limit).ok()),
            // Resolved separately, because reading it needs the vault.
            proxy_url: None,
            upnp: settings.torrent_upnp_enabled,
            announce_port: settings.torrent_announce_port,
            sharing: settings.torrent_sharing_enabled,
        }
    }

    /// Whether moving to `next` requires tearing the session down.
    ///
    /// Rate limits are live-settable on the engine's `Limits`; everything else is baked
    /// into the session at construction time.
    pub(crate) fn needs_rebuild(&self, next: &Self) -> bool {
        self.listen_port != next.listen_port
            || self.bind_interface != next.bind_interface
            || self.blocklist_url != next.blocklist_url
            || self.listen_mode != next.listen_mode
            || self.peer_limit != next.peer_limit
            || self.proxy_url != next.proxy_url
            || self.upnp != next.upnp
            || self.announce_port != next.announce_port
            // Uploading is a session option, not a live rate: turning sharing on or off
            // takes a rebuild.
            || self.sharing != next.sharing
    }
}

/// Converts a byte count into the engine's `NonZeroU32` rate limit.
fn bps(limit: Option<rd_core::ByteCount>) -> Option<NonZeroU32> {
    limit
        .and_then(|limit| u32::try_from(limit.get()).ok())
        .and_then(NonZeroU32::new)
}

/// One live session together with the config it was built from.
pub(crate) struct SessionSlot {
    pub session: Arc<Session>,
    pub config: SessionConfig,
    /// Incarnation counter, incremented on every rebuild. Handles and statistics samples
    /// carry it so a client can tell that the engine restarted underneath them.
    pub generation: u64,
}

impl TorrentService {
    /// The shared session, built on first use from the stored settings.
    pub(crate) async fn session(&self) -> Result<Arc<Session>> {
        Ok(self.session_slot().await?.0)
    }

    /// The shared session together with its incarnation number.
    pub(crate) async fn session_slot(&self) -> Result<(Arc<Session>, u64)> {
        if let Some(slot) = self.inner.session.read().await.as_ref() {
            return Ok((slot.session.clone(), slot.generation));
        }
        let mut guard = self.inner.session.write().await;
        // Another task may have built it while this one waited for the write lock.
        if let Some(slot) = guard.as_ref() {
            return Ok((slot.session.clone(), slot.generation));
        }
        let config = self.session_config().await?;
        let slot = build(&self.inner, config).await?;
        let result = (slot.session.clone(), slot.generation);
        *guard = Some(slot);
        Ok(result)
    }

    /// The session config for the stored settings, with the proxy resolved.
    ///
    /// A configured proxy that cannot be resolved is an error rather than a fall back to a
    /// direct connection: silently leaving the proxy out would expose the user's address,
    /// which is the one thing the setting exists to prevent.
    pub(crate) async fn session_config(&self) -> Result<SessionConfig> {
        let mut config = SessionConfig::from_settings(&*self.inner.settings.read().await);
        config.proxy_url = self.proxy_url().await?;
        // A scheduled profile and the torrent setting both apply; the stricter one wins.
        if let Some(bandwidth) = &self.inner.bandwidth {
            let (download, upload) = bandwidth.torrent_rates().await;
            config.download_bps = stricter(config.download_bps, download);
            config.upload_bps = stricter(config.upload_bps, upload);
        }
        Ok(config)
    }

    /// The current session incarnation, or `0` when no session has been built yet.
    pub async fn session_generation(&self) -> u64 {
        self.inner
            .session
            .read()
            .await
            .as_ref()
            .map_or(0, |slot| slot.generation)
    }

    /// What the engine behind this service supports.
    #[must_use]
    pub fn capabilities(&self) -> TorrentEngineCapabilities {
        CAPABILITIES
    }

    /// Applies changed settings to the running session.
    ///
    /// Rate limits are applied in place. A change that the engine can only take at
    /// construction time rebuilds the session; the previous session stays in place if the
    /// rebuild fails, so a bad port cannot leave the service without an engine.
    ///
    /// A new session that wants the port the running one listens on cannot be built beside
    /// it -- the bind fails with "Address already in use", and every rebuild with a fixed port
    /// failed so, sharing switched off included (RD-1240-28). The running session is stopped
    /// first then, and a rebuild that still fails brings it back with its own settings.
    pub async fn reconfigure(&self) -> Result<()> {
        let next = self.session_config().await?;
        let mut guard = self.inner.session.write().await;
        let Some(slot) = guard.as_mut() else {
            // Nothing built yet: the next `session()` picks the new settings up anyway.
            return Ok(());
        };
        if !slot.config.needs_rebuild(&next) {
            apply_live_limits(&slot.session, &next);
            slot.config = next;
            return Ok(());
        }
        // Pause everything first so no torrent keeps writing while the engine is replaced. One
        // the engine refuses is named in the log (RD-1120-04); while the kill switch holds the
        // traffic, its next check pauses it again.
        let registered = kill_switch::session_torrents(
            &slot.session,
            self.inner.registry.read().await.snapshot(),
        );
        let mut paused = HashSet::new();
        kill_switch::hold_torrents(&registered, &mut paused, REBUILD).await;
        let generation = slot.generation;
        let previous = slot.config.clone();
        let stopped = holds_port(slot.session.listen_addr().map(|addr| addr.port()), &next);
        if stopped {
            slot.session.stop().await;
        }
        match build(&self.inner, next.clone()).await {
            Ok(rebuilt) => {
                tracing::info!(
                    from = generation,
                    to = rebuilt.generation,
                    "torrent session rebuilt for changed settings"
                );
                *guard = Some(rebuilt);
                drop(guard);
                *self.inner.rebuild_error.write().await = None;
                // Entries of the old incarnation refer to torrent ids that no longer exist.
                let retired = self
                    .inner
                    .registry
                    .write()
                    .await
                    .retire_before(generation + 1);
                for (download_id, entry) in retired {
                    tracing::debug!(
                        %download_id,
                        info_hash = %entry.info_hash,
                        "torrent must be re-added after the session rebuild"
                    );
                }
                Ok(())
            }
            Err(error) if stopped => {
                *self.inner.rebuild_error.write().await = Some(format!("{error:#}"));
                // The old engine is gone: bring it back as it was. Its torrents are re-added
                // by their runners, as after any rebuild.
                match build(&self.inner, previous).await {
                    Ok(restored) => *guard = Some(restored),
                    Err(restore) => {
                        tracing::warn!(
                            error = %format!("{restore:#}"),
                            "the previous torrent session could not be restored either"
                        );
                        *guard = None;
                    }
                }
                drop(guard);
                self.inner
                    .registry
                    .write()
                    .await
                    .retire_before(generation + 1);
                Err(error).context("rebuild torrent session")
            }
            Err(error) => {
                *self.inner.rebuild_error.write().await = Some(format!("{error:#}"));
                // Keep the old session and let what was paused here run again; a torrent the
                // user had stopped stays stopped, and one the engine refuses is logged.
                kill_switch::resume_held(&registered, &mut paused, REBUILD).await;
                Err(error).context("rebuild torrent session")
            }
        }
    }
}

/// Whether a session for `next` would bind the port the running session listens on
/// (`bound`), so the running one has to let go of it first. A random port (`None`) never
/// does: the system hands out a free one.
fn holds_port(bound: Option<u16>, next: &SessionConfig) -> bool {
    next.listen_port.is_some() && next.listen_port == bound
}

/// The stricter of two optional rates; `None` means unlimited and therefore never wins.
fn stricter(configured: Option<NonZeroU32>, profile: Option<u64>) -> Option<NonZeroU32> {
    let profile = profile
        .and_then(|value| u32::try_from(value).ok())
        .and_then(NonZeroU32::new);
    match (configured, profile) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (value, None) | (None, value) => value,
    }
}

/// Re-applies the session rates when the active bandwidth profile changes.
///
/// The engine is not part of the scheduler's limiter chain — it owns its own sockets — so
/// the profile has to be pushed to it instead of being acquired from.
pub(crate) async fn watch_bandwidth(service: TorrentService) {
    if service.inner.bandwidth.is_none() {
        return;
    }
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(15));
    let mut applied: Option<(Option<NonZeroU32>, Option<NonZeroU32>)> = None;
    loop {
        tokio::select! {
            () = service.inner.shutdown.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let Ok(config) = service.session_config().await else {
            continue;
        };
        let next = (config.download_bps, config.upload_bps);
        if applied == Some(next) {
            continue;
        }
        applied = Some(next);
        if let Err(error) = service.reconfigure().await {
            tracing::warn!(%error, "torrent rates could not be applied");
        }
    }
}

/// Applies the rate limits the engine can change without a restart.
fn apply_live_limits(session: &Session, config: &SessionConfig) {
    session.ratelimits.set_upload_bps(config.upload_bps);
    session.ratelimits.set_download_bps(config.download_bps);
}

/// Constructs a session from the given config.
///
/// Torrents in the persisted list that no queue row claims are struck first: librqbit
/// restores every entry, and restoring one creates its files (RD-120-68).
async fn build(inner: &ServiceInner, config: SessionConfig) -> Result<SessionSlot> {
    crate::forget::drop_orphans(inner).await;
    let mut options = SessionOptions {
        fastresume: true,
        persistence: Some(SessionPersistenceConfig::Json {
            folder: Some(crate::forget::session_folder(&inner.data_dir)),
        }),
        ..Default::default()
    };
    let mut listen = ListenerOptions::default();
    if let Some(port) = config.listen_port {
        listen.listen_addr.set_port(port);
    }
    listen.mode = match config.listen_mode {
        rd_core::TorrentListenMode::TcpOnly => librqbit::ListenerMode::TcpOnly,
        rd_core::TorrentListenMode::UtpOnly => librqbit::ListenerMode::UtpOnly,
        rd_core::TorrentListenMode::TcpAndUtp => librqbit::ListenerMode::TcpAndUtp,
    };
    options.bind_device_name = config.bind_interface.clone();
    // A blocklist that fails to load is the engine's problem alone: it never touches a
    // non-torrent download.
    options.blocklist_url = config.blocklist_url.clone();
    options.peer_limit = config.peer_limit;
    if let Some(proxy) = config.proxy_url.clone() {
        // Outgoing peer connections only; tracker traffic goes out beside it.
        options.connect = Some(librqbit::ConnectionOptions {
            proxy_url: Some(proxy),
            ..Default::default()
        });
    }
    listen.enable_upnp_port_forwarding = config.upnp;
    listen.announce_port = config.announce_port;
    options.listen = Some(listen);
    options.ratelimits.upload_bps = config.upload_bps;
    options.ratelimits.download_bps = config.download_bps;
    options.client_name_and_version = Some(format!(
        "rDownloader {}",
        option_env!("CARGO_PKG_VERSION").unwrap_or("dev")
    ));
    let session = Session::new_with_opts(inner.default_output.clone(), options)
        .await
        .context("start torrent session")?;
    let generation = inner
        .generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        + 1;
    Ok(SessionSlot {
        session,
        config,
        generation,
    })
}

#[cfg(test)]
#[path = "session_rebuild_tests.rs"]
mod rebuild_tests;

#[cfg(test)]
mod tests {
    use rd_core::{ByteCount, TorrentSettings};

    use super::{CAPABILITIES, SessionConfig, holds_port};

    #[test]
    fn only_construction_time_settings_force_a_rebuild() {
        let base = SessionConfig::from_settings(&TorrentSettings::default());
        let faster = SessionConfig::from_settings(&TorrentSettings {
            torrent_upload_limit_bytes_per_second: ByteCount::new(1_024).ok(),
            ..TorrentSettings::default()
        });
        // A rate limit is live-settable.
        assert!(!base.needs_rebuild(&faster));

        let other_port = SessionConfig::from_settings(&TorrentSettings {
            torrent_listen_port: Some(6_881),
            ..TorrentSettings::default()
        });
        assert!(base.needs_rebuild(&other_port));
    }

    /// RD-1240-28: switching sharing off with a fixed port built the new session beside the
    /// old one, which still listened on that port; the bind failed and the old engine, sharing
    /// on, kept running.
    #[test]
    fn a_rebuild_on_the_running_session_s_port_releases_it_first() {
        let fixed = SessionConfig::from_settings(&TorrentSettings {
            torrent_listen_port: Some(51_413),
            torrent_sharing_enabled: false,
            ..TorrentSettings::default()
        });
        assert!(holds_port(Some(51_413), &fixed));
        assert!(!holds_port(Some(6_881), &fixed));
        assert!(!holds_port(None, &fixed));
        let random = SessionConfig::from_settings(&TorrentSettings::default());
        assert!(!holds_port(Some(51_413), &random));
    }

    #[test]
    fn an_unsupported_rate_limit_is_dropped_rather_than_truncated() {
        let huge = SessionConfig::from_settings(&TorrentSettings {
            torrent_upload_limit_bytes_per_second: ByteCount::new(u64::from(u32::MAX) + 1).ok(),
            ..TorrentSettings::default()
        });
        assert!(huge.upload_bps.is_none());
    }

    #[test]
    fn the_capability_matrix_reports_the_known_engine_gaps() {
        // Through `supports` rather than the fields directly, so these stay real
        // assertions instead of constants the compiler folds away.
        let capabilities = CAPABILITIES;
        assert!(capabilities.supports("file_selection"));
        assert!(capabilities.priorities_emulated);
        for missing in [
            "sequential_download",
            "first_last_piece",
            "protocol_encryption",
            "per_class_proxy",
            "natpmp",
            "pcp",
            "web_seeds",
            "per_torrent_limits",
        ] {
            assert!(
                !capabilities.supports(missing),
                "{missing} is not supported"
            );
        }
    }
}
