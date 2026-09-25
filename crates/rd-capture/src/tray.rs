//! Tray/menu-bar icon for the capture agent on Windows and macOS.
//!
//! The tray needs a platform UI event loop on the main thread, so `main` hands
//! this module control: [`prepare`] creates the loop, [`Tray::run`] spawns the
//! agent onto a tokio runtime and never returns. Linux is headless and does not
//! compile — or link — any of this.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tao::{
    event::{Event, StartCause},
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
};
use tokio_util::sync::CancellationToken;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
};
use url::Url;

use crate::{
    DesktopSinks,
    activity::Activity,
    cli::RunArgs,
    config, icon as badge,
    status::{HEALTH_INTERVAL, HealthWatch, ServerStatus, health_probe},
    supervision::AgentNotice,
    tray_state::{IconKind, Surface, TrayState, Update},
};

// The browser extension artwork doubles as the tray icon. Reaching across the
// crate boundary with `include_bytes!` would break `cargo package`; this crate
// is never published, so duplicating the PNG is not worth it.
const ICON_PNG: &[u8] = include_bytes!("../../../extension/icons/icon32.png");

/// How long "Quit" waits for the agent to wind down before exiting anyway.
const QUIT_GRACE: Duration = Duration::from_secs(3);

/// Everything the event loop is woken up for.
enum UserEvent {
    Menu(MenuEvent),
    /// The agent future finished, carrying the error it failed with, if any.
    AgentExited(Option<anyhow::Error>),
    /// The health poll saw the service change state.
    Server(ServerStatus),
    /// The transfer summary changed.
    Transfers(Activity),
    /// The agent has something to say about itself: a task that ended, or an address it did not
    /// get. Nothing puts either right on its own, so it belongs in the status line.
    Notice(AgentNotice),
}

/// A prepared, not yet running event loop.
pub struct Tray {
    event_loop: EventLoop<UserEvent>,
    icon: Icon,
    /// The same mark with a badge, shown while something is transferring.
    busy: Icon,
}

/// Creates the platform event loop and decodes the icon.
///
/// Everything that can fail before the agent starts happens here, so `main` can
/// still fall back to the headless path.
pub fn prepare() -> Result<Tray> {
    let image = decode_image()?;
    let icon = to_icon(image.clone())?;
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
        busy: busy_icon(&image)?,
        icon,
    })
}

impl Tray {
    /// Runs the agent under the tray event loop. Never returns: the process
    /// ends through [`Agent::finish`].
    pub fn run(self, args: RunArgs) -> Result<()> {
        let Self {
            event_loop,
            icon,
            busy,
        } = self;
        let proxy = event_loop.create_proxy();
        let menu_proxy = proxy.clone();
        let health_proxy = proxy.clone();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = menu_proxy.send_event(UserEvent::Menu(event));
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
        runtime.spawn(async move {
            let desktop = DesktopSinks {
                activity: std::sync::Arc::new(move |activity: Activity| {
                    let _ = activity_proxy.send_event(UserEvent::Transfers(activity));
                }),
                notice: std::sync::Arc::new(move |notice: AgentNotice| {
                    let _ = notice_proxy.send_event(UserEvent::Notice(notice));
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
            idle_icon: icon,
            busy_icon: busy,
            state: TrayState::new(service.clone()),
            service: service.unwrap_or_else(config::default_service),
            tray: None,
            shown: false,
            cancellation,
            quit_deadline: None,
            _runtime: runtime,
        };

        event_loop.run(move |event, _target, control_flow| {
            match event {
                Event::NewEvents(StartCause::Init) => agent.show_tray(),
                Event::UserEvent(UserEvent::Menu(event)) => agent.on_menu(&event.id),
                Event::UserEvent(UserEvent::AgentExited(error)) => agent.finish(error),
                Event::UserEvent(UserEvent::Server(status)) => agent.on_server_status(status),
                Event::UserEvent(UserEvent::Transfers(activity)) => agent.on_transfers(activity),
                Event::UserEvent(UserEvent::Notice(notice)) => agent.on_notice(&notice),
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
    /// The two marks, kept for the whole run: the tray is handed one or the other whenever
    /// transfers start or stop.
    idle_icon: Icon,
    busy_icon: Icon,
    /// What the surface should show, and what each event changes on it.
    state: TrayState,
    service: Url,
    tray: Option<TrayHandle>,
    /// Set once the tray has been built or has failed to build; either way it is attempted once.
    shown: bool,
    cancellation: CancellationToken,
    /// Set once the user asked to quit; bounds the wait for the agent.
    quit_deadline: Option<Instant>,
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
            Ok(handle) => self.tray = Some(handle),
            Err(error) => {
                tracing::warn!(%error, "tray icon unavailable; the agent keeps running headless");
            }
        }
    }

    /// The health poll saw the service change state; see [`TrayState::on_server_status`].
    fn on_server_status(&mut self, status: ServerStatus) {
        let update = self.state.on_server_status(status);
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
        if let Some(line) = update.status_line {
            tray.status_item.set_text(line);
        }
        if let Some(tooltip) = update.tooltip {
            let _ = tray.set_tooltip(&tooltip);
        }
    }

    /// The icon handle for a mark the state named.
    fn mark(&self, kind: IconKind) -> Icon {
        match kind {
            IconKind::Idle => self.idle_icon.clone(),
            IconKind::Busy => self.busy_icon.clone(),
        }
    }

    fn on_menu(&mut self, id: &MenuId) {
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        if *id == tray.open {
            tracing::info!(service = %self.service, "opening rDownloader from the tray");
            if let Err(error) = open::that_detached(self.service.as_str()) {
                tracing::warn!(%error, "could not open the rDownloader web interface");
            }
        } else if *id == tray.quit {
            tracing::info!("shutting down on tray request");
            self.cancellation.cancel();
            self.quit_deadline = Some(Instant::now() + QUIT_GRACE);
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
    /// Goes through `crate::report` rather than ending on 1, so that "not paired yet" and
    /// "another Click'n'Load listener has the port" reach a launcher from the tray path too.
    fn finish(&mut self, error: Option<anyhow::Error>) -> ! {
        // The icon lingers in the notification area unless it is dropped before
        // the process goes away.
        self.tray = None;
        let Some(error) = error else {
            std::process::exit(0);
        };
        std::process::exit(i32::from(crate::report(&error)));
    }
}

/// The live tray icon together with the ids of its actionable menu entries.
struct TrayHandle {
    open: MenuId,
    quit: MenuId,
    /// Kept, not just their ids: the status line is rewritten and "Open" is greyed out while
    /// the service is not answering, which needs the items themselves. What to write on them
    /// is `tray_state`'s decision; here they are only written.
    status_item: MenuItem,
    open_item: MenuItem,
    // Dropping this removes the icon from the tray. Kept named rather than `_tray` since the
    // icon and tooltip are now changed while it lives.
    tray: TrayIcon,
}

impl TrayHandle {
    fn set_icon(&self, icon: Icon) -> Result<()> {
        self.tray
            .set_icon(Some(icon))
            .context("replace the tray icon")
    }

    fn set_tooltip(&self, text: &str) -> Result<()> {
        self.tray
            .set_tooltip(Some(text))
            .context("set the tray tooltip")
    }
}

impl TrayHandle {
    /// Builds the icon and its menu as the state describes them at this moment.
    fn build(icon: Icon, surface: &Surface) -> Result<Self> {
        let menu = Menu::new();
        let open = MenuItem::new("Open rDownloader", surface.open_enabled, None);
        let quit = MenuItem::new("Quit", true, None);
        let status_item = MenuItem::new(&surface.status_line, false, None);
        menu.append(&status_item)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&open)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit)?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(&surface.tooltip)
            .with_icon(icon)
            .build()
            .context("create the tray icon")?;
        Ok(Self {
            open: open.id().clone(),
            quit: quit.id().clone(),
            status_item,
            open_item: open,
            tray,
        })
    }
}

/// Reports the service's reachability to the event loop.
///
/// `/api/v1/health` needs no authentication, so this works before the agent is paired and says
/// nothing about the installation beyond whether it answers. A silence right after launch reads
/// as "starting" rather than "not reachable": both are launched together at login, and the
/// service takes far longer to come up than the tray does.
///
/// Nothing here decides anything: the request is made, its outcome is handed to `health_probe`,
/// and what a run of probes means is [`HealthWatch`]. Both live in `status`, which compiles and
/// is tested on every host — this function is only the part that cannot exist without an event
/// loop to send the result to.
fn spawn_health_poll(
    runtime: &tokio::runtime::Runtime,
    service: Url,
    proxy: tao::event_loop::EventLoopProxy<UserEvent>,
) {
    runtime.spawn(async move {
        let Ok(health) = service.join("api/v1/health") else {
            let _ = proxy.send_event(UserEvent::Server(ServerStatus::Unreachable));
            return;
        };
        // Not `unwrap_or_default()`: that produced a client *without* the deadline it was
        // written for, so the poll hung on its first request, the status line froze on whatever
        // it last said, and no line explained it. A client that cannot be built is a fault, and
        // the state it leaves behind is "not reachable" (RD-109-06).
        let client = match crate::client::build(crate::client::Purpose::Health) {
            Ok(client) => client,
            Err(error) => {
                tracing::error!(
                    %error,
                    "the tray cannot build an HTTP client; the service state stays unknown"
                );
                let _ = proxy.send_event(UserEvent::Server(ServerStatus::Unreachable));
                return;
            }
        };
        let started = Instant::now();
        let mut watch = HealthWatch::default();
        loop {
            // The status is the whole of what the rule reads; a transport failure has no status
            // at all, and that absence is what "silence" means one line further down.
            let answer = client
                .get(health.clone())
                .send()
                .await
                .ok()
                .map(|response| response.status());
            let status = watch.observe(health_probe(answer), started.elapsed());
            // Sending on every tick is fine: the agent ignores a status it already holds.
            let _ = proxy.send_event(UserEvent::Server(status));
            tokio::time::sleep(HEALTH_INTERVAL).await;
        }
    });
}

fn decode_image() -> Result<image::RgbaImage> {
    Ok(
        image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
            .context("decode the tray icon")?
            .into_rgba8(),
    )
}

fn to_icon(image: image::RgbaImage) -> Result<Icon> {
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).context("build the tray icon")
}

/// The idle mark with a filled corner badge, for "something is transferring".
///
/// Derived from the same image rather than shipped as a second file: one asset cannot drift from
/// the other, and `web/public/favicon.svg` stays the single source every icon is generated from.
///
/// Where the badge sits and which pixels it covers is [`crate::icon`], which knows nothing about
/// `image` or `tray-icon` and is measured on Linux. What is left here is the pair of conversions
/// those two crates own.
fn busy_icon(base: &image::RgbaImage) -> Result<Icon> {
    let (width, height) = base.dimensions();
    let mut pixels = base.clone().into_raw();
    badge::paint_badge(&mut pixels, width, height);
    Icon::from_rgba(pixels, width, height).context("build the busy tray icon")
}
