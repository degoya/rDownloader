//! Tray/menu-bar icon for the capture agent on Windows and macOS.
//!
//! The tray needs a platform UI event loop on the main thread, so `main` hands
//! this module control: [`prepare`] creates the loop, [`Tray::run`] spawns the
//! agent onto a tokio runtime and never returns. Linux is headless and does not
//! compile — or link — any of this.

use std::time::{Duration, Instant};

use anyhow::Result;
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use rd_core::{CaptureAgentSettings, CaptureCommand, CaptureShortcutReport};
use tao::{
    event::{Event, StartCause},
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
};
use tokio_util::sync::CancellationToken;
use tray_icon::{
    Icon, TrayIconEvent,
    menu::{MenuEvent, MenuId},
};
use url::Url;

use crate::{
    DesktopSinks,
    activity::Activity,
    agent_settings::SettingsRequest,
    cli::RunArgs,
    config,
    controls::{self, Action, Controls},
    hotkeys::{self, Hotkeys},
    self_update::UpdateMenu,
    server_update,
    status::ServerStatus,
    supervision::AgentNotice,
    tray_state::{IconKind, TrayState, Update},
};

#[path = "tray_handle.rs"]
mod handle;
#[path = "tray_health.rs"]
mod health;
#[path = "tray_marks.rs"]
mod marks;

use handle::TrayHandle;
use health::spawn_health_poll;
use marks::Marks;

/// How long "Quit" waits for the agent to wind down before exiting anyway.
const QUIT_GRACE: Duration = Duration::from_secs(3);

/// Everything the event loop is woken up for.
enum UserEvent {
    Menu(MenuEvent),
    /// The agent future finished, carrying the error it failed with, if any.
    AgentExited(Option<anyhow::Error>),
    /// The health poll's reading: the service's state, and its version while it names one.
    Server(ServerStatus, Option<String>),
    /// The transfer summary changed.
    Transfers(Activity),
    /// The agent has something to say about itself: a task that ended, or an address it did not
    /// get. Nothing puts either right on its own, so it belongs in the status line.
    Notice(AgentNotice),
    /// The service changed the clipboard pause, the shortcuts or game mode (RD-1180-01,
    /// RD-1180-03, RD-1240-23).
    /// Boxed: with game mode, the server update and the automatic update it is the largest variant
    /// by far, and every other event would carry its size.
    Settings(Box<CaptureAgentSettings>),
    /// A system-wide shortcut was pressed; carries its registration's id.
    Hotkey(u32),
    /// The agent's own update entries changed (RD-1210-03, RD-1240-27): shown, relabelled,
    /// ticked, or gone.
    SelfUpdate(UpdateMenu),
    /// The service's update entry or its install changed (RD-1240-25).
    ServerUpdate(server_update::View),
}

/// A prepared, not yet running event loop.
pub(crate) struct Tray {
    event_loop: EventLoop<UserEvent>,
    marks: Marks,
}

/// Creates the platform event loop and decodes the icon.
///
/// Everything that can fail before the agent starts happens here, so `main` can
/// still fall back to the headless path.
pub(crate) fn prepare() -> Result<Tray> {
    // `set_activation_policy` needs a mutable loop; on Windows nothing does.
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};

        // Menu-bar extra: no Dock icon and no application menu.
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    Ok(Tray {
        event_loop,
        marks: Marks::prepare()?,
    })
}

impl Tray {
    /// Runs the agent under the tray event loop. Never returns: the process
    /// ends through [`Agent::finish`].
    pub(crate) fn run(self, args: RunArgs) -> Result<()> {
        let Self { event_loop, marks } = self;
        let proxy = event_loop.create_proxy();
        let menu_proxy = proxy.clone();
        let health_proxy = proxy.clone();
        let hotkey_proxy = proxy.clone();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = menu_proxy.send_event(UserEvent::Menu(event));
        }));
        // Only the press: a shortcut held down is one command, not two.
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state == HotKeyState::Pressed {
                let _ = hotkey_proxy.send_event(UserEvent::Hotkey(event.id));
            }
        }));
        // Nothing reacts to clicks on the icon itself. Without a handler the
        // events would pile up in tray-icon's unbounded channel forever.
        TrayIconEvent::set_event_handler(Some(|_event: TrayIconEvent| {}));

        let service = config::stored_service(args.connection.service.clone());
        let runtime = crate::runtime()?;
        let cancellation = CancellationToken::new();
        let agent_cancellation = cancellation.clone();
        let activity_proxy = proxy.clone();
        let notice_proxy = proxy.clone();
        let settings_proxy = proxy.clone();
        let update_proxy = proxy.clone();
        let server_update_proxy = proxy.clone();
        // The menu and the shortcuts name a command, the agent carries it out: the agent holds
        // the token.
        let (controls, inbox) = controls::channels();
        runtime.spawn(async move {
            let desktop = DesktopSinks {
                activity: std::sync::Arc::new(move |activity: Activity| {
                    let _ = activity_proxy.send_event(UserEvent::Transfers(activity));
                }),
                notice: std::sync::Arc::new(move |notice: AgentNotice| {
                    let _ = notice_proxy.send_event(UserEvent::Notice(notice));
                }),
                settings: std::sync::Arc::new(move |settings: CaptureAgentSettings| {
                    let _ = settings_proxy.send_event(UserEvent::Settings(Box::new(settings)));
                }),
                inbox,
                update: std::sync::Arc::new(move |menu: UpdateMenu| {
                    let _ = update_proxy.send_event(UserEvent::SelfUpdate(menu));
                }),
                server_update: std::sync::Arc::new(move |view: server_update::View| {
                    let _ = server_update_proxy.send_event(UserEvent::ServerUpdate(view));
                }),
            };
            let result = crate::run(args, agent_cancellation, Some(desktop)).await;
            let _ = proxy.send_event(UserEvent::AgentExited(result.err()));
        });
        // Polls the unauthenticated health endpoint, so the tray reflects the service whether
        // or not the agent is paired.
        spawn_health_poll(
            &runtime,
            service.clone().unwrap_or_else(config::default_service),
            health_proxy,
        );
        let mut agent = Agent {
            marks,
            state: TrayState::new(service.clone()),
            service: service.unwrap_or_else(config::default_service),
            tray: None,
            shown: false,
            controls,
            hotkeys: None,
            cancellation,
            quit_deadline: None,
            update_menu: UpdateMenu::default(),
            server_view: server_update::View::default(),
            _runtime: runtime,
        };

        event_loop.run(move |event, _target, control_flow| {
            match event {
                Event::NewEvents(StartCause::Init) => agent.show_tray(),
                Event::UserEvent(UserEvent::Menu(event)) => agent.on_menu(&event.id),
                Event::UserEvent(UserEvent::AgentExited(error)) => agent.finish(error),
                Event::UserEvent(UserEvent::Server(status, version)) => {
                    agent.on_server_status(status, version);
                }
                Event::UserEvent(UserEvent::Transfers(activity)) => agent.on_transfers(activity),
                Event::UserEvent(UserEvent::Notice(notice)) => agent.on_notice(&notice),
                Event::UserEvent(UserEvent::Settings(settings)) => agent.on_settings(&settings),
                Event::UserEvent(UserEvent::Hotkey(id)) => agent.on_hotkey(id),
                Event::UserEvent(UserEvent::SelfUpdate(menu)) => agent.on_self_update(menu),
                Event::UserEvent(UserEvent::ServerUpdate(view)) => agent.on_server_update(view),
                _ => {}
            }
            // Checked on every iteration rather than only on `ResumeTimeReached`:
            // an unrelated event can cancel the wait right at the deadline.
            agent.check_quit_deadline();
            *control_flow = agent.control_flow();
        })
    }
}

/// Event-loop state of the running agent.
///
/// What the tray should show is not decided here. Every event goes through [`TrayState`], which
/// compiles and is tested on every host, and hands back an [`Update`]; this struct owns the
/// handles and the two icons and applies the update to them, nothing more.
struct Agent {
    marks: Marks,
    /// What the surface should show, and what each event changes on it.
    state: TrayState,
    service: Url,
    tray: Option<TrayHandle>,
    /// Set once the tray has been built or has failed to build; either way it is attempted once.
    shown: bool,
    /// Where the commands of the menu and the shortcuts go: to the agent's tasks (RD-1100-06,
    /// RD-1180-01, RD-1180-03).
    controls: Controls,
    /// The registered shortcuts. Set up with the tray, on this thread: on Windows and macOS the
    /// thread that registers a shortcut is the one whose event loop receives it.
    hotkeys: Option<Hotkeys>,
    cancellation: CancellationToken,
    /// Set once the user asked to quit; bounds the wait for the agent.
    quit_deadline: Option<Instant>,
    /// The update entries, the agent's and the service's, kept for a tray built after they
    /// arrived and for writing all of them whenever one changes.
    update_menu: UpdateMenu,
    server_view: server_update::View,
    // Owned by the event loop so the agent's tasks keep running: dropping the
    // runtime would abort them.
    _runtime: tokio::runtime::Runtime,
}

impl Agent {
    /// Builds the tray on the first loop iteration; macOS requires this to
    /// happen once the event loop is up, and Windows is happy either way.
    fn show_tray(&mut self) {
        if self.shown {
            return;
        }
        self.shown = true;
        let surface = self.state.surface();
        match TrayHandle::build(self.mark(surface.icon), &surface) {
            Ok(handle) => {
                handle.show_updates(&self.server_view, &self.update_menu);
                self.tray = Some(handle);
            }
            Err(error) => {
                tracing::warn!(%error, "tray icon unavailable; the agent keeps running headless");
            }
        }
        match Hotkeys::new() {
            Ok(mut registered) => {
                if let Some(report) = registered.apply(&surface.accelerators) {
                    self.report(report);
                }
                self.hotkeys = Some(registered);
            }
            Err(reason) => self.report(hotkeys::unavailable(reason)),
        }
    }

    /// The service changed the clipboard pause or the shortcuts; see [`TrayState::on_settings`].
    /// Changed shortcuts are registered again here, without a restart.
    fn on_settings(&mut self, settings: &CaptureAgentSettings) {
        let update = self.state.on_settings(settings);
        let report = match (&update.accelerators, self.hotkeys.as_mut()) {
            (Some(shortcuts), Some(registered)) => registered.apply(shortcuts),
            _ => None,
        };
        if let Some(report) = report {
            self.report(report);
        }
        self.apply(update);
    }

    /// What registering the shortcuts came to, for the settings page.
    fn report(&self, report: CaptureShortcutReport) {
        if self
            .controls
            .settings
            .send(SettingsRequest::Report(report))
            .is_err()
        {
            tracing::debug!("the agent has stopped; the shortcut report was dropped");
        }
    }

    fn on_hotkey(&mut self, id: u32) {
        let Some(command) = self
            .hotkeys
            .as_ref()
            .and_then(|registered| registered.command(id))
        else {
            return;
        };
        tracing::info!(?command, "shortcut pressed");
        self.carry_out(command);
    }

    /// The health poll saw the service change state; see [`TrayState::on_server_status`].
    fn on_server_status(&mut self, status: ServerStatus, version: Option<String>) {
        let update = self.state.on_server_status(status, version);
        self.apply(update);
    }

    /// The transfer poll reported; see [`TrayState::on_transfers`].
    fn on_transfers(&mut self, activity: Activity) {
        let update = self.state.on_transfers(activity);
        self.apply(update);
    }

    /// The agent noticed something about itself; see [`TrayState::on_notice`].
    fn on_notice(&mut self, notice: &AgentNotice) {
        let update = self.state.on_notice(notice);
        self.apply(update);
    }

    /// Puts an update on the handles. Before the tray is built there is nothing to put it on,
    /// and the state has already recorded it for the build.
    fn apply(&self, update: Update) {
        if update.is_empty() {
            return;
        }
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        if let Some(kind) = update.icon {
            let _ = tray.set_icon(self.mark(kind));
        }
        if let Some(enabled) = update.open_enabled {
            tray.open_item.set_enabled(enabled);
        }
        if let Some(queue) = update.queue {
            tray.show_queue(queue);
        }
        if let Some(line) = update.status_line {
            tray.status_item.set_text(line);
        }
        if let Some(line) = update.server_line {
            tray.server_item.set_text(line);
        }
        if let Some(tooltip) = update.tooltip {
            let _ = tray.set_tooltip(&tooltip);
        }
        if let Some(paused) = update.clipboard_paused {
            tray.show_clipboard_paused(paused);
        }
        if let Some(shortcuts) = update.accelerators {
            tray.show_accelerators(&shortcuts);
        }
        if let Some(entry) = update.game_mode {
            tray.show_game_mode(entry);
        }
    }

    /// The icon handle for a mark the state named.
    fn mark(&self, kind: IconKind) -> Icon {
        self.marks.get(kind)
    }

    /// Shows, relabels, ticks or removes the agent's own update entries (RD-1210-03,
    /// RD-1240-27).
    fn on_self_update(&mut self, menu: UpdateMenu) {
        self.update_menu = menu;
        self.show_updates();
    }

    /// The service's update entry and the server line while it installs (RD-1240-25), or while a
    /// restart is pending (RD-1240-32).
    fn on_server_update(&mut self, view: server_update::View) {
        let update = self
            .state
            .on_server_update(view.updating.clone(), view.restart_pending);
        self.apply(update);
        self.server_view = view;
        self.show_updates();
    }

    fn show_updates(&self) {
        if let Some(tray) = self.tray.as_ref() {
            tray.show_updates(&self.server_view, &self.update_menu);
        }
    }

    /// A click: the entry's command, carried out as its shortcut is (RD-1240-24).
    fn on_menu(&mut self, id: &MenuId) {
        let Some(command) = self.tray.as_ref().and_then(|tray| tray.command(id)) else {
            return;
        };
        // The click ticked a check entry already; the agent's settings decide, and the update
        // that follows them sets it again.
        if let Some(tray) = self.tray.as_ref() {
            match command {
                CaptureCommand::ClipboardWatch => {
                    tray.show_clipboard_paused(self.state.surface().clipboard_paused);
                }
                CaptureCommand::GameMode => tray.show_game_mode(self.state.surface().game_mode),
                _ => {}
            }
        }
        if command == CaptureCommand::AutoInstall {
            // The same for the automatic update: the watch answers with the stored switch.
            self.show_updates();
        }
        self.carry_out(command);
    }

    /// One command, from the menu or a shortcut: the same action either way.
    fn carry_out(&mut self, command: CaptureCommand) {
        match controls::action(command) {
            Action::Open => {
                // A shortcut obeys the same rule as the greyed-out entry: the service that is
                // not answering would only show an error page.
                if !self.state.surface().open_enabled {
                    tracing::info!("rDownloader is not answering; not opening it");
                    return;
                }
                tracing::info!(service = %self.service, "opening rDownloader from the tray");
                if let Err(error) = open::that_detached(self.service.as_str()) {
                    tracing::warn!(%error, "could not open the rDownloader web interface");
                }
            }
            // A shortcut obeys the greyed-out entry here too: nothing set up to watch for, or no
            // queue control to switch it with (RD-1240-23).
            Action::ToggleGameMode if !self.state.surface().game_mode.enabled() => {
                let reason = self.state.surface().game_mode.refused.unwrap_or_default();
                tracing::info!(reason, "not switching game mode");
            }
            Action::Quit => {
                tracing::info!("shutting down on tray request");
                self.cancellation.cancel();
                self.quit_deadline = Some(Instant::now() + QUIT_GRACE);
            }
            action => {
                tracing::info!(?action, "request from the tray");
                self.controls.pass(action);
            }
        }
    }

    fn check_quit_deadline(&mut self) {
        if self.quit_deadline.is_some_and(|at| Instant::now() >= at) {
            tracing::warn!("agent did not stop within the shutdown grace period");
            self.finish(None);
        }
    }

    /// Sleeps until the shutdown grace period runs out, or indefinitely while nothing is
    /// waiting on a clock.
    fn control_flow(&self) -> ControlFlow {
        match self.quit_deadline {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        }
    }

    /// Ends the process the way `main` does: the agent's account of the error, if any, on
    /// stderr, and the exit code that goes with it.
    ///
    /// Goes through `crate::conclude` rather than ending on 1, so that "not paired yet" and
    /// "another Click'n'Load listener has the port" reach a launcher from the tray path too, and
    /// an agent whose program was replaced continues as the new one (RD-190-07).
    fn finish(&mut self, error: Option<anyhow::Error>) -> ! {
        // The icon lingers in the notification area unless it is dropped before
        // the process goes away.
        self.tray = None;
        let Some(error) = error else {
            std::process::exit(0);
        };
        std::process::exit(i32::from(crate::conclude(&error)));
    }
}
