# ADR 0009 — No WeTransfer resolver

- **Status:** Accepted
- **Date:** 2026-09-21
- **Job:** RD-103-12
- **Supersedes:** —

## Context

RD-103-12 asks for a signed WeTransfer resolver *if* a stable and permissible route exists,
and for a record of the No-Go otherwise. The feasibility measurement of 2026-09-21 (commands,
status codes and dates in `docs/roadmap/jobs/103-12-wetransfer.md`) found the following. The
reference for behaviour was JDownloader's `WeTransferCom.java` at revision 51834 with its
decrypter `WeTransferComFolder.java` at revision 51712, and pyLoad's `WetransferCom.py` 0.01,
read as the `AGENTS.md` references are meant to be read — for what they do, never for code.

1. **The official API was retired by decision, not by neglect.** The support article
   "We've retired our Public API" (archived copy of 2025-01-16; the live address now
   redirects to the generic help centre) says: "As of May 31, 2022, WeTransfer's Public API is
   no longer available. We first announced in early 2020 that we would no longer offer support
   for the Public API, develop it further, fix broken things, or make it available to new
   users." `developers.wetransfer.com` answers `503 Back-end server is at capacity`, and
   nothing documented took its place.
2. **The terms forbid the only remaining route.** `/explore/legal/terms` (published
   2025-05-21, last changed 2026-08-03), section 7.1 "Prohibited Uses": "g. Use any data
   mining or similar automated or manual data extraction, gathering or scraping methods in
   connection with the Service", "e. Frame, mirror, display or incorporate the Service or any
   portion into any other program, site, service or product", "k. Allow or encourage others
   to do any of the foregoing". A resolver that fetches a transfer's file list and direct
   links through the site's internal API without a browser is (g); shipping it is (e) and (k).
3. **What the reference clients do is the frontend's own XHR traffic.** The download page is
   a Next.js shell that answers `200` for any identifier, with `initialTransfer: null`; the
   data comes from `POST /api/v4/transfers/<id>/prepare-download` (file list, password state,
   `downloader_email_verification`) and `POST …/download` (`direct_link` per file or for the
   whole transfer as a zip). The `download` endpoint answers `400` unless the request carries
   `X-Requested-With: XMLHttpRequest`, and a miss returns an unmasked ActiveRecord message
   ("Couldn't find Transfer with [WHERE `transfers`.`public_id` = ?]"). The page configuration
   carries a server-side bot classification (`susbot`). None of this is documented, versioned
   or promised.
4. **The account half cannot be built at all.** Transfers with
   `downloader_email_verification: "tracking"` answer "No download access to this Transfer"
   without a login, and the only login JDownloader has is an OAuth token JSON the person copies
   out of the browser's local storage, renewed against `auth.wetransfer.com/oauth/token` — a
   credential shape the plugin SDK has no slot for and the terms' "mimic" clause covers.
5. **Nothing can be verified.** A free transfer expires after 3 days, a Pro transfer after 7.
   Neither reference plugin carries a test link, none is findable, and creating one would have
   meant an upload with e-mail verification under the very terms above. Every fixture the
   acceptance criteria ask for would be transcribed from JDownloader, and a resolver whose
   first live run is a user's is the "HTML saved as a file" failure the first criterion exists
   to prevent.

## Options

### 1. No resolver and no crawler; record the No-Go — chosen

`docs/roadmap/jobs/README.md` says a feasibility-gated provider "must prefer a documented
No-Go over fragile scraping or a terms-of-service/security compromise". This case is both,
and the terms half is the one that cannot be engineered around: there is no permitted
interface left to build on, because the operator closed it on purpose.

- Nothing is merged whose legitimacy rests on reading "automated data extraction" narrowly
  enough to exclude a download manager, on behalf of every person who installs the plugin.
- Nothing is merged that has no fixture from a real answer and cannot get one that outlives a
  week.
- The reopening conditions are stated, so the record is a gate and not a verdict.

### 2. Crawler plus resolver on the internal `api/v4` route, as JDownloader — rejected

Technically the right shape: a `crawler-plugin` (`plugins/wetransfer-crawler/`, no provider,
`net_http` for `wetransfer.com`, `we.tl`, `shorturls.wetransfer.com`, `go.wetransfer.com`)
that follows the short link, posts `prepare-download` and emits one `crawled-link` per item
with the folder path as `package-hint`; beside it a `resolver-plugin` (`plugins/wetransfer/`,
`credentials = "none"`, `requires_account = false`) that posts `download` with
`intent: single_file` and hands back `direct_link`. No captcha, no countdown, a wait budget
only for the password round.

- Rejected on the terms alone (7.1.g, e, k) — the job's first cross-cutting requirement admits
  "only official or demonstrably permitted interfaces", and this one is neither.
- Rejected also because it could not be measured: no live transfer exists to record a success,
  an expiry or a password fixture from, and none can exist for longer than a week.
- The whole-transfer zip that both references fall back to arrives without a file extension
  and is unpacked afterwards by the client — a second undocumented convention on top of the
  first.

### 3. Resolver with imported OAuth token for verified-recipient transfers — rejected

Would cover the `tracking` transfers that refuse a guest.

- Rejected with option 2, which it extends, and because the credential is a token JSON copied
  out of a browser session: neither `login`, `api_key` nor `cookies` in the manifest's
  credential modes, and RD-109-34 forbids asserting an account state the call did not read —
  JDownloader itself notes (2025-11-04) that the token's `expiresAt` does not say when it
  expires.

### 4. Drive the real web client in a browser — rejected

A headless browser would send exactly the XHR the frontend sends. It is the same automated
extraction the terms name, entered by a machine at a download manager's rate, and RD-109-11
just finished removing a browser runtime from the plugin platform for captchas.

## Decision

WeTransfer gets no resolver and no crawler in 1.1. RD-103-12 is `Blocked/No-Go` as of
2026-09-21; no manifest, crate or provider entry is added, and `wetransfer.com` and `we.tl`
stay ordinary HTTP hosts that link intake does not claim.

The case reopens when either of two things is true, and the job file names both: WeTransfer
publishes a documented API again, or its terms drop or exempt automated retrieval by a
personal download client from 7.1.g and 7.1.e. The crawler-plus-resolver cut, the manifest
fields and the message codes for that day are written down in the job file so the decision,
if it is ever taken, starts from a plan rather than from this record.

## Consequences

- `docs/roadmap/jobs/103-12-wetransfer.md` carries the measurement, the status and the
  reopening conditions; the No-Go acceptance box is ticked and the others stay open.
- Pasting a `wetransfer.com/downloads/…` or `we.tl/…` link keeps its current behaviour: no
  plugin claims it, so it is handled as an unknown site, and the Next.js shell is what an
  unclaimed download would save — which is why the first acceptance criterion cannot be met
  without one of the two conditions above.
- The provider count in `docs/roadmap/jobs/110-00-release-koordination.md` phase 3 loses one
  candidate; the coordinator records it there.
- Nothing about the plugin contract, the provider registry or the credential modes changes.
  In particular this record does **not** ask for a "browser token" credential mode in the SDK:
  one host's copied session is not a reason to teach every plugin to accept one.
- What is **not** claimed here: that a person cannot download a transfer they were sent. A
  browser can, and that is the route the operator offers. What is claimed is that no route
  exists that this project may build, can verify, and could keep.
