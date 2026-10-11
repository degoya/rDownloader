//! What the tray's menu and the system-wide shortcuts ask of the running agent (RD-1100-06,
//! RD-1180-01, RD-1180-03, RD-1240-23, RD-1240-25, RD-1240-27, RD-1240-24, RD-1240-32), and
//! the tasks that carry it out.
//!
//! Every tray command is a [`CaptureCommand`], whether it came from a click or a key press, and
//! [`action`] is the one place that says what each one does. The tray and, on Linux, the
//! shortcut listener only name the command; the agent's tasks act on it, because they hold the
//! token -- the tray never reads the keyring.

use std::sync::Arc;

use rd_core::{CaptureAgentSettings, CaptureCommand};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::{
    activity::QueueRequest,
    agent_settings::{self, SettingsRequest},
    client::CaptureClient,
    self_update, server_update,
    supervision::{NoticeSink, supervised},
};

/// Hands every change of the agent's settings to whoever shows them: the tray's check mark, icon
/// and accelerators. A callback for the same reason as `ActivitySink`.
pub(crate) type SettingsSink = Arc<dyn Fn(CaptureAgentSettings) + Send + Sync>;

/// The sending half: held by the tray, or by the shortcut listener when there is no tray.
#[derive(Clone)]
pub(crate) struct Controls {
    pub(crate) queue: mpsc::UnboundedSender<QueueRequest>,
    pub(crate) settings: mpsc::UnboundedSender<SettingsRequest>,
    pub(crate) hand_over: mpsc::UnboundedSender<()>,
    /// "Install update to X" (RD-1210-03) and "Install updates automatically" (RD-1240-27), from
    /// the tray or a shortcut.
    pub(crate) self_update: mpsc::UnboundedSender<self_update::Request>,
    /// "Install server update" (RD-1240-25) and "Restart server" (RD-1240-32), from the tray or
    /// a shortcut.
    pub(crate) server_update: mpsc::UnboundedSender<server_update::Request>,
}

/// The receiving half, handed to the agent's tasks.
pub(crate) struct Inbox {
    pub(crate) queue: mpsc::UnboundedReceiver<QueueRequest>,
    pub(crate) settings: mpsc::UnboundedReceiver<SettingsRequest>,
    pub(crate) hand_over: mpsc::UnboundedReceiver<()>,
    pub(crate) self_update: mpsc::UnboundedReceiver<self_update::Request>,
    pub(crate) server_update: mpsc::UnboundedReceiver<server_update::Request>,
}

pub(crate) fn channels() -> (Controls, Inbox) {
    let (queue, queue_inbox) = mpsc::unbounded_channel();
    let (settings, settings_inbox) = mpsc::unbounded_channel();
    let (hand_over, hand_over_inbox) = mpsc::unbounded_channel();
    let (self_update, self_update_inbox) = mpsc::unbounded_channel();
    let (server_update, server_update_inbox) = mpsc::unbounded_channel();
    (
        Controls {
            queue,
            settings,
            hand_over,
            self_update,
            server_update,
        },
        Inbox {
            queue: queue_inbox,
            settings: settings_inbox,
            hand_over: hand_over_inbox,
            self_update: self_update_inbox,
            server_update: server_update_inbox,
        },
    )
}

/// What one command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Open the web interface in the browser.
    Open,
    /// A request to the queue (RD-1100-06).
    Queue(QueueRequest),
    /// Pause or resume clipboard watching (RD-1180-01).
    ToggleClipboard,
    /// Read the clipboard once and hand its links over (RD-1180-03).
    HandOver,
    /// Switch game mode on or off (RD-1240-23).
    ToggleGameMode,
    /// Install the service's offered update, or open its page without the right (RD-1240-25).
    InstallServerUpdate,
    /// Switch the agent's automatic update on or off (RD-1240-27).
    ToggleAutoInstall,
    /// Install the agent's own offered update (RD-1210-03), or say why not (RD-1240-24).
    InstallUpdate,
    /// Restart the service for what waits for the next start (RD-1240-32).
    RestartServer,
    /// End the agent.
    Quit,
}

pub(crate) fn action(command: CaptureCommand) -> Action {
    match command {
        CaptureCommand::Open => Action::Open,
        CaptureCommand::StartAll => Action::Queue(QueueRequest::Resume),
        CaptureCommand::PauseAll => Action::Queue(QueueRequest::Pause { minutes: None }),
        CaptureCommand::PauseHalfHour => Action::Queue(QueueRequest::Pause { minutes: Some(30) }),
        CaptureCommand::PauseHour => Action::Queue(QueueRequest::Pause { minutes: Some(60) }),
        CaptureCommand::ClipboardWatch => Action::ToggleClipboard,
        CaptureCommand::SendClipboard => Action::HandOver,
        CaptureCommand::GameMode => Action::ToggleGameMode,
        CaptureCommand::InstallServerUpdate => Action::InstallServerUpdate,
        CaptureCommand::AutoInstall => Action::ToggleAutoInstall,
        CaptureCommand::Quit => Action::Quit,
        CaptureCommand::AddAllFromLinkGrabber => {
            Action::Queue(QueueRequest::AddLinkGrabber { paused: false })
        }
        CaptureCommand::AddAllFromLinkGrabberPaused => {
            Action::Queue(QueueRequest::AddLinkGrabber { paused: true })
        }
        CaptureCommand::InstallUpdate => Action::InstallUpdate,
        CaptureCommand::RestartServer => Action::RestartServer,
    }
}

impl Controls {
    /// Passes the agent's part of an action to its task. `Open` and `Quit` are the caller's: the
    /// tray opens and quits on its own event loop. A task that has stopped is a log line.
    pub(crate) fn pass(&self, action: Action) {
        let sent = match action {
            Action::Queue(request) => self.queue.send(request).is_ok(),
            Action::ToggleClipboard => self.settings.send(SettingsRequest::ToggleClipboard).is_ok(),
            Action::HandOver => self.hand_over.send(()).is_ok(),
            Action::ToggleGameMode => self.settings.send(SettingsRequest::ToggleGameMode).is_ok(),
            Action::InstallServerUpdate => self
                .server_update
                .send(server_update::Request::Install)
                .is_ok(),
            Action::RestartServer => self
                .server_update
                .send(server_update::Request::Restart)
                .is_ok(),
            Action::ToggleAutoInstall => self
                .self_update
                .send(self_update::Request::ToggleAutoInstall)
                .is_ok(),
            Action::InstallUpdate => self.self_update.send(self_update::Request::Install).is_ok(),
            Action::Open | Action::Quit => true,
        };
        if !sent {
            tracing::warn!(
                ?action,
                "the agent's task has stopped; the request was dropped"
            );
        }
    }
}

/// Starts following the agent's settings and, for a tray, forwarding them to it. Returns the
/// receiver the clipboard loop and the shortcuts read.
pub(crate) fn follow_settings(
    background: &mut tokio::task::JoinSet<()>,
    client: &CaptureClient,
    cancellation: &CancellationToken,
    notice: Option<NoticeSink>,
    requests: mpsc::UnboundedReceiver<SettingsRequest>,
    sink: Option<SettingsSink>,
) -> watch::Receiver<CaptureAgentSettings> {
    let cache = crate::config::config_directory().ok();
    let (sender, receiver) = watch::channel(agent_settings::load_cached(cache.as_deref()));
    let (task_client, task_cancellation) = (client.clone(), cancellation.clone());
    background.spawn(supervised(
        "the settings poll",
        cancellation.clone(),
        notice,
        async move {
            agent_settings::follow(task_client, task_cancellation, sender, requests, cache).await;
            Ok(())
        },
    ));
    if let Some(sink) = sink {
        background.spawn(forward(receiver.clone(), sink, cancellation.clone()));
    }
    receiver
}

/// Hands the settings to the tray now and on every change.
async fn forward(
    mut settings: watch::Receiver<CaptureAgentSettings>,
    sink: SettingsSink,
    cancellation: CancellationToken,
) {
    loop {
        let current = settings.borrow_and_update().clone();
        sink(current);
        tokio::select! {
            () = cancellation.cancelled() => return,
            changed = settings.changed() => if changed.is_err() { return },
        }
    }
}

/// Carries out the queue requests of a run without a tray, where nothing polls the summary:
/// on Linux, the shortcuts for "Start all", the pauses and the LinkGrabber entries.
pub(crate) async fn serve_queue_requests(
    client: CaptureClient,
    cancellation: CancellationToken,
    mut requests: mpsc::UnboundedReceiver<QueueRequest>,
) {
    loop {
        let request = tokio::select! {
            () = cancellation.cancelled() => return,
            request = requests.recv() => match request {
                Some(request) => request,
                None => return,
            },
        };
        if let Err(error) = crate::activity::carry_out(&client, request).await {
            tracing::warn!(%error, ?request, "the shortcut's queue request was not carried out");
        }
    }
}

#[cfg(test)]
mod tests {
    use rd_core::CaptureCommand;

    use super::{Action, action, channels};
    use crate::{activity::QueueRequest, agent_settings::SettingsRequest};

    /// A click and a key press are the same command: each one does what its menu entry says.
    #[test]
    fn every_command_does_what_its_menu_entry_says() {
        assert_eq!(action(CaptureCommand::Open), Action::Open);
        assert_eq!(
            action(CaptureCommand::StartAll),
            Action::Queue(QueueRequest::Resume)
        );
        assert_eq!(
            action(CaptureCommand::PauseAll),
            Action::Queue(QueueRequest::Pause { minutes: None })
        );
        assert_eq!(
            action(CaptureCommand::PauseHalfHour),
            Action::Queue(QueueRequest::Pause { minutes: Some(30) })
        );
        assert_eq!(
            action(CaptureCommand::PauseHour),
            Action::Queue(QueueRequest::Pause { minutes: Some(60) })
        );
        assert_eq!(
            action(CaptureCommand::ClipboardWatch),
            Action::ToggleClipboard
        );
        assert_eq!(action(CaptureCommand::SendClipboard), Action::HandOver);
        assert_eq!(action(CaptureCommand::GameMode), Action::ToggleGameMode);
        assert_eq!(
            action(CaptureCommand::InstallServerUpdate),
            Action::InstallServerUpdate
        );
        assert_eq!(
            action(CaptureCommand::AutoInstall),
            Action::ToggleAutoInstall
        );
        assert_eq!(action(CaptureCommand::Quit), Action::Quit);
        assert_eq!(
            action(CaptureCommand::AddAllFromLinkGrabber),
            Action::Queue(QueueRequest::AddLinkGrabber { paused: false })
        );
        assert_eq!(
            action(CaptureCommand::AddAllFromLinkGrabberPaused),
            Action::Queue(QueueRequest::AddLinkGrabber { paused: true })
        );
        assert_eq!(action(CaptureCommand::InstallUpdate), Action::InstallUpdate);
        assert_eq!(action(CaptureCommand::RestartServer), Action::RestartServer);
    }

    #[test]
    fn each_action_reaches_the_task_that_carries_it_out() {
        let (controls, mut inbox) = channels();
        controls.pass(action(CaptureCommand::PauseHour));
        controls.pass(action(CaptureCommand::ClipboardWatch));
        controls.pass(action(CaptureCommand::SendClipboard));
        controls.pass(action(CaptureCommand::AutoInstall));
        controls.pass(action(CaptureCommand::Open));
        assert_eq!(
            inbox.queue.try_recv().ok(),
            Some(QueueRequest::Pause { minutes: Some(60) })
        );
        assert_eq!(
            inbox.settings.try_recv().ok(),
            Some(SettingsRequest::ToggleClipboard)
        );
        assert_eq!(inbox.hand_over.try_recv().ok(), Some(()));
        assert_eq!(
            inbox.self_update.try_recv().ok(),
            Some(crate::self_update::Request::ToggleAutoInstall)
        );
        assert!(inbox.queue.try_recv().is_err(), "Open is the caller's own");
    }

    /// "Pause while gaming", clicked or pressed, goes to the settings follower, which holds the
    /// token and the switch until the service has it (RD-1240-23).
    #[test]
    fn the_game_mode_switch_reaches_the_settings_task() {
        let (controls, mut inbox) = channels();
        controls.pass(action(CaptureCommand::GameMode));
        assert_eq!(
            inbox.settings.try_recv().ok(),
            Some(SettingsRequest::ToggleGameMode)
        );
    }

    /// "Install server update", clicked or pressed, goes to the server update's task, which holds
    /// the token and decides between installing and opening the update page (RD-1240-25).
    #[test]
    fn the_server_update_reaches_its_task() {
        let (controls, mut inbox) = channels();
        controls.pass(action(CaptureCommand::InstallServerUpdate));
        assert_eq!(
            inbox.server_update.try_recv().ok(),
            Some(crate::server_update::Request::Install)
        );
        assert!(inbox.self_update.try_recv().is_err(), "not the agent's own");
    }

    /// "Restart server", clicked or pressed, goes to the same task, which holds the token and the
    /// reading that says whether a restart is pending (RD-1240-32).
    #[test]
    fn the_server_restart_reaches_the_server_update_task() {
        let (controls, mut inbox) = channels();
        controls.pass(action(CaptureCommand::RestartServer));
        assert_eq!(
            inbox.server_update.try_recv().ok(),
            Some(crate::server_update::Request::Restart)
        );
    }

    /// "Add all from LinkGrabber", clicked or pressed, travels with the queue requests, to the
    /// task that holds the token (RD-1240-07, RD-1240-24); the service refuses an agent without
    /// `capture:queue`, and the notification says so.
    #[test]
    fn adding_from_the_linkgrabber_reaches_the_queue_task() {
        let (controls, mut inbox) = channels();
        controls.pass(action(CaptureCommand::AddAllFromLinkGrabberPaused));
        assert_eq!(
            inbox.queue.try_recv().ok(),
            Some(QueueRequest::AddLinkGrabber { paused: true })
        );
    }

    /// "Install update", clicked or pressed, goes to the agent's own update task, which decides
    /// whether there is anything to install (RD-1240-24).
    #[test]
    fn the_agents_update_reaches_its_task() {
        let (controls, mut inbox) = channels();
        controls.pass(action(CaptureCommand::InstallUpdate));
        assert_eq!(
            inbox.self_update.try_recv().ok(),
            Some(crate::self_update::Request::Install)
        );
        assert!(inbox.server_update.try_recv().is_err(), "not the service's");
    }
}
