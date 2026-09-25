# ADR 0001 — Resolving an address that points at many files

- **Status:** Accepted
- **Date:** 2026-09-08
- **Job:** RD-104-03
- **Supersedes:** —

## Context

`resolve` in the plugin contract is `func(request: resolve-request) -> result<resolved-download,
failure>`: one link in, one file out. Nothing in the contract can say "this address is a
folder". The consequence was visible from the outside long before it was traced: a
Premiumize.me cloud folder pasted into the LinkGrabber ended in
`premiumize.multi_file_source`, whose text told the person to split the source in the
LinkGrabber — a step nobody had implemented. `plugins/premiumize/src/resolver.rs` refused as
soon as `transfer/directdl` answered with more than one entry, and there was no other outcome
available to it.

This is not a Premiumize quirk. It is the whole class of addresses that stand for several
files: cloud folders of every provider in `103-02` … `103-05`, directory shares, and later the
site-specific grabbers (Serienjunkies and comparable) that JDownloader calls
`PluginForDecrypt` and pyLoad calls a crypter. rDownloader had only the hoster half of that
split.

So the decision is not "how do we list a Premiumize folder". It is: **by what mechanism does
rDownloader turn one address into many, from now on.**

## Options

### 1. A world of its own, `crawler` — chosen

`crawl: func(url: string) -> result<list<crawled-link>, failure>` with `claims-url` as the
counterpart of the resolver's `match-url`, importing `host`, `http`, `cookies` and `captcha`
exactly as a resolver does.

- Mirrors the split that has held up in both prior-art projects, and holds up for the same
  reason: fetching a stranger's page and enumerating it is a different risk from unlocking a
  file for an account.
- The sandbox boundary does not move. A crawler reaches what its own manifest declares and
  nothing else, and its answer is a proposal that goes through the same review, blocklist and
  routing a pasted link does.
- Fits `rd-provider-registry`'s rule that nothing is compiled in: a crawler exists exactly as
  long as its plugin is installed, so a site-specific grabber is an install and not a release.
- Costs a WIT interface, ten byte-identical copies under `sdk/templates/*/wit/`, a template, a
  value in `plugin new --type`, and a conformance arm in the host.

### 2. Extend the `intake` world — rejected

`intake` already returns a list and already imports `http`, so on paper it is the cheap answer.
Two things are wrong with it.

`parse` is applied to *pasted text* (`crates/rd-api/src/collector_handlers.rs`), not to a
fetched resource; making it accept a URL would give one call two jobs that differ in risk —
splitting text reaches nothing, opening an address reaches a stranger's server.

Worse, `normalize` must not change a link's host (`crates/rd-plugin-ext/src/intake.rs:127`).
That rule is what stops an installed parser quietly redirecting somebody's download to an
address of its own. A crawler has to return links on *other* hosts — that is what enumerating
a folder means — so this option can only work by lifting the rule for the type that most needs
it. A separate interface keeps the rule intact for parsers and states the difference plainly:
`normalize` rewrites one link and may not move it, `crawl` answers what lies behind one and
must be able to.

### 3. Make `resolved-download` a variant, `file | folder(list<…>)` — rejected

The smallest edit on paper and the most expensive in practice. Changing the shape of an
exported function's return type changes the world every resolver satisfies, so **every
`.rdplug` already signed and installed in the field stops instantiating** — including
third-party ones this project cannot rebuild. Nothing about a folder is worth that.

### 4. Native code in `rd-collector` — rejected

Fast for Premiumize alone and wrong as a mechanism. Every further site would become a
compile-time decision and a release, which is precisely the model the plugin platform replaced
in 0.7. It would also put a fetcher for arbitrary third-party pages inside the process rather
than inside the sandbox.

## Decision

**Option 1.** A `crawler` interface and a `crawler-plugin` world in `rdownloader:plugin`, one
SDK template, and `plugins/premiumize-crawler/` as the first real implementation.

Three properties are part of the decision rather than of the implementation:

- **Limits belong to the plugin, budgets to the host.** Depth, breadth and cycles are refused
  inside the walk (`plugins/premiumize-crawler/src/walk.rs`); fuel and the execution deadline
  sit underneath as the last resort. A crawler that relied on the budget would report a
  timeout where it should report "this folder is larger than I will list", and a cycle would
  look like a slow folder rather than an endless one.
- **Empty is a failure, not a result.** An empty, missing or unreachable folder returns a
  failure with a stable code, translated by the catalogue the package shipped. Returning an
  empty list would create a package with nothing in it and nothing to explain why — which is
  the shape of the defect this job was opened for.
- **A crawler proposes.** Its answer is capped, its URLs are re-parsed, an answer equal to its
  own input is dropped, and everything that survives goes through the blocklist, the
  disabled-service refusal, the review and the routing rules.

## `api_version` stays at `0.6.0`

RD-104-03's acceptance criteria asked for a bump. That is refused here, deliberately and in
writing rather than by omission:

- **Precedent.** RD-103-00 added the `oauth` interface and world to `0.6.0` and shipped;
  RD-105-01 confirmed it when it added the template. A mixed rule — `oauth` at `0.6.0`,
  `crawler` at `0.7.0` — would be worse than either consistent answer.
- **Cost.** Seven open 1.0.3 jobs name "Vorhandenes `rdownloader:plugin@0.6.0` Plugin-SDK" as
  their dependency, word for word. A bump pulls their stated precondition out from under them.
- **It is additive.** Adding an interface and a world takes nothing away from an existing
  world, so a plugin built against the contract without `crawler` in it still satisfies
  `resolver-plugin` — `crates/rd-plugin-host/tests/signed_resolver.rs` signs, verifies and
  instantiates one to prove it. In the other direction an older host refuses a `crawler`
  package by itself: `PluginType::Crawler` does not deserialize there, so the manifest is
  rejected as an unknown plugin type rather than half-loaded.

A bump becomes right when the contract stops being backwards compatible — a changed signature,
a removed field, a renamed record. Reserve it for that.

## Consequences

- `rdownloader plugin new --type crawler` scaffolds a working plugin; the tenth SDK template
  ships a byte-identical copy of the contract, and two existing tests keep it that way.
- The host links `cookies` and `captcha` for an extension type when its manifest grants them,
  which no extension type needed before this one.
- The LinkGrabber gained one step before everything else it does: an address a crawler claims
  becomes the files behind it, with their names, their sizes and the folder they sat in. The
  folder becomes the package suggestion, which is why `GroupInput` grew `package_hint`.
- What is **not** decided here: captcha-protected containers, site-specific grabbers, and a
  folder browser in the interface. All three are separate jobs; this one delivers the
  mechanism and one reference implementation.
- An end-to-end run against a real Premiumize account with a counted number of files is not
  claimed. There is no account in this checkout; the evidence is the unit and contract level.
