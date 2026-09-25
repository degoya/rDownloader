//! RD-120-38 — a provider whose row declares `transfer_auth = "basic"` downloads with its
//! account's own credential, and that credential reaches its `secret_domains` and nothing else.
//!
//! Run through the real scheduler, a real TLS listener and a real vault, because the leak this
//! pins is not in any one function. The probe follows the source's redirects and reqwest strips
//! `Authorization` at the change of host; the engine then fetches the chunks from where the
//! probe *ended*, directly. Before this job the headers that went there were the probe's own,
//! so a credential a redirect had just stripped was sent straight to the foreign host again.
//!
//! One listener serves two host names: `127.0.0.1` is the provider (its one secret domain) and
//! `localhost` is the foreign host a redirect points at. Different hosts to reqwest and to the
//! domain gate, one certificate and no hosts-file entry.
//!
//! The canary: every case runs under a subscriber that records everything down to `TRACE`,
//! and neither the password nor the encoded pair may appear in it or in any failure.

use std::{
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex, Once, OnceLock},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rd_core::{AccountId, DownloadFile, DownloadState};
use rd_provider_registry::{
    CredentialKind, DynamicProvider, ProviderKind, ProviderSource, ProviderSpec, SecretFilledBy,
    SecretSlot, TransferAuth,
};
use rd_scheduler::{FileSpec, PackageSpec, RuntimeSettings, SchedulerConfig, SchedulerHandle};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const PASSWORD: &str = "canary-password-6f1d0c";
const USERNAME: &str = "person@example.test";
const PAYLOAD: &[u8] = b"the file itself, all of it";
/// A provider that requires a user name (Seedr's shape) and one that does not (Pixeldrain's).
const REQUIRED: &str = "fixture_named";
const OPTIONAL: &str = "fixture_keyed";

fn pair(username: &str) -> String {
    BASE64_STANDARD.encode(format!("{username}:{PASSWORD}"))
}

// -- the log canary ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn captured_log() -> &'static Captured {
    static LOG: OnceLock<Captured> = OnceLock::new();
    LOG.get_or_init(|| {
        let log = Captured::default();
        let writer = log.clone();
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .try_init();
        log
    })
}

/// The canary over everything the subscriber recorded — which must be something, or the
/// absence of the credential would prove nothing.
fn assert_log_clean(log: &Captured) {
    let text = String::from_utf8_lossy(&log.0.lock().expect("log")).into_owned();
    assert!(
        text.contains("download_id"),
        "the subscriber recorded no download at all"
    );
    assert_no_credential_in("the log", &text);
}

fn assert_no_credential_in(what: &str, text: &str) {
    for canary in [PASSWORD, &pair(USERNAME), &pair("")] {
        assert!(
            !text.contains(canary),
            "{what} carries the credential ({canary})"
        );
    }
}

// -- the provider rows ------------------------------------------------------------------------

fn row(slug: &str, username_required: bool) -> DynamicProvider {
    DynamicProvider {
        plugin_id: format!("plugin-{slug}"),
        spec: ProviderSpec {
            slug: slug.to_owned(),
            display_name: slug.to_owned(),
            kind: ProviderKind::Hoster,
            credentials: if username_required {
                CredentialKind::UsernamePassword
            } else {
                CredentialKind::ApiKey
            },
            username_required,
            transfer_auth: TransferAuth::Basic,
            secrets: vec![SecretSlot {
                reference: format!("{slug}_secret"),
                // The provider's one secret domain. `localhost` is deliberately not here.
                domains: vec!["127.0.0.1".to_owned()],
                mode: None,
                filled_by: SecretFilledBy::Person,
            }],
            request_domains: vec!["127.0.0.1".to_owned()],
            cookie_scope: None,
            // No claimed host: the source is fetched as it is, with no resolver in between.
            match_hosts: Vec::new(),
            host_aliases: Vec::new(),
            source: ProviderSource::Plugin,
            plugin_id: Some(format!("plugin-{slug}")),
            plugin_version: Some("1.0.0".to_owned()),
        },
    }
}

fn register_providers() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let rejected =
            rd_provider_registry::replace_dynamic(vec![row(REQUIRED, true), row(OPTIONAL, false)]);
        assert!(rejected.is_empty(), "{rejected:?}");
    });
}

// -- the TLS listener -------------------------------------------------------------------------

/// Host header, path and `Authorization` of every request the listener saw.
type Seen = Arc<Mutex<Vec<(String, String, Option<String>)>>>;

struct Fixture {
    address: SocketAddr,
    ca_pem: String,
    seen: Seen,
}

impl Fixture {
    fn origin(&self, path: &str) -> url::Url {
        format!("https://127.0.0.1:{}{path}", self.address.port())
            .parse()
            .expect("url")
    }

    fn seen(&self) -> Vec<(String, String, Option<String>)> {
        self.seen.lock().expect("seen").clone()
    }
}

/// Serves `/redirect` (302 to the same path on `localhost`), `/payload` (the file) and
/// `/denied` (401, echoing the `Authorization` it was sent, as a careless server might).
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
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match tls.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => request.extend_from_slice(&buffer[..read]),
                    }
                }
                let text = String::from_utf8_lossy(&request).into_owned();
                let mut lines = text.lines();
                let mut first = lines.next().unwrap_or_default().split(' ');
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
                let host = header("host").unwrap_or_default();
                let authorization = header("authorization");
                log.lock()
                    .expect("seen")
                    .push((host.clone(), path.clone(), authorization.clone()));
                let port = address.port();
                let response = match path.as_str() {
                    "/redirect" => format!(
                        "HTTP/1.1 302 Found\r\nlocation: https://localhost:{port}/payload\r\n\
                         content-length: 0\r\nconnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    "/payload" => {
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
                    _ => {
                        let body = format!("Authorization: {}", authorization.unwrap_or_default());
                        format!(
                            "HTTP/1.1 401 Unauthorized\r\nwww-authenticate: Basic realm=\"x\"\r\n\
                             content-type: text/plain\r\ncontent-length: {}\r\n\
                             connection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .into_bytes()
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
    let database = rd_db::Database::open(directory.join("transfer-credential.sqlite3"))
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

async fn account(
    database: &rd_db::Database,
    secrets: &rd_secrets::SecretStore,
    provider: &str,
    username: Option<&str>,
) -> AccountId {
    let secret_ref = secrets
        .put_string(PASSWORD.to_owned())
        .await
        .expect("vault");
    database
        .create_account(rd_db::NewAccount {
            provider: provider.to_owned(),
            label: "Test".to_owned(),
            username: username.map(str::to_owned),
            credential_mode: None,
            secret_ref: Some(secret_ref),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account")
        .id
}

/// Enqueues one running download of `source` through `account` and waits until it finished
/// or failed.
async fn download(
    scheduler: &SchedulerHandle,
    database: &rd_db::Database,
    directory: &Path,
    source: url::Url,
    account: AccountId,
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
        account_id: Some(account),
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

// -- the cases --------------------------------------------------------------------------------

/// The acceptance case: `Authorization: Basic` to the provider's own host, and to no other —
/// including the foreign host its redirect points at, where the bytes then come from.
#[tokio::test]
async fn the_credential_reaches_its_host_and_not_the_one_a_redirect_leads_to() {
    let log = captured_log();
    register_providers();
    let fixture = tls_fixture().await;
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, secrets) = scheduler_over(directory.path(), &fixture.ca_pem).await;
    let account = account(&database, &secrets, REQUIRED, Some(USERNAME)).await;

    let finished = download(
        &scheduler,
        &database,
        directory.path(),
        fixture.origin("/redirect"),
        account,
    )
    .await;
    assert_eq!(
        finished.state,
        DownloadState::Completed,
        "{:?}",
        finished.last_error
    );

    let seen = fixture.seen();
    let expected = format!("Basic {}", pair(USERNAME));
    let at_origin: Vec<_> = seen
        .iter()
        .filter(|(host, _, _)| host.starts_with("127.0.0.1"))
        .collect();
    assert!(
        !at_origin.is_empty(),
        "the provider was never asked: {seen:?}"
    );
    for (_, path, authorization) in &at_origin {
        assert_eq!(authorization.as_deref(), Some(expected.as_str()), "{path}");
    }
    let foreign: Vec<_> = seen
        .iter()
        .filter(|(host, _, _)| host.starts_with("localhost"))
        .collect();
    // The probe's hop and the transfer itself both reach the foreign host.
    assert!(foreign.len() >= 2, "{seen:?}");
    for (_, path, authorization) in &foreign {
        assert_eq!(
            authorization, &None,
            "the credential followed a redirect to a foreign host at {path}"
        );
    }
    assert_log_clean(log);
}

/// The address the transfer goes to is on a foreign host from the start: nothing is sent,
/// not even to the probe.
#[tokio::test]
async fn a_source_on_a_foreign_host_carries_nothing() {
    register_providers();
    let fixture = tls_fixture().await;
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, secrets) = scheduler_over(directory.path(), &fixture.ca_pem).await;
    let account = account(&database, &secrets, REQUIRED, Some(USERNAME)).await;
    let source: url::Url = format!("https://localhost:{}/payload", fixture.address.port())
        .parse()
        .expect("url");

    let finished = download(&scheduler, &database, directory.path(), source, account).await;
    assert_eq!(finished.state, DownloadState::Completed);
    assert!(
        fixture.seen().iter().all(|(_, _, auth)| auth.is_none()),
        "{:?}",
        fixture.seen()
    );
}

/// Pixeldrain's shape: no user name, and the row does not require one — the pair goes out as
/// `base64(":key")`.
#[tokio::test]
async fn a_provider_without_a_required_name_sends_the_key_as_the_password() {
    register_providers();
    let fixture = tls_fixture().await;
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, secrets) = scheduler_over(directory.path(), &fixture.ca_pem).await;
    let account = account(&database, &secrets, OPTIONAL, None).await;

    let finished = download(
        &scheduler,
        &database,
        directory.path(),
        fixture.origin("/payload"),
        account,
    )
    .await;
    assert_eq!(
        finished.state,
        DownloadState::Completed,
        "{:?}",
        finished.last_error
    );
    let expected = format!("Basic {}", pair(""));
    let seen = fixture.seen();
    assert!(!seen.is_empty());
    for (_, path, authorization) in &seen {
        assert_eq!(authorization.as_deref(), Some(expected.as_str()), "{path}");
    }
}

/// Seedr's shape with the name missing: refused with a stable code before anything is sent,
/// rather than sent as half a credential.
#[tokio::test]
async fn a_provider_that_requires_a_name_refuses_an_account_without_one() {
    let log = captured_log();
    register_providers();
    let fixture = tls_fixture().await;
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, secrets) = scheduler_over(directory.path(), &fixture.ca_pem).await;
    let account = account(&database, &secrets, REQUIRED, None).await;

    let failed = download(
        &scheduler,
        &database,
        directory.path(),
        fixture.origin("/payload"),
        account,
    )
    .await;
    let failure = failed.last_error.expect("a failure");
    assert_eq!(failure.code.as_deref(), Some("plugin.username_missing"));
    assert!(fixture.seen().is_empty(), "{:?}", fixture.seen());
    assert_no_credential_in(
        "the failure",
        &serde_json::to_string(&failure).expect("json"),
    );
    assert_log_clean(log);
}

/// The canary on the failure path: a provider that refuses the credential and echoes it back
/// leaves it in no failure and no log line.
#[tokio::test]
async fn a_refused_credential_appears_in_no_failure_and_no_log() {
    let log = captured_log();
    register_providers();
    let fixture = tls_fixture().await;
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database, secrets) = scheduler_over(directory.path(), &fixture.ca_pem).await;
    let account = account(&database, &secrets, REQUIRED, Some(USERNAME)).await;

    let failed = download(
        &scheduler,
        &database,
        directory.path(),
        fixture.origin("/denied"),
        account,
    )
    .await;
    let failure = failed.last_error.expect("a failure");
    // The listener did receive it — the canary is meaningful only if the value was in play.
    let expected = format!("Basic {}", pair(USERNAME));
    assert!(
        fixture
            .seen()
            .iter()
            .any(|(_, _, auth)| auth.as_deref() == Some(expected.as_str())),
        "{:?}",
        fixture.seen()
    );
    assert_no_credential_in(
        "the failure",
        &serde_json::to_string(&failure).expect("json"),
    );
    assert_log_clean(log);
}
