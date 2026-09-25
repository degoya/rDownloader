# ADR 0012 — No rules for sites behind a browser gate

- **Status:** Accepted
- **Date:** 2026-09-21
- **Job:** RD-110-14
- **Supersedes:** —

## Context

RD-110-14 asks whether rDownloader can reach five services that live but refuse every simple
HTTP request: `serienfans.org`, `filmfans.org`, `newalbumreleases.net`, `multipaste.org` and
`zpaste.net`. The job offered four outcomes per service — a rule, a rule carrying a session the
person established in their own browser, the browser extension, or a documented No-Go — and
said in its own text that a documented No-Go is a valid result.

The job's starting position, measured 2026-09-20, recorded only that all five answer `403` at
the domain root with and without a browser user agent. Nobody had yet looked at a **content
page**, at what the `403` body actually is, or at whether a route exists that does not pretend
to be a browser. The feasibility measurement of 2026-09-21 did.

### What was measured, 2026-09-21

All probes with `curl -A 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) … Chrome/130.0 Safari/537.36'`,
redirects followed, no account, no session taken from a browser, nothing solved. Content
identifiers come from the CDX index of the Internet Archive and from nowhere else. The egress
address was `93.127.252.127`, the Cloudflare colo `MUC`. The full table per service is in
`docs/roadmap/jobs/110-14-seiten-mit-bot-schutz.md`.

1. **It is one gate, five times.** Every one of the five sits behind Cloudflare and answers
   `403` with `cf-mitigated: challenge` on every path of its own application. The body is
   always the same interstitial: `<title>Just a moment...</title>`,
   `Enable JavaScript and cookies to continue`, `window._cf_chl_opt = { … cType: 'managed' … }`,
   and a script from `/cdn-cgi/challenge-platform/h/g/orchestrate/chl_page/v1`. The response's
   own CSP admits exactly one foreign origin, `https://challenges.cloudflare.com`, and
   `accept-ch`/`critical-ch` ask for sixteen `Sec-CH-UA-*` client hints. This is Cloudflare's
   **managed challenge** — not the operator's proof of work that ADR 0010 found at filecrypt,
   not a cookie wall, not a login, not a missing header.

2. **The content pages answer the same thing, and that is the decisive measurement.** Not the
   roots — the pages: `serienfans.org/daemons-of-the-shadow-realm` (5 678 B),
   `serienfans.org/…/api/v1/<id>/season/3?lang=ALL` (5 851 B),
   `filmfans.org/alpha_1/Alpha.2025.GERMAN.DL.1080P.BLURAY.AVC-UNDERTAKERS` (5 809 B),
   `newalbumreleases.net/100706/black-grape-pop-voodoo-2017-3/` (5 650 B),
   `multipaste.org/paste/437` (5 474 B), `zpaste.net/p/41tia`, `/p/4tcoa`, `/p/0wm8h`
   (5 486 B each). Every identifier is a real one from the archive. Not one response contains a
   line of the service's own content.

3. **There is no legitimate route past it.** `robots.txt` is challenged on all five, so the
   operator cannot show a non-browser even its own crawling policy — there is no permission and
   no prohibition to appeal to. `sitemap.xml` is challenged on all five. The feeds are
   challenged (`serienfans.org/feed`, `/rss`, `filmfans.org/feed`,
   `newalbumreleases.net/feed/`). The WordPress API is challenged
   (`newalbumreleases.net/wp-json/wp/v2/posts`). The two sites that have an API of their own —
   `/api/v1/<32 characters>`, the same shape RD-110-13 describes at serienjunkies.org — serve it
   from inside the same zone, behind the same rule, and document it nowhere. No mirror and no
   ungated variant was found: `www.`, IPv4 only, `http://`, `HEAD` all end at the same `403`.

4. **A handed-over session does not arise, and could not be sent.** The challenge response sets
   **no** cookie at all; a second request carrying the resulting (empty) jar is refused
   identically. `cf_clearance` only comes into existence after the challenge script has run in
   a browser, and Cloudflare binds it to the client that earned it. Independently of that,
   `FetchRequest` in `crates/rd-siterules/src/exec/ports.rs` carries `url`, `addresses`,
   `method`, `form`, `max_bytes` and `timeout` — no headers and no cookie — and the rule format
   has no field for one. The session hand-over the product does have (auth profiles with
   `origin = browser_capture`, `POST /api/v1/capture/cookies`, the extension's optional
   `cookies` permission) is not connected to the rule executor.

## Decision

**No rule is written for any of the five services, and the class as a whole is a No-Go.**

The bar ADR 0010 set for filecrypt applies unchanged and for the same reason: passing this gate
means running the operator's challenge script or imitating a browser's fingerprint, and this
project does neither. Where filecrypt's gate was hand-rolled and filecrypt-specific, this one is
a product bought off the shelf; the conclusion is identical, and the reason is shared by all
five services rather than coincidental — which is why this is one record rather than five
entries in a job file.

A second reason stands on its own, and would hold even if the gate fell: **nothing behind it was
ever seen.** A rule is a description of a response. There is no response to describe, nothing to
record a test against, and no way to tell from outside whether these services still carry links
at all.

## Consequences

- `docs/roadmap/jobs/110-14-seiten-mit-bot-schutz.md` carries the measurement of 2026-09-21, the
  verdict per service and the status `Blocked/No-Go`. The acceptance boxes that were met are
  ticked; the one about rules is answered by this record, not by rules.
- `docs/site-rules.md` gains "No browser, either", next to "No JavaScript interpreter": the two
  are the same decision seen from two sides.
- The shipped pack (`crates/rd-siterules/resources/site-rules.json`) gains nothing. None of the
  five appears in it, now or as a stub.
- **The error code for this case already exists and is confirmed by the measurement.**
  `RunError::Blocked` maps `401`, `403` and `429` to `site_rules.blocked`
  (`crates/rd-siterules/src/exec/run.rs`), and the four catalogue entries are in
  `web/src/locales/{de,en,es,fr}/server.json`. It says the site refused the request — never that
  the page was empty. RD-110-09's `blockiert` state is fed from this code and from no other.
  No second code was introduced.
- **The extension question is left open on purpose, exactly as ADR 0010 left it.** Reading a
  gated page in the person's own browser, where the challenge is solved anyway, and handing the
  links over is technically the remaining route. It costs
  `optional_host_permissions: ["http://*/*", "https://*/*"]` and a protocol of its own, and it
  is a decision about what the extension may do, not a finding a measurement can produce. This
  record neither takes it nor prejudges it.
- **What is not claimed here:** that these services are dead, or that their pages hold nothing.
  Both are unknown, and the measurement says only that they cannot be established from outside.
  Nor is it claimed that the gate is aimed at this project — a managed challenge can depend on
  the requesting address's reputation, and the Internet Archive holds content captures from
  these very sites (filmfans to 2026-07-12, serienfans to 2026-07-06, multipaste to 2025-11-04,
  zpaste to 2026-01-28), most likely because archive crawlers are on Cloudflare's verified-bot
  list. rDownloader is not on it and cannot join it in a stranger's zone.

The case reopens when any one of these is true:

1. A service serves its content pages without the challenge again, or behind one the contract
   already expresses — a reCAPTCHA, a CutCaptcha, a click-point image — that a solver service or
   the person can answer.
2. A service offers a documented route for download clients: a feed, an API, a sitemap that is
   not itself challenged.
3. The project decides, as separate work with its own record, that the extension may read a
   gated page in the person's browser and hand the links over.
