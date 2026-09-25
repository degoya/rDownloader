# Plugins: Building, Signing, Shipping

rDownloader loads hoster/multihoster resolvers and transfer backends as WebAssembly components from signed `.rdplug` packages. A plugin is self-contained: its manifest declares the provider it serves, and it ships its own translations, so a third party can publish a resolver for a **new** hoster without any change to this repository.

Twenty-seven resolvers are built this way and shipped with every release (counted from `plugins/*/manifest.toml` on 2026-09-23): hosters DDownload, Rapidgator, Nitroflare, KatFile, 1fichier, Keep2Share, FileJoker, MediaFire, KrakenFiles, Turbobit, HitFile, Pixeldrain and TorBox, the cloud drives Google Drive, OneDrive/SharePoint, Dropbox, Box and pCloud, Put.io and Seedr with their own torrent clients, multihosters Premiumize, AllDebrid, Debrid-Link, LinkSnappy, Real-Debrid and Offcloud, and the account-less XFileSharing resolver. Fifteen of them additionally have natively compiled-in variants that remain active as a fallback -- for a revoked key, a damaged package, or the first start before the bundled packages are installed; the newer ones (Real-Debrid, Box, the cloud drives, TorBox, Put.io, Offcloud, Pixeldrain, Seedr) and XFileSharing ship as components only, and the fifteen are the ones `crates/rd-plugin-host/src/native/mod.rs` registers. Installed components take precedence. Both builds of a plugin read the same `manifest.toml`, so they report one identity and one version and are confined by the same capability grants. The release packages every `plugins/` directory that carries a `manifest.toml` -- 72 in all: the resolvers plus the transfer, intake, authentication, OAuth, crawler, enricher, notifier, post-processing, storage, remote-job and stream-transform plugins described below.

## Package layout

A `.rdplug` is a ZIP with exactly these members; anything else is rejected:

| Member | Required | Limit |
|---|---|---|
| `manifest.toml` | yes | 1 MiB |
| `component.wasm` | yes | 64 MiB |
| `signature.ed25519` | yes outside development mode | 1 KiB |
| `locales/<lang>.json` | optional, `en` required if any | 256 KiB each, 16 files |

The signature covers all of it: the signed digest is `len‖manifest ‖ len‖component ‖` every locale file by name, so translations are exactly as tamper-evident as the code.

## Manifest (version 3)

```toml
manifest_version = 3
plugin_type = "resolver"                       # resolver | transfer
api_version = "0.7.0"                          # rdownloader:plugin WIT version you built against
id = "019d0000-0000-7000-8000-000000000003"   # UUIDv7, stable across versions
name = "FastShare"                             # default display name
version = "1.0.0"                              # strict SemVer
key_id = "fastshare-release-v1"                # names your signing key
public_key = "…base64 Ed25519…"                # the key's public half (see Trust)

match_domains = ["fastshare.example"]          # links this plugin claims
download_domains = ["*.fastshare.example"]     # allowlist for the resolved file URL
secret_fragment_domains = []                   # hosts whose link fragment is key material
max_concurrent_downloads = 5
requires_account = true                        # false opts into account-less (free) resolving

[capabilities]                                 # every permission, in one place
cookies = true                                 # read the account's cookies for cookie_scope
captcha = true                                 # hand a challenge to the solver or the user
secrets = ["fastshare_password"]               # {{secret:…}} markers you may expand

[capabilities.net_http]
domains = ["fastshare.example", "*.fastshare.example", "api.fastshare.example"]

[metadata]
description = "Resolves FastShare links with a premium account."  # required
author = "FastShare GmbH"                      # required
homepage = "https://fastshare.example"         # optional, https
support_url = "https://fastshare.example/help" # optional, https
license = "MIT"                                # optional, SPDX
min_app_version = "0.7.0"                      # optional; refuses older cores

[provider]
slug = "fastshare"              # ^[a-z0-9_]{2,32}$; stored as accounts.provider
kind = "hoster"                 # or "multihoster"
credentials = "username_password"  # api_key | username_password | api_key_or_cookies | cookies | login_or_api_key | oauth | none
username_required = true
secret_reference = "fastshare_password"       # must appear in capabilities.secrets
secret_domains = ["api.fastshare.example"]    # hosts the credential may reach
transfer_auth = "basic"         # optional; the engine sends Basic username:secret to secret_domains on the transfer
cookie_scope = "https://fastshare.example/"   # optional, https; needs the cookies capability
host_aliases = [["fs.example", "fastshare.example"]]

# Only for credentials = "login_or_api_key": one slot per mode, replacing the singular
# secret_reference/secret_domains pair above. Each mode's credential stays pinned to its own
# hosts, and the first entry is the default an account stored before the choice falls back to.
[[provider.secrets]]
reference = "fastshare_password"
domains = ["fastshare.example"]
mode = "login"

[[provider.secrets]]
reference = "fastshare_api_key"
domains = ["api.fastshare.example"]
mode = "api_key"

[limits]
memory_bytes = 67108864
fuel = 2000000000
timeout_milliseconds = 15000
max_response_bytes = 8388608
wait_budget_milliseconds = 600000              # optional; waiting only, not compute time
```

### A link fragment that is key material

`secret_fragment_domains` is empty in almost every manifest, and that is the point. Intake
strips the fragment off **every** address before a candidate row exists, because nothing
distinguishes a share password from an anchor name and a password in `link_candidates.url`
would be printed in every LinkGrabber row. A provider that encrypts on the client cannot live
with that: its file key rides in exactly that fragment and never reaches the provider at all.

Naming the hosts here is how a plugin says so. For an address on one of them the intake puts
the fragment in the credential vault and writes only the reference beside the row; at resolve
time the host reads it back and restores it onto the address one call before it hands it to
the plugin, the same way it expands a `{{secret:…}}` marker on the way out. The plugin sees
the address it would have seen if nothing had ever been stripped, and no row, event, log
record, diagnostic bundle, OpenAPI answer or interface ever carries the key.

```toml
secret_fragment_domains = ["mega.nz", "mega.co.nz"]
```

Four things are worth knowing before reaching for it:

- **Write it above the first `[table]` header.** It is a top-level key, and a bare key written
  after `[capabilities.net_http]` belongs to that table instead — where it is silently ignored.
- **The catch-all `*` is refused.** An exact host or `*.suffix` for its sub-domains, nothing
  else: `*` would turn every fragment on the internet into vaulted key material.
- **Declare it once, per host, not per world.** A crawler that emits child links of the same
  provider needs no declaration of its own — the address decides, not the plugin that produced
  it. `plugins/mega` declares the hosts and `plugins/mega-crawler` does not.
- **The secret is owned by exactly one row.** It moves from the LinkGrabber candidate to the
  queue entry when the link is enqueued, and it is removed from the vault when the row that
  owns it is deleted. Nothing else keeps a copy.

Without the declaration nothing changes and the fragment is dropped as it always was. See
`docs/adr/0011-mega-a-stream-the-host-must-decrypt.md` for the provider this exists for.

### Capabilities

Each grant is one entry here and one WIT interface in the host, and a component may import
only the interfaces its manifest names. There is no way to ask for something at runtime: a
component that imports an interface it was not granted is refused when the package is built
and again when it is installed, and it cannot be instantiated at all.

| Capability | Interface | What it allows |
|---|---|---|
| *(always)* | `rdownloader:plugin/host` | `log`, `wait`, `secret-available`, `now-unix-seconds`, `random-bytes` |
| *(plugin type)* | `rdownloader:plugin/sink` | writing into this transfer's own destination |
| `net_http` | `rdownloader:plugin/http` | HTTPS requests, confined to `capabilities.net_http.domains` |
| `cookies` | `rdownloader:plugin/cookies` | reading the account's cookies for one URL |
| `captcha` | `rdownloader:plugin/captcha` | handing a challenge to the configured solver or the user |
| `secrets` | *(no interface)* | expanding the listed `{{secret:…}}` and `{{basic:…}}` markers |
| `net_stream` | `rdownloader:plugin/net` | raw TCP/TLS to the declared hosts and ports (transfer backends and storage destinations) |
| `key_derivation` | `rdownloader:plugin/key-derivation` | asking the host to compute over a credential you named but never see |

### Sending a credential you never see

A request template is where a plugin names a credential without holding one. Four markers are
expanded by the host, in the query, the headers and a UTF-8 body, and the substituted value is
encoded for the document it lands in -- JSON-escaped for `application/json`, percent-encoded for
`application/x-www-form-urlencoded`; query and header values are encoded by the HTTP layer
itself.

| Marker | Expands to | Gates |
|---|---|---|
| `{{secret:<reference>}}` | the account's secret in that slot | the slot has to be the account's active one and to allow this address |
| `{{secret}}` | the one secret this invocation was granted | for plugin types with no provider account behind them; also expands inside the request *address*, for a webhook whose token is part of its path |
| `{{username}}` | the account's stored username | credential material, so the same domain gate as the secret |
| `{{basic:<reference>}}` | base64 of `username:secret`, for `Authorization: Basic …` | both of the above at once |

**One request may name several references, and each marker gets its own** (RD-120-39). An OAuth
renewal with an application the person registered has to: it sends the client secret and the
refresh material side by side. Every distinct `{{secret:…}}`/`{{basic:…}}` reference is loaded
on its own and passes its own gate; one that fails refuses the whole request before anything is
sent, and a marker whose reference was not loaded is `plugin.secret_missing`, never a
neighbour's value. The same reference may appear any number of times. At most four distinct
references per request, above that `plugin.secret_references_exceeded`.

**`{{basic:<reference>}}` exists for a provider whose whole API is HTTP Basic** (RD-120-04), and
it is the only marker that could not have been composed out of the others. `Authorization: Basic
{{username}}:{{secret:…}}` sends the pair unencoded, and a guest cannot encode them itself
because a marker substitutes on the way *out* -- a guest can send a credential, it can never
compute with one. So the host does the one thing a guest cannot and no more than that: what
comes back is base64 of `username:secret`, with no header name and no scheme word, so the plugin
still writes `Basic ` itself. Both halves have to be present, and a missing one refuses under
`plugin.secret_missing` or `plugin.username_missing` rather than substituting an empty string:
`Basic base64(":password")` reaches the provider and comes back 401, which reads as an expired
sign-in on an account that was never complete. **The one exception is the provider row's own**
(RD-120-38): a row with `username_required = false` may build the pair with an empty name,
because for such a provider that is the credential's shape rather than half of it -- Pixeldrain
takes its API key as the Basic password under an empty user name. The allowance covers the
`{{basic:…}}` pair alone; a bare `{{username}}` still refuses an empty name. A user name
containing a colon is refused under `plugin.basic_username_invalid`, as RFC 7617 requires.
`plugins/seedr/`, `plugins/seedr-jobs/` and `plugins/pixeldrain/` use it.

Where a request came from travels to third parties, so no marker is expanded into `Referer`,
`Origin` or `X-Requested-With`; a template that tries ends with `plugin.secret_target_not_allowed`.

### Computing over a credential you never see

`capabilities.key_derivation = true` grants `rdownloader:plugin/key-derivation`, whose single
function is:

```wit
derive: func(secret: secret-handle, steps: list<step>) -> result<list<u8>, failure>;
```

It exists for one shape of provider: the ones whose sign-in does not take the password but a
value derived from it. MEGA's `us` call wants the second half of
PBKDF2-HMAC-SHA512(password, salt, 100 000), and the first half unwraps the account's master
key. `{{secret:…}}` cannot help there — it substitutes on the way *out*, so a guest can send a
credential but not compute with one.

**You name the credential; the host computes.** A `secret-handle` carries a reference your own
manifest lists under `capabilities.secrets`, exactly as a `{{secret:<reference>}}` marker would.
The steps form a chain: each step's output is the next one's input, the first step's input is
the credential, and **only the last step's output comes back**.

| step | what it does |
|---|---|
| `pbkdf2-hmac-sha512(pbkdf2)` | PBKDF2-HMAC-SHA512 over the running value, with your salt and rounds |
| `aes-ecb-decrypt(list<u8>)` | AES-128-ECB over those bytes, keyed by the first sixteen bytes of the running value |
| `take(span)` | narrows the running value to a window of itself |

What the host refuses:

- over a credential a person typed, a chain that does not **begin** with
  `pbkdf2-hmac-sha512`, or one asking for fewer than 100 000 rounds
  (`plugin.key_derivation_needs_one_way`, `plugin.key_derivation_rounds_too_low`). The first
  thing that happens to a guessable value is the one-way thing, or this interface would be an
  oracle a dictionary runs through;
- over the key material a sign-in stored (below), a chain that does not begin with
  `aes-ecb-decrypt` (`plugin.key_derivation_needs_key_step`);
- a chain that begins with `take`, whatever the credential: it would answer with the
  credential itself, or read a stored key a byte at a time;
- a reference your manifest does not list, or the capability not being declared;
- a plugin whose `capabilities.net_http.domains` reach beyond the domains the credential's own
  slot allows (`plugin.key_derivation_reach_too_wide`). Derived bytes are not domain-gated once
  they are in your memory, so you may compute over a credential exactly when everything you can
  reach is somewhere that credential could itself have been sent. Compared as patterns: a
  `*.suffix` reach needs that wildcard (or a wider one) in the slot too, and a slot's `*.suffix`
  entry admits sub-domains for sending, never the bare suffix (RD-120-30);
- more than eight steps, more than 64 bytes out of a PBKDF2 step, or more than 4 KiB into an
  AES step.

**Which rule applies is the host's decision, from where it reads the value (RD-120-30).** A
reference whose slot the person fills is read from the account's credential and opens with
PBKDF2. A reference whose slot is declared `filled_by = "flow"` is read from beside the sign-in,
and a chain over it runs over the **key material** the sign-in stored there — never over the
token beside it, never over anything a person typed. Your guest cannot say which kind it means:
the handle is a name, and nothing else.

A sign-in leaves key material by storing its session in exactly this shape:

```json
{"token": "<what {{secret:<flow slot>}} sends>", "key": "<base64url, exactly 16 bytes>"}
```

The host splits it: the token goes where a marker finds it, the key where only `derive` does.
Any other JSON object is refused (`plugin.store_token_session_invalid`) rather than stored as a
token, because a token is sent whole and this one would carry the key. A plain string is a plain
token, as before. MEGA is the user: `plugins/mega-auth` stores the session identifier and the
master key, and `plugins/mega` and `plugins/mega-crawler` unwrap a node key with a single
`aes-ecb-decrypt` step over `mega_session` — they get the node's key back, never the master key.
A `filled_by = "flow"` slot is allowed on `credentials = "oauth"` and, since RD-120-30,
`"username_password"` providers, beside exactly one slot the person fills.
`docs/adr/0020-the-host-computes-over-the-secret.md` has the argument, including what a hostile
guest gains by it.

**It costs fuel, at the price the same computation costs inside the sandbox.** 33 383 per PBKDF2
round per hash block and 8 166 per AES block, both measured (RD-120-11). Computing on the host
is therefore never a discount — it only moves the credential. Declare the budget your sign-in
needs in `[limits] fuel`; `plugins/mega-auth` declares 12 000 000 000 for three chains.

**Ask for as little as you can.** A single PBKDF2 step with `length: 32` would hand you both
halves and let you do the AES yourself for almost nothing — and would put the password key, the
one value that is a direct function of the password, inside the sandbox. `plugins/mega-auth`
makes three separate calls instead, so that value never crosses. See
`docs/adr/0020-the-host-computes-over-the-secret.md`.

`capabilities.net_http.domains` is the sandbox allowlist and the only authority for it. In a
domain pattern, `*.suffix` matches sub-domains and **not** the bare suffix, so list
`example.test` as well if you need it. The list holds for every redirect hop too, and each is
checked **before** it is followed (RD-130-24): a hop outside the list — or outside the one host
an invocation was narrowed to — is never requested, and the call fails with
`plugin.redirect_outside_domains`. A redirect to a CDN therefore needs that CDN's domain in the
list. The file system, other processes and WASI are not
capabilities: there is no interface for them, so they cannot be granted at all.

**`credentials = "oauth"` also puts the account's token on the transfer (RD-106-04).** For every
other credential kind, `secret_domains` bounds where a *plugin's* request may carry the secret.
For an OAuth provider it bounds one thing more: the download engine attaches
`Authorization: Bearer <access token>` to the transfer itself when the resolved address is one of
those exact hosts, over TLS, and never otherwise. That exists because a cloud drive's bytes come
from an API address that answers with the token and with nothing else, and a resolver has no way
to state one — `store-oauth-token` is deliberately write-only, so no plugin ever holds a
credential to put in a header. If you declare an OAuth provider, keep `secret_domains` as narrow
as the hosts that actually serve bytes: it is now the list that decides where a person's token is
sent by the application, not only by your plugin. Note that a resolver may still answer with a
*different* host — a CDN, say — and the token simply will not follow it there.

**`transfer_auth = "basic"` does the same for HTTP Basic (RD-120-38).** A provider whose file
addresses answer the account's own Basic credential and nothing else declares it on its
`[provider]` row, and the download engine then sends `Authorization: Basic
base64(username:secret)` on the transfer — built by the same host code as `{{basic:…}}`, with the
same empty-name rule — to the exact hosts of that slot's `secret_domains`, over TLS, and never
otherwise. It needs `credentials = "api_key"` or `"username_password"` and exactly one secret
slot with domains; anything else is refused when the manifest is read. Seedr and Pixeldrain are
the two rows that declare it. **The address checked is the one each request goes to.** The
engine probes the source, reqwest drops `Authorization` at any change of host on the way, and
the chunks are then fetched from where the probe ended; the credential is decided again for that
address. Until this job the probe's headers were reused there, so a redirect to a foreign host
received the very header reqwest had just stripped — for the OAuth `Bearer` token as well.
`crates/rd-scheduler/tests/provider_transfer_credential.rs` pins it against a real TLS
listener.

### Captchas

`interface captcha` knows six kinds of challenge, and two calls to hand one over. Which call
depends on the shape of the answer, not on who answers it:

| Kind | Record | Answered by | Answer |
|---|---|---|---|
| `recaptcha-v2`, `hcaptcha`, `turnstile` | `widget-challenge` (site key, page, invisible) | a solver service, or the person in their own browser through the extension | token |
| `image` | `image-challenge` (raw bytes, MIME type, optional prompt) | a solver service, or the person in the web interface | typed text |
| `click-point` | `image-challenge` — the same picture, a different question | a solver service (`CoordinatesTask`), or the person clicking the picture in the web interface | `click-point { x, y }`, in pixels of the image **as you served it** |
| `cutcaptcha` | `cutcaptcha-challenge` (`site-key` — the widget's `data-apikey`, `misery-key` — the page's `CUTCAPTCHA_MISERY_KEY`, page) | a solver service, and nobody else | token |

- `solve-captcha: func(captcha-challenge) -> result<captcha-solution, failure>` is the token
  form. It answers the first four kinds and `cutcaptcha`. A `click-point` challenge is refused
  here with `captcha.answer_shape` **before** any service or person is asked, because
  `captcha-solution { token }` cannot carry the answer.
- `solve-challenge: func(captcha-challenge) -> result<captcha-answer, failure>` takes every
  kind and returns `variant captcha-answer { token(string), point(click-point) }` — a point for
  `click-point`, a token for everything else. New code can use it for all six kinds.
- The page of a widget — `cutcaptcha` included — must lie inside your declared domains, or the
  call ends with `plugin.captcha_target_not_allowed` (RD-107-03). Both CutCaptcha keys are held
  to the site-key rule: empty or over 256 bytes is `captcha.site_key_invalid`. A picture above
  512 KiB is `captcha.image_invalid`.
- A CutCaptcha on an installation without a solver service ends **at once** with
  `captcha.cutcaptcha_needs_solver` (category `needs-captcha`): no browser rDownloader drives
  can read its token, so it is neither queued for a person nor offered to the extension. A
  service that does not know the task answers with its own error, passed on as
  `captcha.solver_failed` with the service's `reason`.
- The coordinate a person clicks is translated by the interface into the image's own pixels
  whatever size it was rendered at; your plugin submits it as the hoster expects, usually as
  the click position relative to the served image.

In `plugin-common`, `CaptchaChallenge::ClickPoint(ImageChallenge)` and
`CaptchaChallenge::Cutcaptcha(CutcaptchaChallenge)` are the two cases, and
`PluginHost::solve_challenge` returns `CaptchaAnswer::{Token, Point}`; `solve_captcha` keeps its
old signature and refusal.

### Transfer backends

`plugin_type = "transfer"` is the second kind of plugin. A backend carries the bytes of a
protocol the core does not know; the host decides where they land and whether they count.

```toml
plugin_type = "transfer"

[transfer]
slug = "fastftp"                    # message namespace, as provider.slug is for a resolver
schemes = ["fastftp", "fastftps"]   # links carrying these schemes route to this backend

[capabilities.net_stream]
hosts = ["files.example.net", "*.example.net"]
ports = [21, 990]                   # no wildcard: a backend free to reach any port on a host
                                    # it named is a port scanner with a manifest
```

A transfer backend exports `probe` and `run` and imports `host`, `net`, `sink` and `http`;
its world links neither `cookies` nor `captcha`. It declares no `[provider]`, which is
refused for this type — it serves URL schemes, not an account.

The contract is deliberately lopsided:

- **The plugin never sees a path.** `sink` has no handle and no file name; the store already
  knows which download it belongs to, so `write-at` writes into *this* transfer's part file
  and there is nothing to address another one with.
- **The host owns the sockets.** `net::connect` returns an opaque number, valid only inside
  the invocation it was issued in. Every socket is closed when the invocation ends. Reads are
  paced by the transfer's bandwidth limiter before the bytes reach the plugin.
- **The host decides what "finished" means.** It checks the length against the probe and
  performs the promotion into the package folder itself, so a backend that reports success on
  a short file produces a failed attempt, not a truncated file presented as complete.
- **The checkpoint is opaque.** `stopped` and `complete` may carry any bytes the backend
  wants; the host stores them and hands them back verbatim. It also stores which version
  wrote them, and resumes on exactly that version — a build whose checkpoint format differs
  is never handed somebody else's state. An upgrade takes effect the next time a job starts.
- **Resume is validated, not assumed.** Size and modification time are recorded on the first
  attempt and rechecked before every continuation; a mismatch keeps the partial file and
  refuses the transfer. A backend reporting `resumable: false` starts over instead.

`plugins/example-transfer/` is the reference implementation, and
`crates/rd-plugin-transfer/tests/transfer_contract.rs` is what the contract actually
guarantees, in runnable form.

### Extension types

Beyond resolvers and transfer backends the contract has ten more worlds. They share one
manifest section:

```toml
plugin_type = "intake"

[extension]
slug = "my_parser"
# What the plugin claims, read per type. Empty means "offered for everything of this type".
claims = []
```

A manifest of one of these types must not declare `[provider]` or `[transfer]`, and a
resolver or transfer manifest must not declare `[extension]`. Each section grants a different
thing, and a manifest naming two of them is either confused or trying for both.

| `plugin_type` | World | What it does |
| --- | --- | --- |
| `intake` | `intake-plugin` | Turns text and URLs into LinkGrabber candidates |
| `auth` | `auth-plugin` | Runs a provider authentication flow |
| `oauth` | `oauth-plugin` | Runs an OAuth redirect or device sign-in and renews its token |
| `crawler` | `crawler-plugin` | Turns one address into the files behind it |
| `enricher` | `enricher-plugin` | Adds metadata to a link or a queued item |
| `notifier` | `notifier-plugin` | Delivers notifications to another destination |
| `postprocess` | `postprocess-plugin` | Runs one post-processing step |
| `storage` | `storage-plugin` | Uploads finished files to a destination |
| `remote-job` | `remote-job-plugin` | Runs a job that lives at the provider and outlives the call |
| `stream-transform` | `stream-transform-plugin` | Answers with an address and how the bytes behind it become a file |

Three interfaces are granted by *type* rather than by capability, because they are what the
type is rather than something it asks for. A transfer backend imports `sink`; a
post-processing step and a storage destination import `source`; an authentication plugin
imports `credentials`. Neither of the first two can name a file: a `source` reader is given a
package handle and the list of files it may read, and a read outside that list is refused even
when the path would resolve inside the directory. `source` is not read-only: `rename` gives one
file from that list a new bare name inside the same package, and a name carrying a separator, or
one that already exists, is refused. No manifest can ask for any of the three, so
being the type is the only way to reach them.

Every extension type reaches the network the same way a resolver does — through the host,
confined to `capabilities.net_http.domains`, with `{{secret:<reference>}}` expanded on the way
out and never handed back. An account's secret is reachable only from an invocation that was
started for that account.

#### Intake parsers

An intake parser is asked `claims` before it is shown anything, so a parser for one format is
not handed every paste in the application. What it returns are *proposals*: they go through
the same LinkGrabber review, the same domain blocklist and the same routing rules a pasted
link does. A parser cannot queue anything, and it never supplies request metadata — that
would be a credential path it has no claim to.

`normalize` may tidy an address. A rewrite that is not a URL, or that changes the host or the
scheme, is discarded: a normalizer canonicalises, it does not redirect.

Two are bundled: **Metalink**, which reads `.metalink` and `.meta4` documents and proposes the
files they list, and **Crawljob**, which reads JDownloader `.crawljob` files and proposes the
links and package names in them.

Start one with `rdownloader plugin new --type intake`.

#### Authentication providers

An authentication plugin signs an account in without a password ever being typed into
rDownloader: `begin` starts the flow and returns what the person has to do, `poll` asks the
provider whether they have done it. The waiting between polls is the host's, not the plugin's.

What makes this safe to hand to a third party is that the plugin never sees a credential and
never names one. It is given an opaque `credential-ref`, and what it obtained goes back
through `credentials.store-token`, which takes an account and a value — no reference. The host
looks up which vault reference the account's provider owns, refuses if that provider keeps no
credential of its own, writes the new value and only then drops the old one. There is no call
that reads a value back, so the interface can only ever add a secret the plugin already held.

Three things confine it, and all three are needed: `credentials` is linked into no other
world, the account is compared with the one the invocation was started for, and the reference
is the host's to choose. A verification URL must lie on a domain the manifest declares under
`net_http`, and it is shown rather than opened — a plugin that could send someone to an
address of its choosing would be a phishing page with a signature on it.

A guest is instantiated fresh for every call and remembers nothing, so whatever the next poll
needs — a device code, a PIN check value — goes back in `user-prompt.flow-state`. The host
stores it verbatim, hands it to `poll`, and never shows it. That, plus the flow living in the
database rather than in memory, is what lets somebody close the browser in the middle of a
sign-in, or restart the service, and find the flow still running.

The waiting is the host's too: a plugin answers `pending(n)` and the sweep loop asks again in
`n` seconds. A plugin that slept would spend its own execution budget doing nothing.

Three are bundled, one provider each and two flow shapes between them: **Debrid-Link** and
**Premiumize** as OAuth device flows, **AllDebrid** as a PIN flow. A contract that fitted only
one of those shapes would be a contract for OAuth rather than for signing in.

Start one with `rdownloader plugin new --type auth`.

#### OAuth providers

The sibling of the above, and separate for the reason the WIT says: the older world has no way
to say that a token dies in an hour, so teaching it would have meant an export every shipped
authentication plugin would have had to grow. `begin` builds the address the person is sent to,
`poll` exchanges the code the callback carried, and `refresh` mints a new access token from
stored refresh material — driven by the host's renewal sweep rather than by the person, because
the point of it is that nobody is asked twice.

**Two ways in, one way on (RD-106-01).** Beside the redirect, `device-begin` and `device-poll`
run a device code: the plugin asks the provider for one, the person types a short code on
another screen, and the host polls until they have. There is no callback, so there is no
`state` to echo — what survives from one call to the next is the device code, in `flow-state`,
which the host stores verbatim and never shows. It is deliberately the *same* interface as the
redirect rather than a world of its own, because `refresh` is here: whichever entrance somebody
came through, the renewal that follows is the same one, and nobody is asked to type a code
twice. `docs/adr/0002-a-device-sign-in-that-can-be-renewed.md` records why, and what was
rejected.

Which entrances a plugin serves is stated in its manifest, not guessed at:

```toml
oauth_flows = ["redirect", "device"]   # in the order this plugin prefers them
```

The field is top-level — written *above* the first `[table]` header, or TOML makes it a key of
that table — and refused on any other plugin type. Leaving it out means `["redirect"]`, which
is what every OAuth manifest written before RD-106-01 meant. The host calls only what is named
there, so a provider that offers one way in needs no stub for the other; being asked anyway is
a refusal with a stable code, never a trap. The first entry is what the accounts form offers,
so a provider serving both is one plugin, one package and one signature.

A device flow's `verification_url` is held to the same rule as an authorization URL: it must be
on a domain the manifest declares. Both are addresses put in front of a person with an
invitation to sign in there, and a signed plugin free to name any of them would be an installed
phishing page.

`authorization_pending` and `slow_down` are both *waiting*, not failure. They come back as
`pending(n)`, and only a refusal the provider actually made becomes `failed` — a plugin that
read "not yet" as an ending would kill sign-ins while the person was still walking to the other
screen.

The redirect comes back to one fixed address, `/api/v1/oauth/callback`, because a redirect URI
has to be registered with the provider before it is first used and a per-account path could
not be. What ties an arriving callback to an account is the `state` the plugin generated and
the provider echoed back; a callback quoting a value no flow claims belongs to nobody.

Everything an exchange produces goes to the vault through `credentials.store-oauth-token` —
three values rather than one, because a host that cannot tell an access token from refresh
material and from an expiry cannot renew anything. Both tokens are encrypted; only the expiry
is kept in the clear, so the sweep can ask whether a flow is due without decrypting a secret to
find out. A later request reaches the stored value only as the template
`{{secret:<reference>}}`, which the host expands on the way out.

**PKCE, and where its entropy comes from.** The scaffold runs authorization code with PKCE and
computes the `S256` challenge itself: a guest has no WASI, so SHA-256 and base64url are written
out in `src/pkce.rs`. The randomness underneath comes from `host.random-bytes`, which hands out
bytes from the operating system's own generator — 32 of them per value, base64url without
padding, which is the 43 unreserved characters RFC 7636 asks for at minimum. The verifier and
the `state` are drawn separately, so neither is computable from the other.

Nothing else is an acceptable source, and the first version of this scaffold is the reason the
paragraph says so: it derived both values from `host.now-unix-seconds`, the account id and a
salt that stood in the published template. Every input was public — the account ids are readable
with `api:read`, and a flow's lifetime narrows the second to a few hundred candidates — so both
values could be recomputed, which makes a stolen authorization code redeemable again and the
`state` forgeable. That the values never leave the application does not help: a value anybody
can *derive* is not secret because it is not *stored* anywhere. `host.random-bytes` refuses a
request for nothing or for more than 1024 bytes by answering with an empty list, and a guest
must fail the flow on a short answer rather than fall back to something it computed itself.

**A refusal and an outage are not the same answer.** A provider that refuses ends the flow:
`token-outcome.failed` records a failure the person is shown, and the renewal stops asking. A
provider that could not be reached must *not* — return the failure `http-request` gave you and
the host keeps the stored token and tries again later. A plugin that confused the two would
sign people out every time their connection dropped. A rate limit is the third answer,
`pending(n)`, carrying the provider's own `Retry-After`.

**Nothing a provider wrote is repeated verbatim.** An error document that echoed part of a
token into its `error_description` would otherwise publish it through a log line and the
interface. The scaffold keeps the RFC 6749 error code — lowercase letters and underscores, at
most forty of them — and drops anything else whole rather than filtering it, because filtering
keeps the digits of a leaked token. The three bundled authentication plugins follow the same
rule since RD-106-01; before that they quoted the provider's sentence into the failure the
person saw, and reported every refusal there is under one code that said the sign-in code had
expired.

**Where the renewal material actually comes from.** `refresh` is handed a `credential-ref` and
writes it as `{{secret:<reference>}}` like any other credential — but that reference is one the
*host* minted when it stored the token, not one of the provider's declared secret slots. The host
recognises it as this account's own renewal reference and expands it; a reference that is not
exactly the one stored on this account's flow row resolves to nothing, and the address is still
held to the hosts the provider's own credential may reach. Until RD-106-03 that branch did not
exist, so every renewal was refused as a target the provider does not declare, and the contract
tests could not see it because they answer at the host boundary and never run the expansion.

**When the person registers the application (RD-106-03).** Some providers run their flow against
an application registered by whoever installed rDownloader, not by whoever wrote the plugin — and
a client secret shipped in an open-source repository would not be secret in any case, besides
putting every installation under one rate limit. Such a provider takes the client id as the
account's username and the client secret as its credential, and the plugin only names them:
`{{username}}` and `{{secret:<reference>}}`.

That means two credentials have to exist at once, because the sign-in produces a third value the
resolver beside it needs. One stored value cannot be both, so the provider declares two slots and
says which is which:

```toml
[[provider.secrets]]
reference = "example_client_secret"      # what the person registered and typed
domains = ["api.example.test"]

[[provider.secrets]]
reference = "example_access_token"       # what the sign-in obtained
domains = ["api.example.test"]
filled_by = "flow"
```

`filled_by = "flow"` is what stops `store-oauth-token` writing the token over the registration —
which would destroy the value every later renewal needs. The host keeps a flow-filled slot beside
the flow instead, `secret-available` answers about the flow for it (so an account that has only
registered an application does not read as signed in), and everything else — the domain gate, the
`{{secret:…}}` marker, the sandbox — is unchanged. It may only appear on a `credentials = "oauth"`
provider, at most once, and only beside exactly one slot the person fills.

A plugin should refuse *before* its first request when the registration is missing:
`host.secret-available` answers whether a credential exists without revealing it, and a refusal
that says where to register beats the provider's own `invalid_client`, which means "this
application was refused" rather than "there is none".

`plugins/realdebrid-auth/` is the first shipped provider of this world: Real-Debrid's device flow,
`oauth_flows = ["device"]`, with `plugins/realdebrid/` beside it reading the token the sign-in
stored. It is also worth reading for a path it deliberately does **not** take. Real-Debrid offers
an open-source flow that hands out a `client_id` and `client_secret` per person — and a renewal
needs those two *plus* the refresh token, while `store-oauth-token` keeps one value that the host
later expands as one marker. A composite cannot be split back apart, because the host
percent-encodes what it substitutes into a form body. So a provider whose renewal needs more than
one stored value uses a registered application and the two slots above, and a plugin author meeting
that wall should know it is the contract and not their reading of the API.

`plugins/example-oauth/` is the reference plugin: the same code the template scaffolds, pointed
at `oauth.example.invalid`, which resolves nowhere. It serves both entrances, because it exists
to be driven through both by the contract tests in
`crates/rd-plugin-ext/tests/oauth_contract.rs`, which stand in for the authorization server,
check the PKCE challenge the way a provider does, and carry a device sign-in through to a
stored token and the renewal that follows it. Their fixtures live in
`crates/rd-plugin-ext/tests/fixtures/oauth/` and carry placeholder tokens only; a test fails if
one of them ever carries anything else.

`plugins/google-drive-oauth/` is the first real one (RD-106-04): authorization code with PKCE
against Google's own endpoint, redirect only, scoped `drive.readonly`. Two parameters in its
authorization URL are worth copying rather than rediscovering — `access_type=offline` and
`prompt=consent`. Without the first Google issues no refresh material at all; without the second
it issues it exactly once, so an account signed in a second time comes back with nothing to renew
from and the person is asked again every hour thereafter.

`plugins/onedrive-oauth/` is the second real one, and the first shipped plugin to serve **both
entrances** (RD-106-05): `oauth_flows = ["redirect", "device"]`, the browser redirect with PKCE
against `login.microsoftonline.com/common` where a browser can reach the application, and the
device code typed at `microsoft.com/devicelogin` where none can — both ending in the same
`store-oauth-token` call with the same refresh material, and both renewed by the same `refresh`.
Three things to copy from it rather than rediscover. The verification address a device flow
returns is gated like an authorization address, so the host it points at — `microsoft.com` — has
to be in the manifest's `net_http` domains even though the plugin never requests anything from
it. `authorization_pending` and `authorization_declined` are Microsoft's device-flow spellings of
"not yet" and "the person said no"; the first is a wait and must never end the flow, the second
ends it under the same code as `access_denied`. And Microsoft's `error_description` is never read:
it is a sentence with an `AADSTS` code, a timestamp, a trace id and a correlation id in it, and no
shape check makes a sentence safe. The scope is repeated on the refresh request, because
Microsoft's token endpoint answers a narrower token when it is left out.
`plugins/dropbox-oauth/` (RD-106-06) is the same flow against Dropbox, and its one parameter to
copy is `token_access_type=offline`: without it Dropbox issues a four-hour access token and no
refresh material at all. Dropbox's scopes are named in the App Console rather than granted on
request, so the plugin asks for exactly the four it needs — `account_info.read`,
`files.metadata.read`, `files.content.read`, `sharing.read` — and the setup instructions have to
say which boxes to tick, or the sign-in is refused before it starts.

`plugins/pcloud-oauth/` (RD-120-06) is the one that breaks the pattern, and it is worth reading
before assuming a provider offers what Google Drive, OneDrive, Dropbox and Box did. pCloud's `oauth2_token` documents
`client_id`, `client_secret` and `code` — **no PKCE at all** — and issues **no refresh token**,
because its access token lives until somebody revokes it. So the plugin is a confidential client
with two credential slots on its provider row (`pcloud_client_secret`, typed by the person;
`pcloud_access_token`, `filled_by = "flow"`), `begin` returns `flow_state: None` rather than
inventing bookkeeping that proves nothing, and `refresh` is an honest refusal. That last one is
safe because the host's renewal sweep selects flows carrying **both** refresh material and an
expiry, and `poll` deliberately stores neither — so the refusal answers a question that never
arrives. The second thing to copy is the region: pCloud's redirect states the account's data
centre as `hostname` and `locationid`, but `poll` receives the `code` alone, and widening the WIT
to carry the rest would make all 72 signed components stale for one provider. So the code is
offered to one installation and then the other; a code an installation never issued cannot be
spent there, so the first attempt consumes nothing.

**Never put a client id in your plugin.** Write the marker `{{client_id}}` where the client id
belongs, and the host substitutes what *this installation* registered — in the outbound request,
and in the authorization URL your `begin` returns, which is the one place the host expands
anything into a string it did not send itself. Three reasons, in order of weight: providers count
their quotas per client, so a compiled-in one would have every installation sharing a single
allowance; a client id in a public repository sits in the git history, in every signed `.rdplug`
and in every release artifact, revocable by nobody; and the client *type* — which decides whether
the provider's token endpoint wants a `client_secret` — is then each installation's own answer
rather than your guess.

The account's username field is where it lives, so declare `username_required = true` on your
provider row and give `username_label` and `username_hint` in all four catalogues; the hint is
where the registration steps belong, because it is what somebody is looking at when they need
them. It is stored in the clear on purpose: a client id identifies the *application* to the
provider, not the person to the application, and the provider publishes it in the address the
person is sent to. Consequently `{{client_id}}` is not a secret marker — it does not make a
request carry credentials and it does not narrow a redirect — and its gates are only that the
account's provider is `credentials = "oauth"` and that the address is one your manifest allows.
An account with no client id refuses under `oauth.client_not_configured` before anybody is sent
anywhere.

A client *secret* has nowhere to live today, and that is a limit rather than an oversight: an
account carries exactly one secret reference, and `store-oauth-token` overwrites it with the
access token at the first sign-in. Write against a public client with PKCE.

Start one with `rdownloader plugin new --type oauth`.

#### Folder crawlers

A crawler answers the question a resolver cannot: *what lies behind this address?* `resolve`
returns exactly one file, so a cloud folder, a directory share or a release page has no shape
to come back in. `crawl` returns a list, and everything in that list then goes through the
same review, the same domain blocklist and the same routing rules a pasted link does. The
reasoning, and the three designs that were rejected, are in
`docs/adr/0001-resolving-an-address-that-points-at-many-files.md`.

`claims-url` is asked first and reaches nothing: a plugin that has no business with an address
never fetches it. This matters more here than for a parser, because the multihoster resolvers
beside it claim *every* http(s) address — a crawler that did the same would try to list a
folder behind every link in the queue.

`[extension] claims` names the **provider slug** whose account the crawl runs as. The host
looks the account up and the plugin never learns which one it is; its requests carry
`{{secret:<reference>}}`, expanded on the way out and only towards the hosts that reference is
allowed to reach. A crawler for a public share names no provider and runs without an account.

Four rules a crawler has to keep, because a folder is a stranger's data structure:

- **Bound the walk yourself.** Maximum depth, maximum number of files, maximum number of
  folders — and a set of the folder ids already visited, or a folder that contains itself is
  walked for ever while every single request it makes looks perfectly reasonable. The fuel and
  timeout budget underneath is the last resort, not the plan: a crawler that leant on it would
  report a timeout where it should report "this folder is larger than I will list".
- **An empty answer is a failure.** A folder that is empty, gone or unreachable returns a
  failure with a stable code from your own catalogue, so the person is told in their own
  language. Returning an empty list would create a package with nothing in it and nothing to
  explain why.
- **Names come from strangers.** A file name or a folder name may not carry a path separator
  out of where it belongs; the host strips them again, and so should you.
- **Never hand back your own input.** The host drops it, and a crawler that returned the
  address it was given would otherwise be a loop with extra steps.

`package-hint` is where a file sat, relative to the crawled address — `Show/Season 1`. Links
sharing a hint become one package, so the structure a crawler walked survives into the
LinkGrabber; an explicit package name still overrides all of them.

`plugins/premiumize-crawler/` is the reference implementation: `folder/list` and
`item/details` against the Premiumize API, walked under the limits in `src/walk.rs`. It is a
*sibling* of `plugins/premiumize/` rather than part of it, because a manifest carries exactly
one `plugin_type` — the same pattern as `premiumize-auth`, `alldebrid-auth` and
`debridlink-auth`. The contract tests in `crates/rd-plugin-ext/tests/crawler_contract.rs`
drive it as a real component against a mock Premiumize API.

`plugins/google-drive-crawler/` is the second, and it adds the limit a paginating API needs
(RD-106-04). Drive answers `files.list` a page at a time and hands back a `nextPageToken`, so a
folder wide enough produces pages for as long as anybody asks: the walk follows at most ten of
them per folder and records that it stopped, exactly as it records the other three limits. If
the folder API you are writing against pages — Microsoft Graph and Dropbox both do — copy that
limit rather than the loop that would run without it.

`plugins/onedrive-crawler/` is the third (RD-106-05), and it draws the line between the siblings
in a place Google Drive did not have to: a sharing link's type letter says what it stands for —
`1drv.ms/f/…` and `…sharepoint.com/:f:/…` are folders, every other letter a file — but the long
`onedrive.live.com` address says neither. The crawler claims those undecided addresses, asks
Graph, and answers with one file when that is what it finds; the resolver never sees the pasted
address, only the canonical one behind it. Two more things worth copying: the whole sharing link
is handed to Graph as the share id `u!<base64url>` Microsoft documents, and the canonical address
the crawler emits keeps that share — `graph.microsoft.com/v1.0/shares/<share>/items/<item>` —
because a link shared with the account grants access *through the share*, and an item reached by
its own id alone can be refused. Graph names the next page in `@odata.nextLink` as a whole
address; it is followed as given, but only while it still points at Graph, and a next page
anywhere else is refused before it is fetched.
`plugins/dropbox-crawler/` (RD-106-06) is the third, and it moves the cursor out of the loop. A
Dropbox `list_folder` answers with a `cursor` and `has_more`, and the walk in `src/walk.rs`
keeps that cursor in the pending entry for the folder rather than in a local: a folder with more
pages is put back at the front of the queue carrying the cursor its last page ended on, a page
does not count as a second folder, and the walk is one queue of "folders and where I was in
each". That is what "the cursor survives a restart" means at the plugin level — any page can be
resumed from its entry — and it is the shape to copy for `@odata.nextLink` too. The other thing
the Dropbox crawler shows is a resolver and a crawler telling their claims apart by one query
parameter, `preview`, where the provider's own address spelling does not separate files from
folders. Its resolver sibling, `plugins/dropbox/`, is also the reference for a provider whose
content endpoint names the file in a header rather than in the address: the resolver states the
stable endpoint as the URL and the file in a `Dropbox-API-Arg` header, the scheduler carries every
resolved header onto the probe and the transfer, and the account's token is added there because
the content host is listed under `secret_domains`.

`plugins/box-crawler/` (RD-120-05) is the fourth cloud drive, and it is the one where the next
page is a number rather than a token. Box paginates `/2.0/folders/<id>/items` by `offset` and
states `total_count`, so there is no continuation address an answer could point anywhere — the
walk asks for the next offset, up to ten pages, and stops when Box stops saying there are more.
Two other things it settles for whoever copies it. **An address the provider does not spell the
kind of belongs to the crawler**: Box writes `/s/<name>` for a shared file and for a shared
folder alike, so the crawler claims it, asks `/2.0/shared_items` — the endpoint whose whole job
is to say what a link points at — and answers with one file when that is what it turns out to
be, the same arrangement OneDrive's long personal address uses. **A password has to travel in the
hand-over address**, because nothing else passes between two sibling packages: the canonical
`app.box.com/s/<name>/file/<id>?shared_link_password=…` is what the crawler hands the resolver,
the parameter is in `rd_core::SIGNED_QUERY_SECRETS` so it is struck out of every log line, and it
never reaches the address the bytes come from. Its resolver sibling, `plugins/box/`, is the
reference for **pinning a download to a version**: `/2.0/files/<id>/content?version=<id>` rather
than `/content` alone, so the stable address a resume asks for again still means the bytes the
partial file came from. And `plugins/box-oauth/` is the second plugin after `realdebrid-auth` to
need two credential slots at once (RD-106-03) — Box's token endpoint requires a `client_secret`
on every grant and accepts no PKCE, so what the person registered and what the sign-in obtained
each get a slot and neither overwrites the other.

`plugins/pcloud-crawler/` (RD-120-06) is the fifth cloud drive's, and it is the one to read when
the API you are writing against does **not** page. pCloud's `listfolder` takes no cursor, offset
or limit and answers a folder whole; `showpublink` answers a public folder link with its **entire
tree** in one document. So `src/walk.rs` deliberately carries no page cap — a limit on something
that never happens reads as protection and is not — and the two sources share one walk keyed by
`folderid`, one fetching each node and one reading it out of the document already in hand. Three
more things worth copying. Its claim line is `fileid` rather than a path shape, because a pCloud
link code is opaque and nothing else in an address says whether a link is a file or a folder; a
bare public link is therefore the crawler's even when it holds one file, and comes back as that
one file spelled with its `fileid`. Its refusals carry pCloud's **decimal `result`** as a
parameter and never its `error`, which is an English sentence — a number is the one provider value
that needs no sanitising, because there is nothing in it that could have been a token. And its
region correction is the shape to copy for any provider with more than one installation: the
address names one, **only** the refusals that can mean "wrong installation" are retried, **once**,
at the other, and the one that answered is pinned and written into every address handed on.

Its resolver sibling, `plugins/pcloud/`, is the reference for a provider whose download address
**expires and has no stable form**. `getfilelink` hands back content servers, a path and an
`expires`, and pCloud offers no equivalent of Dropbox's content endpoint. The answer is not to
store the ticket with a lifetime beside it: it is to never store one. The durable address is the
canonical `fileid` or link code the queue already holds, `rd_scheduler::worker::run` re-resolves
from it on every attempt and `rd_scheduler::replay::before_resume` before every resume, and the
size and checksum read afresh with each ticket are what say it is still the same bytes. The
account's token does not travel to the content host either — the ticket is already authorised,
and pCloud picks its content servers per request, so they are named by `*.pcloud.com` in
`download_domains` and are deliberately absent from `secret_domains`.

`plugins/mediafire-crawler/` (RD-103-06) is the first crawler for a hoster rather than a cloud
drive, and it runs with no account: MediaFire's documented `folder/get_info` and
`folder/get_content` answer for a public folder without a session token, so the manifest names
the one host and no secret, and `[extension] claims` is empty. `get_content` pages by
`chunk` number per content type (`folders`, then `files`), and the guest reads one folder's
chunks up to a limit before handing the walk the whole folder; the walk in `src/walk.rs`
records the cut as a limit of its own. What it adds to the "not mine" refusal of RD-107-05 is
a crawler that *names* its domains and still cannot decide from the address alone: MediaFire's
`/?<key>` is a file or a folder, and only the service knows which. The crawler claims it, asks
`folder/get_info`, and when the API reports the key as no folder — as a missing session token,
error 104, not as 112 — it answers `unsupported`, so the selection carries on to the sibling
resolver `plugins/mediafire/`, which asks `file/get_info` and treats the key as the file it
is. The two share `plugins/mediafire-common/` — the address forms and the API envelope — and
nothing else; each keeps its own catalogue.

`plugins/pixeldrain-crawler/` (RD-120-07) is the smallest crawler in the tree, and it is worth
reading for exactly that. Pixeldrain publishes two public link shapes and they are decided by
one path segment -- `/u/{id}` is a file, `/l/{id}` is a list -- so there is no "ask the service
what this is" step, no walk, no paging and no recursion: one `GET /api/list/{id}`, one candidate
per entry, done. The sibling split is still a split, because a manifest carries exactly one
`plugin_type` and `resolve` answers with exactly one download; what it shows is that the split
costs a manifest and a hundred lines even when the reading is trivial. Like `mediafire-crawler`
it runs with no account and `[extension] claims` is empty. Its `package-hint` is the list's own
title, so a list arrives in the LinkGrabber as one package under the name its owner gave it.

##### A crawler for a service nobody hosts centrally

`plugins/nextcloud-crawler/` and `plugins/directory-index-crawler/` (RD-107-05) are the first
two crawlers with no service behind them. A Nextcloud is wherever somebody put theirs and an
open directory listing is any web server at all, so neither could name a domain in its manifest
and neither could recognise its own addresses by host. Three things in the host exist for them,
and a third-party crawler of the same shape gets all three:

- **`PROPFIND` is a reading method.** It used to sit behind the write gate that only a storage
  destination passes, purely for being an uncommon verb. `GET`, `POST`, `HEAD` and `PROPFIND`
  are now what every plugin may send; `PUT`, `DELETE` and `MKCOL` are still a storage
  destination's alone. The `Depth` header a listing needs is allowed with it.
- **`*` in `capabilities.net_http.domains`, narrowed per crawl.** The manifest's `*` means
  "wherever the address points". For the duration of one `crawl` the host builds the store with
  exactly one reachable host — the host of the address that was pasted — so the plugin reaches
  that server and no other, and a redirect off it is refused with
  `plugin.redirect_outside_domains` exactly as before. A crawler that *named* its domains is
  not narrowed: `premiumize-crawler` is asked about `premiumize.me` and fetches
  `www.premiumize.me`, and taking that away would break it. Both are in
  `crates/rd-plugin-host/src/extension/mod.rs`, `crawl_host`.
- **A refusal that hands the address on.** A crawler that claims by the shape of a path —
  `/s/<token>` is not Nextcloud-specific, and "ends in a slash" is not a directory index — is
  sometimes wrong, and being wrong used to end the link, because the selection took the first
  claimer and returned its answer, refusal included. A `crawl` that fails with the kind
  `unsupported` now means "not mine after all": the selection carries on to the next crawler,
  and an address every crawler disclaims stays exactly what it was. Every other refusal still
  ends the search, because "this folder is empty" is an answer.

A crawler that claims by shape rather than by service declares `generic = true` in its
`[extension]` section and is asked after every crawler that names one; within each group the
order is by name, so it does not depend on the order the installer read the directory in. The
flag is refused on any other plugin type.

`nextcloud-crawler` speaks the public DAV endpoint `/public.php/dav/files/<token>` that
Nextcloud 29 and later serve — with `X-Requested-With: XMLHttpRequest`, which Nextcloud demands
on everything that is not a `GET` — and falls back to `/public.php/webdav/` for older Nextcloud
and for ownCloud, where the user name is the share token. A password-protected share is opened
by appending the password to the pasted address after a `#`; a fragment is never sent to a
server, so the password reaches only the `Authorization: Basic` header the plugin builds.
Without one, a protected share refuses with `nextcloud_crawler.password_required` rather than
looking empty, and a wrong one with `nextcloud_crawler.password_wrong` rather than as an empty
listing.

##### A protected share is loaded, not only listed

RD-108-07 closed the half of that which was missing: the addresses a protected share was listed
at carried no credential, so the queue fetched them unauthenticated and every one failed. Two
decisions, and both apply to any crawler, not only to this one:

- **The password reaches the crawler as the fragment of the pasted address, and stays that
  way.** A prompt would be friendlier, but `crawl` runs inside the sandbox under a fuel and time
  budget and has nobody to ask; the captcha broker's shape does not transfer, because its plugin
  *waits* for the answer and a crawl may not. A fragment is the one part of a URL that is never
  sent to a server, so it costs nothing on the way in.
- **A protected share is not an account — it is an auth profile.** An account is a per-provider
  login with a life of its own, listed, checked with `check-account`, reused by every link of
  that provider. A share password authenticates one share, has no provider, and means nothing
  anywhere else. An auth profile is exactly a credential scoped to a host and a path prefix,
  with its secret held in `rd-secrets` as a `vault://` reference — the same home WebDAV
  addresses already use, which is why `adopt_remote_credentials` skips them.

The mechanism, because a crawler has no channel for a credential: `crawled-link` carries an
address and nothing else, so a crawler that needs a login puts the **user name** in the
address's userinfo — `https://anonymous@cloud.example.org/public.php/dav/files/<token>/file.bin`
— and never a password. The host (`rd_plugin_ext::split_crawled_address`) lifts the user name
out, **drops any link whose userinfo carries a password**, and keeps the bare address. It then
derives one credential for the whole answer (`rd_plugin_ext::share_login`): the same user name
on the same host for every file, scoped to the longest path prefix they all share. A
disagreement, a second host, or a prefix that has shrunk to `/` ends in no profile at all, so a
crawler can never have a credential minted for a whole host.

`rd-api` takes the password out of the fragment, encrypts it into the vault and writes a
`Basic` auth profile with that scope. Nothing else stores it: not the candidate's address, not
the profile row, not a REST answer (`secret_ref` is `#[serde(skip_serializing)]`), and not a log
line — the one line that prints a crawled address prints it without its fragment. Every
candidate is queued with `AuthProfileSelection::Auto`, so the queue picks the profile up by
matching the address it is about to fetch, and both the online check and the transfer send the
same `Authorization: Basic` the crawl used.

**And when no crawler claims the address at all** — one wrong letter in the host name, a share
whose plugin is not installed — the fragment is dropped instead of stored (RD-109-32). It is not
vaulted: the vault needs an owner, and an unclaimed link yields neither the user name nor the
path prefix that `share_login` derives from a crawler's answer, so nothing could ever read the
secret back. The rule is stated once, in `rd_db::collector_store`, the writer every intake path
ends in, and it is blunt on purpose: nothing tells a password in a fragment from an anchor name,
and a fragment is the one part of a URL that is never sent to a server anyway. Crawling is
unaffected, because a crawler is handed the pasted address before any of this — fragment
included.

`directory-index-crawler` claims any address whose path ends in a slash, and recognises a
listing by the one thing Apache, nginx, Caddy and lighttpd all do — marking the way back up,
as `href="../"` or as the words "Parent Directory". Its parser keeps only links that resolve
strictly one level below the page's own path on the page's own origin, which is what drops the
parent link, the sort links and anything pointing elsewhere. It reads the byte count nginx and
lighttpd print and declines to guess at Apache's `1.2K`, because a missing size costs nothing
and a wrong one is shown to somebody as a fact.

##### A crawler for a link protector

`plugins/peeplink-crawler/` (RD-110-17) is the third crawler that serves no provider, and the
only one of eight link-protection services whose measurement carried. It names its two domains
in the manifest — `peeplink.in` and `alfalink.to`, each with its `www.` form — so it is not
narrowed per crawl and does not claim by shape: two named hosts and a path that is one
hexadecimal identifier.

An entry page is a single `GET`, and the hoster links stand in clear text inside its
`<article>`. Nothing is in front of it: the reCAPTCHA, hCaptcha and QapTcha markers those pages
carry belong to the login and register popups, so the plugin declares neither `captcha` nor
`cookies` and has no captcha branch at all. Nothing outside the `<article>` is read either,
which is what keeps the popups, the navigation and the CDN scripts out of the answer. The two
domains write the same thing two ways — `peeplink.in` as `<a href>`, `alfalink.to` as bare text
inside `<article class="articless">` — and the reader takes every absolute address in the
element either way, de-duplicated and with the service's own addresses dropped.

Four refusals, and the service tells three of them apart itself: an identifier it never knew
answers `404` (`peeplink_crawler.entry_not_found`), one it has deleted answers `200` at the
front page — so the *address* the answer came from is checked, not only its status — and an
entry that names nothing is `peeplink_crawler.entry_empty`. The fourth is the access password:
it is recognised by `value="Enter Access Password"` and posted as `pwd`, and it is **untested**,
because no protected entry was findable and an invented page would prove nothing. `name="pwd"`
and `type="password"` are deliberately not used as the marker — every page of the service
carries both, in the popups.

`alfalink.to/robots.txt` says `Disallow: /`. The owner's decision (2026-09-22) is that this
directive addresses search-engine spiders, and a resolver opening an address the person pasted
themselves is not one — the same line taken for MediaFire. The plugin fetches only the entry
that was pasted: no index is crawled and no link on the page is followed. The third alias
`alfalink.info`, which that same `robots.txt` names as its `Host`, is recognised as the
service's own address but is neither claimed nor granted: it ran into a 25 s timeout on both
measuring days.

Start one with `rdownloader plugin new --type crawler`.

#### Remote jobs

A remote job is work that happens at somebody else's provider and outlives the call that
started it. A magnet handed to a debrid account answers with an identifier, not a file: it runs
for minutes or hours, it stops half-way and refuses to continue until a person has said which
files they want, and the account keeps it afterwards whether anybody wanted that or not. None
of the other worlds can hold that — `resolve` answers with one file, `crawl` with a list in one
call, `run` carries bytes for exactly as long as one download lasts. The reasoning, and the
four designs that were rejected, are in
`docs/adr/0003-a-job-that-runs-at-the-provider.md`.

The split is the whole design: **every function a remote-job plugin exports is short and none
of them waits.** Each returns on the provider's next answer. What has to last — the row, the
remote identifier, the clock, the person's answer, the restart — belongs to the host, which
writes it down. That is what lets a remote job keep a fuel and timeout budget a crawler waiting
for a torrent could never have kept.

Nine functions:

| Function | What it does | Reaches the provider |
| --- | --- | --- |
| `cache-kinds` | Which kinds of source the provider's cache can be asked about | no |
| `check-cached` | Whether the provider holds each source ready right now | yes, read-only |
| `claims` | Whether this plugin takes the source at all | no |
| `identify` | The content key of a source, derived locally | no |
| `submit` | Hands the source over; answers with the job | yes |
| `adopt` | The job the provider already holds for a content key | yes |
| `poll` | Where the job stands now | yes |
| `choose` | Answers the question `awaiting-choice` asked | yes |
| `discard` | Removes the job at the provider | yes |

Four rules a remote-job plugin has to keep:

- **`identify` never makes a request, and never invents a key.** It is what stops a duplicate:
  the host writes the key into the row *before* it calls `submit`, and a unique index on
  `(account, key)` then makes a second submit of the same content impossible before any network
  call happens. A key that changed between two calls, or that needed a request to compute,
  would defeat the guard it exists for. For BitTorrent it is the info hash, and a magnet and
  the matching `.torrent` file must answer with the same one.
- **`submit` does not retry, and does not pretend to be idempotent.** At every provider this
  world was designed for it is not: calling it twice creates two jobs and charges for both.
  Everything protecting that is on the host's side — the key, the unique row, the two-attempt
  ceiling and `adopt` between the attempts. A retry hidden inside a plugin would be that
  duplicate, moved somewhere nobody would think to look for it.
- **The host owns the clock.** `preparing` may carry a suggested wait in seconds; the host
  clamps it into its own bounds and decides when the next `poll` happens. A plugin that could
  set the interval could spend an account's whole request budget on one job — at Real-Debrid
  that budget is 250 requests a minute, shared with the resolver unrestricting this very job's
  links. A job in `awaiting-choice` is not polled at all: nothing changes until a person
  answers.
- **`discard` is called from one place and never as a side effect.** Removing the local package
  does not reach it, a failed job does not reach it, no cleanup sweep reaches it. What
  rDownloader did not put in somebody's account on its own it does not take out on its own.

**The cache question (RD-130-11, contract `0.9.0`).** `cache-kinds` and `check-cached` reach no
job: they are the LinkGrabber's question, asked during the online check *before* anybody hands
a source over. The host decides what a link is -- `torrent` (a magnet, or the magnet of a stored
`.torrent`'s info hash), `usenet` (an indexer's NZB address) or `hoster` (a file address that an
installed hoster plugin resolves) -- because an address alone cannot say it; your plugin derives
its own key from the source, exactly as `identify` does. The rules:

- **`cache-kinds` is local and asked once per load.** An empty list means the host never calls
  `check-cached`; a kind named twice counts once.
- **One answer per query, in the order given.** The host pairs answers by position and refuses a
  list of any other length as a whole (`remote_job.cache_answer_misaligned`) -- a shifted list
  would put one link's cache on another. At most 100 queries arrive per call, only of the kinds
  you named, and a source carrying a credential marker (RD-120-66) never reaches you: the host
  answers `unknown` in its place.
- **Read-only at the provider.** It creates nothing -- no job, no transfer, no folder. A provider
  whose only way to find out is to submit answers `unknown`.
- **`cached`, `known`, `unknown` -- and never `offline`.** `cached` is held ready now, `known` is
  known but not held (the host stores it as `online`), `unknown` is everything else, including
  "not in my cache": a cache that does not hold a file has not shown the file is gone. A
  `failure` drops the whole call's answers and changes no link.

What the host does with the answers: it asks **one** enabled account per provider slug, at most
500 queries per provider and check run, before the links are checked, and merges each answer
into that link's result when it is written. A cache answer only raises -- `online`/`unknown` to
`cached`, `unknown` to `online` on `known` -- and never lowers a result, changes the link's
provider or its route. A hoster link nothing here can check stays `unsupported` with its
message and still carries the stamp. `link_candidates.cached_by` names the provider next to
`cached_at`, and the chip's tooltip shows its display name. TorBox answers all three kinds over
`torrents|usenet|webdl/checkcached` and reports only what it holds, so never `known`;
Premiumize's transfers plugin answers `torrent` over `cache/check`, because its resolver asks
the same endpoint about hoster links already; the other four remote-job plugins name no kind.

What the host does with those calls, since 1.0.8 (RD-108-03), is worth knowing when writing
one, because it is the other half of the contract:

- A person's source goes through `claims` and `identify` first, both local, and the answer is
  written into a row under `UNIQUE(account, key)` *before* anything else happens. A second
  submit of the same content on the same account is refused at that row, with no request made.
- A sweep reads every due row every five seconds and does exactly one thing per row: `submit`
  for a fresh row, with the attempt counted before the call goes out; `adopt` for a row that
  was submitted and never answered, which is what a restart mid-call looks like; one more
  `submit` if adoption found nothing; and after two attempts the row fails under
  `remote_job.submit_unconfirmed` rather than trying a third time. A row with an identifier
  is only ever polled.
- The wait you suggest on `preparing`, and the `Retry-After` a rate limit carries, are clamped
  into five seconds to fifteen minutes. A refusal whose category is transient, rate-limited or
  IP-blocked keeps the row and waits; every other category ends it under your code. A guest
  that traps or runs out of fuel ends the row under `plugin.remote_job_failed`.
- `awaiting-choice` parks the row: it is not polled again until `choose` was called with at
  least one of the entries you offered. An empty entry list is refused as
  `plugin.remote_job_empty`, because a question with no options cannot be answered.
- **Answer `awaiting-choice` only if your provider keeps the answer** (RD-120-35). `choose`
  returns nothing, and the host never hands the recorded choice back to the guest, so the
  poll after `choose` has to read the answer back from the provider. Real-Debrid can do that:
  once `selectFiles` succeeds, its `status` is no longer `waiting_files_selection`. A provider
  that cannot select server-side (TorBox, Premiumize, Put.io, Seedr, Offcloud) never asks, and
  refuses `choose` under its own code. The host enforces this: if a job asks again after its
  question was answered, it ends under `remote_job.choice_not_kept` and is never put back in
  `awaiting_choice`.
- `ready` becomes one LinkGrabber batch: http(s) addresses only, at most 500, names and
  `package-hint`s reduced to paths that cannot leave the package, labelled with your plugin's
  name and checked online like a pasted list. The row is closed with the package it produced
  and never polled again. A `ready` with nothing usable in it is `plugin.remote_job_empty`.
- `discard` is called from nothing in the sweep. Since 1.0.8 (RD-108-04) it has exactly one
  caller: `POST /api/v1/remote-jobs/{id}/discard` with `confirmed: true` in the body. A request
  without that value is refused under `remote_job.not_confirmed` and reaches no plugin, so
  "was this confirmed" is answerable from the request rather than from a client's good
  intentions. Afterwards the row stays, in `discarded`, still naming the job you created — an
  account that lost a torrent can be shown which request removed it and when. Removing the row
  from the person's list is a different endpoint (`DELETE /api/v1/remote-jobs/{id}`) that calls
  nothing at all.

What a finished job returns is *addresses*, not bytes. At a debrid provider the links a
finished torrent carries are still restricted ones, so they go to the resolver that already
exists and then down the ordinary resumable queue. An empty `ready` is a failure, for the same
reason an empty `crawl` is: a package with nothing in it and nothing to explain why.

`plugins/realdebrid-torrents/` is the reference implementation, and a *sibling* of
`plugins/realdebrid/` rather than part of it, because a manifest carries exactly one
`plugin_type` — the same pattern as the auth plugins. Real-Debrid names ten torrent states and
the contract has five, and three of the mappings are worth copying: `queued` is `preparing` and
not "downloading at zero percent", `compressing` and `uploading` are `preparing` too because
they happen after the download and before the links exist, and a state the plugin does not
recognise waits rather than failing — a provider that adds a word must not cost somebody a
torrent that was going perfectly well.

**`plugins/torbox-jobs/` is the second, and it is the one to read for a provider whose world is
wider than one kind of job** (RD-120-01). TorBox runs torrents, Usenet downloads and web
downloads behind three sets of endpoints, and all three fit the contract without a line of it
changing: `job-source` already carries `magnet`, `container` and `address`, and a container is
a `.torrent` or an `.nzb` depending on its bytes. Four decisions in it are worth copying.

- **The content key carries the kind.** `identify` answers `torrent:<sha1>`, `usenet:<md5>` or
  `web:<md5>`, so a `poll` knows which endpoints to use without a lookup, an `adopt` knows which
  list to read from the key alone, and the account's unique index cannot collide an NZB's digest
  with a link's. The three digests are TorBox's own -- they are what its `checkcached` endpoints
  take and what its `mylist` entries carry -- which is the only reason an adoption can work at
  all; a key the plugin invented would match nothing at the provider.
- **The kind also travels on the handle**, in `job-state`. That field exists for exactly this: a
  guest remembers nothing between calls, and the host stores what it is handed and gives it back.
- **A provider that asks nobody anything never answers `awaiting-choice`.** TorBox fetches whole
  jobs and offers no call that means "these three files and not the others", so the plugin
  refuses `choose` under its own code rather than reporting a selection nothing acted on, and the
  person picks in the LinkGrabber the finished addresses land in. Answering `ok` would be the
  worse lie.
- **The finished address carries no credential, deliberately.** TorBox's `requestdl` needs the
  account's key as a query parameter and a plugin never holds one, so what the job hands back is
  the stable address that names the file, and the resolver sibling `plugins/torbox/` adds the key
  and mints the short-lived ticket. That is also what makes the ticket renewable: `file.source`
  stays the stable address, `worker::run` re-resolves it on every attempt and
  `replay::before_resume` re-resolves it before reusing a partial file, so a pause of an hour
  costs one request rather than a failed resume. A minted address written into the row would be
  dead the next time anybody looked at it.

`plugins/putio-transfers/` is the third (RD-120-03), and it is the one to read when the provider
offers **less** than Real-Debrid rather than more. Three of its answers are different, and each
is a decision a fourth provider will have to take too:

- **No selection at the provider.** Put.io fetches a torrent whole and its files exist only once
  it has finished, so there is no moment at which `awaiting-choice` could be asked. `poll` never
  answers it: it answers `ready` with the complete tree — every file with its name, its size and
  the folder it sat in — and the choice is made in the LinkGrabber, which is still before
  anything is fetched to this machine. `choose` exists because the world requires it and answers
  a stable code -- the same answer `plugins/torbox-jobs/` reached independently. The alternative
  considered and rejected was expressing a selection by deleting the unwanted files at Put.io,
  which is exactly the implicit remote deletion ADR 0003 forbids. Note that the contract could
  not have expressed it either: `choose` returns no handle, and nothing the host stores about
  the answer reaches the guest again, so a guest cannot tell a question it has already had
  answered from one nobody has answered yet.
- **A container becomes a magnet.** `transfers/add` takes one address, and Put.io's only way to
  accept the bytes of a `.torrent` is a resumable upload protocol on a second host — several
  requests and a session a restart could lose, inside a world whose calls are all meant to be
  short. So the container is read locally and re-expressed as a magnet carrying the same info
  hash, the torrent's name and its trackers. The content key is unchanged by that, which is what
  keeps a magnet and the matching file one job.
- **The addresses it hands back are stable ones.** Put.io will answer `files/{id}/url` with a
  signed storage address, and this plugin never asks: that address expires, and a job that waited
  an hour for its turn in the queue would fail with a refusal nobody can act on. What goes into
  the LinkGrabber is `api.put.io/v2/files/<id>/download`, which the resolver sibling claims and
  which the host attaches the account's token to because the provider row declares `api.put.io`
  among its secret domains. A provider whose finished job only ever yields short-lived addresses
  has a real problem here; one whose files have stable names does not.

**A resolver rather than a remote job, and the other end of that same problem:
`plugins/pixeldrain/`** (RD-120-07). It is here because it is the shape to copy when a provider
hands out addresses that do not expire. What goes into the LinkGrabber is
`pixeldrain.com/api/file/<id>?download` -- no signature, no deadline, no session, just the file's
identifier. So there is nothing to renew, nothing to re-resolve per attempt and no window to miss;
the metadata is still re-read on every call, because that is what turns a file deleted in the
meantime into an honest refusal rather than a 404 in the middle of a transfer. Two other things
it settles. **A credential slot is only worth declaring if something can send it**: Pixeldrain
authenticates its API key as an HTTP *Basic* password under an empty user name, and a plugin
never holds its credential. Until RD-120-38 nothing could build that pair, so the row said
`credentials = "none"` rather than offer a field nothing could use. Now it is an optional
`api_key` account: the requests write `{{basic:pixeldrain_api_key}}`, the host builds the pair
with an empty name because the row does not require one, and `transfer_auth = "basic"` puts the
same pair on the transfer. Without an account everything runs as before. **A
quota is checked before the queue is handed anything**: the bytes are fetched by the transfer
engine, not by the plugin, so a spent allowance has to become a scheduled wait at resolve time
or it arrives as a 429 mid-transfer. `GET /api/misc/rate_limits` is that check, and an answer it
cannot read is deliberately not a refusal -- blocking a good file would be worse than the 429 it
guards against.

**`plugins/offcloud-cloud/` is the fourth, and the one to read for what a provider is allowed
to leave out** (RD-120-02). It differs from the other three in three places, and each
difference is the provider's rather than the plugin's. **It has no selection step at all**: it
fetches the whole of what it was given, so `poll` never answers `awaiting-choice` and `choose`
refuses under a stable code rather than asking a question the provider will ignore. **Its
sources have two key spaces rather than one**: `POST /api/cloud` takes an ordinary web address
as readily as a magnet, and an address carries no info hash — so the key is the hash of the
address, and the two derivations get the prefixes `btih:` and `url:` because
`UNIQUE(account, key)` is one space and two bare digests in it could collide. **Its file tree
costs a second request**: the status call knows at most one address and no paths at all, so the
folders come from `cloud/explore` and a finished job is two calls rather than one.

One trap from that plugin is worth repeating here, because it is not about Offcloud. Serde
builds a struct from a JSON *sequence* as readily as from a map, taking the elements in field
order — so an error envelope read straight out of a bare array of addresses becomes
`{error: <first>, not_available: <second>}`, a refusal invented out of a perfectly good answer.
Check that a body is an object before reading a refusal out of it.
`plugins/premiumize-transfers/` is the fifth, and it is the one to read for what the contract
does **not** reach (RD-120-23). Premiumize's `POST /api/transfer/create` takes `src` as a URI
**or** as a multipart file upload, so this plugin claims all three of `job-source`: a magnet and
a plain address go out as a form body, and a container goes out as the upload under a file name
the bytes are sniffed for — a `.torrent`, an `.nzb`, an `.rsdf` or a `.dlc`. A `.ccf` carries no
marker of its own and is refused rather than uploaded under a guessed extension.

Three of its decisions are worth reading before writing a sixth such plugin:

- **The body is the answer, not the status line.** Premiumize reports every refusal with HTTP
  `200` and `{"status":"error","code":"…"}`, so a reader that believed the status would take
  `Not logged in` for a success. The `code` is stable and documented and travels as `api_code`;
  the `message` beside it is classified and then dropped, because a provider's sentence is not a
  string this project can translate.
- **`seeding` is `ready`.** The content has been fetched in full and is being offered back to the
  swarm; the files, the folder and the addresses exist and will not change. Waiting for seeding
  to end would postpone a download that is ready, for an end nobody controls. The transfer goes
  on seeding at the provider afterwards, undisturbed — nothing here deletes it.
- **It answers no `awaiting-choice`, and that is a limit of the contract rather than a shortcut.**
  Premiumize has no call that tells a transfer which files to keep, so a question could be asked
  and not answered: `choose` returns `result<_, failure>` and `job-state` is written only when
  `submit` or `adopt` names the job, so a guest cannot remember that a question was answered and
  the next `poll` would ask it again. Every file a finished transfer produced is handed over as
  one package instead, and the LinkGrabber is where the person takes what they want.
  `awaiting-choice` is for providers that have to be told *before* they fetch.

Its `adopt` answers `none` on purpose: `transfer/list` carries nothing derived from what was
handed over, so a transfer this installation created cannot be told from a stranger's, and
matching on `name` would eventually poll somebody else's transfer as this job. The duplicate
guard is then entirely the host's row keyed by `(account_id, content_key)` — which is what that
row is for.

**`plugins/seedr-jobs/` is the sixth, and the one to read for a provider whose finished job
stops existing** (RD-120-04). Seedr documents `GET /rest/transfer/{id}` and it reports a running
transfer perfectly well -- but when the torrent finishes, Seedr moves it out of the transfer list
and turns it into a folder. From that endpoint the finish is indistinguishable from a deletion:
both are "this id is not here any more". So the poll is `GET /rest/folder`, the account's root
listing, which shows the running transfer and the folder it became as two entries in one
document, and answers both questions in one request. Three more of its decisions are worth
reading.

- **Its credential could not be sent at all before this job.** Seedr's REST v1 is HTTP Basic and
  nothing else, by its own documentation, and the host gained `{{basic:<reference>}}` for it --
  see *Sending a credential you never see* above. Nothing in the WIT contract changed.
- **The finished transfer's folder is never removed.** `discard` deletes the *transfer*, and a
  transfer that Seedr has already turned into a folder is a folder full of somebody's files; a
  confirmed request to discard a job did not ask for those. A 404 on that call is therefore
  success -- there is nothing left to remove -- rather than a refusal nobody can act on.
- **A transfer that finished inside the crash window cannot be adopted**, and the plugin says so
  rather than guessing. Once it is a folder it carries no info hash, so there is nothing left to
  compare a content key with; matching the folder by name instead would be a guess with somebody
  else's account on the other end. The row falls through to the host's two-attempt ceiling
  instead. Note that `poll` *does* match on the name -- but only after `submit` or `adopt` wrote
  that name onto the handle, so it is this installation's own transfer being recognised.

A Seedr download address needs HTTP Basic, and a resolver states download headers as *values*
-- which this one cannot, because it never holds the password; the same wall RD-106-04 hit from
the other side. Until RD-120-38 the bytes therefore needed a separate authentication profile for
`www.seedr.cc` holding the password a second time. Now the `seedr` row declares
`transfer_auth = "basic"` and the engine attaches the account's own pair, to `www.seedr.cc` only
-- see the `transfer_auth` paragraph above. No second profile is needed.

`rdownloader plugin new --type remote-job` scaffolds one (RD-108-05), and it is where to start
rather than a copy of the reference plugin. The scaffold is a working remote job of its type: the
seven calls, the content key derived locally in `src/source.rs` from a magnet or a bencoded
container with SHA-1 written out beside it, and the provider's own state vocabulary mapped in
`src/reply.rs`, where the arm that matters is the last one — an unknown state waits. It carries
its own byte-identical copy of the contract, so it builds with no checkout of this repository,
and `cargo test` runs its twelve unit tests before a line of it is changed.

#### Notification destinations

A destination delivers one message and reports how it went. It does **not** decide whether to
try again: retries, backoff and quiet hours stay with the notification hub, which already owns
them for the built-in webhook, SMTP and Apprise targets. A plugin that could set its own retry
policy would be a way for one destination to keep the delivery queue busy for everybody.
The category of the failure it reports is what the hub goes by: a retryable one (`transient`,
`rate-limited`, `offline`) walks the backoff, anything else — `permanent`, `auth-required` —
ends the delivery as failed on the first attempt.

Create a target of kind `plugin` and name the destination; `endpoint` is where it sends — a
topic, a chat id — and the token goes into the vault as usual. The plugin receives the
destination and a `has-secret` flag, never the token: it writes the reference-less marker
`{{secret}}` into a header, a query value, the body or even the address, and the host
substitutes the one secret this delivery was granted on the way out. A marker written without
a grant is refused rather than sent empty.

Three are bundled, and together they cover the three shapes a token takes: **ntfy** (an
optional bearer token, which is why `has-secret` exists), **Discord** (the token *is* part of
the webhook address, so the marker goes into the URL — the host expands it after the domain
gate and checks afterwards that the host has not changed) and **Telegram** (a bot token in the
vault plus a chat id in plain sight, two values rather than one).

**A destination on a server of its own (RD-130-15).** A notifier may add `*` to its
`net_http` domains, next to the services it names — ntfy declares `["ntfy.sh", "*"]`. The
manifest's `*` is never what the sandbox receives. Every delivery is narrowed to exactly one
list before the plugin runs (`destination_reach` in
`crates/rd-plugin-host/src/extension/notifier.rs`): a destination written as an `http(s)://`
address reaches that address's host and nothing else, and anything else — a bare topic —
reaches the named services, without the `*`. So a token reaches the one host its destination
names, and a redirect off that host is refused before it is followed (RD-130-24): neither
the token nor the message reaches anywhere else. `https` is required; `http://` is
accepted only for an address inside the person's own network — the private IPv4 ranges,
loopback, link-local, IPv6 unique-local, `localhost`, `*.lan`, `*.local` and a single-label
name such as `ntfy` (a Docker or LAN service name; a public name always has a dot) — and
anything else is refused as `plugin.destination_not_encrypted` before a request is made. The
same decision refuses such a target already when it is saved, with the same code. Loopback is
reachable on purpose, so whoever may configure notification targets may point one at this
machine — the trust model of a WebDAV storage destination. A manifest without `*` keeps its
list for every destination, whatever the destination says.

An extension type's requests are measured against its own manifest alone. A resolver is
additionally held to the provider registry's request domains, but no provider declares
`ntfy.sh`, and inventing one so a notifier could pass a gate meant for hosters would be
paperwork rather than safety.

Start one with `rdownloader plugin new --type notifier`.

#### Post-processing steps

A step runs after cleanup and before a user script: by then the package is what it will
finally be, and the script stays the last word. It is switched on globally or per category —
installing one changes nothing until somebody enables it — and the list is ordered, so "first
this, then that" is expressible. A category's empty list is not "inherit" but "none here",
which is how a category switches a globally enabled step off.

A step is given a package handle and the list of files it may read. It cannot name a file
outside that list, because it cannot name paths at all. It may rename a file from that list —
to a bare name inside the same package, never over an existing one — which is what the
file-name tidier below does. Stopping is normal: return `stopped`
with a checkpoint and the host stores it, so a service restart resumes the step instead of
running it again from the start. `skipped` exists because "nothing to do here" is an ordinary
outcome and reporting it as a failure would fail every package in a category the step was
switched on for.

Three are bundled: **SHA-256** and **MD5**, each verifying its own sidecar format, and **Tidy
file names**, which replaces spaces with dots and collapses runs of separators in the package's
file names. The two checksum steps are two plugins rather than one so each can be updated and
switched off separately; with both enabled a package is read twice, which is the price of that and is worth saying out loud.

Start one with `rdownloader plugin new --type postprocess`.

#### Upload destinations

Set the post-processing upload target to `plugin:<plugin-id>/<destination>` and the package
goes there instead of through rclone. Everything after the first slash is the plugin's own
vocabulary — for WebDAV, the address of the collection — and the core does not interpret it.

**Commit before delete** is the whole point of the type. `put` uploads a file and returns the
destination's own identifier for it; `verify` is a *separate* call that asks whether the
destination really holds it, and only a `true` allows the local copy to be removed under
`upload_mode = "move"`. A server that answers `201` and stores nothing would otherwise take
the only copy with it — a gap `rclone move` still has, and the reason this call exists on its
own rather than being something `put` reports.

Two grants are narrower than they look. A storage manifest declares `domains = ["*"]`, because
where somebody put their server is not knowable in advance; what actually applies is the host
of the destination *this upload is for*, and nothing else. And a storage plugin — alone among
the types — may use `PUT`, `MKCOL`, `PROPFIND` and `DELETE`, because uploading is what it does;
every other type is held to `GET`, `POST` and `HEAD`.

Credentials come from the stored remote logins the FTP, SFTP and WebDAV transports already use,
matched on the destination's host and port. The plugin is given the user name and told whether
a password exists; the password itself reaches the request as `{{secret}}` and never the guest.

`source.progress(done, total)` moves the upload step's own percentage, in the row and the
display the rclone path writes to. The numbers are the plugin's own and count the file it is
sending; the host adds what the earlier files of the package already contributed, so the bar
does not fall back to zero at every file boundary, and throttles the writes as the rclone path
does — a plugin may report per chunk, and that is not worth a database write per chunk
(RD-108-18).

Start one with `rdownloader plugin new --type storage`.

#### Streams the host has to transform

A few providers encrypt every file on the client and keep the key out of their own reach: it
rides in a link fragment, the storage server never sees it, and what a plain `GET` returns is
ciphertext. `resolver` cannot carry that. `resolved-download` is an address, a name, a size and
headers, and there is no field key material could travel in; smuggling it into a header would
put it in every log line and every replayed chunk request.

So the key travels through this world instead, and **the bytes never leave the host**. A plugin
answers with the address *and a declarative description of the transform*: a named primitive
with its parameters. The host owns the byte stream, applies the primitive on its own write path
in `rd-http` — the buffer is already allocated at an offset that is already known, so there is
no second pass and no second copy on disk — and checks the provider's integrity value before
the part file is promoted. `docs/adr/0011-mega-a-stream-the-host-must-decrypt.md` records the
decision and the three designs that were refused.

Four properties are part of that decision rather than of the implementation, and they are what
an author has to write against:

- **The plugin describes, the host computes.** What crosses the boundary is parameters for
  primitives this repository implements, never code and never a callback per buffer. A
  primitive the host does not know is a refusal when the description is taken in — with the
  stable code `transform.cipher_unknown` or `transform.integrity_unknown` — and never a
  fallback. Which ciphers exist in the product therefore stays a decision of the core.
- **Key material is a secret from the moment it arrives.** The host puts the bytes in the vault
  and keeps a reference. Nothing that is written down, logged, broadcast or shown carries the
  key; the type that holds it has no `Serialize` and prints a placeholder.
- **The integrity value is checked, and a failure keeps the partial file.** A wrong key
  otherwise produces a file of exactly the right length under exactly the right name. A
  mismatch is `transform.integrity_mismatch`, a failed attempt, and the partial file stays.
- **Resume validates the transform, not only the bytes.** The checkpoint carries the finished
  chunk MACs and a fingerprint of the description that wrote them. A continuation whose
  description differs starts over rather than resuming somebody else's state.

Two functions:

| Function | What it does | Reaches the provider |
| --- | --- | --- |
| `claims-url` | Whether this plugin takes the address at all | no |
| `resolve` | The address, and how its bytes become a file | yes |

The primitives the host implements today are the ones one provider needs, and no more:

| Field | Name | What the host does |
| --- | --- | --- |
| `cipher.algorithm` | `aes-128-ctr` | AES-128 in counter mode, counter block `nonce` (8 bytes) followed by a big-endian u64 block index starting at `first-block`. Every 16-byte boundary decrypts on its own, which is what keeps ranges, resume and parallel chunks working. |
| `integrity.algorithm` | `cbc-mac-chain` | One CBC-MAC per chunk over the plaintext, zero-padded to 16 bytes, keyed with the cipher's key and starting from `iv`; then a second CBC-MAC over those in order under the same key and a zero IV; then the 16 bytes folded to 8 as `m[0..4] xor m[4..8]` followed by `m[8..12] xor m[12..16]`. |

`integrity.boundaries` are absolute plaintext offsets, strictly ascending, the last one the
file's size. They are also the only offsets at which the host may still split the download
across parallel connections: the MAC is sequential *within* a chunk and independent *between*
chunks. A run whose connection boundaries do not line up with them **falls back to a single
connection** rather than condensing a value it cannot compute — correct and slower, never fast
and wrong. The host bounds the list at 200 000 entries.

There is no `plugin new --type stream-transform` yet: no SDK template has been written for this
world. Start from any template — every one of them ships the complete contract in
`wit/rdownloader.wit` — set `plugin_type = "stream-transform"` in `manifest.toml` and point
`[package.metadata.component.target] world` at `stream-transform-plugin`.
`plugins/example-stream-transform/` in this repository is a working reference, and
`crates/rd-plugin-ext/tests/stream_transform_contract.rs` drives it.

**MEGA is the first real plugin of this world** (RD-103-02). `plugins/mega/` reads a MEGA file
address, asks the documented command endpoint `POST https://g.api.mega.co.nz/cs` for the storage
address, decrypts the 64-byte attribute block to learn the name and the modification time, and
answers with the address plus the description: `aes-128-ctr` under the 16-byte key folded out of
the link's fragment, the 8-byte counter prefix beside it, `cbc-mac-chain` at MEGA's own chunk
boundaries (128 KiB × 1…8, then a mebibyte), and the 8-byte condensed value the fragment carries.
It computes no payload byte: the keystream and the MAC chain are the host's, which is the rule
ADR 0011 was accepted on. Its sibling `plugins/mega-crawler/` lists a public folder — one call,
every node, no cursor, so the bound is local — and emits each file as
`…/folder/<handle>#<key>/file/<node>`, because a node's key lives in the listing and in no
address MEGA defines. `plugins/mega-common/` holds what both need and neither may own twice.
`crates/rd-plugin-ext/tests/mega_contract.rs` drives both components against a mock serving
answers recorded from the public API on 2026-09-22 and proves, among the rest, that **no request
either plugin makes ever carries the key** — MEGA is the one party that must never learn it.

Three properties of that pair are worth knowing before writing a plugin of this world
(RD-120-11). First, **the key has to reach the vault before the engine can use it**: a
description answers with `key_reference: None`, and `rd_http::StreamTransform::new` refuses one
that still has none, because the reference is part of `ContentTransform::fingerprint`. The
scheduler calls `Database::adopt_transform_key`, which is idempotent — the same key gets the same
reference back, so a continuation still recognises the chunk MACs it wrote itself, while a
different key gets a new reference and the download starts over. Second, **the provider's address
is not its identity**: MEGA answers a fresh storage path on every call, so nothing that has to
survive a restart may be derived from it; node, key and size reach the fingerprint through the
nonce, the reference, the condensed value and the chunk boundaries. Third, **a crawler's own
limit should be the host's**. `MAX_CRAWLED_LINKS` trims a crawler's answer to a thousand links
without saying so, so `mega-crawler` refuses at exactly that number: a folder comes back whole or
comes back as `mega_crawler.too_many_files`, never as a package quietly missing its tail.

A refusal carries the wait the provider asked for. `mega_common::api::retry_after` reads MEGA's
own `X-MEGA-Time-Left` first and `Retry-After` second, and a `509` becomes `rate-limited` with
that number rather than a generic `transient`. The date form of `Retry-After` is deliberately not
read: a guest has no clock to subtract it from, and a guessed number is one the scheduler waits
out literally.

#### Metadata enrichers

An enricher adds fields to a link that has just resolved. It **adds**: a field whose name
collides with something the core resolved — `file_name`, `size`, `provider`, `media` and the
rest — is dropped and counted, so a plugin cannot rewrite what the online check found by
returning a field that happens to be called one of those. What it contributes is stored beside
the core fields, with the plugin that said so and when, and shown that way — on the candidate
in the LinkGrabber, and from RD-107-02 on the package and the queue row it becomes, so the
answer outlives the few seconds an auto-queueing subscription leaves a candidate standing.

Enrichment is **global opt-in**, off by default, and the switch is checked before the plugins
are even compiled. An enricher reaches a service outside the machine, and doing that for every
media link on the strength of somebody having installed a plugin would be a decision nobody
made. `[extension] claims` narrows it further: a plugin that claims `youtube.com` is not told
about anything else, which keeps addresses it could not answer for away from that service.

A plugin may also claim nothing, and the second bundled enricher does. `claims` only helps
where the address carries the responsibility; for an indexer hit it does not, because the URL
points at the indexer and the content is in the file name. Such a plugin is asked about every
link and has to decide for itself — which makes the cheap negative its most important
property, not a detail of it.

Two enrichers are bundled:

**SponsorBlock** reports how much of a YouTube video is sponsor, self-promotion or intro,
before the download starts. It sends a video id and nothing else — no account, no cookies —
and refuses to ask about anything that is not an eleven-character video id.

**Film and series metadata** (`plugins/metadata-enricher/`) reads the release name an indexer
or a torrent hands over — dots instead of spaces, resolution, source, group at the end — and
adds rating, year, genre and runtime. An episode is recognised as one and carries its series,
season and number instead of a film title. The source is Cinemeta at `v3-cinemeta.strem.io`,
the one entry in its allowlist, and it was chosen because it is keyless: an enricher is
instantiated without an account, so the host expands no `{{secret:…}}` on this path and a
service wanting an API key could not be reached at all.

What counts as an episode is either half of the marker: a season, or an episode number on its
own. `Dragon.Ball.DAIMA.E15` names no season and is still an episode — anime and a good deal of
German television number straight through — so the season is simply not shown rather than
guessed at, and the series catalogue is the one asked. A lone `e`-and-digits token behind the
quality and codec markers stays what it usually is, a group name.

A hit is only believed when its own name reads like the one searched for (RD-108-14). Cinemeta's
search is fuzzy — asked for "Apollo Has Fallen" it answers "Paris Has Fallen" — and the measure
is a Dice coefficient over normalised words, articles and punctuation dropped: 75 of 100, or
half that when the entry's year is exactly the year in the release name. Two things keep that
bar off the wrong hits only: accents are folded onto ASCII, because release names are written
that way and catalogues are not ("Shogun" against "Shōgun"), and a trailing qualifier the
catalogue adds to tell like-named entries apart — "Skins (UK)", "Doctor Who (2005)" — is not
counted as part of the title, which for a one-word name is the whole difference between a match
and a refusal. Below the bar there is no field at all, which also means a German release title
the catalogue holds only in English stays silent rather than wrong.

Its negative path is where the work is. A name counts as a release name only when it carries a
release marker — a year, a season/episode marker, or one of the quality, source and codec
tokens — and an extension that settles the question ends it earlier still. `setup.exe`,
`holiday-photos.zip` and `holiday.mkv` produce no field and reach nobody. Nothing it does is a
failure either: a source that is silent, slow or talking nonsense leaves the row exactly as the
online check left it, and an episode keeps its season and number regardless, because those came
from the name and never depended on an answer.

##### What `known` carries

`enrich-subject.known` is "what the core already knows about this subject, as JSON". Its shape
is now something a plugin author can rely on (RD-107-02):

- The resolved media metadata sits at the **top level** of the object, exactly where it always
  did. A plugin that parses `known` as a media object keeps working unchanged.
- An added **`indexer`** key holds a flat string-to-string map of what the indexer declared
  about the subscription hit this link came from — `imdb`, `imdbscore`, `imdbplot`, `coverurl`,
  `resolution`, `size` and whatever else that feed emitted. The key is absent for every link no
  subscription produced: a pasted address, a captured browser download, a container import.

```json
{
  "title": "Some Release",
  "duration_seconds": 5400,
  "indexer": {
    "imdb": "tt0111161",
    "imdbscore": "9.3",
    "coverurl": "https://indexer.example/cover.jpg",
    "password": "1"
  }
}
```

The attributes ride inside `known` rather than in a record field of their own on purpose. A
field added to a WIT record is **not** additive for a component already shipped: every
installed enricher would have to be rebuilt and re-signed before the host could get an answer
out of it again. An added JSON key costs nothing to a reader that does not know it. A case
added to a WIT variant is binary-breaking in the same way — Wasmtime checks a variant for its
exact number of cases when it instantiates a component — and only source-additive; see
[Compatibility](#compatibility).

What reaches a plugin is filtered, and the filter is applied on the way out, not only on the
way into the database:

- Attributes whose **name** is a credential (`apikey`, `api_key`, `auth`, `passkey`,
  `rsstoken`, `secret`, `token`) are dropped whole.
- A credential **inside** a value — the passkey Torznab indexers like to put in `magneturl` or
  `nfo` — is redacted, keeping the structure and losing the secret.
- `coverurl` and `backdropcoverurl` survive only as absolute `http`/`https` addresses; a
  `data:` or relative address is dropped.
- Newznab's `password` is a **flag** (`1` or `2`), never a password. An indexer that wrote a
  real password there has it taken away from the map and kept for the extractor alone; the
  plugin sees `"1"`.
- At most 40 attributes, 1024 bytes per value, 8192 bytes in total.

Start one with `rdownloader plugin new --type enricher`.

### Compatibility

`manifest_version = 3` is the only revision this core accepts, and `api_version` must be one
this build supports. A package written for an older contract is refused with
`plugin.manifest_outdated`, and one asking for an unknown type or capability with
`plugin.capability_unknown`; both stay listed in the plugin manager so they can be recognised
and removed rather than silently disappearing. There is no compatibility promise across these
revisions before 1.0 — rebuild your plugin against the current WIT.

What moves `api_version` and what does not, as decided so far: a new world, a new interface or
a new function on an interface a component already imports is additive in the binary sense — a
component built before the change does not import it and instantiates unchanged — and has left
the version at `0.6.0` every time (crawler world, remote-job world, stream-transform world, the
OAuth functions). RD-110-33 is the plainest case of it: the twelfth interface and world take
nothing away from an existing world, no exported function's signature changes, no record loses
a field and nothing is renamed, so every plugin built against the contract without them still
satisfies the world it declares. An older host refuses a package of the new type outright —
`stream-transform` does not deserialize into its `PluginType`, so the manifest is rejected as an
unknown plugin type rather than half-loaded. A new
**record field** or a new **variant case** is additive only at the source level: your plugin
builds unchanged against the new contract, but a component built before the change fails to
instantiate, because the canonical ABI checks records and variants for their exact shape. The
two captcha kinds of RD-110-15 were added that way, as cases of `captcha-challenge`, and
`api_version` stayed at `0.6.0` because there was no installed base to protect and the bundled
components are rebuilt and re-signed for every release. RD-110-28 went one step further on the
same ground: it **replaced** `account-status.label: string` with `list<label-part>`, which
breaks a plugin at the source level too — a component built before it fails to instantiate,
and a plugin written against the old field fails to compile — and the version still stayed at
`0.6.0`, for the same reason. Once there is a public release, a change of either kind moves the
version.

**RD-120-20 moved it to `0.7.0` anyway**, and it is worth knowing why the rule above did not
decide it. Two changes travelled together: `interface key-derivation` with its world imports,
which is additive in the binary sense and would have left the version alone by the rule; and a
third case on `job-source`, which is additive only at the source level — a remote-job component
built before it fails to instantiate. The second kind had stayed at `0.6.0` twice before. What
made this one different is the *number of copies*: raising the version drags all eleven
`sdk/templates/*/wit/rdownloader.wit` files and every bundled manifest with it, CI checks the
parity byte for byte, and doing that twice in one release is pure churn. So the two changes were
put in one bump, on the project owner's decision of 2026-09-22, and the next addition to either
has the same occasion available to it.

**RD-120-36 moved it to `0.8.0`**, for one new case on `enum link-status`: `cached`. By the rule
above that case alone could have stayed on `0.7.0` — there is still no installed base — and a
component built before it would fail at instantiation either way. It was raised because the
case changes what a check *means*, and a guest built against `0.7.0` cannot say it: better the
clean refusal of a stale package than a check that silently reports a cache as `online`. Every
bundled manifest raised its `api_version` and its own `version`. **`0.7.0` is not kept
alongside:** `SUPPORTED_API_VERSIONS` was `["0.8.0"]` and the linker bound only the `@0.8.0`
interface names, so a `0.7.0` package is refused at the manifest check (`plugin.capability_unknown`,
unknown `api_version`) and stays listed in the plugin manager rather than loading.

**RD-130-11 moved it to `0.9.0`**, for two new functions on `interface remote-job`:
`cache-kinds` and `check-cached`, with the types they carry (`cache-kind`, `cache-query`,
`cache-state`, `cache-answer`). A remote-job component built before them fails to instantiate,
and a plugin of any other world is untouched in source but was rebuilt, re-signed and raised in
`version` like every bundled plugin, because the WIT package version is part of every interface
name the linker binds. `SUPPORTED_API_VERSIONS` is `["0.9.0"]`; a `0.8.0` package is refused
at the manifest check under `plugin.capability_unknown`, the same way `0.7.0` was. It lands
before plugins are distributed through repositories (RD-140-01), on the owner's decision of
2026-09-25.

**What `cached` means.** `online` says the file exists. `cached` says the provider holds it in its
own cache **right now** and can hand it over at once, which is stronger and expires without
notice. The host stores the answer with the time of the check (`cached_at` on the candidate), and
the LinkGrabber shows it as a neutral chip with that time. It never replaces the state word:
the link stays `online`, and the chip shows when the cache was measured, not a promise.
Answer `cached` only for a file your provider said is cached; a file it knows but has not
fetched is `online`. Premiumize's `cache/check` is the first plugin to use it. Since RD-130-11
a `remote-job` plugin can answer the same question for sources no resolver reaches -- a magnet,
an NZB link, a hoster link resolved by another plugin (see "The cache question" above) -- and
`cached_by` names the provider that answered.

`[provider]` is what makes a plugin standalone — and since 1.0.1 it is the *only* thing that makes a provider at all. Every installed manifest contributes a row to the provider registry, so the new slug immediately supports account creation, credential gating, link intake and the HTTP sandbox; nothing is compiled into the binary any more, and the registry is rebuilt whenever a plugin is installed, removed or switched off. An installation with no plugins therefore offers no providers, which is the honest answer: an account for a provider nothing can resolve was never usable.

Constraints the core enforces:

- A plugin can never take a slug another plugin claimed first (a newer version of the *same*
  plugin id replaces its own row). Until 1.0.1 eleven slugs were additionally reserved by rows
  compiled into the binary; that reservation is gone, and what protects a well-known provider now
  is signature trust — an untrusted package does not load — together with the secret-reference
  rule below, which is the half that was ever load-bearing.
- A plugin can never claim a secret reference another provider owns — that would widen where an existing credential may be sent.
- Every secret reference must be listed in `capabilities.secrets`, and `cookie_scope` needs the `cookies` capability: the manifest must not show a grant the runtime does not enforce.
- Every slot's domains and `cookie_scope` must be covered by `capabilities.net_http.domains`.
- `transfer_auth = "basic"` needs `credentials = "api_key"` or `"username_password"` and exactly
  one secret slot with domains: the engine sends that one credential to those hosts and nowhere
  else (RD-120-38).
- `credentials = "login_or_api_key"` offers the user a choice between signing in with a username
  and password and pasting a ready-made API key. Such a row describes both in
  `[[provider.secrets]]`, one entry per mode, instead of the singular
  `secret_reference`/`secret_domains` pair — the two spellings cannot be combined. Each account
  then stores which mode it uses, and the host admits only that mode's slot: a resolver in
  `api_key` mode cannot have the key posted to a login form, and one in `login` mode cannot have
  the password sent to the API host. The resolver learns the mode from `secret-available`, which
  answers per reference, so the mode itself never crosses the sandbox boundary.
- `credentials = "oauth"` declares a provider signed in through a redirect and kept alive by
  renewal. The single secret slot holds the credentials of the person's own registered OAuth
  client — the project ships no client id of its own — and everything after that is fetched by
  the flow: the code the callback carries, the access token, and the material that mints the
  next one. Both tokens go to the vault through `store-oauth-token`, and only the expiry is
  kept in the clear, because the renewal sweep has to be able to ask whether a flow is due
  without decrypting a secret to find out. The flow itself belongs to an `oauth` plugin; a
  provider declaring this without one installed is offered no sign-in.
- `credentials = "none"` declares a provider that takes no account: it registers for resolving
  and never appears in the accounts settings. Such a row must not describe an account either —
  no `secret_reference`, no `secret_domains`, no `cookie_scope`, no `username_required` — because
  each would be a credential nobody can enter. It exists for a resolver that spans several
  installations of the same hosting script, where an account at one is not an account at
  another, and only the account-less flow can be shared.

## Translations

Everything specific to a provider lives in the package, not the core: its failure codes, display name, description and credential labels.

```json
{
  "name": "FastShare",
  "description": "Resolves FastShare links with a premium account.",
  "account": {
    "secret_label": "Account password",
    "secret_hint": "Enter your account e-mail as the user and your password as the secret."
  },
  "codes": {
    "fastshare.bad_credentials": "FastShare login or password is wrong",
    "fastshare.limit_reached": "The FastShare traffic limit was reached"
  }
}
```

Every key under `codes` must start with `<provider.slug>.`, so a plugin can never override the core's `plugin.*` catalogue or another provider's strings. `locales/en.json` is mandatory whenever any locale ships; the UI serves the active language and falls back to the plugin's English, then to the English text the backend sent.

The resolver reaches the UI through `Failure { code, params }`: emit a stable code plus flat parameters, and the web app interpolates the translation. Codes outside your namespace (`link.*`, `plugin.*`) are core codes translated by the core.

### The account label

`account-status.label` goes through the same channel as a failure. It is a **list** of
`label-part { code, params, message }` records: the interface translates every part on its own
— `code` in the active language, then in English, with `params` interpolated — and joins them
with ` · `. `message` is the English, redaction-safe text for a code no catalogue knows; it is
the last rung, never the channel. A part whose `code` is empty is refused by the host with
`plugin.account_label_invalid`, so free text cannot be smuggled past the catalogue. An empty
list is a valid answer: it says the check has nothing to add to `valid` and `premium`.

What every plugin says the same way has a **core code**, translated in four languages by
`web/src/locales/*/server.json` and constructed once in `plugin-common` (`Label` in
`plugins/common/src/label.rs`), so no plugin formulates or translates it:

| Code | Params | Says |
|---|---|---|
| `plugin.account.user` | `user` | Signed in as *user* — the name or address the provider echoed, bounded to 80 characters and stripped of controls |
| `plugin.account.premium_until` | `until` | Premium until *until*, the date as the provider states it |
| `plugin.account.premium_lifetime` | — | a subscription that never runs out |
| `plugin.account.premium_expired` | — | a subscription the provider reports as run out |
| `plugin.account.premium_unchecked` | — | the check proved the credentials and read nothing about the subscription (see below) |
| `plugin.account.cookies` | `count` | *count* cookies loaded; zero says a cookie session is required |
| `plugin.account.signed_in` | — | the plugin signed in with the stored credentials during this check |
| `plugin.account.session_active` | — | the check found the session already established |

What only your provider says stays in your namespace and your catalogue, exactly like a failure
code: `LabelPart::coded("<slug>.account.tier", "Tier gold").with_param("tier", "gold")` with
`"<slug>.account.tier": "Tier {tier}"` in each `locales/*.json`. Two bundled plugins do this —
`premiumize.account.fair_use` for the consumed fair-use share and `1fichier.account.access`
for a plan only 1fichier has.

Leave out what the row already shows: the provider's name (the account row names it) and the
word "premium" (the interface prints `premium` itself, as *Premium active* or *premium status
not confirmed*). Explanations about the *application* — why the browser stays signed out, what
a cookie session is — belong in `web/src/locales/`, not in a label.

## What an account check may assert

`account-status.premium` is a finding, not a default. Answer `true` only where this call read
the subscription — an expiry from the provider's API, a plan the account page states — and
`false` everywhere else, including where the call proved the *credentials* and nothing more: a
sign-in that succeeded, a cookie session that carries the sign-out link, an API key that
answered. Those prove the account works; none of them says it is paid for, and a free account
sent through the same code path would be reported as premium. Where the value is unread, say so
in the label — the part `plugin.account.premium_unchecked` exists for exactly this — rather than
leaving the caller to guess, because `false` on its own cannot be told apart from a subscription
that has lapsed — which is why the interface reports it as *premium status not confirmed*
rather than as an absence (RD-109-34).

## How strictly a provider's answer may be read

A provider's documentation states a shape; production sends that shape plus whatever it has
always sent. Both halves of that are a plugin author's problem.

- **Read every field you do not need as optional.** Premiumize documents `transfer/directdl`'s
  `content[].path` as a string and `content[].size` as a byte count, and the same account's
  `cache/check` quotes a size as a string in its own published example. A `String` or a `u64`
  without `Option` turns any of that into a total read failure, when a missing name or an
  unreadable size would have cost nothing — the link is the only field a resolver cannot do
  without. Accept a number, a float and a quoted string wherever a count arrives, treat `null`
  as an absent field, and keep a fallback where the provider offers one, deprecated or not.
- **A read failure is `Permanent`, never `Transient`.** A body whose shape does not fit will not
  fit on the next attempt either, so a transient classification walks the job to `max_retries`
  and fails identically every time. This was exactly the reported premiumize symptom: a link
  stuck in `Retrying` under *Invalid Premiumize response* (RD-109-44).
- **Say which field failed, and only which field.** Discarding the serde error leaves nobody
  able to name what broke, including the next person to read the report.
  `serde_path_to_error::deserialize` keeps the path — `content[0].size` — which is what a
  failure message needs. Do not put the *value* in the message: a response body may hold a
  signed delivery link or a token, and a `Failure` message is meant to be redaction-safe.
- **Match the provider's error codes, not remembered ones.** Where a provider publishes a stable
  `code` vocabulary, match that vocabulary and keep old strings only as aliases. Codes that no
  longer exist match nothing and every real answer falls through the catch-all arm, which is how
  premiumize's `service_down` came to be permanent and its `service_unsupported` came to be a
  generic failure. Where no code is sent at all, the message is the only other thing the envelope
  carries — read it rather than calling the answer permanent unseen.

## Keys and trust

```bash
rdownloader plugin keygen --output ~/.config/rdownloader
```

produces `rdownloader-plugin.key` (PKCS#8 PEM, private, permissions 0600) and `rdownloader-plugin.pub` (Base64). Put the Base64 public half into your manifest's `public_key`; the private key stays out of the repository (for our releases, the GitHub secret `RDOWNLOADER_PLUGIN_SIGNING_KEY`).

The release key is embedded in the `EMBEDDED_KEYS` table in `crates/rd-sign/src/roots.rs`, alongside the roots for application updates and the external-tool manifest; `crates/rdownloader/src/trusted_keys.rs` re-exports its id as `RELEASE_KEY_ID` (`rdownloader-release-v1`), matching the bundled manifests' `key_id`.

**Third-party keys use trust on first use.** Installing a package signed by an unknown key does not fail outright: the core verifies the package against the key the manifest itself declares (proving the author holds it), then reports the key's fingerprint. Settings → Plugins shows it and asks for confirmation; confirming records the key so the plugin still verifies after a restart. From the CLI:

```bash
rdownloader plugin keys trust --from-package fastshare-1.0.0.rdplug --yes
rdownloader plugin keys list
rdownloader plugin keys revoke fastshare-release-v1
```

Publish your fingerprint somewhere users can check it — that comparison is the whole security of the flow. Revoking a key blocks new installs at once and skips its already-installed plugins from the next start (with a warning; startup never fails because of one bad package).

A trusted `key_id` is pinned to its key: a package cannot claim someone else's key id with a different key. `--trusted-plugin-key KEY_ID=BASE64` still pre-trusts a key at startup, and `--no-default-plugin-key` disables the embedded release key.

Rotation: generate a new key, set the manifest `key_id` to `…-v2` and `public_key` to the new key, ship one release trusting both, then revoke the old one.

**Withdrawing one version, without revoking its key.** Revoking a key takes down every plugin its author ever signed, which is far more than a single bad release deserves. The second axis of the trust store withdraws one exact package by its *content digest* — the frozen length-prefixed hash over `manifest.toml`, the component and the locales, the same payload the signature covers, and not a hash of the `.rdplug` file:

```
GET    /api/v1/plugins/revocations
POST   /api/v1/plugins/revocations     {"digest": "<64 hex>", "reason": "…"}
POST   /api/v1/plugins/revocations     {"plugin_id": "fastshare", "version": "1.0.0", "reason": "…"}
DELETE /api/v1/plugins/revocations/{digest}
```

All three cost `api:secrets`, like the key routes. The id-and-version spelling hashes what is installed here and is how "withdraw the version I am looking at" is expressed; the digest spelling works for a package that is not installed at all. Withdrawals are stored (`plugin_digest_revocations`, migration 0073) and read back at start, so they survive a restart, and the plugin's author keeps their key. Nothing already running is torn down: the refusal applies the next time plugins are loaded, exactly as a revoked key does.

A *signed*, remotely fetched revocation list is deliberately not built: it needs its own trust anchor and a distribution decision, and neither belongs in the local trust store.

## Building a package

```bash
cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-ddownload
rdownloader plugin package \
  --manifest plugins/ddownload/manifest.toml \
  --component target/wasm32-unknown-unknown/release/rd_plugin_ddownload.wasm \
  --locales plugins/ddownload/locales \
  --key ~/.config/rdownloader/rdownloader-plugin.key \
  --output dist/plugins/ddownload-0.10.1.rdplug
rdownloader plugin verify dist/plugins/ddownload-0.10.1.rdplug
```

`--development` produces an unsigned package that only `serve --plugin-development-mode` or `verify --development-mode` accepts. The packager validates the manifest, the locales and the component (no WASI imports, sandbox compilation), checks that `public_key` matches the signing key, and re-verifies the finished archive before writing it.

`scripts/build-plugins.sh` does the same for every bundled plugin and leaves exactly one
`.rdplug` per plugin in `dist/plugins`: the file name carries the plugin's own version, so it
removes the packages the new one supersedes instead of letting them pile up. Without that, a
version bump left two packages for one plugin and the count check of the packaging scripts
stopped the release build on the surplus.

`target/wasm32-unknown-unknown/release/` is per checkout and no `cargo test` ever fills it, so a
component built before a branch was merged sits there unchanged while the plugin's sources move
on. The contract tests compare the two before using a component and fail with the build command
when it is behind; `scripts/build-plugins.sh --list-stale` answers the same question without
running anything, and `scripts/check.sh` asks it before it starts.

A component that was never built fails the same way, and since RD-108-16 that is the point: it
used to make the test return early and count as *passed*, so a fresh checkout could report a
full contract suite green in three seconds having loaded no component at all — the same number
the real run produces in two minutes. `scripts/build-plugins.sh --list-missing` names the ones
that are not there, and `scripts/check.sh` stops on them before it spends anything. A checkout
that cannot build components at all — no `wasm32-unknown-unknown` target, no `cargo-component` —
leaves those tests out on purpose with `cargo nextest run -P no-components`
(`.config/nextest.toml`), which counts them as *skipped* instead. CI does exactly that in its
`rust` job and runs them for real in the `components` job.

## Superseded versions

Installing never removes the version it replaces. Two version directories under one plugin id
are the normal state after an upgrade, and the highest installed SemVer is the one that loads;
the other one stays because a job already under way keeps the version that started it.

Settings -> Plugins shows that as one card per plugin, not per directory: the loaded version is
the card, and the superseded ones sit under it in a collapsed sub-entry. The count beside the
heading and on every type chip therefore counts plugins, which is the number to compare with the
`.rdplug` files a release ships.

A superseded version can be removed by hand, from the sub-entry or with
`DELETE /api/v1/plugins/{id}/{version}`. The request is refused with `409` and the code
`plugin.version_in_use` while unfinished work is bound to that exact version:

- a **resolver pin** (`download_resolver_pins`), claimed by the job the first time it resolved,
- a **transfer checkpoint** (`plugin_transfers`), which only the version that wrote it can read.

Only a **completed** download holds nothing -- its rows merely remember which version once did
the work. Everything else still holds its version, a cancelled job included: cancelling keeps the
partial data and resuming puts the job back in the queue, so its checkpoint is a claim like any
other. A job that is really gone is deleted, and deleting it takes its pin and its checkpoint
with it. The guard applies to the loaded version too, so uninstalling a plugin cannot pull a
running job's component away either.

The refusal names the first blocking downloads as well as their number, so the job that is in
the way can be found in the queue instead of searched for.

This is deliberately not the in-memory lease `rd-tools` keeps for external binaries. A lease is
forgotten on restart, which is right for a running process and wrong here: a paused job has to
still hold its version after a restart, and the two records above are exactly that claim, on
disk.

Nothing removes a superseded version on its own. It is the user's artefact, and a plugin
directory that vanishes without a word explains nothing.

## Diagnostics

Every plugin invocation is recorded: which plugin, which version, which entry point, how it
ended and how long it took. Settings → Plugins shows the newest entries per plugin, and
`GET /api/v1/plugins/{id}/executions` returns the same list.

The outcome separates the things a user acts on differently — `ok`, `failed` (the plugin
worked, the hoster said no), `crash`, `timeout`, `fuel`, `memory`, `host_error` and `denied`
(it asked for something its manifest does not grant). A run of `denied` means a manifest that
promises too little; a run of `crash` is a bug report for the plugin's author.

The message passes the same redaction every persisted failure passes, so a credential or a
session token in a plugin's error text never reaches the history. Each entry carries a bare
correlation id that names the entry and nothing else, so it can be quoted in a report. The
history is capped per plugin and is not backed up: it is a diagnostic, not a record.

## SDK

`sdk/` in this repository holds what a third party needs:

- `sdk/templates/{resolver,transfer,intake,auth,oauth,crawler,enricher,notifier,postprocess,storage,remote-job}`
  — the scaffolds `rdownloader plugin new` writes, one per type. They carry their own copy of
  the WIT contract, so a scaffolded plugin builds with no checkout of this repository, and CI
  here fails if that copy drifts from `crates/rd-plugin-api/wit/`. `plugin new` itself reads
  the templates from `sdk/templates` in the current directory, so it runs from a checkout.
- `sdk/ci/plugin.yml` — a GitHub Actions workflow that builds, reads the component's import
  section with `wasm-tools` to reject accidental WASI imports, packages, verifies and runs
  conformance against a pinned rDownloader release.
- `sdk/README.md` — the short version of this page for someone starting out.

```bash
rdownloader plugin new --type resolver --out ./myhoster
```

writes a plugin that compiles, packages and passes conformance before a line of it is changed,
along with its own Ed25519 key pair, so the first failure an author sees is about their code.

`rdownloader plugin conformance <package> [--json]` answers one question: would this core run
this package? It verifies archive, manifest and signature exactly as installation does, checks
the ABI version, instantiates the component against the world its type declares, and — for a
hoster resolver — asks the component whether it claims links belonging to no hoster it declares,
which is the misconfiguration that turns every direct download into "this hoster needs an
account". It exits non-zero if anything failed.

It does not check that a plugin resolves a real link: that needs a hoster, an account and a
network. A package that passes every check can still be wrong about its provider.

## Distribution

- The web UI installs a package at Settings → Plugins; the CLI does the same with `rdownloader plugin install`.
- On startup, `serve` installs all `*.rdplug` from `plugins/` next to the executable (or `--bundled-plugins` / `RDOWNLOADER_BUNDLED_PLUGINS`), provided the version is not yet installed or is newer than every installed version. Older versions are left in place, so jobs pinned to one keep resolving.
- **A restart is required before a newly installed resolver runs.** Resolvers are built once when the scheduler starts. The provider row and the plugin's translations, however, are served from disk and take effect immediately, so an account can be created right after installing.
- Release workflow: the `plugins` job builds the components, packages and signs them with the secret and stores them as an artifact; the binary archives receive a `plugins/` directory, the container image copies them to `/usr/local/lib/rdownloader/plugins`.
- WIT version: the plugin package is `rdownloader:plugin@0.9.0`. A resolver exports `match-url`, `check-account`, `resolve`, `check` and `hosters`, and imports `host` plus whichever of `http`, `cookies` and `captcha` its manifest grants; `captcha` offers `solve-captcha` (token answers) and `solve-challenge` (any answer shape, including the click-point coordinate). Incompatible installed components are logged with plugin id and version at startup and skipped; a bundled newer version can thereby replace them.
