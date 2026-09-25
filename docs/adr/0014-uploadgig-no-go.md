# ADR 0014 — No UploadGig resolver

- **Status:** Accepted
- **Date:** 2026-09-22
- **Job:** RD-120-08
- **Supersedes:** —

## Context

RD-120-08 asks for a resolver plugin for `uploadgig.com` against `rdownloader:plugin@0.6.0`,
under the same feasibility gate as its counterparts in 1.1: assess an official API or a
maintainable, permissible download route first, and if none exists, record the No-Go rather
than merge fragile code.

The job carried no measurement of its own. The gate was measured on 2026-09-22 as part of
RD-120-00's wave 0.

### What was measured, 2026-09-22

DNS was checked first, because a slow resolver reads like a dead service: a guaranteed
NXDOMAIN answered in 31 ms and `example.com` in 10 ms, so resolution on this machine was
healthy when the probes ran. `uploadgig.com` resolves to `185.178.208.165` in 467 ms.

1. **The operator lives.** `https://uploadgig.com/` answers `200 text/html`. Per the rule
   sharpened by RD-110-16, that proves only that somebody still runs the domain.

2. **`robots.txt` disallows the download path itself.** The whole file:

   ```
   User-agent: *
   Disallow: /file*
   Disallow: /ticket*
   Disallow: /premium*
   Disallow: /payment*
   ```

   `/file*` is where a shared file lives. This is not a crawler-hygiene rule about search
   indexes on the periphery of the service — it is the operator declining automated access to
   the exact route a resolver would have to walk.

3. **No official interface is offered.** `/(api).html`, `/faq`, `/pages/api` and `/developers`
   all answer `404`; `/api` answers `307` to the site itself. A search for official API
   documentation returns only third-party "premium link generator" sites, none of them run by
   the operator.

4. **A file path answers `404` for an invented identifier**, which tells us nothing further —
   there was no real, currently shared file to measure against, and acquiring one would not
   change points 2 and 3.

## Decision

**No resolver plugin for UploadGig.** No code, no plugin directory, no provider entry; the
bundled component count does not change. RD-120-08 takes status `Blocked/No-Go`.

## Consequences

Two of the three cross-cutting requirements this project puts on every hoster job cannot be
met here. "Only official or demonstrably permissible interfaces; no anti-bot circumvention"
fails at point 2: the operator's own `robots.txt` withholds `/file*`. "Deliver sanitised
success/failure/expiry/quota fixtures and contract tests" has nothing to be a fixture *of*,
because there is no documented contract to hold a fixture against — only a page layout that
changes when the operator changes it.

The third-party premium-link generators found in point 3 are not an alternative route. Routing
rDownloader's users through an unrelated operator's scraper is neither official nor
permissible, and it would make this project's behaviour depend on a service it does not
control and cannot test.

If UploadGig publishes an API, this record is superseded rather than edited.
