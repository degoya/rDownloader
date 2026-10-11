//! The running agent's side of its own update (RD-1210-03): the check after the start and then
//! daily, the tray's "Install update to X", and the service's request, which this agent's own
//! configuration has to allow; with "Install updates automatically" on, the check installs what it
//! found itself (RD-1240-27, `auto`).

use std::path::Path;

use rd_update::agent::{AgentSetup, apply};
use rd_update::install::{DOWNLOAD_DIR, InstallError, update_dir};
use rd_update::{HttpFetcher, download_verified, verified_file};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{
    CHECK_INTERVAL, Config, FIRST_CHECK_DELAY, OfferSink, Request, Shared, State, UpdateMenu, auto,
    check_now, entry_for, install_refusal, remote_decision, report_of,
};
use crate::{client::CaptureClient, config, supervision::supervised};

/// Starts the watch beside the agent's other tasks. `requests` are the tray's clicks on its
/// update entries and their shortcuts; `sink` shows the entries.
pub(crate) fn spawn(
    background: &mut tokio::task::JoinSet<()>,
    client: &CaptureClient,
    cancellation: &CancellationToken,
    sink: Option<OfferSink>,
    requests: mpsc::UnboundedReceiver<Request>,
) {
    let (shared, task_cancellation) = (client.self_update().clone(), cancellation.clone());
    background.spawn(supervised(
        "the agent's update check",
        cancellation.clone(),
        None,
        async move {
            watch(shared, task_cancellation, sink, requests).await;
            Ok(())
        },
    ));
}

/// What the watch holds between two events.
struct Watch {
    shared: Shared,
    sink: Option<OfferSink>,
    setup: AgentSetup,
    settings: Config,
    state: State,
    directory: std::path::PathBuf,
    running: &'static str,
}

async fn watch(
    shared: Shared,
    cancellation: CancellationToken,
    sink: Option<OfferSink>,
    mut requests: mpsc::UnboundedReceiver<Request>,
) {
    let Ok(directory) = config::config_directory() else {
        return;
    };
    let mut watch = Watch {
        settings: Config::load(&directory),
        state: State::load(&directory),
        setup: AgentSetup::detect(),
        shared,
        sink,
        directory,
        running: env!("CARGO_PKG_VERSION"),
    };
    watch.publish(None);
    let active = watch.setup != AgentSetup::WithService && watch.settings.check;
    if !active {
        tracing::info!(
            setup = watch.setup.as_str(),
            check = watch.settings.check,
            "the agent does not check for its own update: the service beside it updates it, or the check is off"
        );
    }
    let mut next = Instant::now() + FIRST_CHECK_DELAY;
    let mut poll = tokio::time::interval(crate::config::STATUS_POLL_INTERVAL);
    // A run whose tray or shortcut listener has gone still checks; it only takes no clicks.
    let mut listening = true;
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return,
            () = tokio::time::sleep_until(next), if active => {
                watch.check().await;
                next = Instant::now() + CHECK_INTERVAL;
            }
            request = requests.recv(), if listening => match request {
                Some(Request::Install) => watch.install_requested().await,
                Some(Request::ToggleAutoInstall) => watch.toggle_auto_install(),
                None => listening = false,
            },
            _ = poll.tick() => watch.answer_service().await,
        }
    }
}

impl Watch {
    /// Tells the service and the tray where the update stands.
    fn publish(&self, installing: Option<&str>) {
        let report = report_of(
            self.setup,
            &self.settings,
            &self.state,
            self.running,
            installing,
        );
        self.shared.set_report(&report);
        let Some(sink) = &self.sink else {
            return;
        };
        let offer = match installing {
            Some(version) => Some(super::OfferEntry {
                label: format!("Installing update to {version}..."),
                enabled: false,
            }),
            None => self
                .state
                .current_offer(self.running)
                .and_then(|offer| entry_for(self.setup, offer)),
        };
        sink(UpdateMenu {
            offer,
            auto_install: auto::auto_install_entry(self.setup, &self.settings),
        });
    }

    /// "Install updates automatically", from the tray or its shortcut: switched and stored where
    /// it applies, and shown either way, so the tray's tick follows the setting, not the click.
    fn toggle_auto_install(&mut self) {
        match auto::toggle(self.setup, &mut self.settings) {
            Ok(on) => {
                tracing::info!(on, "the agent's automatic update was switched");
                if let Err(error) = self.settings.store(&self.directory) {
                    tracing::warn!(%error, "the agent's update switches could not be stored");
                }
            }
            Err(why) => tracing::info!(why, "the agent's automatic update does not apply here"),
        }
        self.publish(None);
    }

    /// "Install update", from the tray or its shortcut: installed, or a notification why not.
    async fn install_requested(&mut self) {
        match install_refusal(self.setup, self.state.current_offer(self.running)) {
            None => self.install_offer().await,
            Some(why) => {
                tracing::info!(%why, "not installing the agent's update");
                crate::notify::toast(why).await;
            }
        }
    }

    async fn check(&mut self) {
        if let Some(channel) = self.shared.channel() {
            self.state.service_channel = Some(channel);
        }
        if let Err(error) = check_now(self.setup, None, &mut self.state, self.running).await {
            tracing::warn!(code = error.code(), %error, "the agent cannot check for its own update");
            return;
        }
        if let Err(error) = self.state.store(&self.directory) {
            tracing::warn!(%error, "the agent's update state could not be stored");
        }
        let offer = self.state.current_offer(self.running);
        if auto::installs_now(self.setup, &self.settings, offer) {
            tracing::info!("installing the agent's update automatically");
            self.install_offer().await;
            return;
        }
        if let Some(offer) = offer
            && self.sink.is_none()
        {
            tracing::info!(
                version = %offer.version,
                "run `rdownloader-capture update` to install it"
            );
        }
        self.publish(None);
    }

    /// The tray's "Install update to X", or a service request this agent allows.
    async fn install_offer(&mut self) {
        let Some(version) = self
            .state
            .current_offer(self.running)
            .map(|offer| offer.version.clone())
        else {
            return;
        };
        self.publish(Some(version.as_str()));
        match install(
            self.setup,
            &self.state,
            &self.directory,
            self.running,
            std::env::args_os()
                .skip(1)
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
        )
        .await
        {
            Ok(version) => tracing::info!(
                %version,
                "the update is handed to the updater; the agent restarts as the new version"
            ),
            Err(error) => {
                tracing::error!(code = error.code, detail = %error.detail, "the agent's update could not start");
                self.publish(None);
            }
        }
    }

    /// Reads a request the service made, and carries it out only where this agent allows it.
    async fn answer_service(&mut self) {
        let Some(requested) = self.shared.take_request() else {
            return;
        };
        match remote_decision(&self.settings, &self.state, self.running, &requested) {
            Ok(()) => {
                tracing::info!(version = %requested, "the service asked this agent to install its update");
                self.install_offer().await;
            }
            Err(why) => {
                tracing::warn!(version = %requested, why, "the service's request to install an update is refused");
            }
        }
    }
}

/// Downloads the offered archive, checks it against the signed manifest and hands it to the
/// updater. Returns the version being installed.
///
/// # Errors
///
/// `update.not_offered` without a newer version, `update.not_installable` for an agent that does
/// not install itself, the download's code, or the hand-over's.
pub(crate) async fn install(
    setup: AgentSetup,
    state: &State,
    directory: &Path,
    running: &str,
    args: Vec<String>,
) -> Result<String, InstallError> {
    let offer = state
        .current_offer(running)
        .ok_or_else(|| InstallError::new("update.not_offered", "no newer version is offered"))?;
    let installs = matches!(
        setup.action(&offer.version),
        Some(rd_update::UpdateAction::Install)
    );
    let artifact = offer.artifact.clone().filter(|_| installs).ok_or_else(|| {
        InstallError::new(
            "update.not_installable",
            format!(
                "{} {} does not install itself",
                setup.as_str(),
                offer.version
            ),
        )
    })?;
    let executable = std::env::current_exe().map_err(|error| {
        InstallError::new(
            "update.plan_invalid",
            format!("the agent's executable: {error}"),
        )
    })?;
    let downloads = update_dir(directory).join(DOWNLOAD_DIR);
    let download = match verified_file(&artifact, &downloads).await {
        Some(path) => path,
        None => download_verified(&HttpFetcher::new(), &artifact, &downloads)
            .await
            .map_err(|error| InstallError::new(error.code(), error.to_string()))?,
    };
    let plan = apply::plan(
        &executable,
        directory,
        (running, &offer.version),
        &download,
        &artifact,
        args,
    )?;
    apply::hand_over(plan)?;
    Ok(offer.version.clone())
}
