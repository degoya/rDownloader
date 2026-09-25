# ADR 0005 — GoFile: no resolver, because the only open route is the site's bot gate

- **Status:** Accepted
- **Date:** 2026-09-21
- **Job:** RD-103-07
- **Supersedes:** —

## Context

RD-103-07 asked for a signed GoFile resolver "sofern eine stabile und zulässige
Integrationsroute nachgewiesen ist", and named the alternative in its own acceptance
criteria: an ADR that records the No-Go instead of fragile code. This is that record. The
measurements behind it — commands, status codes, the files read and the dates — are in the
job file, `docs/roadmap/jobs/103-07-gofile.md`, section "Feasibility-Messung, 2026-09-21".

What was found, in short. gofile.io is alive and answers `200` for every path, including
content ids that do not exist, with one 3358-byte JavaScript shell; all data comes from
`api.gofile.io`. The documented API mints a guest token with `POST /accounts`, but the one
endpoint a resolver needs, `GET /contents/{contentId}`, is documented as Premium-only and
answers `error-notPremium` to everything else. The site's own web client lists folders for
guests all the same, by sending a second credential beside the bearer token: an
`X-Website-Token` computed by `/js/wt.obf.js`, an obfuscated script whose secret the operator
rotates server-side. The bridge module `/js/core/wt.js` says what happens to a client that
keeps using a retired secret: its IP is banned on the next content request. The terms of
service prohibit circumventing rate limits "through automated means" and announce automatic
IP bans for repeated limit violations; they declare the API a BETA "subject to change".

Both prior-art projects took the guest route. JDownloader's crawler recomputes the website
token locally — a SHA-256 over the user agent, the bearer token, a four-hour time bucket and
a static suffix — which is a re-implementation of the obfuscated generator and therefore the
thing the rotation is built to retire. pyLoad hard-coded one token value in 2022; the plugin
has been broken since the spring of 2024 (pyload/pyload#4445, #4453) and the value answers
`error-token` today.

## Options

### 1. Recompute the website token, as JDownloader does — rejected

It works until the next rotation, and the job's own scope excludes it twice: "Anti-Bot-Bypass"
is out of scope, and the first cross-cutting requirement allows only official or demonstrably
permitted interfaces. The operator has written down that this mechanism is a gate and that a
stale token earns an IP ban. Shipping it would put a person's home address on that list for
the sake of a folder listing, and the plugin would need an emergency release every time the
secret moves — the exact failure mode the plugin platform exists to avoid, only faster.

### 2. A Premium-only plugin over the documented API — not now

This route is permitted: the REST API is the operator's intended programmatic access, and a
premium account's token lists a folder with `GET /contents/{contentId}`, password as SHA-256,
pagination and depth. It is the shape a GoFile integration should take if one is built. It
is refused for this release for two reasons that are about evidence, not about legitimacy.
Nobody in this checkout can run the route once: there is no premium account, this phase
creates none, and whether the store hosts that serve the bytes want the `accountToken`
cookie, the bearer header or a direct link is a guess without one. And the operator calls
the API a BETA subject to change; the last third-party client that relied on it lasted a
year. A resolver whose every fixture is transcribed from a documentation page, and which
cannot be smoke-tested, does not meet the project's rule that a completion claim carries a
measured result.

### 3. A hard-coded website token, as pyLoad did — rejected

Measured dead: `?wt=4fd6sg89d7s6` gets `error-token`. It was never a route, only a snapshot
of one.

### 4. Drive the real web client in a browser — rejected

A headless browser would execute `wt.obf.js` as written and produce valid tokens without
re-implementing anything. It is still the guest gate, entered by a machine, at the rate a
download manager enters it — the behaviour the terms name. It also drags a browser runtime
into a sandboxed WebAssembly plugin platform for one hoster, which RD-109-11 just finished
removing for captchas.

## Decision

**No resolver and no crawler for GoFile in 1.1.0.** RD-103-07 is `Blocked/No-Go`. The
guest route is a bot gate by the operator's own description, and the permitted route cannot
be measured here.

Two things reopen it, either one alone:

- A GoFile premium API token is available for a measured run. Then option 2 is the plan, as
  a `crawler-plugin` (`plugins/gofile-crawler/`, claims the `gofile` provider) beside a
  `resolver-plugin` (`plugins/gofile/`, `credentials = "api_key"`, `requires_account = true`),
  with the manifest fields, message codes and fixtures sketched in the job file. The first
  measurement of that run is which credential the store hosts accept for the bytes.
- GoFile opens `GET /contents/{contentId}` to guest tokens without the website token.

## Consequences

- Pasting a `gofile.io/d/…` link into the LinkGrabber keeps its current behaviour: no plugin
  claims it, so it is handled as an unknown site — and the HTML shell is what an unclaimed
  download would save, which is why the job's first acceptance criterion cannot be met
  without one of the two conditions above.
- `docs/roadmap.md` names the closed gate beside the other six, and the "Remaining Gaps"
  list RD-110-00 collects at the end of the release is where it belongs.
- The measured facts stay in the job file and are dated. A later reading should re-measure
  `GET /contents/{contentId}` with a guest token and re-read `/js/core/wt.js` before trusting
  them; the operator rewrote the whole front end at least once between JDownloader's and
  this project's readings.
