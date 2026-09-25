//! Routing torrent peer traffic through a proxy, and mapping the listen port.
//!
//! librqbit takes a single SOCKS5 URL and applies it to **outgoing peer connections only**.
//! Tracker announces and metadata fetches go out over the engine's own HTTP path and are
//! not covered, and there is no way to give the three traffic classes separate proxies.
//! That is a real limitation and it is reported as one — `per_class_proxy` and
//! `tracker_proxy` are `false` in the capability matrix and the UI says so next to the
//! control, rather than letting a user believe their tracker traffic is proxied when it is
//! not.
//!
//! Port mapping is UPnP IGD only; NAT-PMP and PCP do not exist in this engine.

use anyhow::{Context, Result, bail};
use rd_core::ProxyKind;
use secrecy::ExposeSecret;

use crate::TorrentService;

impl TorrentService {
    /// Builds the SOCKS5 URL librqbit expects, or `None` when no proxy is configured.
    ///
    /// Reuses the existing proxy profiles rather than adding a torrent-specific credential
    /// store: the password stays in the vault and only the assembled URL, which never
    /// leaves this process, carries it.
    pub(crate) async fn proxy_url(&self) -> Result<Option<String>> {
        let Some(profile_id) = self.inner.settings.read().await.torrent_proxy_profile_id else {
            return Ok(None);
        };
        let Some(secrets) = self.inner.secrets.as_ref() else {
            // No vault wired up (as in tests): a proxy that cannot be resolved must not
            // silently turn into a direct connection.
            bail!("the secret store is not available, so the torrent proxy cannot be used");
        };
        let profile = self
            .inner
            .database
            .proxy_profile(profile_id)
            .await?
            .context("torrent proxy profile disappeared")?;
        if !matches!(profile.kind, ProxyKind::Socks5) {
            bail!("the torrent proxy must be SOCKS5");
        }
        let host = profile
            .endpoint
            .host_str()
            .context("SOCKS5 proxy has no hostname")?
            .to_owned();
        let port = profile
            .endpoint
            .port_or_known_default()
            .context("SOCKS5 proxy has no port")?;
        let credentials = match (profile.username.as_deref(), profile.secret_ref.as_deref()) {
            (Some(username), Some(reference)) => {
                let password = secrets.get(reference).await?;
                Some(format!(
                    "{}:{}@",
                    urlencode(username),
                    urlencode(password.expose_secret())
                ))
            }
            (Some(username), None) => Some(format!("{}@", urlencode(username))),
            _ => None,
        };
        Ok(Some(format!(
            "socks5://{}{host}:{port}",
            credentials.unwrap_or_default()
        )))
    }
}

/// Percent-encodes the characters that would otherwise break a userinfo component.
fn urlencode(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => character.to_string(),
            other => {
                let mut buffer = [0_u8; 4];
                other
                    .encode_utf8(&mut buffer)
                    .bytes()
                    .map(|byte| format!("%{byte:02X}"))
                    .collect()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::urlencode;

    #[test]
    fn userinfo_special_characters_are_encoded() {
        assert_eq!(urlencode("user"), "user");
        // A password with `@` or `:` would otherwise split the URL in the wrong place.
        assert_eq!(urlencode("p@ss:word"), "p%40ss%3Aword");
        assert_eq!(urlencode("a b"), "a%20b");
    }

    #[test]
    fn unreserved_characters_survive_untouched() {
        assert_eq!(urlencode("aZ0-._~"), "aZ0-._~");
    }
}
