use anyhow::{Context, Result, bail};
use rd_core::{ProxyKind, UsenetServerId};
use rd_db::Database;
use rd_secrets::SecretStore;

use crate::{NntpServerConfig, Socks5Proxy};

const MAX_ARTICLE_BYTES: usize = 128 * 1024 * 1024;

/// Resolves one redaction-safe database record into a connection configuration.
///
/// The endpoint is trusted against the platform store alone. Callers that know the operator's
/// custom CA - everything on the download path does, through the scheduler's network defaults -
/// use [`server_config_with_ca`] instead, so a news server is trusted by the same rule as an
/// HTTP host and an FTPS server.
pub async fn server_config(
    database: &Database,
    secrets: &SecretStore,
    id: UsenetServerId,
) -> Result<Option<NntpServerConfig>> {
    server_config_with_ca(database, secrets, id, &[]).await
}

/// [`server_config`] with the operator's custom CA bundles, in the `Vec<Vec<u8>>` shape
/// `rd_http::NetworkDefaults` carries them.
pub async fn server_config_with_ca(
    database: &Database,
    secrets: &SecretStore,
    id: UsenetServerId,
    custom_ca_pem: &[Vec<u8>],
) -> Result<Option<NntpServerConfig>> {
    let Some(stored) = database.usenet_connection_config(id).await? else {
        return Ok(None);
    };
    if !stored.server.enabled {
        return Ok(None);
    }
    let password = match stored.password_ref {
        Some(reference) => Some(secrets.get(&reference).await?),
        None => None,
    };
    let proxy = socks_proxy(database, secrets, stored.server.proxy_profile_id).await?;
    Ok(Some(NntpServerConfig {
        host: stored.server.host,
        port: stored.server.port,
        tls: stored.server.tls,
        custom_ca_pem: custom_ca_pem.to_vec(),
        username: stored.server.username,
        password,
        proxy,
        max_article_bytes: MAX_ARTICLE_BYTES,
        max_connections: stored.server.max_connections,
    }))
}

/// Loads all enabled servers in persisted priority order, trusting `custom_ca_pem` beside the
/// platform store on every one of them.
pub async fn enabled_server_configs(
    database: &Database,
    secrets: &SecretStore,
    custom_ca_pem: &[Vec<u8>],
) -> Result<Vec<NntpServerConfig>> {
    let mut configs = Vec::new();
    for server in database.list_usenet_servers().await? {
        if server.enabled
            && let Some(config) =
                server_config_with_ca(database, secrets, server.id, custom_ca_pem).await?
        {
            configs.push(config);
        }
    }
    Ok(configs)
}

/// What the enabled connection settings are, as one value that changes whenever they do.
///
/// A pool that outlives a single file (RD-108-26) has to be given up when the settings behind
/// it change. The password is represented by its reference, never its value: `SecretStore::put`
/// mints a fresh reference per stored secret, so a changed password changes the fingerprint
/// without the secret ever leaving the store.
pub async fn connection_fingerprint(database: &Database) -> Result<String> {
    let mut parts = Vec::new();
    for server in database.list_usenet_servers().await? {
        if !server.enabled {
            continue;
        }
        let Some(stored) = database.usenet_connection_config(server.id).await? else {
            continue;
        };
        parts.push(format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            stored.server.host,
            stored.server.port,
            stored.server.tls,
            stored.server.username.unwrap_or_default(),
            stored.password_ref.unwrap_or_default(),
            stored
                .server
                .proxy_profile_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            stored.server.priority,
            stored.server.max_connections,
        ));
    }
    Ok(parts.join(";"))
}

async fn socks_proxy(
    database: &Database,
    secrets: &SecretStore,
    proxy_id: Option<rd_core::ProxyProfileId>,
) -> Result<Option<Socks5Proxy>> {
    let Some(proxy_id) = proxy_id else {
        return Ok(None);
    };
    let proxy = database
        .proxy_profile(proxy_id)
        .await?
        .context("NNTP proxy profile disappeared")?;
    if !matches!(proxy.kind, ProxyKind::Socks5) {
        bail!("NNTP proxy must be SOCKS5");
    }
    let host = proxy
        .endpoint
        .host_str()
        .context("SOCKS5 proxy has no hostname")?
        .to_owned();
    let port = proxy
        .endpoint
        .port_or_known_default()
        .context("SOCKS5 proxy has no port")?;
    let password = match proxy.secret_ref {
        Some(reference) => Some(secrets.get(&reference).await?),
        None => None,
    };
    Ok(Some(Socks5Proxy {
        host,
        port,
        username: proxy.username,
        password,
    }))
}
