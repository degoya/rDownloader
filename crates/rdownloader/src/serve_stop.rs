//! What ends `rdownloader serve`: the signals, the deadline that bounds a stop, and the local
//! control file whose removal tells `rdownloader stop --wait` and the updater the stop is done.

use std::{
    future::Future,
    sync::{Arc, Mutex, Weak},
};

use rd_api::local_control::ControlFileGuard;
use tokio_util::sync::CancellationToken;

/// How long a stop may take, from the request to the end of the process (RD-180-02).
///
/// The listener drains for at most `rd_api::CONNECTION_DRAIN`, the scheduler gives its transfers
/// ten seconds to checkpoint, the rest stops in moments; this bounds all of it together and what
/// nothing else bounds -- a blocking task the runtime waits for on its way out, a stop that hangs.
/// Below the updater's wait for the stop (120 s), so a slow stop no longer fails an update. What
/// a forced end leaves is what a crash leaves, and every persistence path survives a crash
/// (`crates/rd-core/recovery-matrix.md`).
const STOP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// The exit code of a stop a second signal cut short, the shell convention for an interrupt.
const INTERRUPTED_EXIT_CODE: i32 = 130;

/// The local control file, removed as the last step of a stop: its removal is what
/// `rdownloader stop --wait` and the updater read as the end of the process. Empty until the
/// start writes it.
pub(crate) type ControlFile = Arc<Mutex<Option<ControlFileGuard>>>;

pub(crate) fn remove_control_file(control_file: &ControlFile) {
    if let Ok(mut file) = control_file.lock() {
        file.take();
    }
}

/// Cancelled by a signal, and by `POST /api/v1/system/shutdown` (RD-180-02): the one way to
/// stop the service gracefully on Windows, where a console-less process gets no Ctrl-C.
///
/// A signal while a stop is already under way ends the process at once, the control file
/// removed first: the runtime keeps the signal handler installed, so without this a second
/// Ctrl-C would do nothing until [`STOP_DEADLINE`] ran out. Held weakly, so a start that fails
/// drops the control file as before.
pub(crate) fn shutdown_on_signal(
    control_file: Weak<Mutex<Option<ControlFileGuard>>>,
) -> CancellationToken {
    let shutdown = CancellationToken::new();
    let requests = shutdown.clone();
    tokio::spawn(async move {
        wait_for_second_request(requests, wait_for_shutdown_signal).await;
        tracing::warn!("a second stop signal: the process ends without the rest of the stop");
        end_now(&control_file, INTERRUPTED_EXIT_CODE);
    });
    shutdown
}

/// Cancels `shutdown` on the first request unless something else already did, and returns on
/// the request after that.
async fn wait_for_second_request<Request, Requested>(
    shutdown: CancellationToken,
    mut request: Request,
) where
    Request: FnMut() -> Requested,
    Requested: Future<Output = ()>,
{
    tokio::select! {
        () = request() => shutdown.cancel(),
        () = shutdown.cancelled() => {}
    }
    request().await;
}

/// Ends the process now, the control file removed first so whoever asked for the stop sees
/// it done.
fn end_now(control_file: &Weak<Mutex<Option<ControlFileGuard>>>, code: i32) -> ! {
    if let Some(control_file) = control_file.upgrade() {
        remove_control_file(&control_file);
    }
    std::process::exit(code);
}

/// Ends the process [`STOP_DEADLINE`] after `shutdown` was cancelled, if it is still running.
///
/// On a thread of its own, so a runtime whose workers are all stuck still ends. The control file
/// goes first, as at the end of an ordinary stop, so whoever asked for the stop sees it done;
/// held weakly, so a start that fails drops it as before.
pub(crate) fn end_after_stop_deadline(
    shutdown: CancellationToken,
    control_file: Weak<Mutex<Option<ControlFileGuard>>>,
) {
    tokio::spawn(async move {
        shutdown.cancelled().await;
        std::thread::spawn(move || {
            std::thread::sleep(STOP_DEADLINE);
            tracing::error!(
                seconds = STOP_DEADLINE.as_secs(),
                "the stop did not finish in time; the process ends without the rest of it"
            );
            end_now(&control_file, 1);
        });
    });
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            result = tokio::signal::ctrl_c() => { let _ = result; }
            _ = terminate.recv() => {}
        }
    }
    // Ctrl-C, the console window closed, and the system shutting down (RD-180-02). Windows ends
    // the process a few seconds after a close or shutdown event whatever it does, so this is
    // the best effort; `rdownloader stop` is the stop that waits for the queue.
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_close, ctrl_shutdown};

        let (Ok(mut close), Ok(mut system)) = (ctrl_close(), ctrl_shutdown()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            result = tokio::signal::ctrl_c() => { let _ = result; }
            _ = close.recv() => {}
            _ = system.recv() => {}
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::wait_for_second_request;

    /// A request source driven by the test: each `send` is one signal.
    fn requests() -> (
        mpsc::UnboundedSender<()>,
        impl FnMut() -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel::<()>();
        let receiver = std::sync::Arc::new(tokio::sync::Mutex::new(receiver));
        let next = move || {
            let receiver = receiver.clone();
            Box::pin(async move {
                let _ = receiver.lock().await.recv().await;
            }) as std::pin::Pin<Box<dyn Future<Output = ()> + Send>>
        };
        (sender, next)
    }

    #[tokio::test]
    async fn the_first_signal_stops_gracefully_and_the_second_ends_the_wait() {
        let shutdown = CancellationToken::new();
        let (signal, next) = requests();
        let waiting = tokio::spawn(wait_for_second_request(shutdown.clone(), next));

        signal.send(()).expect("first signal");
        tokio::time::timeout(Duration::from_secs(5), shutdown.cancelled())
            .await
            .expect("the first signal cancels the stop token");
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished(), "one signal only starts the stop");

        signal.send(()).expect("second signal");
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the second signal ends the wait")
            .expect("task");
    }

    #[tokio::test]
    async fn after_a_stop_asked_for_otherwise_one_signal_ends_the_wait() {
        let shutdown = CancellationToken::new();
        let (signal, next) = requests();
        let waiting = tokio::spawn(wait_for_second_request(shutdown.clone(), next));

        // The stop route cancels the token itself.
        shutdown.cancel();
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());

        signal.send(()).expect("signal");
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("a signal during a stop ends the wait")
            .expect("task");
    }
}
