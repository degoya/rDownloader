//! `rdownloader stop`: the graceful stop a launcher or the updater uses (RD-180-02).
//!
//! Reads the local control file the running service wrote into its data directory
//! (`rd_api::local_control`), asks `POST /api/v1/system/shutdown` with the token in it, and —
//! unless `--wait 0` — waits until the service removed the file, which it does as the very last
//! step after the queue was checkpointed. The exit codes are the remote commands' own: `3` when
//! the service cannot be reached, `4` when it refused the token, `1` when it did not end in
//! time, so a launcher can fall back to ending the process by force.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use clap::Args;

use crate::remote::{Client, CommandError, Failure};

/// How often the waiting looks at the file.
const POLL: Duration = Duration::from_millis(200);

#[derive(Args)]
pub struct StopArgs {
    /// The SQLite database file of the service to stop; its folder holds the control file.
    #[arg(
        long,
        env = "RDOWNLOADER_DATABASE",
        default_value = "data/rdownloader.sqlite3"
    )]
    database: PathBuf,
    /// Seconds to wait for the service to end; `0` returns once it accepted the request.
    #[arg(long, default_value_t = 60)]
    wait: u64,
}

pub async fn run(args: StopArgs) -> Result<()> {
    let data_directory = args
        .database
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    if stop(&data_directory, Duration::from_secs(args.wait)).await? {
        println!("rDownloader stopped gracefully.");
    } else {
        println!("rDownloader is not running.");
    }
    Ok(())
}

/// The service accepted the stop and is still running when the wait ends: the one failure after
/// which the updater may end it by force (RD-180-02). Exit code `1`, as every other failure that
/// is not one of the remote commands' own.
#[derive(Debug)]
pub(crate) struct NotEnded {
    /// The process the control file names.
    pub pid: u32,
    pub seconds: u64,
}

impl std::fmt::Display for NotEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rDownloader (process {}) accepted the stop but has not ended within {} seconds",
            self.pid, self.seconds
        )
    }
}

impl std::error::Error for NotEnded {}

/// Stops the service of `data_directory`; `false` when none is running there. The updater stops
/// the service the same way (RD-180-02).
pub(crate) async fn stop(data_directory: &Path, wait: Duration) -> Result<bool> {
    let Some(control) = rd_api::local_control::read(data_directory)? else {
        return Ok(false);
    };
    let client = Client::local(
        &format!("http://{}", control.address),
        Some(control.token.clone()),
        30,
    )?;
    let asked: Result<serde_json::Value> = client
        .post("/api/v1/system/shutdown", &serde_json::json!({}))
        .await;
    if let Err(error) = asked {
        // A file left by a process that was killed: nothing listens there any more.
        if error
            .downcast_ref::<CommandError>()
            .is_some_and(|command| command.failure == Failure::Unreachable)
            && rd_api::local_control::read(data_directory)?.is_some_and(|file| file == control)
        {
            return Err(CommandError::new(
                Failure::Unreachable,
                format!(
                    "no service answers at {} although {} names it; if none runs, the file \
                     is left over from a process that was ended by force and the next start \
                     replaces it",
                    control.address,
                    data_directory.join(rd_api::local_control::FILE).display()
                ),
            )
            .into());
        }
        return Err(error);
    }
    if wait.is_zero() {
        return Ok(true);
    }
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        // Gone, or replaced by a later start: either way the process asked has ended.
        let current = rd_api::local_control::read(data_directory).ok().flatten();
        if current.is_none_or(|file| file.token != control.token) {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(NotEnded {
                pid: control.pid,
                seconds: wait.as_secs(),
            }
            .into());
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use axum::{
        Router,
        http::{HeaderMap, StatusCode},
        routing::post,
    };

    use super::*;

    /// A stand-in for the service: accepts the stop with the right token, and like the real
    /// one removes its control file once it has ended.
    async fn service(
        data: PathBuf,
        remove_after: Option<Duration>,
    ) -> (SocketAddr, Arc<Mutex<u32>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let calls = Arc::new(Mutex::new(0_u32));
        let counted = calls.clone();
        let router = Router::new().route(
            "/api/v1/system/shutdown",
            post(move |headers: HeaderMap| {
                let data = data.clone();
                let counted = counted.clone();
                async move {
                    let file = rd_api::local_control::read(&data)
                        .expect("read")
                        .expect("file");
                    let expected = format!("Bearer {}", file.token);
                    if headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        != Some(expected.as_str())
                    {
                        return (StatusCode::UNAUTHORIZED, "{}".to_owned());
                    }
                    *counted.lock().expect("calls") += 1;
                    if let Some(delay) = remove_after {
                        tokio::spawn(async move {
                            tokio::time::sleep(delay).await;
                            std::fs::remove_file(data.join(rd_api::local_control::FILE))
                                .expect("remove");
                        });
                    }
                    (StatusCode::ACCEPTED, r#"{"stopping":true}"#.to_owned())
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve");
        });
        (address, calls)
    }

    fn control_file(data: &Path, address: SocketAddr, token: &str) {
        let file = rd_api::local_control::ControlFile {
            address: address.to_string(),
            token: token.to_owned(),
            pid: 4242,
        };
        std::fs::write(
            data.join(rd_api::local_control::FILE),
            serde_json::to_vec(&file).expect("json"),
        )
        .expect("write");
    }

    #[tokio::test]
    async fn without_a_control_file_nothing_is_running() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(
            !stop(directory.path(), Duration::from_secs(1))
                .await
                .expect("stop")
        );
    }

    #[tokio::test]
    async fn the_stop_is_asked_with_the_token_and_waits_until_the_service_ended() {
        let directory = tempfile::tempdir().expect("tempdir");
        let data = directory.path().to_path_buf();
        let (address, calls) = service(data.clone(), Some(Duration::from_millis(500))).await;
        control_file(&data, address, "the-token");
        assert!(stop(&data, Duration::from_secs(10)).await.expect("stop"));
        assert_eq!(*calls.lock().expect("calls"), 1);
        assert!(rd_api::local_control::read(&data).expect("read").is_none());
    }

    #[tokio::test]
    async fn a_service_that_does_not_end_in_time_is_a_failure_the_launcher_can_act_on() {
        let directory = tempfile::tempdir().expect("tempdir");
        let data = directory.path().to_path_buf();
        let (address, _) = service(data.clone(), None).await;
        control_file(&data, address, "the-token");
        let error = stop(&data, Duration::from_millis(600))
            .await
            .expect_err("timed out");
        let not_ended = error.downcast_ref::<NotEnded>().expect("not ended");
        assert_eq!(not_ended.pid, 4242);
        assert!(error.downcast_ref::<CommandError>().is_none());
    }

    #[tokio::test]
    async fn a_file_nobody_answers_for_is_reported_as_unreachable() {
        let directory = tempfile::tempdir().expect("tempdir");
        // A port that was just free: nothing listens there.
        let address = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind")
            .local_addr()
            .expect("address");
        control_file(directory.path(), address, "the-token");
        let error = stop(directory.path(), Duration::from_secs(1))
            .await
            .expect_err("unreachable");
        let failure = error.downcast_ref::<CommandError>().expect("command error");
        assert_eq!(failure.failure, Failure::Unreachable);
    }
}
