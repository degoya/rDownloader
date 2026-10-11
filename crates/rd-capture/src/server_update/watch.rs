//! The running agent's side of the server update (RD-1240-25): the reading every minute, every
//! few seconds while an install runs, the tray's entry and its shortcut, and the notifications;
//! and the restart of the service from the tray (RD-1240-32).

use std::path::PathBuf;

use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{
    CONFIRM_WINDOW, Choice, Pending, READ_INTERVAL, Reading, Request, Resolution, View, ViewSink,
    choose, refused, resolve,
    restart::{self, RestartChoice},
    started, update_page, view,
};
use crate::{
    client::CaptureClient,
    config,
    notify::toast,
    supervision::{NoticeSink, supervised},
};

/// Starts the watch beside the agent's other tasks. `requests` are the tray's clicks on its entry
/// and the presses of its shortcut; `sink` shows the entry, and is `None` without a tray.
pub(crate) fn spawn(
    background: &mut tokio::task::JoinSet<()>,
    client: &CaptureClient,
    service: Url,
    cancellation: &CancellationToken,
    notice: Option<NoticeSink>,
    sink: Option<ViewSink>,
    requests: mpsc::UnboundedReceiver<Request>,
) {
    let mut watch = Watch {
        client: client.clone(),
        service,
        sink,
        directory: config::config_directory().ok(),
        reading: None,
        pending: None,
        confirm_until: None,
        confirm_restart_until: None,
        shown: None,
    };
    let task_cancellation = cancellation.clone();
    background.spawn(supervised(
        "the server update watch",
        cancellation.clone(),
        notice,
        async move {
            watch.run(task_cancellation, requests).await;
            Ok(())
        },
    ));
}

/// What the watch holds between two readings.
struct Watch {
    client: CaptureClient,
    service: Url,
    sink: Option<ViewSink>,
    /// Where the marker of a started install lives; without one, an install is followed only
    /// while this process runs.
    directory: Option<PathBuf>,
    reading: Option<Reading>,
    pending: Option<Pending>,
    /// Until when a choice installs while downloads run: the service refused the last one for
    /// them, and the notification said that choosing again installs anyway.
    confirm_until: Option<Instant>,
    /// The same for "Restart server" (RD-1240-32).
    confirm_restart_until: Option<Instant>,
    /// What the tray was last given, so a reading that changes nothing sends nothing.
    shown: Option<View>,
}

impl Watch {
    async fn run(
        &mut self,
        cancellation: CancellationToken,
        mut requests: mpsc::UnboundedReceiver<Request>,
    ) {
        self.pending = self.directory.as_deref().and_then(Pending::load);
        // A run whose tray or shortcut listener has gone still follows an install it started.
        let mut listening = true;
        loop {
            self.read().await;
            let wait = if self.pending.is_some() || self.installing() {
                config::STATUS_POLL_INTERVAL
            } else {
                READ_INTERVAL
            };
            tokio::select! {
                () = cancellation.cancelled() => return,
                () = tokio::time::sleep(wait) => {}
                request = requests.recv(), if listening => match request {
                    Some(Request::Install) => self.carry_out().await,
                    Some(Request::Restart) => self.restart().await,
                    None => listening = false,
                },
            }
        }
    }

    fn installing(&self) -> bool {
        self.reading
            .as_ref()
            .and_then(|reading| reading.install.as_ref())
            .is_some_and(|install| !install.ended())
    }

    /// Reads the service, announces the outcome of an install this agent started, and hands the
    /// tray what changed. A service that does not answer keeps the last reading: it is most
    /// likely restarting, and the health line says whether it is away.
    async fn read(&mut self) {
        match self.client.server_update().await {
            Ok(reading) => self.reading = Some(reading),
            Err(error) => tracing::debug!(%error, "could not read the server update"),
        }
        if let Some(pending) = &self.pending
            && let Resolution::Ended(text) =
                resolve(pending, self.reading.as_ref(), chrono::Utc::now())
        {
            tracing::info!(target = %pending.target, outcome = %text, "the server update ended");
            self.pending = None;
            if let Some(directory) = &self.directory {
                Pending::clear(directory);
            }
            toast(text).await;
        }
        self.publish();
    }

    fn publish(&mut self) {
        let Some(sink) = &self.sink else {
            return;
        };
        let current = view(self.reading.as_ref(), self.pending.as_ref());
        if self.shown.as_ref() == Some(&current) {
            return;
        }
        sink(current.clone());
        self.shown = Some(current);
    }

    /// The entry was chosen or its shortcut pressed: read again, then do what the reading says.
    async fn carry_out(&mut self) {
        self.read().await;
        match choose(self.reading.as_ref(), self.pending.as_ref()) {
            Choice::Install => self.install().await,
            Choice::OpenPage => match update_page(&self.service) {
                Some(page) => {
                    tracing::info!(%page, "opening the update page for the server update");
                    open_in_browser(&page);
                }
                None => tracing::warn!("the update page has no address"),
            },
            Choice::Tell(text) => toast(text).await,
        }
    }

    /// "Restart server" was chosen or its shortcut pressed: read again, then restart when the
    /// reading allows it (RD-1240-32).
    async fn restart(&mut self) {
        self.read().await;
        if let RestartChoice::Tell(text) = restart::choose(self.reading.as_ref()) {
            toast(text).await;
            return;
        }
        let allow_active = self
            .confirm_restart_until
            .take()
            .is_some_and(|until| Instant::now() < until);
        match self.client.restart_server(allow_active).await {
            Ok(how) => {
                tracing::info!(%how, "the server restart was started from the tray");
                toast(restart::started(&how)).await;
            }
            Err(error) => {
                tracing::warn!(%error, "the server restart was not started");
                let (text, confirm) = restart::refused(&error);
                if confirm {
                    self.confirm_restart_until = Some(Instant::now() + CONFIRM_WINDOW);
                }
                toast(text).await;
            }
        }
    }

    async fn install(&mut self) {
        let allow_active = self
            .confirm_until
            .take()
            .is_some_and(|until| Instant::now() < until);
        match self.client.install_server_update(allow_active).await {
            Ok(installing) => {
                let pending = Pending {
                    target: installing.target_version,
                    requested_at: chrono::Utc::now(),
                };
                tracing::info!(target = %pending.target, "the server update was started from the tray");
                if let Some(directory) = &self.directory
                    && let Err(error) = pending.store(directory)
                {
                    tracing::warn!(%error, "the server update's marker could not be stored; its outcome is announced only while this agent runs");
                }
                let text = started(&pending.target);
                self.pending = Some(pending);
                self.publish();
                toast(text).await;
            }
            Err(error) => {
                tracing::warn!(%error, "the server update was not started");
                let (text, confirm) = refused(&error);
                if confirm {
                    self.confirm_until = Some(Instant::now() + CONFIRM_WINDOW);
                }
                toast(text).await;
            }
        }
    }
}

/// The desktop's browser: `open` where the tray links it.
#[cfg(any(windows, target_os = "macos"))]
fn open_in_browser(page: &Url) {
    if let Err(error) = open::that_detached(page.as_str()) {
        tracing::warn!(%error, "could not open the update page");
    }
}

/// The desktop's own opener on the headless build, which links nothing that opens a desktop
/// application -- as the Linux shortcut listener opens the web interface.
#[cfg(not(any(windows, target_os = "macos")))]
fn open_in_browser(page: &Url) {
    if let Err(error) = std::process::Command::new("xdg-open")
        .arg(page.as_str())
        .spawn()
    {
        tracing::warn!(%error, "could not open the update page");
    }
}
