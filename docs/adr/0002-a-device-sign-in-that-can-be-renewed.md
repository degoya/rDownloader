# ADR 0002 — A device sign-in that can be renewed

- **Status:** Accepted
- **Date:** 2026-09-08
- **Job:** RD-106-01
- **Supersedes:** —

## Context

RD-103-00 left two worlds, and neither covers the combination the next four provider jobs
need.

`world auth-plugin` can run a device code: `auth-state::user-action` carries a
`verification-url`, a `user-code` and a window. What it cannot do is say that the token the
flow produced dies in an hour, and it has no `refresh`. A person who signs in that way is
asked again the next time the token ages out, and there is no call in the contract that could
prevent it.

`world oauth-plugin` has exactly the missing half. `refresh` lives in `interface oauth`, and
the host's renewal sweep (`crates/rd-api/src/auth_flow_service.rs`) already drives it. What
that world cannot do is start a device flow: `begin` must return an `authorization-request`
carrying an `authorization-url` **and** a callback `state`, and `poll` demands a `code` from a
redirect. A device flow has neither.

Real-Debrid (RD-106-03) and OneDrive (RD-106-05) both sign in by device code and both hand out
tokens that expire. Without a way to combine the two, the first of those jobs would hit a
contract wall halfway through its implementation — which is the outcome RD-103-00 existed to
prevent.

So the decision is not "how do we sign Real-Debrid in". It is: **by what mechanism does a
sign-in without a redirect reach the renewal path, from now on.**

## Options

### 1. `device-begin` and `device-poll` in `interface oauth` — chosen

Two new functions in the interface that already has `refresh`. `begin`, `poll` and `refresh`
are untouched; a new record `device-authorization` carries the address, the short code, the
window, the interval and the plugin's own bookkeeping.

- **It is the whole point.** `refresh` is already here. A device flow that enters this
  interface inherits renewal instead of needing a second copy of it, and that inheritance is
  the thing the job is for.
- **Additive in the sense that matters here: nothing anybody depends on misses its world.** A
  new export does change what `oauth-plugin` demands, and it is worth being exact about who
  that reaches. Exactly one manifest in the tree declares `plugin_type = "oauth"`:
  `plugins/example-oauth/`, the contract tests' fixture — and, contrary to what the job file
  assumed, it *is* packaged and shipped, because `scripts/build-plugins.sh` packages every
  directory carrying a `manifest.toml` and `sync_bundled` installs every `.rdplug` beside the
  binary. So there is one signed `oauth` package in the field. It claims the provider slug
  `example`, which no resolver defines and no account can carry, so nothing can be signed in
  through it and nothing breaks when it is replaced. Its version is bumped to `0.2.0` in the
  same change, because `sync_bundled` installs only a strictly newer version and an unbumped
  package would leave the stale component installed for ever. No third-party `oauth` plugin
  exists to break: the world was only made scaffoldable in RD-105-01, one release ago.

  The three shipped auth plugins export `auth`, a different interface in a different world,
  and the contract change does not touch them.
- **One provider, one plugin.** A manifest carries exactly one `plugin_type`, so a provider
  offering both ways in can only be one plugin if both entrances live in one interface.
- Costs two functions, one record, ten byte-identical copies under `sdk/templates/*/wit/`, one
  manifest field, and one branch in the host and the sweep.

### 2. A variant inside `authorization-request`, so `begin` answers both shapes — rejected

Superficially the smallest change: teach the existing `begin` to return either an
authorization address or a device prompt.

It changes the signature of an exported function. That is not additive — it is precisely the
class of change `@0.6.0` may not carry, by the rule ADR 0001 wrote down: a bump is for a
changed signature, a removed field, a renamed record. Every plugin built against `0.6.0` would
have to be rebuilt against a contract with the same version number, which is worse than a
bump, not better.

### 3. An eleventh world, `device-oauth-plugin` — rejected

The cleanest option on the additive axis: nothing existing changes at all, and the new world
imports and exports exactly what a device flow needs.

It fails the fourth acceptance criterion. A manifest names one `plugin_type`, so a provider
that offers both a redirect and a device code would need **two plugins, two packages and two
signatures** for one account — and the person would have to know which to install. It also
duplicates `refresh` and the whole renewal contract in a second place, which is the thing that
rots first.

### 4. Add `refresh` to `interface auth` — rejected

The comment at `crates/rd-plugin-api/wit/rdownloader.wit` above `interface oauth` records why
RD-103-00 already refused this once, and the reason has not changed: a new export in `auth`
means **every plugin exporting `auth` stops satisfying `auth-plugin`**. There are three of them
in the field — AllDebrid, Debrid-Link, Premiumize — and they would all have to be rebuilt and
re-signed. The second acceptance criterion of RD-106-01 forbids exactly that.

It is worth being honest about the pull of this option: `auth` is where a device code already
lives, and in a year, when `auth` is due for a breaking revision anyway, folding the two
together will look obvious. That is a `0.7.0` conversation. It is not this one.

## Decision

**Option 1.** `device-begin` and `device-poll` join `interface oauth`. The package version
stays `rdownloader:plugin@0.6.0`.

Three properties are part of the decision rather than of the implementation:

- **The manifest says which entrances a plugin serves, and the host calls no other.** A new
  top-level `oauth_flows = ["redirect", "device"]` states them in the order the author
  prefers; absent means `["redirect"]`, which is what every OAuth manifest written before this
  meant. The host reads it before it calls anything
  (`crates/rd-plugin-ext/src/oauth.rs::require_flow`), so nobody writes a stub whose only
  purpose is to be refused, and a plugin is never asked for a way in it does not have. Being
  asked anyway is a typed, terminal refusal — `ProviderError::UnsupportedFlow` — and never a
  sign-in that dies in front of somebody.
- **The address gate applies to both entrances.** A device flow's `verification-url` is put in
  front of a person and they are asked to sign in there, exactly as an authorization URL is,
  so it is held to the same rule: it must be on a domain the manifest they installed declares.
  A signed plugin naming any address would be an installed phishing page whichever entrance
  produced it.
- **Waiting is not refusal.** `authorization_pending` and `slow_down` both come back as
  `pending`, and only a refusal the provider actually made becomes `failed`. A device flow that
  read "not yet" as failure would end sign-ins while the person was still walking to the other
  screen.

## Consequences

- `plugins/example-oauth/` and `sdk/templates/oauth/` serve both entrances, and the contract
  tests in `crates/rd-plugin-ext/tests/oauth_contract.rs` drive a device sign-in through to a
  stored token *and* through the renewal that follows it — the outcome the job is named for.
- The renewal sweep needed no new branch: a device sign-in that ends in `Authorized` carries
  the same expiry and refresh reference a redirect does, because the host writes both from
  inside its own `store-oauth-token` call, and `due_refresh_auth_flows` finds the row either
  way.
- `sweep_sign_ins` now polls two worlds. Which one a due row belongs to is read from the row's
  `plugin_id`, not guessed: only one plugin claims a provider in each world.
- The three shipped auth plugins keep their world. They were touched in the same job for an
  unrelated reason — they quoted a provider's error text verbatim — and rebuilt from source
  and re-signed as any plugin revision is, with their manifest version moved to `0.9.1`
  because `sync_bundled` installs only what is strictly newer and a fix in an unbumped package
  reaches nobody who already has it. The contract change alone required neither.
- What is **not** decided here: whether a provider that offers both should ask the person which
  to use. Today the manifest's first entry wins. If that turns out to be wrong, the place to
  fix it is the handler, not the contract.
