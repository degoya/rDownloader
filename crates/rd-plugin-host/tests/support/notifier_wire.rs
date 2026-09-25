//! The wire the notifier tests read from (RD-120-60, RD-120-65).
//!
//! The service names stay the real ones (`api.telegram.org`, `ntfy.sh`, `discord.com`): the
//! manifests allow nothing else, and two of the addresses are fixed in their plugins. They are
//! reached through a global HTTP proxy profile pointing at a local listener that answers
//! `CONNECT` and then terminates TLS with a certificate for the requested name, issued by a test
//! CA the client trusts through `custom_ca_pem`. Nothing leaves the machine.

// Shared by two test binaries, and neither uses every helper.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use rd_http::{ClientPool, NetworkDefaults};
use rd_plugin_host::{
    PluginManifest, ResolverService, artifact::component, extension::NotifierPlugin,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::RwLock,
};

/// One request as it arrived: the tunnel it came through, then what was sent inside it.
#[derive(Clone, Debug)]
pub struct Arrived {
    pub tunnel: String,
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Arrived {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn headers_named(&self, name: &str) -> usize {
        self.headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .count()
    }

    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }

    pub fn query(&self) -> HashMap<String, String> {
        let url = url::Url::parse(&format!("https://wire.invalid{}", self.target)).expect("url");
        url.query_pairs().into_owned().collect()
    }
}

/// A path prefix the wire answers with a `307` rather than `200`: `/redirect-to/<host>/<rest>`
/// is sent on to `https://<host>/<rest>`, query included, so a test can watch whether a
/// plugin's request follows a redirect to a host it may or may not reach (RD-130-15,
/// RD-130-24).
pub const REDIRECT_TO: &str = "/redirect-to/";

pub struct Wire {
    pub proxy: SocketAddr,
    pub ca_pem: String,
    pub arrived: Arc<Mutex<Vec<Arrived>>>,
}

/// Reads one HTTP/1.1 message head, and as much body as its `content-length` names.
async fn read_message<S: AsyncRead + Unpin>(stream: &mut S) -> Option<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let end = loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    let mut body = bytes[end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Some((head, body))
}

/// A proxy that accepts `CONNECT`, then plays the service at the far end over TLS and answers
/// every request `200 {"ok":true}`.
pub async fn wire() -> Wire {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let mut ca_params = rcgen::CertificateParams::new(Vec::new()).expect("ca params");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_key = rcgen::KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca cert");
    let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);
    let server_params = rcgen::CertificateParams::new(vec![
        "api.telegram.org".to_owned(),
        "ntfy.sh".to_owned(),
        "discord.com".to_owned(),
        "linksnappy.com".to_owned(),
        "api.torbox.app".to_owned(),
        "cloud.example".to_owned(),
        "ntfy.example.org".to_owned(),
        "push.example.org".to_owned(),
    ])
    .expect("server params");
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
    let proxy = listener.local_addr().expect("address");
    let arrived: Arc<Mutex<Vec<Arrived>>> = Arc::default();
    let log = Arc::clone(&arrived);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let Some((connect, _)) = read_message(&mut stream).await else {
                    return;
                };
                let tunnel = connect
                    .lines()
                    .next()
                    .and_then(|line| line.strip_prefix("CONNECT "))
                    .and_then(|rest| rest.split(' ').next())
                    .unwrap_or_default()
                    .to_owned();
                if stream
                    .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .await
                    .is_err()
                {
                    return;
                }
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let Some((head, body)) = read_message(&mut tls).await else {
                    return;
                };
                let mut lines = head.lines();
                let mut first = lines.next().unwrap_or_default().split(' ');
                let method = first.next().unwrap_or_default().to_owned();
                let target = first.next().unwrap_or_default().to_owned();
                let headers = lines
                    .filter_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        Some((key.trim().to_owned(), value.trim().to_owned()))
                    })
                    .collect();
                let redirect = target
                    .strip_prefix(REDIRECT_TO)
                    .map(|rest| format!("https://{rest}"));
                log.lock().expect("arrived").push(Arrived {
                    tunnel,
                    method,
                    target,
                    headers,
                    body,
                });
                let answer = "{\"ok\":true}";
                let response = if let Some(location) = redirect {
                    format!(
                        "HTTP/1.1 307 Temporary Redirect\r\nlocation: {location}\r\n\
                         content-length: 0\r\nconnection: close\r\n\r\n"
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{answer}",
                        answer.len()
                    )
                };
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    Wire {
        proxy,
        ca_pem: ca_cert.pem(),
        arrived,
    }
}

/// The application's host, sending through the wire's proxy, and the vault reference of
/// `secret` in its store.
pub async fn host_over(
    directory: &std::path::Path,
    wire: &Wire,
    secret: &str,
) -> (Arc<dyn rd_plugin_api::ResolverHost>, String) {
    let parts = service_over(directory, wire).await;
    let reference = parts
        .secrets
        .put_string(secret.to_owned())
        .await
        .expect("put secret");
    (parts.service.host(), reference)
}

/// The application's resolver service and its host, sending through the wire's proxy, with
/// the database and the vault behind them for a test that needs accounts.
pub struct Service {
    pub service: ResolverService,
    pub database: rd_db::Database,
    pub secrets: rd_secrets::SecretStore,
}

pub async fn service_over(directory: &std::path::Path, wire: &Wire) -> Service {
    let database = rd_db::Database::open(directory.join("db.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secret store");
    let proxy = database
        .create_proxy_profile(rd_db::NewProxyProfile {
            name: "wire".to_owned(),
            kind: rd_core::ProxyKind::Http,
            endpoint: format!("http://{}", wire.proxy).parse().expect("proxy url"),
            username: None,
            secret_ref: None,
        })
        .await
        .expect("proxy profile");
    let defaults = NetworkDefaults {
        global_proxy_profile_id: Some(proxy.id),
        custom_ca_pem: vec![wire.ca_pem.clone().into_bytes()],
        tls_revision: 1,
    };
    let service = ResolverService::new(
        database.clone(),
        ClientPool::default(),
        secrets.clone(),
        Arc::new(RwLock::new(defaults)),
        None,
    );
    Service {
        service,
        database,
        secrets,
    }
}

pub fn notifier(
    source: &str,
    package: &str,
    host: Arc<dyn rd_plugin_api::ResolverHost>,
) -> NotifierPlugin {
    let manifest: PluginManifest = toml::from_str(source).expect("bundled manifest");
    NotifierPlugin::new(manifest, &component(package), Some(host)).expect("compile")
}

/// The provider rows an account is checked against, from the bundled manifests.
pub fn register_providers() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let root = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
        )
        .join("../../plugins");
        let manifests: Vec<PluginManifest> = std::fs::read_dir(root)
            .expect("plugins directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("manifest.toml"))
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .filter_map(|text| toml::from_str(&text).ok())
            .collect();
        let rows: Vec<_> = manifests
            .iter()
            .filter_map(rd_plugin_host::provider_spec_from_manifest)
            .collect();
        let rejected = rd_provider_registry::replace_dynamic(rows);
        assert!(rejected.is_empty(), "rejected rows: {rejected:?}");
    });
}

pub async fn account(
    parts: &Service,
    provider: &str,
    username: &str,
    secret: &str,
) -> rd_core::AccountId {
    let secret_ref = parts
        .secrets
        .put_string(secret.to_owned())
        .await
        .expect("put secret");
    parts
        .database
        .create_account(rd_db::NewAccount {
            provider: provider.to_owned(),
            label: "Test".to_owned(),
            username: Some(username.to_owned()),
            credential_mode: None,
            secret_ref: Some(secret_ref),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("create account")
        .id
}
