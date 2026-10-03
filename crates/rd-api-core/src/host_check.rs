//! The `Host` a request names, checked before anything routes on it (security review
//! 2026-09-28, finding 3).
//!
//! A DNS rebinding page reaches this service under the page's own name. The decision which
//! names pass is `rd_authn::host`; this is the plumbing that asks it for every request — the
//! API, the compatibility surfaces, MCP and the web interface's own files alike, because the
//! page an attacker loads under their name would otherwise be this service's interface.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{ApiError, AppState, client::ListenAddress, dto::SettingsResponse};

/// The proxy contract a settings document describes, the allowed host list included.
///
/// The one place the four fields become a [`rd_authn::ProxyConfig`], so the validation on save,
/// the live apply and the start cannot build it three different ways.
pub fn proxy_config(
    settings: &SettingsResponse,
) -> Result<rd_authn::ProxyConfig, rd_authn::ProxyConfigError> {
    rd_authn::ProxyConfig::parse(
        &settings.trusted_proxies,
        settings.external_url.as_deref(),
        settings.cookie_security,
    )?
    .with_allowed_hosts(&settings.allowed_hosts)
}

/// Applies the stored proxy contract before the listener opens.
///
/// Until now only a saved settings document set it, so after a restart the service knew no
/// external URL, no trusted proxy and no mount point until somebody saved again. With the host
/// check that gap would refuse the external URL's own name, so it is closed here.
pub async fn load(state: &AppState) {
    match crate::settings_store::stored_settings(&state.database).await {
        Ok(settings) => match proxy_config(&settings) {
            Ok(config) => *state.proxy.write().await = config,
            Err(error) => {
                tracing::warn!(%error, "the stored proxy configuration is not usable");
            }
        },
        Err(error) => {
            tracing::warn!(code = %error.code(), "the stored settings could not be read");
        }
    }
}

/// Refuses a request whose `Host` or URI authority names this service by a name it does not
/// answer to.
///
/// A request without either passes: every browser sends `Host`, so its absence means a client
/// that is not a browser, and a rebinding attack needs one. The refusal names the host, so
/// whoever typed it can add it to the list: as a parameter of the JSON error, and on a small
/// page with the way to the setting when a browser opens it (RD-190-17).
pub async fn require_known_host(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // Owned before anything is awaited: a `&Request` held across the lock below would make
    // this middleware's future non-`Send` (see `auth::RequestFacts`).
    let mut named = Vec::with_capacity(2);
    if let Some(authority) = request.uri().authority() {
        named.push(authority.host().to_owned());
    }
    if let Some(value) = request.headers().get(header::HOST) {
        // A value that is not text cannot be a name this service answers to.
        named.push(value.to_str().unwrap_or_default().to_owned());
    }
    let refused = {
        let proxy = state.proxy.read().await;
        named.into_iter().find(|host| !proxy.host_allowed(host))
    };
    let Some(host) = refused else {
        return next.run(request).await;
    };
    tracing::warn!(
        %host,
        "a request named a host this service does not answer to; add it to the allowed hosts \
         if it is yours"
    );
    if wants_page(&request) {
        let local = request
            .extensions()
            .get::<ListenAddress>()
            .map_or(FALLBACK_ADDRESS, |ListenAddress(bound)| {
                crate::local_control::reachable(*bound)
            });
        return refusal_page(&host, local);
    }
    refusal(&host).into_response()
}

/// Where the page sends the operator when the listener's address is unknown: the default one.
const FALLBACK_ADDRESS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8710);

/// The refusal a client reads: the code, and the host it named so the message can repeat it.
fn refusal(host: &str) -> ApiError {
    ApiError::forbidden(
        "request.host_not_allowed",
        "This service does not answer to this host name; open it by its address or add the \
         name to the allowed hosts in the settings",
    )
    .with_param("host", host)
}

/// Whether the caller is a browser opening a page rather than a client reading JSON.
///
/// A navigation asks for `text/html`; the interface's own `fetch` calls and every API client
/// do not, so they keep the coded JSON refusal.
fn wants_page(request: &Request) -> bool {
    request
        .headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

/// The refusal a browser shows: which name was refused, where to allow it and from where.
///
/// Small and self-contained on purpose: the interface's bundle is refused under this name
/// like everything else, and the page must not load anything from it (RD-190-17). The host is
/// whatever the caller sent, so it only ever reaches the page escaped.
fn refusal_page(host: &str, local: SocketAddr) -> Response {
    let named = escape_html(&rd_core::redact_text(host));
    // The entry the list takes is the bare name; a value that is no name at all gets none.
    let entry = match rd_authn::host::parse_request_host(host) {
        Some(rd_authn::host::RequestHost::Name(name)) => format!(
            "<p>The entry to add is <code>{}</code> — without a scheme or port.</p>",
            escape_html(&name)
        ),
        _ => String::new(),
    };
    let body = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Host name not allowed</title>
<style>
body {{ font-family: system-ui, sans-serif; max-width: 40rem; margin: 3rem auto; padding: 0 1rem; line-height: 1.5; color: #1f2937; background: #ffffff; }}
code {{ background: #f3f4f6; padding: 0.1rem 0.3rem; border-radius: 0.25rem; word-break: break-all; }}
small {{ color: #6b7280; }}
@media (prefers-color-scheme: dark) {{
  body {{ color: #e5e7eb; background: #111827; }}
  code {{ background: #1f2937; }}
  small {{ color: #9ca3af; }}
}}
</style>
</head>
<body>
<h1>rDownloader does not answer to this host name</h1>
<p>This request named <code>{named}</code>, which is not on the list of host names this service answers to. The check keeps pages on other sites from reaching the service under their own name (DNS rebinding).</p>
<p>If the name is yours — a reverse proxy, a Cloudflare Tunnel, a name in your own domain — add it under <strong>Settings → Security → Reverse proxy → Allowed host names</strong>, or set the address it is reached at as the external URL there.</p>
{entry}
<p>Make the change from an address the service always answers to, such as <code>http://{local}</code> on the machine it runs on, or its LAN address.</p>
<p><small>Code: request.host_not_allowed</small></p>
</body>
</html>
"#
    );
    (
        StatusCode::FORBIDDEN,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            // Nothing to load and nothing to run: only the inline style above.
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'",
            ),
        ],
        body,
    )
        .into_response()
}

/// Escapes text for an HTML element's content or a quoted attribute.
fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_names_the_host() {
        let refused = refusal("rd.example.com").into_message();
        assert_eq!(refused.code, "request.host_not_allowed");
        assert_eq!(
            refused.params.get("host").map(String::as_str),
            Some("rd.example.com")
        );
    }

    #[test]
    fn markup_in_a_host_is_escaped() {
        assert_eq!(
            escape_html(r#"<script>alert("x")</script>&'"#),
            "&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;&amp;&#39;"
        );
    }
}
