# Authentication Profiles: Scope and Secret Boundaries

An authentication profile lets rDownloader reuse a browser session, or send HTTP
credentials, to one explicitly approved domain. This document records where the boundaries
are — and, just as importantly, where they are not.

## The model

A profile is one **scope** plus exactly one **method**, and optionally a **client
certificate**:

| Field | Meaning |
|---|---|
| `host` | Normalised host: lowercased, IDNA/punycode, no trailing dot |
| `include_subdomains` | Whether `*.host` is covered too |
| `path_prefix` | Optional; selects *which* profile matches (see below) |
| `method` | `cookies`, `basic` or `bearer` |
| `certificate_ref` | Optional client certificate, independent of the method |
| `expires_at` | Optional; an expired profile is listed but never applied |
| `origin` | `manual` or `browser_capture` |
| `enabled` | Whether the profile may be used at all |

Normalisation goes through `Url::parse`, which supplies IDNA encoding, lowercasing and
default-port removal. Unlike the other host helpers in this codebase it does **not** strip
`www.`: a session for `www.example.com` is not a session for `example.com`.

## What is enforced, and what is not

**Host containment is real.** A profile is only selected for a URL whose host equals its
scope host, or is a subdomain of it when `include_subdomains` is set. The leading dot in
the subdomain check is what keeps `evil-example.com` and `example.com.evil.tld` from
matching a scope of `example.com`.

**Path containment is not.** `path_prefix` decides which profile applies to the *initial*
URL and nothing more. No HTTP mechanism keeps a credential from travelling to another path
of the same host across a redirect, and cookies do not treat paths as a security boundary
at all. Treat `path_prefix` as a selector, not a fence.

**Redirects are contained per method**, because the three methods have genuinely different
exposure:

| Method | What contains it | Redirects followed? |
|---|---|---|
| Cookies | The cookie jar refuses to emit for a foreign host | Yes |
| Basic / Bearer | reqwest strips `Authorization` on any host, port or scheme change | Yes |
| Client certificate | Nothing — it is a property of the client and is offered during the TLS handshake, before any of our code runs again | **No**, only within the request's own origin |

Confining *every* profile to its scope would break ordinary downloads: hosters routinely
hand the payload off to a foreign CDN, and the transfer has to follow. So only the
certificate case installs a bounded redirect policy, and its bar is the request's own
origin (scheme, host, port) rather than the profile scope — a scope may cover subdomains,
and offering a client certificate to a sibling host is exactly the leak being prevented.
That is also the definition of "cross-origin" reqwest itself uses when stripping headers.

## Secret boundaries

Credential values live only in the encrypted secret store (`rd-secrets`,
XChaCha20-Poly1305, one `0600` file per secret, master key in the OS keyring with a file
fallback). Everything else holds an opaque `vault://<uuid>` reference.

- `AuthProfile::secret_ref` and `certificate_ref` are `#[serde(skip_serializing)]` and
  `#[schema(ignore)]`, so they appear in neither a REST response nor the OpenAPI document.
  The API exposes `has_secret` / `has_client_certificate` booleans instead.
- Request DTOs mark every credential field `#[schema(write_only)]`. Editing a profile with
  an empty credential field keeps the stored value; it is never sent back to the client to
  be echoed.
- In-flight plaintext is held as `secrecy::SecretString`, whose `Debug` redacts.
- SSE carries only `{"resource": "auth_profile"}` — no profile fields at all.
- A settings backup exported **without** secrets contains the profile but no slot and no
  reference. With secrets, values live in the encrypted blob and the bundle body carries
  only anonymous slot names (`s0`, `s1`, …), re-minted into fresh references on import.

## Client certificates

The certificate is one PEM bundle containing the private key and the chain — the format
`reqwest::Identity::from_pem` expects, and ASCII, which the secret store requires. It is
parsed at save time, not at first use, so an unusable bundle is reported while the user is
still looking at the form.

Passphrase-protected keys (`ENCRYPTED PRIVATE KEY`) are refused with their own error code:
nothing in this pipeline can decrypt them. DER and PKCS#12 are not supported.

## Selection per job

A download stores one of three states, as an id plus a pinned flag:

- **auto** — the most specific enabled, unexpired profile whose scope matches wins
  (most host labels, then longest path prefix, then oldest id as a stable tiebreaker);
- **none** — send nothing, even where a profile would match;
- **pinned** — use exactly this profile.

The two modes fail differently on purpose. A pinned profile that is disabled, expired or
out of scope **aborts the job**: continuing unauthenticated would write a login page over
the user's file. Deleting a profile clears the pin instead, and the job falls back to
automatic matching. Auto-matching silently finds nothing instead, because it
is a convenience and a stale session must not turn every download of that host into a hard
failure.

An account already carries its own provider-scoped cookie jar, so a domain profile is not
layered on top of one. Resolver plugins likewise keep using provider accounts and never
see auth profiles — a profile applies to the transfer, not to the resolve step.

## Cookies from the browser

The extension's page context menu offers to share the current site's session. It requests
the `cookies` permission together with a host permission for that **one** origin, from the
user's own click, so:

- installing the extension warns about no cookie access at all (`cookies` is an
  *optional* permission);
- the browser renders the consent prompt itself, per origin and revocable;
- nothing is read before the grant, and the grant is dropped again right after the read.

Cookies are posted to `POST /api/v1/capture/cookies` on the capture router. A capture token
lives in a browser, so it must not be able to mint a usable credential: profiles created
this way always land with `enabled = false` and `origin = browser_capture`, enforced in the
store layer rather than the handler, and are only used once a person approves them in the
web UI. The payload is validated against the approved domain before anything is stored, and
a Netscape import inherits the browser's own earliest cookie expiry.

## Which hosts an imported cookie reaches

One rule, `rd_http::CookieScope`, decides it for every import: a profile's cookies, an
account's cookies and the browser handover below (RD-120-49). A cookie reaches the scope's
host, plus its subdomains when the scope includes them, and nothing wider — whatever domain
it arrived with.

- **A row's domain must be the scope's host or a domain above it.** Anything else refuses the
  whole set.
- **A domain above the scope must not be a public suffix** — `com`, `co.uk`, `github.io`, or a
  single label such as `lan`. The check reads Mozilla's Public Suffix List, compiled into the
  `psl` crate; there is no runtime fetch, so a list update is a dependency update. A name the
  list does not know counts as its own suffix. The refusal has its own code,
  `authprofile.cookie_public_suffix` or `browser_session.cookie_public_suffix`. The scope's host
  itself is let through even when it is one, as an intranet `nas` is, and stays host-only.
- **A profile is checked when it is saved, not first at the download** (RD-120-54). Creating,
  editing and capturing a cookie profile run the same import against the profile's scope and
  refuse with `authprofile.cookie_public_suffix` or `authprofile.cookie_outside_scope`; an edit
  that changes the host or the subdomain setting re-checks the cookies it keeps.
- **A row for a domain above the scope is stored for the scope's host**, not for the domain it
  came with, so `.example.com` in a profile for `www.example.com` never reaches
  `dl.example.com`. It is accepted at all because the browser returns a parent domain's cookies
  for a page (`cookies.getAll({ url })` in `extension/src/session.js`, RD-109-22).
- **`include_subdomains` decides the cookie's reach**, for Netscape rows and header cookies
  alike: without it no `Domain=` is written and the jar keeps the cookie host-only. A provider's
  `cookie_scope` always includes subdomains, which is where hosters keep their API and download
  servers.

The yt-dlp cookie file for media downloads (`rd-media/src/cookies.rs`, RD-120-52) asks the same
rule, built from the profile the way the HTTP engine builds it: a row that
`CookieScope::admit` refuses is left out of the file, and every row that stays is written for
the scope's host with the reach `CookieScope::reaches_subdomains` gives it — header cookies
included, which `rd_core::cookie_file` parses with subdomains. Leaving a refused row out instead
of refusing the set is the one difference, and a deliberate one: stripping a browser export's
foreign rows is what that file's filter has always been for. A profile left with nothing for
the page reports `media.cookie_scope_empty`.

## A browser session for a provider account

A profile never reaches a resolver, so it cannot help a hoster sign-in that the browser skips
because it is logged in already (RD-120-45). That case has its own path, and it keeps the rule
above — a capture token must not be able to mint a usable credential — by splitting the act in
two:

- A person opens the request at the account (`POST /api/v1/accounts/{id}/browser-session`,
  secrets scope). The site is the provider's `cookie_scope` from the installed plugin's manifest,
  `https` only; there is no parameter for it.
- The extension answers with the capture token: it lists the waiting requests, and after the
  person's click in its popup and the browser's own prompt posts that one host's cookies to
  `POST /api/v1/capture/browser-sessions/{id}`. The service refuses the whole set when one
  Netscape row lies outside the scope or is set for a public suffix above it (the rule above,
  `rd_http::CookieScope::admit`, which the jar applies again when it is loaded), and stores it
  in the vault behind `accounts.cookie_ref` — where cookies typed into the account form go —
  with the other account fields unchanged.

A request waits five minutes, is answered once, lives in memory only, and is not reachable
over MCP. The extension's side of it is described in `extension/README.md`.
