//! RD-1240-08 — the proxy a download tool is handed: resolved like every transfer's, its
//! credentials only in the child's environment, and never a direct connection in its place.

use std::{path::Path, sync::Arc};

use rd_core::{ProxyKind, ProxyProfileId};
use secrecy::SecretString;
use url::Url;

use super::{ToolNetwork, ToolNetworkSource, ToolProxy, proxy_auth_failed, proxy_unavailable};

const PASSWORD: &str = "pr0xy+pass:w@rd";

fn endpoint(text: &str) -> Url {
    Url::parse(text).expect("endpoint")
}

fn credentialed() -> ToolProxy {
    ToolProxy::new(
        ProxyKind::Socks5,
        endpoint("socks5h://proxy.example:1080"),
        Some("alice"),
        Some(&SecretString::from(PASSWORD.to_owned())),
    )
    .expect("proxy")
}

fn plain() -> ToolProxy {
    ToolProxy::new(
        ProxyKind::Http,
        endpoint("http://proxy.example:3128"),
        None,
        None,
    )
    .expect("proxy")
}

/// The environment `network` gives a command, as `(name, value)` with removals as `None`.
///
/// Names upper-cased: on Windows the two spellings are one variable, and the command keeps one.
fn environment(network: &ToolNetwork) -> Vec<(String, Option<String>)> {
    let mut command = tokio::process::Command::new("tool");
    network.apply(&mut command);
    command
        .as_std()
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy().to_ascii_uppercase(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect()
}

#[test]
fn a_proxy_without_credentials_is_the_tools_own_option() {
    let network = ToolNetwork::with_proxy(plain());
    assert_eq!(network.proxy_argument(), Some("http://proxy.example:3128/"));
    assert!(!network.proxy_in_environment_only());
    let environment = environment(&network);
    assert!(environment.contains(&(
        "HTTPS_PROXY".to_owned(),
        Some("http://proxy.example:3128/".to_owned())
    )));
    // The profile has no exceptions; the service's own must not route around it.
    assert!(environment.contains(&("NO_PROXY".to_owned(), None)));
}

#[test]
fn credentials_reach_the_environment_and_never_an_argument() {
    let network = ToolNetwork::with_proxy(credentialed());
    assert_eq!(network.proxy_argument(), None);
    assert!(network.proxy_in_environment_only());
    let environment = environment(&network);
    let value = environment
        .iter()
        .find(|(name, _)| name == "ALL_PROXY")
        .and_then(|(_, value)| value.clone())
        .expect("ALL_PROXY");
    // Encoded, so `+`, `:` and `@` reach Python as themselves.
    assert_eq!(
        value,
        "socks5h://alice:pr0xy%2Bpass%3Aw%40rd@proxy.example:1080"
    );
    for name in ["HTTP_PROXY", "HTTPS_PROXY"] {
        assert!(
            environment
                .iter()
                .any(|(set, set_value)| set == name && set_value.as_deref() == Some(&*value)),
            "{name}"
        );
    }
}

#[test]
fn a_proxy_never_shows_its_password_in_a_log_line() {
    let proxy = credentialed();
    let debug = format!("{proxy:?} {:?}", ToolNetwork::with_proxy(proxy.clone()));
    assert!(!debug.contains("pr0xy"), "{debug}");
    assert!(debug.contains("proxy.example"), "{debug}");
    assert_eq!(proxy.endpoint().as_str(), "socks5h://proxy.example:1080");
    // A tool that quotes the address it was handed is redacted on the way to the row.
    let quoted = rd_core::redact_text(
        "ERROR: Unable to connect to proxy socks5h://alice:pr0xy%2Bpass%3Aw%40rd@proxy.example:1080",
    );
    assert!(!quoted.contains("pr0xy"), "{quoted}");
}

#[test]
fn a_direct_network_sets_nothing() {
    let network = ToolNetwork::direct();
    assert_eq!(network.proxy_argument(), None);
    assert!(environment(&network).is_empty());
    assert!(
        network
            .unsupported_proxy("yt-dlp", "Unsupported proxy type")
            .is_none()
    );
}

#[test]
fn a_tool_that_cannot_use_the_proxy_fails_with_a_code() {
    let network = ToolNetwork::with_proxy(credentialed());
    for stderr in [
        "ERROR: Unsupported proxy type: \"socks5h\". Supported: http",
        "requests.exceptions.InvalidSchema: Missing dependencies for SOCKS support.",
    ] {
        let failure = network
            .unsupported_proxy("gallery-dl", stderr)
            .expect("failure");
        assert_eq!(failure.code.as_deref(), Some("proxy.unsupported_by_tool"));
        assert!(!failure.category.is_retryable());
    }
    assert!(
        network
            .unsupported_proxy("gallery-dl", "HTTPError 503")
            .is_none()
    );
}

/// RD-1240-29: the words the tools' HTTP stacks print for a proxy's 407 are one permanent
/// failure; a 407 that is no proxy's answer, a 4070 and a proxy that cannot be reached are none.
#[test]
fn a_proxy_refusing_its_password_fails_without_a_retry() {
    for output in [
        "ProxyError('Unable to connect to proxy', OSError('Tunnel connection failed: 407 Proxy \
         Authentication Required'))",
        "<urlopen error Tunnel connection failed: 407 Proxy Authentication Required>",
        "HTTP Error 407: Proxy Authentication Required",
        "curl: (56) CONNECT tunnel failed, response 407",
    ] {
        let failure = proxy_auth_failed(output).expect(output);
        assert_eq!(failure.code.as_deref(), Some("proxy.auth_failed"));
        assert!(!failure.category.is_retryable(), "{output}");
    }
    for output in [
        "HTTPError 503",
        "ERROR: [generic] Unable to download episode 407: HTTP Error 404: Not Found",
        "Unable to connect to proxy: response 4070",
        "Unable to connect to proxy: [Errno 111] Connection refused",
    ] {
        assert!(proxy_auth_failed(output).is_none(), "{output}");
    }
}

async fn source(directory: &Path) -> (ToolNetworkSource, rd_db::Database, rd_secrets::SecretStore) {
    let database = rd_db::Database::open(directory.join("tool-network.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let defaults = Arc::new(tokio::sync::RwLock::new(rd_http::NetworkDefaults::default()));
    (
        ToolNetworkSource::new(database.clone(), secrets.clone(), defaults),
        database,
        secrets,
    )
}

async fn profile(database: &rd_db::Database, secret_ref: Option<String>) -> rd_core::ProxyProfile {
    database
        .create_proxy_profile(rd_db::NewProxyProfile {
            name: "office".to_owned(),
            kind: ProxyKind::Http,
            endpoint: endpoint("http://proxy.example:3128"),
            username: secret_ref.as_ref().map(|_| "alice".to_owned()),
            secret_ref,
        })
        .await
        .expect("proxy profile")
}

#[tokio::test]
async fn the_jobs_profile_is_resolved_with_its_password() {
    let directory = tempfile::tempdir().expect("directory");
    let (source, database, secrets) = source(directory.path()).await;
    let reference = secrets
        .put(SecretString::from(PASSWORD.to_owned()))
        .await
        .expect("secret");
    let profile = profile(&database, Some(reference)).await;
    let page = endpoint("https://video.example/watch");

    let network = source
        .resolve(None, Some(profile.id), &page)
        .await
        .expect("network");
    let proxy = network.proxy().expect("proxy");
    assert!(proxy.has_credentials());
    assert_eq!(network.proxy_argument(), None);

    // Without a profile on the job, account or service: no proxy at all.
    let network = source.resolve(None, None, &page).await.expect("network");
    assert!(network.proxy().is_none());
    assert!(network.trust_bundle().is_none());
}

#[tokio::test]
async fn the_global_profile_applies_when_the_job_names_none() {
    let directory = tempfile::tempdir().expect("directory");
    let (source, database, _secrets) = source(directory.path()).await;
    let profile = profile(&database, None).await;
    source.defaults.write().await.global_proxy_profile_id = Some(profile.id);

    let network = source
        .resolve(None, None, &endpoint("https://gallery.example/set/1"))
        .await
        .expect("network");
    assert_eq!(network.proxy_argument(), Some("http://proxy.example:3128/"));
}

#[tokio::test]
async fn a_proxy_that_cannot_be_resolved_fails_instead_of_going_direct() {
    let directory = tempfile::tempdir().expect("directory");
    let (source, database, _secrets) = source(directory.path()).await;
    let page = endpoint("https://video.example/watch");

    // A password the vault does not hold.
    let broken = profile(&database, Some("vault://missing".to_owned())).await;
    let error = source
        .resolve(None, Some(broken.id), &page)
        .await
        .expect_err("no network without the password");
    let failure = proxy_unavailable(&error);
    assert_eq!(failure.code.as_deref(), Some("proxy.unavailable"));
    assert!(!failure.category.is_retryable());

    // A profile that is gone.
    let error = source
        .resolve(None, Some(ProxyProfileId::new()), &page)
        .await
        .expect_err("no network without the profile");
    assert_eq!(
        proxy_unavailable(&error).code.as_deref(),
        Some("proxy.unavailable")
    );
}

/// RD-1240-22: a request outside the queue takes the global profile, and a profile that cannot
/// be used is the check's own failure, never a direct request.
#[tokio::test]
async fn a_request_outside_the_queue_takes_the_global_profile() {
    let directory = tempfile::tempdir().expect("directory");
    let (source, database, _secrets) = source(directory.path()).await;
    let page = endpoint("https://video.example/watch");

    let network = source.for_request(&page).await.expect("network");
    assert!(network.proxy().is_none());

    let global = profile(&database, None).await;
    source.defaults.write().await.global_proxy_profile_id = Some(global.id);
    let network = source.for_request(&page).await.expect("network");
    assert_eq!(network.proxy_argument(), Some("http://proxy.example:3128/"));

    let broken = profile(&database, Some("vault://missing".to_owned())).await;
    source.defaults.write().await.global_proxy_profile_id = Some(broken.id);
    let failure = source
        .for_request(&page)
        .await
        .expect_err("no network without the password");
    assert_eq!(failure.code.as_deref(), Some("proxy.check_unavailable"));
    assert!(!failure.category.is_retryable());
}
