# ADR 0015 — No SwissTransfer resolver

- **Status:** Accepted
- **Date:** 2026-09-22
- **Job:** RD-120-09
- **Supersedes:** —

## Context

RD-120-09 asks for a resolver plugin for `swisstransfer.com` (Infomaniak) against
`rdownloader:plugin@0.6.0`, under the feasibility gate: an official API or a maintainable,
permissible download route, or a documented No-Go.

Measured on 2026-09-22 as part of RD-120-00's wave 0. DNS was healthy when the probes ran
(NXDOMAIN in 31 ms); `swisstransfer.com` resolves to `185.125.25.84` in 41 ms.

### What was measured, 2026-09-22

1. **The operator lives.** `https://swisstransfer.com/` answers `308` to
   `https://www.swisstransfer.com/`. As always, that alone proves only the operator.

2. **`robots.txt` disallows the transfer path.** The whole file:

   ```
   User-agent: *
   Crawl-delay: 1
   Disallow: /d/*
   Noindex: /d/*
   Sitemap: https://www.swisstransfer.com/sitemap.xml
   ```

   `/d/<uuid>` is the address a SwissTransfer link has. The operator disallows it and adds
   `Noindex` on top.

3. **There is no public API.** `https://api.infomaniak.com/1/swisstransfer` answers `404
   application/json` — Infomaniak runs a documented API platform, and SwissTransfer is not on
   it. `/fr/api` answers `404`. A search for official SwissTransfer API documentation returns
   the iOS and Android apps and the support FAQ, no developer reference.

4. **An internal API exists and says so.** `https://www.swisstransfer.com/api/links` answers
   `403 application/json` — not `404`. Something is there, it is reachable, and it refuses.
   Infomaniak publishes the iOS client as open source (`Infomaniak/ios-SwissTransfer`), from
   which that internal contract can be read.

## Decision

**No resolver plugin for SwissTransfer.** RD-120-09 takes status `Blocked/No-Go`.

Point 4 is the one worth spelling out, because it is the tempting one: the endpoint is
reachable, its shape is readable from a repository the operator publishes, and a plugin built
on it would work today. It is still a No-Go. A private endpoint that the operator has not
offered to third parties carries no contract: no version, no deprecation notice, no error
vocabulary this project may rely on. Building on it means the plugin breaks on a day Infomaniak
never told anyone about, and every such break lands on a person whose download stopped working.
That an application's source is open says what the endpoint does; it does not say that anyone
else may call it or that it will keep doing it.

Point 2 settles it independently: the operator withholds `/d/*` from automated clients.

## Consequences

No code, no plugin directory, no provider entry; the bundled component count does not change.

The same reasoning is on file for WeTransfer (ADR 0009) and applies for the same reason — a
consumer file-transfer service whose links are meant for a browser and whose machine surface is
internal. It is worth reading the two together before a third such job is cut.

If Infomaniak publishes SwissTransfer on its API platform, this record is superseded rather
than edited.
