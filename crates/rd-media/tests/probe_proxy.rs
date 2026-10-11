//! RD-1240-22 — the LinkGrabber's media probe goes through the global proxy profile: the
//! profile reaches yt-dlp as its own option or, with credentials, through its environment
//! only, and a profile that cannot be used stops the probe before yt-dlp is asked anything.

#![cfg(unix)]

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use rd_core::{MediaSettings, ProxyKind};
use rd_db::Database;
use rd_media::{MediaProbe, YtDlpProbe};
use rd_scheduler::ToolNetworkSource;
use secrecy::SecretString;
use tokio::sync::RwLock;

const PASSWORD: &str = "pr0xy-secret";

struct Harness {
    _temp: tempfile::TempDir,
    database: Database,
    secrets: rd_secrets::SecretStore,
    defaults: rd_http::SharedNetworkDefaults,
    probe: YtDlpProbe,
    /// What the wrapper saw: one `ARGS` and one `HTTPS_PROXY` line per invocation.
    log: PathBuf,
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"))
        .join(format!("tests/fixtures/{name}.sh"))
}

/// A yt-dlp that writes its argument list and proxy variable to `log`, then answers as the
/// shared fixture does.
fn recording_ytdlp(directory: &Path, log: &Path) -> PathBuf {
    let script = directory.join("yt-dlp");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             {{ printf 'ARGS %s\\n' \"$*\"; printf 'HTTPS_PROXY=%s\\n' \"${{HTTPS_PROXY-}}\"; }} >> '{}'\n\
             exec '{}' \"$@\"\n",
            log.display(),
            fixture("fake-yt-dlp").display()
        ),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

async fn harness() -> Harness {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = temp.path().join("ytdlp.log");
    let ytdlp = recording_ytdlp(temp.path(), &log);
    let database = Database::open(temp.path().join("media.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(temp.path().join("secrets"))
        .await
        .expect("secrets");
    let defaults: rd_http::SharedNetworkDefaults =
        Arc::new(RwLock::new(rd_http::NetworkDefaults::default()));
    let settings = Arc::new(RwLock::new(MediaSettings {
        media_ytdlp_executable: Some(ytdlp.to_string_lossy().into_owned()),
        media_ffmpeg_executable: Some(fixture("fake-yt-dlp").to_string_lossy().into_owned()),
        media_default_variant: "720p".to_owned(),
        ..MediaSettings::default()
    }));
    let probe = YtDlpProbe::new(settings).with_tool_network(ToolNetworkSource::new(
        database.clone(),
        secrets.clone(),
        defaults.clone(),
    ));
    Harness {
        _temp: temp,
        database,
        secrets,
        defaults,
        probe,
        log,
    }
}

impl Harness {
    /// Makes a profile the global one; `secret_ref` is its password's vault reference.
    async fn global_profile(&self, secret_ref: Option<String>) {
        let profile = self
            .database
            .create_proxy_profile(rd_db::NewProxyProfile {
                name: "office".to_owned(),
                kind: ProxyKind::Http,
                endpoint: "http://proxy.example:3128".parse().expect("endpoint"),
                username: secret_ref.as_ref().map(|_| "alice".to_owned()),
                secret_ref,
            })
            .await
            .expect("proxy profile");
        self.defaults.write().await.global_proxy_profile_id = Some(profile.id);
    }

    async fn probe(&self) -> Result<Vec<rd_core::MediaCandidate>, rd_core::Failure> {
        self.probe
            .probe(&"https://www.youtube.com/watch?v=abc".parse().expect("url"))
            .await
    }

    /// The `-J` invocation's argument line and the proxy variable it was started with.
    fn metadata_call(&self) -> Option<(String, String)> {
        let log = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        lines.chunks(2).find_map(|call| match call {
            [args, proxy] if args.contains(" -J ") => Some((
                (*args).to_owned(),
                proxy.trim_start_matches("HTTPS_PROXY=").to_owned(),
            )),
            _ => None,
        })
    }
}

#[tokio::test]
async fn the_probe_goes_through_the_global_profile() {
    let harness = harness().await;
    harness.global_profile(None).await;

    harness.probe().await.expect("probe");
    let (args, proxy) = harness.metadata_call().expect("metadata call");
    assert!(
        args.contains("--proxy http://proxy.example:3128/"),
        "{args}"
    );
    assert_eq!(proxy, "http://proxy.example:3128/");
}

#[tokio::test]
async fn credentials_reach_the_probe_through_its_environment_only() {
    let harness = harness().await;
    let reference = harness
        .secrets
        .put(SecretString::from(PASSWORD.to_owned()))
        .await
        .expect("secret");
    harness.global_profile(Some(reference)).await;

    harness.probe().await.expect("probe");
    let (args, proxy) = harness.metadata_call().expect("metadata call");
    assert!(!args.contains(PASSWORD), "{args}");
    assert!(!args.contains("--proxy"), "{args}");
    assert_eq!(
        proxy,
        format!("http://alice:{PASSWORD}@proxy.example:3128/")
    );
}

#[tokio::test]
async fn without_a_global_profile_the_probe_has_no_proxy() {
    let harness = harness().await;

    harness.probe().await.expect("probe");
    let (args, proxy) = harness.metadata_call().expect("metadata call");
    assert!(!args.contains("--proxy"), "{args}");
    assert_eq!(proxy, "");
}

#[tokio::test]
async fn an_unusable_profile_stops_the_probe_before_yt_dlp_is_asked() {
    let harness = harness().await;
    // A password the vault does not hold.
    harness
        .global_profile(Some("vault://missing".to_owned()))
        .await;

    let failure = harness.probe().await.expect_err("no probe past the proxy");
    assert_eq!(failure.code.as_deref(), Some("proxy.check_unavailable"));
    assert!(!failure.category.is_retryable());
    assert!(
        harness.metadata_call().is_none(),
        "yt-dlp was asked directly"
    );
}
