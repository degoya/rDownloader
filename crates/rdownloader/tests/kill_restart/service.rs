//! One installation of the service under test: its data directory, port and API token.

use std::{
    future::Future,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

use sha2::{Digest, Sha256};
use tokio::net::TcpListener;

const TOKEN: &str = "axis-b-token";

pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn binary() -> PathBuf {
    std::env::var_os("RD_AXIS_B_BINARY").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_rdownloader")),
        PathBuf::from,
    )
}

/// Asks `probe` every 50 ms until it answers, and fails with `what` when `within` runs out.
pub async fn wait_for<T, F, Fut>(what: &str, within: Duration, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if let Some(value) = probe().await {
            return value;
        }
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Every file below `directory`.
fn files_below(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

pub fn file_named(directory: &Path, name: &str) -> Option<PathBuf> {
    files_below(directory)
        .into_iter()
        .find(|path| path.file_name().and_then(|file| file.to_str()) == Some(name))
}

/// Part files and extraction staging: what an interruption must not leave behind.
pub fn leftovers(directory: &Path) -> Vec<PathBuf> {
    files_below(directory)
        .into_iter()
        .filter(|path| {
            path.extension().and_then(|extension| extension.to_str()) == Some("part")
                || path.components().any(|component| {
                    component
                        .as_os_str()
                        .to_str()
                        .is_some_and(|name| name.starts_with(".rd-x"))
                })
        })
        .collect()
}

/// A running service, killed with `SIGKILL` and reaped when dropped — a failed assertion too
/// leaves no process behind.
pub struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One data directory, one port and one API token, for as many starts as a case needs.
pub struct Service {
    directory: PathBuf,
    port: u16,
    client: reqwest::Client,
}

impl Service {
    /// A fresh installation with an API token and `settings` as its service settings.
    pub async fn prepare(directory: &Path, settings: serde_json::Value) -> Self {
        let data = directory.join("data");
        for folder in [
            data.join("scripts"),
            directory.join("downloads"),
            directory.join("no-bundled-plugins"),
        ] {
            std::fs::create_dir_all(folder).expect("folder");
        }
        let database = rd_db::Database::open(data.join("rdownloader.sqlite3"))
            .await
            .expect("database");
        database
            .create_capture_token(
                rd_core::CaptureTokenId::new(),
                "axis-b".to_owned(),
                digest(TOKEN.as_bytes()),
                vec![rd_core::API_SCOPE.to_owned()],
            )
            .await
            .expect("token");
        database
            .set_setting("service.settings".to_owned(), settings)
            .await
            .expect("settings");
        drop(database);
        let port = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("port")
            .local_addr()
            .expect("port")
            .port();
        Self {
            directory: directory.to_path_buf(),
            port,
            client: reqwest::Client::new(),
        }
    }

    pub fn database_path(&self) -> PathBuf {
        self.directory.join("data").join("rdownloader.sqlite3")
    }

    pub fn downloads(&self) -> PathBuf {
        self.directory.join("downloads")
    }

    pub fn scripts(&self) -> PathBuf {
        self.directory.join("data").join("scripts")
    }

    /// Starts `rdownloader serve`; `run` numbers its log file.
    pub fn start(&self, run: usize) -> Running {
        let log = std::fs::File::create(self.directory.join(format!("service-{run}.log")))
            .expect("log file");
        let errors = log.try_clone().expect("log file");
        let child = Command::new(binary())
            .arg("serve")
            .arg("--database")
            .arg(self.database_path())
            .arg("--downloads")
            .arg(self.downloads())
            .arg("--plugin-root")
            .arg(self.directory.join("data").join("plugins"))
            .arg("--bundled-plugins")
            .arg(self.directory.join("no-bundled-plugins"))
            .arg("--no-default-plugin-key")
            .arg("--listen")
            .arg(format!("127.0.0.1:{}", self.port))
            .current_dir(&self.directory)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(errors)
            .spawn()
            .expect("start rdownloader serve");
        Running(child)
    }

    /// The end of every log so far, for a failure message.
    fn logs(&self) -> String {
        let mut text = String::new();
        for run in 0..2 {
            let path = self.directory.join(format!("service-{run}.log"));
            if let Ok(log) = std::fs::read_to_string(path) {
                text.push_str(&format!("--- service-{run}.log\n{log}\n"));
            }
        }
        let mut cut = text.len().saturating_sub(12_000);
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        text[cut..].to_owned()
    }

    pub async fn ready(&self, service: &mut Running) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(status) = service.0.try_wait().expect("service status") {
                panic!("rdownloader serve exited with {status}\n{}", self.logs());
            }
            if self.get("/api/v1/downloads").await.is_some() {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the service did not come up\n{}",
                self.logs()
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    async fn get(&self, path: &str) -> Option<serde_json::Value> {
        let response = self
            .client
            .get(self.url(path))
            .bearer_auth(TOKEN)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json().await.ok()
    }

    pub async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: serde_json::Value,
    ) -> serde_json::Value {
        let response = self
            .client
            .request(method, self.url(path))
            .bearer_auth(TOKEN)
            .json(&body)
            .send()
            .await
            .expect("request");
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        assert!(status.is_success(), "{path}: {status} {text}");
        serde_json::from_str(&text).expect("JSON")
    }

    /// Row `id` of `list` (`downloads` or `packages`), or `null` when it is not there.
    pub async fn row(&self, list: &str, id: &str) -> serde_json::Value {
        self.get(&format!("/api/v1/{list}"))
            .await
            .and_then(|rows| rows.as_array()?.iter().find(|row| row["id"] == id).cloned())
            .unwrap_or_default()
    }
}
