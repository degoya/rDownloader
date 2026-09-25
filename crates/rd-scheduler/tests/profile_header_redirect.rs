//! RD-120-43 — the headers of an HTTP authentication profile reach the hosts the profile covers,
//! and not the host a redirect leads to.
//!
//! The same shape as `provider_transfer_credential.rs` (RD-120-38), for the same reason: the leak
//! is not in any one function. The probe follows the source's redirects and reqwest strips
//! `Authorization` at the change of host; the engine then fetches the chunks from where the
//! probe *ended*, directly. Before this job the profile's header went there regardless, because
//! the profile's scope was only ever checked against the address the download was added with.
//!
//! One listener serves two host names: `127.0.0.1` is the host the profile covers and
//! `localhost` the foreign host a redirect points at — one certificate, no hosts-file entry.

use std::{
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use rd_core::{AuthMethod, AuthOrigin, AuthScope, DownloadFile, DownloadState};
use rd_scheduler::{FileSpec, PackageSpec, RuntimeSettings, SchedulerConfig, SchedulerHandle};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const TOKEN: &str = "profile-canary-3b9e71";
const COOKIE: &str = "session=cookie-canary-5a20d4";
const PAYLOAD: &[u8] = b"the file itself, all of it";

// -- the TLS listener -------------------------------------------------------------------------

/// One request as the listener saw it.
#[derive(Clone, Debug)]
struct Request {
    host: String,
    path: String,
    authorization: Option<String>,
    cookie: Option<String>,
}

struct Fixture {
    address: SocketAddr,
    ca_pem: String,
    seen: Arc<Mutex<Vec<Request>>>,
}

impl Fixture {
    fn origin(&self, path: &str) -> url::Url {
        format!("https://127.0.0.1:{}{path}", self.address.port())
            .parse()
            .expect("url")
    }

    fn seen(&self) -> Vec<Request> {
        self.seen.lock().expect("seen").clone()
    }

    fn at(&self, host: &str) -> Vec<Request> {
        self.seen()
            .into_iter()
            .filter(|request| request.host.starts_with(host))
            .collect()
    }
}

/// Serves `/redirect` (302 to `/payload` on the foreign `localhost`), `/within` (302 to
/// `/payload` on `127.0.0.1` itself) and `/payload` (the file).
async fn tls_fixture() -> Fixture {
    let mut ca_params = rcgen::CertificateParams::new(Vec::new()).expect("ca params");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_key = rcgen::KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca cert");
    let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);
    let mut server_params =
        rcgen::CertificateParams::new(vec!["localhost".to_owned()]).expect("server params");
    server_params
        .subject_alt_names
        .push(rcgen::SanType::IpAddress(std::net::IpAddr::from([
            127, 0, 0, 1,
        ])));
    let server_key = rcgen::KeyPair::generate().expect("server key");
    let server_cert = server_params
        .signed_by(&server_key, &issuer)
        .expect("server cert");
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(server_cert.der().to_vec())],
            PrivateKeyDer::try_from(server_key.serialize_der()).expect("server key der"),
        )
        .expect("server config");
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let seen: Arc<Mutex<Vec<Request>>> = Arc::default();
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut raw = Vec::new();
                let mut buffer = [0_u8; 4096];
                while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
                    match tls.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => raw.extend_from_slice(&buffer[..read]),
                    }
                }
                let text = String::from_utf8_lossy(&raw).into_owned();
                let mut first = text.lines().next().unwrap_or_default().split(' ');
                let method = first.next().unwrap_or_default().to_owned();
                let path = first.next().unwrap_or_default().to_owned();
                let header = |name: &str| {
                    text.lines().find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.trim()
                            .eq_ignore_ascii_case(name)
                            .then(|| value.trim().to_owned())
                    })
                };
                log.lock().expect("seen").push(Request {
                    host: header("host").unwrap_or_default(),
                    path: path.clone(),
                    authorization: header("authorization"),
                    cookie: header("cookie"),
                });
                let port = address.port();
                let redirect = |host: &str| {
                    format!(
                        "HTTP/1.1 302 Found\r\nlocation: https://{host}:{port}/payload\r\n\
                         content-length: 0\r\nconnection: close\r\n\r\n"
                    )
                    .into_bytes()
                };
                let response = match path.as_str() {
                    "/redirect" => redirect("localhost"),
                    "/within" => redirect("127.0.0.1"),
                    _ => {
                        let mut head = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n",
                            PAYLOAD.len()
                        )
                        .into_bytes();
                        if method != "HEAD" {
                            head.extend_from_slice(PAYLOAD);
                        }
                        head
                    }
                };
                let _ = tls.write_all(&response).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    Fixture {
        address,
        ca_pem: ca_cert.pem(),
        seen,
    }
}

// -- the scheduler ----------------------------------------------------------------------------

async fn scheduler_over(
    directory: &Path,
    ca_pem: &str,
) -> (SchedulerHandle, rd_db::Database, rd_secrets::SecretStore) {
    let database = rd_db::Database::open(directory.join("profile-redirect.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig::for_directory(directory.join("downloads")),
        secrets.clone(),
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    scheduler
        .update_runtime_settings(RuntimeSettings {
            custom_ca_pem: Some(ca_pem.to_owned()),
            max_retries: 0,
            ..RuntimeSettings::default()
        })
        .await
        .expect("trust the fixture CA");
    (scheduler, database, secrets)
}

/// A profile for `127.0.0.1` only, carrying the stored `secret_ref` the way `method` says.
async fn profile(database: &rd_db::Database, method: AuthMethod, secret_ref: String) {
    database
        .create_auth_profile(rd_db::NewAuthProfile {
            name: "fixture".to_owned(),
            scope: AuthScope::parse("127.0.0.1", false).expect("scope"),
            method,
            origin: AuthOrigin::Manual,
            enabled: true,
            expires_at: None,
            username: None,
            secret_ref: Some(secret_ref),
            certificate_ref: None,
        })
        .await
        .expect("profile");
}

/// Enqueues one running download of `source` and waits until it finished or failed.
async fn download(
    scheduler: &SchedulerHandle,
    database: &rd_db::Database,
    directory: &Path,
    source: url::Url,
) -> DownloadFile {
    let spec = PackageSpec {
        name: "Transfer".to_owned(),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: false,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let files = vec![FileSpec {
        source,
        file_name: "file.bin".to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
        kind: rd_core::DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
    }];
    let (_, files) = scheduler
        .enqueue_package(spec, files)
        .await
        .expect("enqueue");
    let id = files.first().expect("one file").id;
    for _ in 0..300 {
        let current = database
            .get_download(id)
            .await
            .expect("read")
            .expect("download");
        if current.state == DownloadState::Completed || current.last_error.is_some() {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the download neither finished nor failed");
}

/// A scheduler whose one profile is of `method`, with the matching canary in the vault.
async fn setup(
    fixture: &Fixture,
    directory: &Path,
    method: AuthMethod,
) -> (SchedulerHandle, rd_db::Database) {
    let (scheduler, database, secrets) = scheduler_over(directory, &fixture.ca_pem).await;
    let value = if method == AuthMethod::Cookies {
        COOKIE
    } else {
        TOKEN
    };
    let secret_ref = secrets.put_string(value.to_owned()).await.expect("vault");
    profile(&database, method, secret_ref).await;
    (scheduler, database)
}

fn completed(file: &DownloadFile) {
    assert_eq!(
        file.state,
        DownloadState::Completed,
        "{:?}",
        file.last_error
    );
}

// -- the cases --------------------------------------------------------------------------------

/// The acceptance case: the profile's `Authorization` reaches the host it covers, and not the
/// foreign host the redirect points at — where the bytes then come from.
#[tokio::test]
async fn the_profile_header_reaches_its_host_and_not_the_one_a_redirect_leads_to() {
    let fixture = tls_fixture().await;
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = setup(&fixture, directory.path(), AuthMethod::Bearer).await;

    let finished = download(
        &scheduler,
        &database,
        directory.path(),
        fixture.origin("/redirect"),
    )
    .await;
    completed(&finished);

    let expected = format!("Bearer {TOKEN}");
    let at_origin = fixture.at("127.0.0.1");
    assert!(!at_origin.is_empty(), "the source was never asked");
    for request in &at_origin {
        assert_eq!(
            request.authorization.as_deref(),
            Some(expected.as_str()),
            "{request:?}"
        );
    }
    let foreign = fixture.at("localhost");
    // The probe's hop and the transfer itself both reach the foreign host.
    assert!(foreign.len() >= 2, "{:?}", fixture.seen());
    for request in &foreign {
        assert_eq!(
            request.authorization, None,
            "the profile header followed a redirect to a foreign host: {request:?}"
        );
    }
}

/// A redirect inside the profile's scope keeps the header, on the probe's hop and on the
/// transfer from where it ended.
#[tokio::test]
async fn a_redirect_within_the_scope_keeps_the_header() {
    let fixture = tls_fixture().await;
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = setup(&fixture, directory.path(), AuthMethod::Bearer).await;

    let finished = download(
        &scheduler,
        &database,
        directory.path(),
        fixture.origin("/within"),
    )
    .await;
    completed(&finished);

    let seen = fixture.seen();
    let expected = format!("Bearer {TOKEN}");
    let payload: Vec<_> = seen
        .iter()
        .filter(|request| request.path == "/payload")
        .collect();
    assert!(payload.len() >= 2, "{seen:?}");
    for request in &seen {
        assert!(request.host.starts_with("127.0.0.1"), "{request:?}");
        assert_eq!(
            request.authorization.as_deref(),
            Some(expected.as_str()),
            "{request:?}"
        );
    }
}

/// A cookie profile is not on this path at all: its cookies sit in the client's jar bound to
/// the profile's host, and the jar decides per request — so the foreign host gets none, with
/// or without the worker's help.
#[tokio::test]
async fn profile_cookies_do_not_follow_a_redirect_to_a_foreign_host() {
    let fixture = tls_fixture().await;
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = setup(&fixture, directory.path(), AuthMethod::Cookies).await;

    let finished = download(
        &scheduler,
        &database,
        directory.path(),
        fixture.origin("/redirect"),
    )
    .await;
    completed(&finished);

    let at_origin = fixture.at("127.0.0.1");
    assert!(!at_origin.is_empty(), "the source was never asked");
    for request in &at_origin {
        assert_eq!(request.cookie.as_deref(), Some(COOKIE), "{request:?}");
    }
    let foreign = fixture.at("localhost");
    assert!(foreign.len() >= 2, "{:?}", fixture.seen());
    for request in &foreign {
        assert_eq!(request.cookie, None, "{request:?}");
    }
}
