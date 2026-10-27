//! A run of the agent in its two phases (RD-1390-02).
//!
//! An agent without a token used to end at once (exit code 10), so a freshly installed one showed
//! nothing at all. Now it runs **unlinked**: the instance lock, the listener and the tray are up,
//! and the Click'n'Load port carries only the link page ([`link_page`]) -- no Click'n'Load route,
//! so a hoster page sees no Click'n'Load. Once the page made a link, the same process runs
//! **linked**: [`agent_run::run_linked`] with Click'n'Load and the link page on the port. A new
//! link while linked ends that phase and starts the next with the new address and token, again
//! without a restart.
//!
//! Beside the phases, a run keeps:
//! - the page's address in `link.key` for `rdownloader-capture link` and the tray, and a
//!   `link.html` that forwards the browser to it, both written at the start, readable by this
//!   account only, removed at the end (`link_phase/files.rs`);
//! - the marker `link-page-shown`: an unlinked agent opens the page by itself once per
//!   installation, never again at a later login, a relaunch or after an update; `run
//!   --no-link-page` not even once;
//! - the tray's inbox, relayed to each linked phase, whose tasks end with it.

use std::{future::Future, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::Router;
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::sync::CancellationToken;
use url::Url;

// `link.key` and `link.html`; crate-wide for the tests of `link`.
pub(crate) mod files;

use files::LinkKey;
pub(crate) use files::{open_link_page, open_redirect, page_of_running_agent};

use crate::{
    DesktopSinks, agent_run,
    cli::RunArgs,
    cnl,
    config::{self, Connection},
    controls,
    link_page::{self, LinkSecret, LinkState, Linked, Store},
    relaunch,
    supervision::{NoticeSink, wind_down},
};

/// Present once the agent has opened the link page by itself; empty.
pub(crate) const LINK_PAGE_SHOWN_FILE: &str = "link-page-shown";

/// What the tray shows about the link. Read by the tray only, which Linux does not have.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) enum LinkStatus {
    Unlinked,
    Linked {
        service: Url,
        /// Plain `http` off this machine; the tray's server line says "(unencrypted)".
        insecure: bool,
    },
}

/// `http://127.0.0.1:<port>/link/<secret>`: the page of this agent.
pub(crate) fn link_page_url(port: u16, secret: &LinkSecret) -> Url {
    Url::parse(&format!("http://127.0.0.1:{port}/link/{}", secret.as_str()))
        .expect("a loopback address with a port and a hex path is a URL")
}

/// Finds the connection to run with, or `None` while there is no token.
pub(crate) type Connect = Arc<dyn Fn(&RunArgs) -> Result<Option<Connection>> + Send + Sync>;

/// Opens the link page in the browser; an error when it could not be started.
pub(crate) type Open = Arc<dyn Fn(&Url) -> std::io::Result<()> + Send + Sync>;

/// What a run reads and changes outside itself, passed in so a test brings its own.
pub(crate) struct Surroundings {
    /// The configuration directory: `link.key`, the marker, `capture.json`.
    pub directory: PathBuf,
    /// Keeps a verified link (`config::save`).
    pub store: Store,
    /// The connection to run with, or `None` while there is no token.
    pub connect: Connect,
    /// The agent's own program file, watched for an update that replaces it (RD-190-07); `None`
    /// when the system does not say which file this process runs from.
    pub program: Option<PathBuf>,
    /// How often the program file is looked at.
    pub check_every: std::time::Duration,
    /// Opens the link page in the browser: through `link.html`, never with the page's address,
    /// secret and all, on a command line every local account can read.
    pub open: Open,
}

impl Surroundings {
    /// This account's: its configuration, its keyring, its browser.
    fn of_this_user() -> Result<Self> {
        let directory = config::config_directory()?;
        Ok(Self {
            directory: directory.clone(),
            store: Arc::new(config::save),
            connect: Arc::new(|args: &RunArgs| {
                if !config::is_paired(args.connection.token.as_deref()) {
                    return Ok(None);
                }
                config::load(
                    args.connection.service.clone(),
                    args.connection.token.clone(),
                )
                .map(Some)
            }),
            program: std::env::current_exe().ok(),
            check_every: relaunch::CHECK_INTERVAL,
            open: Arc::new(move |_page: &Url| open_redirect(&directory)),
        })
    }
}

/// Runs the agent until `cancellation` fires, a listener fails or an updated program takes over:
/// unlinked, then linked, and linked again after every new link.
pub(crate) async fn run(
    args: RunArgs,
    cancellation: CancellationToken,
    desktop: Option<DesktopSinks>,
) -> Result<()> {
    agent_run::require_loopback(&args.cnl_listen)?;
    agent_run::cancel_on_shutdown_signal(&cancellation);
    run_phases(
        args,
        cancellation,
        desktop,
        Surroundings::of_this_user()?,
        agent_run::run_linked,
    )
    .await
}

/// How a phase ended.
enum Next {
    /// The run is over.
    Stop,
    /// A new link: the next phase starts with it.
    Relink,
}

/// The run of [`run`], with the linked phase passed in so a test can stand in for the agent's
/// background tasks: the phases, and around them the watch for an updated program, one for the
/// whole run with the program file as it was at its start, whichever phase the update meets.
pub(crate) async fn run_phases<F, Fut>(
    args: RunArgs,
    cancellation: CancellationToken,
    desktop: Option<DesktopSinks>,
    around: Surroundings,
    linked: F,
) -> Result<()>
where
    F: FnMut(RunArgs, Connection, CancellationToken, Option<DesktopSinks>, Router) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    // After a self-update the agent continues as the new program (RD-190-07).
    let replacement = around.program.clone().map(|program| {
        let watch = relaunch::watch(program.clone(), cancellation.clone(), around.check_every);
        (program, tokio::spawn(watch))
    });
    let ended = phases(args, cancellation.clone(), desktop, around, linked).await;
    // The phases have closed their listeners by now, so the new program gets the port.
    cancellation.cancel();
    let replaced = match replacement {
        Some((program, watch)) => watch.await.unwrap_or(false).then_some(program),
        None => None,
    };
    match (ended, replaced) {
        (Err(error), _) => Err(error),
        (Ok(()), Some(program)) => Err(anyhow::Error::new(relaunch::Replaced { program })),
        (Ok(()), None) => Ok(()),
    }
}

/// The phases one after the other: unlinked, linked, linked again after every new link.
async fn phases<F, Fut>(
    mut args: RunArgs,
    cancellation: CancellationToken,
    mut desktop: Option<DesktopSinks>,
    around: Surroundings,
    mut linked: F,
) -> Result<()>
where
    F: FnMut(RunArgs, Connection, CancellationToken, Option<DesktopSinks>, Router) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let secret = Arc::new(LinkSecret::generate());
    let port = args.cnl_listen.first().map_or(9666, SocketAddr::port);
    let page = link_page_url(port, &secret);
    let _key = LinkKey::write(&around.directory, &page);
    let (announce, mut links) = mpsc::unbounded_channel();
    let state = LinkState {
        secret,
        port,
        linked: announce,
        directory: around.directory.clone(),
        store: around.store.clone(),
    };
    loop {
        let next = match (around.connect)(&args)? {
            None => {
                publish(desktop.as_ref(), LinkStatus::Unlinked);
                let notice = desktop.as_ref().map(|sinks| &sinks.notice);
                let listen = Listen {
                    args: &args,
                    notice,
                    page: &page,
                };
                unlinked(listen, &state, &around, &cancellation, &mut links).await?
            }
            Some(connection) => {
                let insecure =
                    config::ensure_transport_is_safe(&connection.service, false).is_err();
                publish(
                    desktop.as_ref(),
                    LinkStatus::Linked {
                        service: connection.service.clone(),
                        insecure,
                    },
                );
                let phase = cancellation.child_token();
                let (phase_desktop, relay_to) = match desktop.as_ref() {
                    Some(sinks) => {
                        let (controls, inbox) = controls::channels();
                        (Some(sinks.with_inbox(inbox)), Some(controls))
                    }
                    None => (None, None),
                };
                let running = linked(
                    args.clone(),
                    connection,
                    phase.clone(),
                    phase_desktop,
                    link_page::router(state.clone()),
                );
                let inbox = desktop.as_mut().map(|sinks| &mut sinks.inbox);
                linked_phase(
                    running,
                    relay(inbox, relay_to, phase.clone()),
                    &phase,
                    &mut links,
                )
                .await?
            }
        };
        match next {
            Next::Stop => return Ok(()),
            // The page stored the new link: `capture.json` and the keyring name it now, not an
            // address or a token the agent was started with.
            Next::Relink => {
                args.connection.service = None;
                args.connection.token = None;
            }
        }
    }
}

/// Where the unlinked phase listens, whom it tells about an address it did not get, and the
/// page it may open.
struct Listen<'a> {
    args: &'a RunArgs,
    notice: Option<&'a NoticeSink>,
    page: &'a Url,
}

/// The unlinked phase: the link page alone on the Click'n'Load addresses, until a link is made
/// or the run ends.
async fn unlinked(
    listen: Listen<'_>,
    state: &LinkState,
    around: &Surroundings,
    cancellation: &CancellationToken,
    links: &mut mpsc::UnboundedReceiver<Linked>,
) -> Result<Next> {
    tracing::info!("not linked yet; the link page waits at the Click'n'Load port");
    let phase = cancellation.child_token();
    let listeners = agent_run::bind_listeners(
        listen.args.cnl_listen.clone(),
        listen.notice,
        &mut JoinSet::new(),
        &phase,
    )
    .await?;
    // Only now: a page opened before the port is bound would show a browser error.
    open_once(around, listen.args.no_link_page, listen.page);
    let router = link_page::router(state.clone());
    let mut servers = JoinSet::new();
    for (address, listener) in listeners {
        servers.spawn(cnl::serve(address, listener, router.clone(), phase.clone()));
    }
    let next = tokio::select! {
        Some(result) = servers.join_next() => {
            // A listener that ends before the run does has failed; the others go with it.
            phase.cancel();
            wind_down(&mut JoinSet::new(), &mut servers).await;
            result??;
            Next::Stop
        }
        Some(linked) = links.recv() => {
            tracing::info!(service = %linked.service, "linked; Click'n'Load starts");
            Next::Relink
        }
        () = cancellation.cancelled() => Next::Stop,
    };
    phase.cancel();
    wind_down(&mut JoinSet::new(), &mut servers).await;
    Ok(next)
}

/// One linked phase and the relay of the tray's requests to it, until the phase ends by itself or
/// a new link ends it.
async fn linked_phase(
    running: impl Future<Output = Result<()>>,
    relaying: impl Future<Output = ()>,
    phase: &CancellationToken,
    links: &mut mpsc::UnboundedReceiver<Linked>,
) -> Result<Next> {
    let watching = async {
        tokio::pin!(running);
        let next = tokio::select! {
            result = &mut running => result.map(|()| Next::Stop),
            Some(linked) = links.recv() => {
                tracing::info!(service = %linked.service, "linked anew; Click'n'Load restarts");
                phase.cancel();
                (&mut running).await.map(|()| Next::Relink)
            }
        };
        // Whatever ended the phase, the relay ends with it.
        phase.cancel();
        next
    };
    let (next, ()) = tokio::join!(watching, relaying);
    next
}

/// Hands the tray's requests to the tasks of one phase until it ends. Without a tray there is
/// nothing to relay: the phase makes its own channels.
async fn relay(
    inbox: Option<&mut controls::Inbox>,
    to: Option<controls::Controls>,
    until: CancellationToken,
) {
    let (Some(inbox), Some(to)) = (inbox, to) else {
        return;
    };
    loop {
        tokio::select! {
            () = until.cancelled() => return,
            Some(request) = inbox.queue.recv() => { let _ = to.queue.send(request); }
            Some(request) = inbox.settings.recv() => { let _ = to.settings.send(request); }
            Some(()) = inbox.hand_over.recv() => { let _ = to.hand_over.send(()); }
            Some(request) = inbox.self_update.recv() => { let _ = to.self_update.send(request); }
            Some(request) = inbox.server_update.recv() => { let _ = to.server_update.send(request); }
        }
    }
}

/// Tells the tray, if there is one.
fn publish(desktop: Option<&DesktopSinks>, status: LinkStatus) {
    if let Some(sinks) = desktop {
        sinks.link.send_replace(status);
    }
}

/// Opens the page by itself the first time an agent of this installation runs unlinked. The
/// marker is written only once the browser was started: a page that never opened is tried again
/// at the next start.
fn open_once(around: &Surroundings, suppressed: bool, page: &Url) {
    let marker = around.directory.join(LINK_PAGE_SHOWN_FILE);
    if suppressed || marker.exists() {
        return;
    }
    if let Err(error) = (around.open)(page) {
        tracing::warn!(%error, "could not open the link page; `rdownloader-capture link` prints it");
        return;
    }
    let written =
        std::fs::create_dir_all(&around.directory).and_then(|()| std::fs::write(&marker, b""));
    if let Err(error) = written {
        tracing::warn!(%error, "the link page may open by itself again at the next start");
    }
}

#[cfg(test)]
#[path = "link_phase_tests.rs"]
mod tests;
