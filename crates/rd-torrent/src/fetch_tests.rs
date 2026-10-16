//! A `.torrent` address queued without a check is fetched on the row's own network
//! (RD-1320-05): through the global proxy, with the cookies of the profile for its host.
//!
//! Before, librqbit fetched it with a client of its own. The fixture is the proxy: the
//! tracker's name does not resolve, so a request that reaches the fixture went through it.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use librqbit::AddTorrent;
use rd_core::{AuthMethod, AuthOrigin, AuthScope, DownloadFile, DownloadId};
use rd_scheduler::{RuntimeSettings, SchedulerConfig, SchedulerHandle};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use crate::{TorrentService, parse_torrent};

const SOURCE: &str = "http://tracker.test/get/1.torrent";
const COOKIE: &str = "SID=tracker-cookie-7c1e";

/// A directory below the system temp dir, removed again when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rd-torrent-fetch-{}", DownloadId::new()));
        std::fs::create_dir_all(&path).expect("scratch dir");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn bstr(value: &str) -> Vec<u8> {
    let mut out = format!("{}:", value.len()).into_bytes();
    out.extend_from_slice(value.as_bytes());
    out
}

/// A one-byte single-file torrent.
fn torrent() -> Vec<u8> {
    let mut bytes = vec![b'd'];
    bytes.extend(bstr("announce"));
    bytes.extend(bstr("http://tracker.test/announce"));
    bytes.extend(bstr("info"));
    bytes.push(b'd');
    bytes.extend(bstr("length"));
    bytes.extend_from_slice(b"i1e");
    bytes.extend(bstr("name"));
    bytes.extend(bstr("fetched.bin"));
    bytes.extend(bstr("piece length"));
    bytes.extend_from_slice(b"i16384e");
    bytes.extend(bstr("pieces"));
    bytes.extend_from_slice(b"20:");
    bytes.extend_from_slice(&[7_u8; 20]);
    bytes.extend_from_slice(b"ee");
    bytes
}

/// The request line and `Cookie` header of every request the proxy saw.
type Log = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A plain HTTP proxy that answers every request itself with the torrent, and records each
/// request line with its `Cookie` header.
async fn proxy() -> (String, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let seen = Log::default();
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buffer = [0_u8; 4096];
                while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => raw.extend_from_slice(&buffer[..read]),
                    }
                }
                let text = String::from_utf8_lossy(&raw).into_owned();
                let line = text.lines().next().unwrap_or_default().to_owned();
                let cookie = text.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim()
                        .eq_ignore_ascii_case("cookie")
                        .then(|| value.trim().to_owned())
                });
                log.lock().expect("seen").push((line, cookie));
                let body = torrent();
                let mut response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/x-bittorrent\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                response.extend_from_slice(&body);
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (format!("http://{address}"), seen)
}

/// A queue with the fixture as its global proxy and a cookie profile for the tracker, and one
/// torrent row of [`SOURCE`] in it.
async fn queue(scratch: &Path, proxy: &str) -> (SchedulerHandle, TorrentService, DownloadFile) {
    let database = rd_db::Database::open(scratch.join("fetch.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(scratch.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig::for_directory(scratch.join("downloads")),
        secrets.clone(),
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    let proxy = database
        .create_proxy_profile(rd_db::NewProxyProfile {
            name: "fixture".to_owned(),
            kind: rd_core::ProxyKind::Http,
            endpoint: proxy.parse().expect("proxy url"),
            username: None,
            secret_ref: None,
        })
        .await
        .expect("proxy profile");
    scheduler
        .update_runtime_settings(RuntimeSettings {
            global_proxy_profile_id: Some(proxy.id),
            ..RuntimeSettings::default()
        })
        .await
        .expect("global proxy");
    let secret_ref = secrets.put_string(COOKIE.to_owned()).await.expect("vault");
    database
        .create_auth_profile(rd_db::NewAuthProfile {
            name: "tracker".to_owned(),
            scope: AuthScope::parse("tracker.test", false).expect("scope"),
            method: AuthMethod::Cookies,
            origin: AuthOrigin::Manual,
            enabled: true,
            expires_at: None,
            username: None,
            secret_ref: Some(secret_ref),
            certificate_ref: None,
        })
        .await
        .expect("profile");
    let package_id = rd_core::PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: "Fetched".to_owned(),
            destination: scratch.join("downloads").display().to_string(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let file = database
        .create_download(rd_db::NewDownload {
            id: DownloadId::new(),
            package_id,
            source: SOURCE.parse().expect("source"),
            file_name: "1.torrent".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Paused,
            kind: rd_core::DownloadKind::Torrent,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    let settings = crate::shared_settings(&database).await.expect("settings");
    let service = TorrentService::start(
        database,
        settings,
        scratch.to_path_buf(),
        scratch.join("downloads"),
    );
    (scheduler, service, file)
}

/// The acceptance case: the address is fetched through the proxy, with the profile's cookie,
/// and librqbit is handed the bytes rather than the address.
#[tokio::test]
async fn a_torrent_address_is_fetched_through_the_proxy_with_the_profiles_cookies() {
    let scratch = Scratch::new();
    let (proxy, seen) = proxy().await;
    let (scheduler, service, file) = queue(&scratch.0, &proxy).await;

    let network = scheduler.transfer_network(&file, None);
    let request = service
        .request_for(&file, Some(&network))
        .await
        .expect("request");

    let AddTorrent::TorrentFileBytes(bytes) = request else {
        panic!("librqbit was handed the address");
    };
    assert_eq!(bytes.as_ref(), torrent().as_slice());
    let seen = seen.lock().expect("seen").clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].0, format!("GET {SOURCE} HTTP/1.1"));
    assert_eq!(seen[0].1.as_deref(), Some(COOKIE));
    // Kept where an uploaded torrent lives, for a retry and a seed after a restart.
    let info_hash = parse_torrent(&torrent()).expect("torrent").info_hash;
    assert!(
        service
            .torrent_file_directory()
            .join(format!("{info_hash}.torrent"))
            .is_file()
    );
}

/// Without a queue run — a seed re-added after a restart — the address is never handed to
/// librqbit: the copy fetched before is taken, and without one it is an error.
#[tokio::test]
async fn without_a_queue_run_only_the_copy_fetched_before_is_used() {
    let scratch = Scratch::new();
    let (proxy, seen) = proxy().await;
    let (_scheduler, service, file) = queue(&scratch.0, &proxy).await;

    let failure = match service.request_for(&file, None).await {
        Ok(_) => panic!("a request without a fetched copy"),
        Err(failure) => failure,
    };
    assert_eq!(failure.code.as_deref(), Some("torrent.add_failed"));

    let (parsed, _) = service.store_torrent_file(&torrent()).await.expect("store");
    let mut state = service.job_state(file.id).await;
    state.metadata = Some(parsed.metadata);
    service.store_job_state(file.id, state).await;
    let request = service.request_for(&file, None).await.expect("request");
    assert!(matches!(request, AddTorrent::TorrentFileBytes(_)));
    assert!(seen.lock().expect("seen").is_empty(), "nothing was fetched");
}
