# ADR 0016 — No Smash resolver, although the API is good

- **Status:** Accepted
- **Date:** 2026-09-22
- **Job:** RD-120-10
- **Supersedes:** —

## Context

RD-120-10 asks for a resolver plugin for Smash (`fromsmash.com`) against
`rdownloader:plugin@0.6.0`, under the feasibility gate.

Measured on 2026-09-22 as part of RD-120-00's wave 0. DNS healthy (NXDOMAIN in 31 ms);
`fromsmash.com` resolves in 22 ms.

This is the one No-Go in this wave that is **not** about a missing or hostile interface. Smash
has the best-documented API of the five services measured. It is still the wrong one.

### What was measured, 2026-09-22

1. **`robots.txt` is permissive** about the download path. It disallows `/404`, `/expired`,
   `/deleted`, `/suspended`, `/redirect`, `/report/`, `/invalid-configuration` and
   `/service-unavailable`, then `Allow: /`. No objection to automated access as such.

2. **An official, maintained, MIT-licensed SDK exists**: `fromsmash/smash-sdk-js`, published
   by the operator, with a documentation portal at `api.fromsmash.com/docs`.

3. **Every package in it is account-scoped.** The published packages are `billing`,
   `customization`, `discovery`, `directory`, `domain`, `iam`, `image`, `link`, `promotion`,
   `transfer` and `vat`. `iam` and `billing` are the shape of the thing: this is a commercial
   API for an application that *sends* transfers on its own account, priced per account, with
   a 100 GB / 14-day trial and a paid plan after it.

4. **There is no unauthenticated route to somebody else's transfer.**
   `https://transfer.fromsmash.com/transfer/<id>` answers `404` without a key. The quick-start
   downloader example takes a `transferId`, not a public `fromsmash.com/...` link, and neither
   the quick start nor the SDK README documents resolving a transfer created outside the
   caller's own account.

## Decision

**No resolver plugin for Smash.** RD-120-10 takes status `Blocked/No-Go`.

The reason is not the quality of the interface but which side of the transaction it serves.
Smash sells an API to the **sender**: an application that uploads files and hands out links.
rDownloader is the **receiver** — somebody was sent a `fromsmash.com` link and wants the file.
For that position Smash offers its website and nothing else, and the documented API cannot be
pointed at a stranger's transfer.

Paying for a developer account would not change this. It would buy the ability to create
transfers, not to read one somebody else created.

## Consequences

No code, no plugin directory, no provider entry; the bundled component count does not change.

This record exists mainly so the next reader does not repeat the measurement. "Smash has a
public API" is true, prominently advertised, and will look like a missed opportunity to
somebody skimming. The question to ask of a file-transfer service is never *is there an API*
but *does it answer for a link I was sent*.

If Smash documents a public transfer-resolution route, this record is superseded rather than
edited.
