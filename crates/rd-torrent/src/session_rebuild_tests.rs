//! A rebuild on the port the running session listens on (RD-1240-28).
//!
//! The live test of 1.24 switched sharing off with a fixed listen port: the new session was
//! built beside the old one, which still held the port, and failed with "Address already in
//! use" while the settings answered `200`. These tests build a real session on a fixed port.
//! The last two hold a port with another socket first (RD-1250-04): on Windows the engine took
//! such a port beside its holder and started without a word.

use std::path::PathBuf;

use rd_core::CandidateId;

use crate::TorrentService;

/// A directory below the system temp dir, removed again when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rd-torrent-rebuild-{}", CandidateId::new()));
        std::fs::create_dir_all(&path).expect("scratch dir");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn service(scratch: &Scratch) -> TorrentService {
    let database = rd_db::Database::open(scratch.0.join("torrent.sqlite3"))
        .await
        .expect("database");
    let settings = crate::shared_settings(&database).await.expect("settings");
    TorrentService::start(
        database,
        settings,
        scratch.0.clone(),
        scratch.0.join("downloads"),
    )
}

/// A port nothing listens on right now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port")
        .port()
}

#[tokio::test]
async fn sharing_switched_off_on_a_fixed_port_rebuilds_the_session() {
    let scratch = Scratch::new();
    let service = service(&scratch).await;
    let port = free_port();
    {
        // Sharing is off by default: switch it on first, so switching it off is a change.
        let mut settings = service.inner.settings.write().await;
        settings.torrent_listen_port = Some(port);
        settings.torrent_sharing_enabled = true;
    }
    service.session().await.expect("first session");
    let first = service.session_generation().await;

    service.inner.settings.write().await.torrent_sharing_enabled = false;
    service
        .reconfigure()
        .await
        .expect("the rebuild takes the port the old session let go of");

    assert!(service.session_generation().await > first);
    assert_eq!(*service.inner.rebuild_error.read().await, None);
    let session = service.session().await.expect("rebuilt session");
    assert_eq!(session.listen_addr().map(|addr| addr.port()), Some(port));
}

#[tokio::test]
async fn a_rebuild_that_fails_keeps_an_engine_and_says_why() {
    let scratch = Scratch::new();
    let service = service(&scratch).await;
    let port = free_port();
    service.inner.settings.write().await.torrent_listen_port = Some(port);
    service.session().await.expect("first session");

    // The new settings name a blocklist that cannot be read, and the engine refuses to start
    // without it. Not a port another program holds: that is refused before the engine binds,
    // the tests below (RD-1250-04).
    let missing =
        url::Url::from_file_path(scratch.0.join("missing-blocklist.txt")).expect("file URL");
    service
        .inner
        .settings
        .write()
        .await
        .torrent_ip_blocklist_url = Some(missing.to_string());
    assert!(service.reconfigure().await.is_err());
    assert!(service.inner.rebuild_error.read().await.is_some());
    // The previous session runs on: the service is never left without an engine.
    let session = service.session().await.expect("an engine");
    assert_eq!(session.listen_addr().map(|addr| addr.port()), Some(port));
}

/// A socket of another program, listening on a port the system picked.
fn held_by_another_program() -> (std::net::TcpListener, u16) {
    let other = std::net::TcpListener::bind("0.0.0.0:0").expect("another program's socket");
    let port = other.local_addr().expect("address").port();
    (other, port)
}

#[tokio::test]
async fn a_port_another_program_holds_refuses_the_start_and_is_named() {
    let scratch = Scratch::new();
    let service = service(&scratch).await;
    let (_other, port) = held_by_another_program();
    service.inner.settings.write().await.torrent_listen_port = Some(port);

    // `Session` is no `Debug`, so no `expect_err`.
    let Err(refused) = service.session().await else {
        panic!("an engine started beside the program holding port {port}");
    };
    assert_eq!(crate::port_in_use(&refused), Some(port));
    assert_eq!(service.network_status().await.port_in_use, Some(port));
    let report = service.port_test().await;
    assert_eq!(report.verdict, crate::TorrentPortVerdict::Unavailable);
    assert_eq!(report.port_in_use, Some(port));
    assert!(report.error.is_some());
}

/// The case 1.24 w4 found on GitHub's Windows runner: a rebuild onto a port another socket holds
/// was taken without an error. Now it is refused before the engine binds, the previous engine
/// runs on, and the status names the port.
#[cfg(windows)]
#[tokio::test]
async fn a_rebuild_onto_a_port_another_socket_holds_keeps_the_engine_on_windows() {
    let scratch = Scratch::new();
    let service = service(&scratch).await;
    let first = free_port();
    service.inner.settings.write().await.torrent_listen_port = Some(first);
    service.session().await.expect("first session");

    let (_other, port) = held_by_another_program();
    service.inner.settings.write().await.torrent_listen_port = Some(port);
    let refused = service
        .reconfigure()
        .await
        .expect_err("the held port is refused");
    assert_eq!(crate::port_in_use(&refused), Some(port));
    assert!(service.inner.rebuild_error.read().await.is_some());
    let session = service.session().await.expect("an engine");
    assert_eq!(session.listen_addr().map(|addr| addr.port()), Some(first));
    assert_eq!(service.network_status().await.port_in_use, Some(port));
    assert_eq!(service.port_test().await.port_in_use, Some(port));
}
