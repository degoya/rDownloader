# ADR 0007 — No FileFactory resolver

- **Status:** Accepted
- **Date:** 2026-09-21
- **Job:** RD-103-09
- **Supersedes:** —

## Context

RD-103-09 asks for a signed FileFactory resolver *if* a stable and permissible route exists,
and for a record of the No-Go otherwise. The feasibility measurement of 2026-09-21 (commands,
status codes and dates in `docs/roadmap/jobs/103-09-filefactory.md`) found the following. The
reference for behaviour was JDownloader's `FileFactory.java` at revision 52903 and pyLoad's
`FilefactoryCom.py` 0.64, read as the `AGENTS.md` references are meant to be read — for what
they do, never for code.

1. **The official API is gone.** `api.filefactory.com` still resolves (`95.211.200.52`, not
   Cloudflare) but accepts no TCP connection on 80 or 443: every request, including the historic
   `/v1/getSessionKey`, ends in `curl: (28) Connection timed out`. No documentation exists any
   more; a web search for the endpoint names returns only JDownloader sources. JDownloader's
   older plugin used `https://api.filefactory.com/v1` with `getSessionKey`, `getFileInfo`,
   `getDownloadLink` and `getMemberInfo`; the current plugin contains not one reference to the
   host.
2. **The website lives, and it is new.** `https://www.filefactory.com/` answers `200` through
   Cloudflare without a challenge to a plain client, as a Next.js application (`vary: rsc`,
   data in `self.__next_f.push`). JDownloader dates the relaunch to 2026-04-22;
   `classic.filefactory.com` redirects to it. A file page is server-rendered with a JSON model
   (`requestToken`, `requiresPremium`, `isFree`, `requiresPassword`, `userIsPremium`,
   `errorCode`) and answers `200` even for a missing file, the error being in the payload.
3. **The only download route is the site's own internal API.** JDownloader's free flow reads
   the page model, waits out a countdown (45–60 s by the pricing page), and posts
   `{hash, token, type}` to `/api/download/initiate` for a JSON body carrying the link. That
   endpoint answered a probe with `404` and `{"code":"ERR_DL_001"}`, so it exists — and its
   field names changed on 2025-10-01 (`url` → `downloadUrl`, `fileSize`/`filesize`), the site
   was rebuilt on 2026-04-22, and mass link checking broke in both its forms on the same day.
   Three breaks in twelve months, in the reference client's own comments.
4. **The account half cannot be built in the SDK.** Premium status and expiry are read over a
   versioned WebSocket synchronisation (`wss://convex-ulta.filefactory.com/api/1.31.7/sync`)
   after a `/signin` whose reCAPTCHA Enterprise JDownloader skips by setting a
   `recaptcha-verified=true` cookie. A resolver has `http-request` and nothing else, and the
   cookie trick is an anti-bot bypass this project rules out.
5. **The free route could not be verified.** No live public file link exists anywhere reachable
   without an account — not in the reference plugins, not on the site's own public pages, not in
   its sitemap. The one link found (in a Facebook post) points at a deleted file. What a valid
   file's `requestToken`, countdown and `initiate` answer look like is known from JDownloader,
   not measured here.
6. **Terms.** `/legal/terms` forbids "any action that imposes an unreasonable or
   disproportionately large load" and reverse engineering of the "software tools … underlying
   the Services"; it neither allows nor forbids download managers. Guests have "a device daily
   cap" of unstated size.

## Options

### 1. No resolver; record the No-Go — chosen

`docs/roadmap/jobs/README.md` says a feasibility-gated provider "must prefer a documented No-Go
over fragile scraping or a terms-of-service/security compromise". Every route measured is one
of the two: the internal web API of a five-month-old site is fragile by its own history, and
the account path needs either a WebSocket the sandbox does not have or a captcha bypass the
project does not do.

- Nothing is merged that would have to be rewritten at the next field rename, with no
  fixture to catch it — there is no live file to make one from.
- The comparison that matters is DDownload and Katfile, which *are* website flows: XFileSharing
  is a product deployed on 140 measured sites whose login form has been the same for years. A
  single bespoke frontend rebuilt in April is not that.
- What would reopen the case is stated, so the record is a gate and not a verdict.

### 2. Guest-only resolver on the internal web API — rejected

`credentials = "none"`, `requires_account = false`, `match_domains = ["filefactory.com",
"www.filefactory.com"]`, page model → countdown inside the 600 000 ms wait budget →
`/api/download/initiate` → link. No captcha, no login, only the requests a guest browser makes.
It is not against the terms as written, and it is the route JDownloader ships.

- Rejected because it cannot be verified: the acceptance criteria ask for sanitised success,
  failure, rate-limit and expiry fixtures and a native-versus-Wasm parity test, and every one of
  them would be invented from a second-hand reading of another client. A resolver whose first
  live run is a user's is the "HTML saved as a file" failure the first criterion exists to
  prevent.
- Rejected also because the route has changed three times in a year and the field names carry
  the dates; the cost lands on every later release, for one host.

### 3. Premium through imported cookies, account check from the page model — rejected

`credentials = "cookies"`, `cookie_scope = "https://www.filefactory.com/"`, `userIsPremium`
read from any file page, `type: "premium"` on `initiate`. It avoids the login captcha and the
WebSocket by never signing in.

- Rejected with option 2, which it depends on, and because it could report `premium` only as
  "not confirmed": the expiry lives behind the WebSocket, and RD-109-34 forbids asserting a
  subscription the call did not read.

### 4. Reproduce the WebSocket account sync natively — rejected

Native code could open the socket that a Wasm resolver cannot.

- Rejected because the parity criterion makes the native resolver and the component pass the
  same contract tests; a native-only path is a second implementation, not a fallback. And the
  endpoint carries its version in the path (`1.31.7`), which is a promise to break.

## Decision

FileFactory gets no resolver in 1.1. RD-103-09 is `Blocked/No-Go` as of 2026-09-21; no
manifest, crate or provider entry is added, and `filefactory.com` stays an ordinary HTTP host
that link intake does not claim.

The case reopens when either of two things is true, and the job file names both: a
documented API answers on `api.filefactory.com` (the site's CSP still lists it under
`connect-src`), or a live test file exists and the project owner decides to carry the
internal-API guest route despite its history. The manifest, wait budget, credential mode and
message codes for that second case are written down in the job file so the decision, if it
is ever taken, starts from a plan rather than from this record.

## Consequences

- `docs/roadmap/jobs/103-09-filefactory.md` carries the measurement, the status and the
  reopening conditions; the No-Go acceptance box is ticked and the others stay open.
- The provider count in `docs/roadmap/jobs/110-00-release-koordination.md` phase 3 loses one
  candidate; the coordinator records it there.
- Nothing about the plugin contract, the provider registry or the captcha kinds changes.
  In particular this record does **not** ask for a WebSocket capability in the SDK: one host's
  account sync is not a reason to open a second transport to every plugin.
- What is **not** claimed here: that FileFactory cannot be downloaded from at all. A guest in a
  browser can. What is claimed is that no route exists that this project can verify, ship and
  keep.
