# rDownloader browser extension

Companion for NAS/Docker setups where the desktop capture agent cannot be installed: a
context-menu entry **Download with rDownloader** on links and pages, a popup that sends the
current page or the links in your clipboard, interception of regular browser downloads, and a
second context-menu entry that shares a site's session with rDownloader for one domain. That one
is in the page's context menu only — there is no button for it in the toolbar or the popup. The capture agent remains
the option for Click'n'Load 2, clipboard monitoring and `.nzb` file association.

## Download interception

A regular browser download is paused, handed to the LinkGrabber together with the metadata
needed to repeat the request, and only removed from the browser once the server has accepted it.
A failed handoff resumes it in the browser, so an unreachable server never loses a download.
Interception can be switched off globally in the options, and each individual download can be
kept in the browser from its notification.

### What stays in the browser

The handoff carries the address, not the file: rDownloader fetches it again, without the
browser's cookies. Some downloads only the browser can fetch, so the extension leaves them alone
(RD-120-63):

- **NZB, torrent and ZIP files** are never intercepted, and nothing is said about it. They are
  recognised by the type the browser reports (`application/x-nzb`, `application/x-bittorrent`,
  `application/zip`, `application/x-zip-compressed`) or by a `.nzb`, `.torrent` or `.zip` ending
  on the file name, the address or an observed `Content-Disposition`. This is the case the rule
  exists for: a Newznab indexer's cart (NNTmux's `/getnzb/…`) answers only the signed-in
  session, and a fetch without it gets an error page; a one-time link is spent before the
  extension even hears of the download.
- A download whose request the extension **observed as a `POST`** (below).

To get those files into rDownloader anyway, point a **hotfolder** at the browser's download
folder (routing page, **Hotfolders** tab; one poll interval for all watched folders, 30 seconds
by default). It picks up `.nzb` and `.torrent` files once the browser has finished them —
`.part` and `.crdownload` files are not candidates — and routes them like any other intake. A
watched folder does not take `.zip`: unpack an indexer's ZIP into that folder, or upload the NZBs
in the LinkGrabber — or allow the site, below.

### Files from a site you allowed

For a site you allow, its NZB, torrent and ZIP downloads go to rDownloader after all
(RD-130-16). Open the popup **on that site** and press **Hand NZB, torrent and ZIP files from
… to rDownloader**; the browser asks for access to that one host and for `cookies`. The same
button takes the consent back, and the options page lists every allowed site with a **Revoke**
button. Without a consent nothing changes: the file stays in the browser, as above. **Take over
browser downloads** has to be on.

rDownloader then takes the file in one of two ways, and never fetches it a second time:

- **Firefox hands over the bytes it received.** `webRequest.filterResponseData` copies the
  response of the navigation and holds it back from the browser until rDownloader has it. Taken,
  the browser gets nothing and its empty download is removed; refused, the browser receives every
  byte and finishes the download as before. Only the Firefox build carries `webRequestBlocking`
  and `webRequestFilterResponse`. **Firefox may drop such a filter for a response it turns into
  a download** (Mozilla bug 1787119: the stream is retargeted to the download handler before
  the filter sees data); the extension then falls back to the second way.
- **The address and that host's cookies** (Chrome, and Firefox's fallback). The extension reads
  exactly the cookies the browser would send to the download's address (`cookies.getAll({ url
  })`), and posts address, cookies — as the Netscape rows a session share sends — referrer and
  user agent to `POST /api/v1/capture/file`. A cookie the browser would not send to that
  address refuses the hand-over.
  rDownloader fetches it **once**, sends the cookies only to that address's own scheme, host and
  port — a redirect elsewhere goes without them — and keeps them nowhere: no cookie store, no
  row, no log line. The browser has usually fetched the file already, so an indexer counts this
  as a second download (not three or four, as a LinkGrabber hand-over would).

rDownloader takes an NZB, a `.torrent`, or a ZIP of NZBs (NNTmux's `/getnzb?id=…&zip=1`, one
import per NZB inside; a ZIP with one broken NZB is refused whole). An NZB lands as an import in
the LinkGrabber, a torrent as a LinkGrabber package. Anything else — a ZIP of photos, an expired
session's login page — is refused and stays in the browser without a notice; any other failure
says why and leaves the browser its copy. The browser's copy is removed only after rDownloader
took the file. A hotfolder on the browser's download folder can see that copy for a moment; an
NZB that arrives twice is one import, because imports are keyed by the file's hash.

The session share and the session handover below give `cookies` back after their read; while a
site holds this consent, they leave it granted, and they never take that site's own grant.

A download the page started with a **`POST`** is not handed over. Repeating such a request needs
its body, and the extension does not read request bodies: the path that once did was never
reachable in a normal installation — nothing in the extension ever asked for the broad host
access it required — so it was removed rather than made reachable (RD-109-20). Such a download
stays with the browser, which finishes it, and the notification says why. rDownloader's own
capture contract still accepts a body; the extension simply never sends one.

"We watched and saw nothing" and "we could not watch" are two different answers, and the
extension keeps them apart. `webRequest` events only arrive for addresses the extension holds
host access for, so in a normal installation it observes nothing at all — a plain download is
then handed over on what the download API itself reports, as it always was. That has a price
worth saying plainly: neither Chrome's nor Firefox's download API names the method, so **a
normal installation cannot tell a `POST` from a `GET` and still hands such a download over** as a
`GET`. Only a `POST` the extension actually saw is kept, and since the observed-`POST` check runs
before the pause, the browser's download is never paused for it. Where the extension
*does* hold host access for the address and still finds no matching request, the download is
kept by the browser and says so, because a request that was started by a form and then aged out
of the correlation buffer is exactly the one that must not be repeated as a `GET`.

The buffer holds fifty requests, and a `GET` is dropped from it before any `POST`: a page that
polls in the background used to push the one entry that decides a handoff out of the window
within seconds.

A service that does not answer at all is reported as unreachable, not as too old, and the
browser keeps the download. "Too old" is reserved for a build that answered and does not know
the capture contract.

Chrome puts the extension's service worker to sleep after about thirty idle seconds. Everything
that has to be said once — the "configure server and token first" hint, the "this rDownloader is
too old" notice — and the capture version negotiated with the service therefore live in
`storage.session` with an expiry, not in the worker's memory: as memory they came back on every
single download, which turned one hint into a nuisance and put an extra `/capture/ping` in front
of every handoff. They are gone when the browser restarts, which is the right lifetime for them.

Credential-bearing headers such as `Cookie` and `Authorization` are dropped in the extension and
again on the server; they never travel with a captured link.

## Widget captchas

A reCAPTCHA, hCaptcha or Cloudflare Turnstile widget is bound to the hoster's domain. The
desktop agent used to open an embedded WebView for it; Turnstile refused that window, measured
twice, so RD-109-11 removed it. The extension is now the only way a person answers one, and it
does so in the browser you already use:

1. Once paired, the extension polls rDownloader every 30 seconds for waiting widgets. A new one
   is announced with a notification and a badge, and listed in the popup.
2. **Answer on the hoster's page** asks the browser for permission for *that page's origin* —
   `https://ddownload.com/*`, not its subdomains and never all sites — opens the hoster's own
   page in a new tab, and injects a small reader once the page has loaded. The reader watches the
   widget's answer field (`cf-turnstile-response`, `g-recaptcha-response` or
   `h-captcha-response`) and changes nothing on the page.
3. You solve the widget as you would on any site. The token is posted to
   `POST /api/v1/capture/captchas/{id}/token`, the tab closes, and the permission is released.
4. Closing the tab without answering declines the captcha; the download fails with
   `captcha.skipped` instead of waiting out its timeout. **Decline** in the popup does the same.
5. **A page that shows no widget at all is reported, not watched** (RD-120-45). The service met
   the widget on a page it fetched as a guest — DDownload's sign-in, say — while this browser is
   signed in there and is sent straight past the form. If none of the three answer fields, no
   widget container and no widget frame has appeared 15 seconds after the page loaded, the reader
   sends `POST /api/v1/capture/captchas/{id}/no-widget` once and stops. The waiting sign-in ends
   with `captcha.page_without_widget`, which names the hoster and the two ways out: take the
   session over (below) or sign out in the browser and test the account again. The tab stays
   open — it is showing you your own signed-in page — and is forgotten, so closing it declines
   nothing. A widget seen once counts as present for good, so one that resets is never reported.

**The tab closing is not the report — the notification is.** A tab closes on a token the server
rejected exactly as it does on one it took, and it closes again when a poll finds the captcha no
longer waiting. So every ending says which hoster it was about and what the server answered:
*accepted*, *could not be handed over* with the reason, *declined*, *rDownloader was not told that
it was declined* with the reason, or *no longer waiting*. The one message the flow will not give
you is a decline it has not seen confirmed (RD-109-23).

The token is never stored, never shown in a notification and never logged, in the extension or
on the server. Nor is anything else about a captcha: tab ids, origin grants and notification ids
live in `storage.session` and, in a browser without it, in the worker's memory — never on disk,
so a browser restart can never make the extension close a tab it did not open. The web
interface's captcha prompt reports whether an extension is connected, because the poll names
itself (`?client=browser_extension`).

## Sharing a session

**Share session for this site with rDownloader** in the page's context menu asks the browser for
the `cookies` permission together with a host permission for that one site, reads the cookies the
browser would send to the page you are looking at, and posts them to rDownloader. The profile
arrives disabled and has to be approved in the web interface; no cookie value is ever shown in a
notification or written to a log.

Which cookies that is, is a question the browser answers: `cookies.getAll({ url })` returns
exactly what it would send to that address, **including the ones a parent domain set**. That
matters, because a login cookie is usually set on `.hoster.com` while you are on
`www.hoster.com` — reading by domain alone would return everything *below* the host and nothing
above it, and produce a profile that authenticates nobody. Cookies named `__Host-` or `__Secure-`
travel like any other; the prefixes are attribute rules, not a hint that a cookie cannot leave
the browser.

By default the share also covers the site's subdomains, and the permission it asks for says so
(`*://*.hoster.com/*`). A share without them reads only the page's own cookies and asks only for
`*://hoster.com/*` — the option bounds the read it names.

## Handing a session over to an account

Different from the share above, which makes a disabled *domain profile*: this puts a hoster
session on a **provider account**, exactly where cookies typed into the account form go
(RD-120-45). It exists because the service cannot read your browser's cookies, and a sign-in
whose page the browser skips — you are logged in there already — could otherwise only wait.

1. In the web interface, **Take over from browser** on the account (shown only when the
   provider's plugin declares a `cookie_scope`) opens a request. It names the site from that
   scope; nobody types or picks a domain. It waits five minutes.
2. The extension finds it on its next poll (`GET /api/v1/capture/browser-sessions`), announces
   it once, and lists it in the popup with the site and the account's label.
3. **Hand over this session** in the popup is the consent. Its first act is the browser's own
   permission prompt for `cookies` and exactly `https://<scope host>/*`; the consent is recorded
   in the background before the prompt, so a popup the browser closes under the prompt loses
   nothing — click again and it goes through without a second prompt.
4. The background asks the service for the request again and takes the site from **that**
   answer, never from the popup or a page; a consent recorded for another origin is refused. It
   reads the cookies the browser would send to the scope, keeps only those set on the scope's
   host or a domain above it — nothing beside it, below it or elsewhere — and posts them as
   Netscape rows to `POST /api/v1/capture/browser-sessions/{id}` with the capture token, to the
   rDownloader you paired with and nowhere else.
5. `cookies` is removed again right after the read, and the origin too unless it was granted
   before this consent. The web interface shows the session as arrived and checks the account.
   **Decline** tells rDownloader, and the row says so.

The service applies the same domain rule and refuses the whole set if one row is outside the
scope, stores the rows in the vault behind the account's `cookie_ref`, and never echoes them.
A capture token cannot start a request, choose the account or the site, or answer one twice. None
of this is reachable over MCP.

**Why `cookies` stays optional.** Both browsers let `cookies` be an `optional_permissions` entry
and request it at run time together with the origin, from a click. So installing the extension
warns about no cookie access at all, the browser renders the consent itself and names the site,
and the grant does not outlive the read. Asking at install would be a standing grant over every
site's cookies that nobody asked for; the price of asking at run time is one browser prompt per
handover, which is the point.

## Permissions

Requested on installation:

| Permission | Why |
| --- | --- |
| `activeTab` | reading the address of the tab you are on, and only while you use one of the extension's own entries on it — it is what makes "send the current page" work at all |
| `contextMenus`, `storage`, `notifications` | menu entries, saved settings, success/failure notices |
| `clipboardRead` | the popup's "send links from the clipboard" action |
| `downloads`, `webRequest` | pausing, resuming and *observing* a download so it can be handed over: the method of the request, an allowlist of harmless headers, and the `Content-Disposition`. No listener reads a request body |
| `alarms`, `scripting` | polling for waiting widget captchas even after the service worker was put to sleep, and injecting the reader into the hoster's page you chose to open |

Requested only when you use the feature, and revocable at any time in the browser:

| Optional permission | Why |
| --- | --- |
| `cookies` | sharing a site's session for **one** origin you pick, or handing a hoster session over to the account that asked for it — the site named by the provider's plugin; never for all sites, and given back after the read. Kept while a site is allowed to hand files over, which reads the cookies for one download address at a time |
| host permissions | reaching an rDownloader server that is not covered by the built-in `http://127.0.0.1:8710/*` and `http://localhost:8710/*`; per captcha, for the one origin of the hoster's page, released again once answered or declined — opening that page to answer a widget captcha; and `*://<host>/*` for each site you allow to hand files over, until you revoke it |

The Firefox build additionally carries `webRequestBlocking` and `webRequestFilterResponse`,
which copying a response takes; neither reaches a site without the host grant above. Chrome's
build has neither.

The manifest's `optional_host_permissions` carries `http://*/*` and `https://*/*`. Neither is
granted at install time and neither shows a warning; they are what makes the three per-origin
requests above *legal*, because both browsers refuse `permissions.request({ origins })` for an
origin no manifest pattern covers. Every grant is for one origin at a time and, for a captcha,
released again as soon as the widget is answered or declined.

## Build

```bash
scripts/build-extension.sh               # tests, then builds Chrome and Firefox
scripts/build-extension.sh --skip-tests  # build only
scripts/build-extension.sh --test-only   # unit tests only
```

The tests are `node --test extension/test/*.test.mjs` and need nothing but Node — no dependency,
no bundler, no browser. `extension/package.json` exists only to say `"type": "module"`, so Node
does not have to guess the module kind from the syntax; without it the run printed a reparse
warning pointing at a `package.json` outside this repository and failed outright on Node 20.

`extension/test/fakes.mjs` is the one fake browser the suite drives everything through, plus the
two setup helpers over it. It is deliberately not named `*.test.mjs`, so the glob above does not
try to run it as a test file.

What the suite covers beyond the pure helpers: the background's wiring — that every listener is
registered while the worker script is first evaluated, which is what Manifest V3 requires, the
context menu, the send with its unconfigured, success and failure branches, the message routing,
and the `extraHeaders` retry Firefox needs — the configuration and translation layer, a real
`build()` into a temporary directory with both manifests parsed again, the four catalogues
against each other *and* against the code that reads them, and the capture contract version
against `crates/rd-core/src/capture.rs`. A browser run with a real Chrome and a real Firefox is
still the acceptance nobody can do from here.

The build writes `artifacts/browser-extensions/chrome` and
`artifacts/browser-extensions/firefox`, plus one ZIP archive per target in the same directory.
The archives are the release artefact — the release workflow uploads them and a store accepts
nothing else — so `zip` and `unzip` are required, a failed archive fails the build instead of
being skipped silently, the closing line says which archives are actually on disk, and the
wrapper checks afterwards that both exist and each contains a `manifest.json`.

## Version

The extension carries the workspace version. `scripts/set-version.sh <X.Y.Z>` writes it to
`manifest.base.json` along with `Cargo.toml` and `web/package.json`, and
`extension/test/build.test.mjs` fails if the two ever drift apart; a Cargo pre-release suffix is
dropped, because a browser manifest version is dot-separated integers only. Extension and service
are only useful as a pair and negotiate a versioned capture contract, so one number for both is
the number that answers "which build is this?".

Before RD-109-17 the manifest said `0.1.0` in every release ever built. The extension had never
been submitted to AMO or the Chrome Web Store — there is no published `0.1.0` for a store to
compare against — so the jump to the workspace version costs nothing and the first submission
starts from 1.0.8.

## Install

- **Chrome / Edge**: `chrome://extensions` → Developer mode → *Load unpacked* → `artifacts/browser-extensions/chrome`.
- **Firefox**: `about:debugging#/runtime/this-firefox` → *Load Temporary Add-on* → `artifacts/browser-extensions/firefox/manifest.json`
  (or install the zip as a permanent add-on after signing it through AMO).

## Pairing

1. In the rDownloader web interface open **Settings → Desktop client** and create a capture token
   (shown once).
2. Open the extension options, enter the server URL (e.g. `http://127.0.0.1:8710` or `http://nas.local:8710`)
   and the token, press **Test connection**, then **Save**.

**Which addresses need a permission.** The manifest holds `http://127.0.0.1:8710/*` and
`http://localhost:8710/*` from the moment the extension is installed, and nothing else. Every
other address — including a loopback one on a different port, such as `http://127.0.0.1:9000` —
is asked for when you press **Save**. "Loopback" is not the rule; what the manifest declares is,
and `permissions.contains` is what answers. Until RD-109-24 the options page skipped the request
for any loopback address regardless of port, so such a service had no host permission at all and
worked only because rDownloader answers `Access-Control-Allow-Origin: *` — an undocumented
dependency on a server header, which is now gone. An address the browser cannot parse, such as
an unbracketed `::1`, is named as invalid instead of failing silently.

The extension talks to five groups of routes with the bearer token: `POST /api/v1/capture/batches`
for captured links, the `/api/v1/capture/captchas` routes for widget captchas,
`GET /api/v1/capture/ping` for the connection test and the server's version,
`POST /api/v1/capture/cookies` when you share a site's session, and the
`/api/v1/capture/browser-sessions` routes when you hand a session over to an account. The last
two are the ones your cookies travel over, each for the single origin it names. Over plain HTTP the
token is visible on the network; put a reverse proxy with TLS in front of rDownloader for remote
access.

## Supported browsers

Firefox 156 and Chrome 153, and nothing below: the manifests declare those floors
(`strict_min_version`, `minimum_chrome_version`), so an older browser refuses the install rather
than running untested. Raise the constants in `extension/build.mjs` when the supported version
moves; the build's test asserts the manifests against them (RD-109-47).
