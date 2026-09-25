# Running behind a reverse proxy

rDownloader binds to loopback by default and provides no TLS of its own. Remote access means
putting a proxy in front, and that proxy has to be told about — otherwise four things go wrong
in ways that never look like a configuration problem.

## What goes wrong when it is not configured

| Symptom | Cause |
| --- | --- |
| Everyone shares one rate limit; the session list shows one address for every device | No trusted proxy is configured, so the proxy's own address is treated as the client's |
| Signing in appears to work, and the next page is the login screen again | The session cookie is `Secure` but the deployment is plain HTTP, so the browser drops it |
| The page loads blank under a sub-path | The mount point is not configured, so the asset references point at the root |
| Anyone can claim to be any address | A forwarded header is believed from a peer that is not a configured proxy — this one is why nothing is trusted by default |

## The settings

All three live under **Settings → Security**, or in the settings document as
`trusted_proxies`, `external_url` and `cookie_security`. `rdownloader doctor` prints the
resolved contract and warns about half-configured combinations.

**`external_url`** — what the outside world calls this installation, scheme and path included:
`https://rd.example.com` or `https://home.example.com/downloads`. One setting rather than
three, so the origin and the mount point cannot disagree with each other.

**`trusted_proxies`** — the address ranges whose `X-Forwarded-For` is believed, as CIDR or bare
addresses: `127.0.0.1`, `10.0.0.0/8`, `fd00::/8`. Anything not in this list has its forwarded
headers ignored entirely and is treated as the client itself. Empty is the safe default.

**`cookie_security`** — `auto` follows the scheme of the external URL and is right for almost
every deployment. `always` is for a proxy that terminates TLS but is described by an `http`
URL; `never` is for a plain-HTTP LAN deployment, where a `Secure` cookie would simply be
dropped.

## nginx

```nginx
server {
    listen 443 ssl;
    server_name rd.example.com;

    location / {
        proxy_pass http://127.0.0.1:8710;
        proxy_set_header Host              $host;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;   # not read; the scheme comes from external_url

        # The event stream is a long-lived response. Without these, updates arrive in
        # bursts when a buffer fills, or the connection is closed after 60 seconds.
        proxy_buffering off;
        proxy_read_timeout 1h;
        proxy_http_version 1.1;
    }
}
```

Settings: `external_url = https://rd.example.com`, `trusted_proxies = 127.0.0.1`.

Under a sub-path, keep the same path on both sides — rDownloader strips its own mount point,
so the proxy must not strip it as well:

```nginx
location /downloads/ {
    proxy_pass http://127.0.0.1:8710;   # no trailing path: the prefix is passed through
    # …the same headers as above
}
```

Settings: `external_url = https://rd.example.com/downloads`.

## Caddy

```caddyfile
rd.example.com {
    reverse_proxy 127.0.0.1:8710 {
        flush_interval -1   # do not buffer the event stream
    }
}
```

Caddy sets `X-Forwarded-For` itself (and `X-Forwarded-Proto`, which rDownloader does not read —
the scheme comes from `external_url`). Settings:
`external_url = https://rd.example.com`, `trusted_proxies = 127.0.0.1`.

## Traefik

```yaml
http:
  routers:
    rdownloader:
      rule: "Host(`rd.example.com`)"
      service: rdownloader
      tls: {}
  services:
    rdownloader:
      loadBalancer:
        servers:
          - url: "http://127.0.0.1:8710"
        responseForwarding:
          flushInterval: "1ms"   # the event stream again
```

In Docker, the trusted range is the container network rather than loopback — for the default
bridge that is `172.16.0.0/12`. `rdownloader doctor` prints the configured trusted range and its
warnings, but it observes no request; the address requests actually arrive from is in the
proxy's access log or `docker network inspect`.

## Passkeys need the external URL

A passkey is bound to the origin it was created at, and that binding is the whole of its
phishing resistance: the browser will not offer a credential registered for
`https://downloads.example.com` to a page served from anywhere else. rDownloader therefore takes
the relying party from `external_url` whenever it is set, and from the request only when it is
not and the browser's `Origin` is loopback — a `Host` or `Origin` header the caller chooses would
otherwise let the caller choose what the credential protects.

Two consequences worth knowing before somebody debugs them:

- **Without `external_url`, passkeys work only at `http://localhost:<port>`.** That is not a
  loophole: a browser will not send that origin from another site's page, and a name that merely
  ends in something similar (`localhost.example.com`) does not match. Reaching the service at
  `http://127.0.0.1:8710` does *not* work — a relying party id must be a domain, and browsers
  reject an address. rDownloader refuses with that explanation rather than passing it on.
- **Changing `external_url` invalidates existing passkeys.** They were registered against the
  old origin and the browser will not offer them at the new one. The password still signs in;
  enrol the passkeys again afterwards. Moving from a bare host name to a subdomain, or from
  `http` to `https`, both count as a change.

Nothing else about passkeys depends on the proxy. They work under a base path, and the
`X-Forwarded-For` settings do not affect them.

## What is deliberately not supported

**Authentication at the proxy as a replacement for rDownloader's own.** A proxy that injects
an identity header would have to be trusted absolutely, and any request reaching the service
directly — a container on the same network, a misconfigured firewall — would then be
unauthenticated. The administrator password stays the credential.

**Path rewriting in the proxy.** Stripping the prefix at the proxy and having the application
believe it is at the root works until something generates an absolute URL, and then it breaks
in a way that is hard to trace. Pass the prefix through and set `external_url` instead.
