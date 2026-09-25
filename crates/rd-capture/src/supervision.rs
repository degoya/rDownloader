//! Keeping the agent's background work audible: what the agent has to say about itself, how a
//! task's end becomes one of those notices, and how the Click'n'Load addresses are bound and
//! everything is wound down again (RD-109-07).

use std::{net::SocketAddr, time::Duration};

use anyhow::Result;
use tokio_util::sync::CancellationToken;

/// Something about the agent itself that the person should see, next to what the service says.
///
/// Both variants describe the agent doing less than it claims, and nothing puts either right on
/// its own: no background task is restarted, and no address is bound again later (RD-109-07).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentNotice {
    /// A background task ended and will not come back.
    TaskEnded { task: &'static str, reason: String },
    /// Click'n'Load got some of its configured addresses but not all of them.
    PartialBind {
        bound: Vec<SocketAddr>,
        unbound: Vec<SocketAddr>,
    },
}

/// How a background task ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TaskEnd {
    /// It returned `Ok`, which for these four only happens on cancellation.
    Returned,
    /// It returned an error. Carries the error chain as it was logged.
    Failed(String),
    /// It panicked.
    Panicked,
}

/// Decides whether the end of a background task is something to report.
///
/// The ordinary shutdown -- cancelled, then returned -- is not. Everything else is, because the
/// four tasks are the agent's whole job: `watch_clipboard` returning `Err` once meant the
/// clipboard was never read again, with no log line, no restart and no change in what the tray
/// showed. For the person it looked like rDownloader having stopped noticing copied links.
///
/// A pure function next to the state it produces, so it is tested on every host.
pub(crate) fn task_end_notice(
    task: &'static str,
    end: &TaskEnd,
    cancelled: bool,
) -> Option<AgentNotice> {
    let reason = match end {
        TaskEnd::Returned if cancelled => return None,
        TaskEnd::Returned => "it returned on its own".to_owned(),
        TaskEnd::Failed(reason) => reason.clone(),
        TaskEnd::Panicked => "it panicked".to_owned(),
    };
    Some(AgentNotice::TaskEnded { task, reason })
}

/// The line the tray shows for one notice.
///
/// Untranslated, like the rest of the agent's menu: it carries no message catalogue (RD-092-05).
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn notice_label(notice: &AgentNotice) -> String {
    match notice {
        AgentNotice::TaskEnded { task, .. } => format!("{task} stopped"),
        AgentNotice::PartialBind { bound, .. } => {
            format!("Click'n'Load only on {}", join_addresses(bound))
        }
    }
}

/// Addresses in one readable list, for a log line and for the tray.
pub(crate) fn join_addresses(addresses: &[SocketAddr]) -> String {
    addresses
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reports something about the agent itself; `None` on a headless run.
pub(crate) type NoticeSink = std::sync::Arc<dyn Fn(AgentNotice) + Send + Sync>;

/// How long the error path waits for the running tasks before it gives up on them.
///
/// Short, because the process is ending either way; long enough for a request that is in flight
/// to come back. The tray's `finish` calls `std::process::exit`, which runs no destructor and
/// drains nothing -- so a request in flight used to be cut off
/// mid-flight, the token spent and the person asked to solve the same challenge again
/// (RD-109-07).
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// Runs one background task and makes its end audible.
///
/// The `JoinHandle` of all four used to be thrown away, so an `Err` was never read and a panic
/// was just as quiet. The inner `tokio::spawn` is what turns a panic into a value here rather
/// than into silence.
pub(crate) async fn supervised(
    task: &'static str,
    cancellation: CancellationToken,
    notice: Option<NoticeSink>,
    work: impl std::future::Future<Output = Result<()>> + Send + 'static,
) {
    let end = match tokio::spawn(work).await {
        Ok(Ok(())) => TaskEnd::Returned,
        Ok(Err(error)) => TaskEnd::Failed(format!("{error:#}")),
        Err(join) if join.is_panic() => TaskEnd::Panicked,
        Err(join) => TaskEnd::Failed(join.to_string()),
    };
    let Some(ended) = task_end_notice(task, &end, cancellation.is_cancelled()) else {
        tracing::debug!(task, "background task ended with the shutdown");
        return;
    };
    if let AgentNotice::TaskEnded { reason, .. } = &ended {
        tracing::error!(task, %reason, "background task ended and will not restart");
    }
    if let Some(sink) = notice {
        sink(ended);
    }
}

/// What binding the configured Click'n'Load addresses produced.
pub(crate) struct Bindings {
    pub(crate) listeners: Vec<(SocketAddr, tokio::net::TcpListener)>,
    pub(crate) unbound: Vec<SocketAddr>,
}

/// Binds every configured address and keeps both halves of the answer.
///
/// A partial bind is neither a failure nor a success. The agent keeps running, because half the
/// reachability beats none and an environment without IPv6 would otherwise not start at all —
/// but the address a browser resolves `localhost` to may be exactly the one that was lost, so it
/// is reported rather than left to a `warn` nobody reads. The exit code stays reserved for the
/// case where not one address can be had (RD-109-07).
pub(crate) async fn bind_click_n_load(addresses: Vec<SocketAddr>) -> Bindings {
    let mut bindings = Bindings {
        listeners: Vec::new(),
        unbound: Vec::new(),
    };
    for address in addresses {
        match tokio::net::TcpListener::bind(address).await {
            Ok(listener) => bindings.listeners.push((address, listener)),
            Err(error) => {
                tracing::debug!(%address, %error, "Click'n'Load address unavailable");
                bindings.unbound.push(address);
            }
        }
    }
    bindings
}

/// Gives the running tasks a bounded moment to stop before the caller leaves.
pub(crate) async fn wind_down(
    background: &mut tokio::task::JoinSet<()>,
    servers: &mut tokio::task::JoinSet<Result<()>>,
) {
    let drained = tokio::time::timeout(SHUTDOWN_GRACE, async {
        while background.join_next().await.is_some() {}
        while servers.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        tracing::warn!(
            "the agent's tasks did not stop within the shutdown grace period; ending anyway"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentNotice, TaskEnd, bind_click_n_load, notice_label, supervised, task_end_notice,
    };

    /// The defect: `watch_clipboard` returning `Err` once ended clipboard capture for good,
    /// and the discarded `JoinHandle` meant nobody ever read that `Err` (RD-109-07).
    #[tokio::test]
    async fn a_background_task_that_dies_says_so() {
        let seen: std::sync::Arc<std::sync::Mutex<Vec<AgentNotice>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = {
            let seen = seen.clone();
            std::sync::Arc::new(move |notice: AgentNotice| {
                seen.lock()
                    .expect("the recorder is not poisoned")
                    .push(notice);
            })
        };
        let cancellation = tokio_util::sync::CancellationToken::new();

        supervised(
            "clipboard monitoring",
            cancellation.clone(),
            Some(recorder.clone()),
            async { Err(anyhow::anyhow!("joining the service URL failed")) },
        )
        .await;
        // A panic is just as quiet without this, and just as final.
        supervised(
            "the transfer poll",
            cancellation.clone(),
            Some(recorder.clone()),
            async { panic!("the watcher fell over") },
        )
        .await;

        let reported = seen.lock().expect("the recorder is not poisoned").clone();
        assert_eq!(reported.len(), 2, "{reported:?}");
        assert!(
            matches!(
                &reported[0],
                AgentNotice::TaskEnded { task, reason }
                    if *task == "clipboard monitoring" && reason.contains("joining the service URL")
            ),
            "the name and the reason both have to arrive: {reported:?}"
        );
        assert!(
            matches!(
                &reported[1],
                AgentNotice::TaskEnded { task, reason }
                    if *task == "the transfer poll" && reason == "it panicked"
            ),
            "{reported:?}"
        );
        assert_eq!(
            notice_label(&reported[0]),
            "clipboard monitoring stopped",
            "the tray stops reading as unqualified health"
        );

        // The shutdown itself is not a notice: every one of these returns `Ok` when cancelled.
        cancellation.cancel();
        assert_eq!(
            task_end_notice("clipboard monitoring", &TaskEnd::Returned, true),
            None
        );
        // Returning without a shutdown is still a notice -- the task is simply gone.
        assert!(task_end_notice("clipboard monitoring", &TaskEnd::Returned, false).is_some());
    }

    /// Another Click'n'Load listener holding v4 but not v6 used to leave the agent bound to v6
    /// only, reporting "healthy", while browsers that resolve `localhost` to v4 kept handing
    /// their links to the other program (RD-109-07).
    #[tokio::test]
    async fn a_partial_bind_keeps_both_halves_of_the_answer() {
        let taken = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("hold an address the agent will not get");
        let occupied = taken.local_addr().expect("the held address");
        let free = {
            let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("find a free address");
            probe.local_addr().expect("the free address")
        };

        let bindings = bind_click_n_load(vec![occupied, free]).await;
        assert_eq!(bindings.unbound, vec![occupied]);
        assert_eq!(
            bindings
                .listeners
                .iter()
                .map(|(address, _)| *address)
                .collect::<Vec<_>>(),
            vec![free],
            "a partial bind reports what it got as well as what it did not"
        );

        assert_eq!(
            notice_label(&AgentNotice::PartialBind {
                bound: vec![free],
                unbound: vec![occupied],
            }),
            format!("Click'n'Load only on {free}")
        );

        // Not one address is the other case, and that one keeps its exit code.
        let none = bind_click_n_load(vec![occupied]).await;
        assert!(none.listeners.is_empty());
        assert_eq!(none.unbound, vec![occupied]);
    }
}
