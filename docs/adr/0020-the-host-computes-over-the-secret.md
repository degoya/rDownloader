# ADR 0020 — The host computes over the credential, the guest only names it

- **Status:** Accepted by the project owner on 2026-09-22
- **Date:** 2026-09-22
- **Job:** RD-120-20 (measurement from RD-120-11)
- **Supersedes:** —

## Context

`interface auth` states in its own header that a plugin never sees a credential. The host keeps
that promise by substituting `{{username}}`, `{{secret}}` and `{{secret:<reference>}}` on the way
**out** of the guest, into the query, the headers or the body of a request the plugin described
(`crates/rd-plugin-host/src/native/expand.rs`, pattern: `plugins/keep2share/src/api.rs`). A guest
can therefore *send* a credential. It cannot *compute* with one.

That is enough for every provider whose sign-in posts the password, and for none whose sign-in
posts a value derived from it. MEGA is the second kind. `us` wants `uh`, the second half of
PBKDF2-HMAC-SHA512(password, salt, 100 000); the first half unwraps the account's master key,
which unwraps the RSA private key, which decrypts the session identifier. Every stage needs the
password or something derived from it, inside the guest.

RD-120-11 measured what that would cost rather than guessing, and the measurement **overturned
the assumption the job had been postponed for**. RSA in the sandbox is cheap — a 2048-bit private
operation with the Chinese remainder theorem is 189 444 831 fuel, 9.5 % of the default budget and
0.474 % of the largest a manifest may declare. The expensive stage is the password derivation MEGA
prescribes: 3 338 300 549 fuel, 167 % of a default budget. A manifest that declares four to eight
times the default carries the whole sign-in at 8.8 % of the ceiling, and a manifest declaring its
need is the intended way. **Fuel was never the obstacle. The contract was.**

Three ways out were on the table. The project owner chose the first on 2026-09-22:

1. **A host-side key-derivation primitive.** The host computes over the credential and hands the
   guest only the result.
2. **A native sign-in**, as FTP and SFTP are. Refused: it breaks the rule that a provider exists
   exactly as long as its plugin does.
3. **No MEGA account.** Refused.

## Decision

`rdownloader:plugin@0.7.0` carries `interface key-derivation` with one function:

```wit
derive: func(secret: secret-handle, steps: list<step>) -> result<list<u8>, failure>;
```

A `secret-handle` is a **name**, not a value: one of the references the plugin's own manifest
lists under `capabilities.secrets`, exactly as it would appear in a `{{secret:<reference>}}`
marker. A `step` is one of three — `pbkdf2-hmac-sha512`, `aes-ecb-decrypt` (AES-128, keyed by the
first sixteen bytes of the running value) and `take` (a window). The steps form a **chain**: each
step's output is the next one's input, the first step's input is the credential, and **only the
last step's output is returned**.

The chain is the design, and it is what makes the primitive worth having rather than merely
convenient. MEGA's sign-in is three chains:

| what the plugin needs | chain | what it gets |
| --- | --- | --- |
| `uh`, which `us` carries | PBKDF2, then `take(16, 16)` | the half MEGA is meant to receive |
| the master key | PBKDF2, `take(0, 16)`, AES over `k` | the account's master key |
| the RSA private key | the same, then AES over `privk` | the key block |

The **password key** — the first sixteen derived bytes, the one value that is a direct function of
the password — is an intermediate in two of those chains and the output of none. It never enters
the sandbox. `plugins/mega-auth` could have asked for all thirty-two bytes in one call and done
the AES itself for 342 965 fuel; it does not, and
`crates/rd-plugin-ext/tests/mega_auth_contract.rs::the_password_key_never_leaves_the_host` holds
it to that.

Three things bound the primitive, all enforced by the host:

- **The grant.** `capabilities.key_derivation` in the manifest. Without it the interface is not in
  the import allowlist (`crates/rd-plugin-host/src/runtime.rs`) and not linked
  (`crates/rd-plugin-host/src/extension/mod.rs`), so a component that imports it does not
  instantiate. The capability is accepted only on the three plugin types whose worlds import the
  interface — `auth`, `crawler`, `stream-transform` — and only together with at least one
  `capabilities.secrets` entry.
- **The shape.** A chain has to begin with `pbkdf2-hmac-sha512`, at or above 100 000 rounds. See
  below; this is the rule the whole confidentiality argument rests on.
- **The price.** Charged against the caller's fuel budget before the work is done. See below.

## Why the returned bytes do not reconstruct the credential

This is an acceptance criterion of RD-120-20 and it deserves more than a sentence. The parameters
are attacker-chosen: a hostile plugin picks the salt, the round count, the window and the AES
operand. So the argument has to hold for every chain the host accepts.

**1. Every chain begins with a one-way function.** The host refuses a chain whose first step is
anything but `pbkdf2-hmac-sha512` (`plugin.key_derivation_needs_one_way`). Without that rule the
two other steps are each an immediate break: `take` alone returns a window onto the credential —
the credential itself — and `aes-ecb-decrypt` alone is an oracle that transforms the password
under one block of work, which a dictionary runs through. With the rule, the only path from an
output back to the credential is guessing.

**2. The host is never a cheaper oracle than the guest's own arithmetic.** Two things together
make this true. The round floor of 100 000 (MEGA's own) means a single candidate always costs a
full derivation, and the fuel charge is the **measured guest price** of that derivation — 33 383
fuel per round per hash block, taken from RD-120-11's measurement of the same computation inside
the sandbox. So asking the host to try a candidate costs exactly what trying it in the guest
costs. The primitive gives an attacker **no computational advantage at all**; it changes only
where the credential lives.

**3. Fuel bounds how many candidates one invocation can afford.** A candidate is 3.34 billion
fuel. The default budget is 2 billion — not one complete candidate. The largest budget a manifest
may declare is 40 billion — eleven. Fuel is not replenished within an invocation, and an
invocation is started by the host, for a person's own account, against the provider the plugin
claims. A dictionary attack is not a thing that fits in that.

**4. A chosen salt does not help.** A precomputed table has to be recomputed for whatever salt was
used, and point 2 fixes the per-candidate price whatever the salt is.

**5. The later steps cannot undo the first.** `aes-ecb-decrypt` under an unknown key is a
permutation, not an inversion; `take` only discards. Neither recovers anything about the
PBKDF2 input.

**6. What the guest does receive is provider key material, not the credential.** `uh`, the master
key and the wrapped private key are values MEGA hands to any client that signs in. They are not
the password and do not yield it. A guest that holds the master key *and* the wrapped `k` could
test password candidates offline — and that costs the same 3.34 billion fuel per candidate, by
point 2.

**7. Nothing is widened by the grant.** A credential may be *sent* only to the domains its slot
names (`rd_provider_registry::secret_domain_allowed`). Derived bytes are not gated that way: once
they are bytes in the guest's memory, the only thing deciding where they go is the plugin's own
`net_http` list. So `GrantedHost` refuses `derive_from_secret` unless **every** domain the plugin
can reach is a domain the credential itself could have been sent to
(`plugin.key_derivation_reach_too_wide`). A plugin that may compute over a credential can reach
nowhere that credential could not already have gone.

**What this does not claim.** The host guarantees the credential never leaves. It does not
guarantee that a plugin declines material it *could* have asked for: a chain of one PBKDF2 step
with `length: 32` returns both halves, and a plugin that asked for that would hold the password
key. There is no principled line the host could draw there — every byte of a derivation output is
legitimately somebody's key material. What the chain buys is that a correct plugin **need not**
ask, and `plugins/mega-auth` does not.

## The fuel accounting, and why it is this and not something else

RD-120-20 required the accounting to be argued rather than guessed, because the obvious answers
are both wrong:

- **Charging nothing, or a token fee.** The cap exists to bound how much computation a plugin may
  cause. A primitive cheaper than the guest's own arithmetic is a discount — and worse, it is a
  cheap oracle, which is exactly what point 2 above must not allow. This is the "way around the
  cap" the job named.
- **Charging what it costs the host.** The host computes natively, orders of magnitude faster.
  Billing that would make the primitive nearly free and collapse into the first case.

So the rate is **the measured guest price**: what the same computation cost inside the sandbox
when RD-120-11 measured it, to the fuel unit.

| step | rate | where it comes from |
| --- | ---: | --- |
| `pbkdf2-hmac-sha512` | 33 383 per round per hash block | 3 338 300 549 fuel / 100 000 rounds, one block for 32 bytes out |
| `aes-ecb-decrypt` | 8 166 per 16-byte block | 342 965 fuel / 42 blocks, rounded up |
| `take` | 1 per byte kept | a copy the guest would have made |
| the call | 2 | the measured cost of an empty guest call |

The property this gives the contract is **fuel neutrality**: computing a stage on the host costs
a plugin exactly what computing it in the sandbox would have cost. The primitive therefore buys a
plugin *nothing* except that the credential stays where it belongs — which is the only thing it
is supposed to buy. `plugins/mega-auth` declares 12 000 000 000 fuel for three chains plus the
RSA operation, and that declaration is the whole of what the primitive costs it.

The charge is made **before** the work, inside the same call, against the store the guest is
running on. That needed the function to be hand-wired with
`wasmtime::component::LinkerInstance::func_wrap_async` rather than generated by `bindgen!`: a
generated host function is handed the store's *data*, and fuel lives on the store. An accounting
made after the call would be a bill for work already performed, which is not a cap. A chain that
costs more than the invocation has left takes what is left and is refused
(`plugin.key_derivation_budget`) — a guest that would have computed it itself would have burned
the same fuel and trapped.

The wall-clock consequence — the host does in about a hundred milliseconds what would have cost
the guest tens of seconds — is bounded by the *other* cap and not by this one. Host time counts
against the invocation's execution deadline exactly as guest time does, which is why `host.wait`
has to push that deadline forward explicitly and this does not.

## Consequences

- The contract is `rdownloader:plugin@0.7.0`. All eleven `sdk/templates/*/wit/rdownloader.wit`
  copies moved with it and every bundled manifest declares the new `api_version`. There is no
  installed base to keep compatible (`AGENTS.md`, "Scope"), so `0.6.0` is simply gone.
- `plugins/mega` carries a `[provider]` section, which a manifest of that type could not before.
  Until now MEGA had no provider row at all: both its manifests were `[extension]` sections, the
  registry is filled solely from `[provider]`, so no MEGA account could be created and the
  sign-in would have claimed a provider that did not exist. A stream-transform plugin is a
  resolver in everything but the world it exports — it claims the addresses, it talks to the
  provider's API, it answers with the address a download runs on — and it exports the twelfth
  world only because MEGA encrypts on the client (ADR 0011).

  **The sign-in was tried as the owner first, and the codebase refused it twice.** Once on the
  rule that `provider.secret_reference` has to appear in `capabilities.secrets`, which
  `plugins/mega-auth` satisfies and `plugins/mega` then did not. And once, decisively, on
  `PluginManifest::message_slug`: a manifest's message namespace **is** its provider slug when it
  has one, so a `[provider]` on the sign-in would have moved its codes out of `mega_auth.*` and
  into `mega.*`, where the file plugin already owns thirteen — with `api_error`, `rate_limited`
  and `unavailable` colliding outright. One namespace, one owner. The cost of putting the row
  where it belongs is that `plugins/mega` now grants `secrets = ["mega_password"]` without
  expanding the marker today; the slot's own `secret_domains` pin it to the command endpoint
  either way, and the account-file path will need a secret grant there regardless.
- `plugins/mega-auth` is the first user. What it stores is `{"sid": …, "mk": …}`: the session
  identifier and the master key, because an account file's node key is wrapped under the master
  key and nothing else opens it.
- MEGA account **version 1** is refused by name (`mega_auth.account_version_unsupported`). Its
  legacy derivation is 65 536 AES rounds over the password, a fourth step nobody has an occasion
  for yet. Each later addition gets its own; this is not a construction kit.
- The `key-derivation` interface is imported by three worlds — `auth-plugin`, `crawler-plugin`
  and `stream-transform-plugin` — although only `auth` has a user today. That is deliberate: a
  world import cannot be added without another contract bump and another eleven-copy sync, and
  the two other worlds are exactly where MEGA's account *files* will need it. The capability gate
  is what actually decides who gets it, and that moves without a bump.

## Addendum, 2026-09-23 — the first step follows the credential's origin (RD-120-30)

- **Status:** Accepted with RD-120-30, on the project owner's decision of 2026-09-23
- **Replaces:** the rule "a chain has to begin with `pbkdf2-hmac-sha512`" (Decision, "The
  shape", and point 1 above). Nothing else in this ADR changes.

### What the rule was, and why it could not stay

Point 1 made every chain open with PBKDF2 at 100 000 rounds or more. It was uniform, easy to
check and correct for the one kind of credential RD-120-20 had: a password, whose entropy is
whatever a person chose, so guessing it is the threat and the expensive one-way step has to come
first.

The sign-in RD-120-20 built stores `{"sid", "mk"}`, and a file in the account has its node key
wrapped under `mk`. Opening it is one AES block operation under the master key. Under the old
rule that is unreachable: PBKDF2 in front of a key turns it into a different key. And the rule
protected nothing there — MEGA's master key is sixteen random bytes MEGA's own client made, and
nobody guesses it.

Removing the rule was not an option. Without a replacement, `aes-ecb-decrypt` first over **any**
handle is a decryption oracle under the first sixteen bytes of whatever the handle names — for a
password, one block of work per dictionary guess, exactly what point 1 shut; and `take` first is
the credential itself.

### What holds now

The first step is decided by **where the credential came from**, and the host decides that from
**where it reads the value** — never from anything the guest sends:

| origin | read from | written by | the chain begins with |
| --- | --- | --- | --- |
| a person (`SecretOrigin::Person`) | `accounts.secret_ref` | the accounts form | `pbkdf2-hmac-sha512`, ≥ 100 000 rounds — unchanged |
| a sign-in (`SecretOrigin::SignIn`) | `auth_flows.key_ref` (migration `0091`) | only the host's `store-token` path | `aes-ecb-decrypt`, keyed by all sixteen bytes |

`take` first is refused for both. The origin is decided in
`crates/rd-plugin-host/src/native/signin.rs` from the reference's slot in the provider table:
`filled_by = "flow"` means the value is read from the flow row, anything else from the account.
The rule itself is `crates/rd-plugin-host/src/keyderive/origin.rs`.

A sign-in leaves key material by storing `{"token": …, "key": <16 bytes, base64url>}`
(`crates/rd-plugin-host/src/session.rs`). The host splits it into two vault entries: the token
under `auth_flows.access_ref`, which `{{secret:<flow slot>}}` sends, and the key under
`auth_flows.key_ref`, which **no marker, header or route reads**. Any other JSON object is refused
rather than stored as a token and sent whole. Both references are written in one statement, so a
token never pairs with another session's key; a renewal that replaces the token drops the key.

### Can a plugin forge the origin?

No, and each way to try is closed by where things are written, not by a check that could be
skipped:

- **The handle is a name.** `secret-handle` carries a reference and nothing else; the WIT has no
  field for an origin, and this change adds none.
- **Declaring a slot flow-filled moves nothing.** A manifest may declare its own slot
  `filled_by = "flow"`; that only changes where the host *reads* — the flow row — and the
  accounts form never writes there. A typed password stays in `accounts.secret_ref`, where only
  the PBKDF2-first rule reaches it. Test:
  `native::signin::signin_tests::a_person_s_credential_is_never_read_as_sign_in_key_material`.
- **`store-token` writes only what a guest already held.** A guest cannot put a person's
  credential into `key_ref`, because it never has one: the password never enters a sandbox
  (point 6 above). Whatever lands in `key_ref` was in some sign-in plugin's memory already.
- **A plain token is not key material.** A flow that stores a string, and every OAuth token,
  leaves `key_ref` empty; a chain over such a slot finds nothing to compute over
  (`plugin.provider_secret_missing`), never the token.

### What a hostile guest can do now that it could not before

Exactly one thing: **ask for AES-128 decryptions under a sign-in key it may name.** For MEGA that
means everything MEGA wraps under the master key — every node key of the account's own files and
folders, and the RSA private key block. It cannot:

- **recover the key.** It gets decryptions only, never encryptions, under a random 128-bit key;
  recovering an AES key from chosen ciphertexts is not a known attack. The one way it could have —
  a `take` window of fifteen known bytes and one unknown in front of the AES step, compared against
  256 trial decryptions of its own — is refused, because the AES step keys itself with the whole
  stored key and a window first is never admitted
  (`keyderive::origin::origin_tests::a_window_in_front_of_the_key_step_is_refused_because_it_recovers_the_key_bytewise`);
- **reach a typed credential** — the other origin's rule is unchanged, and the origins are kept
  apart by storage location;
- **send what it derives anywhere new.** Point 7's reach rule applies unchanged, over the session
  slot's own hosts. It is now compared as **patterns**: a `*.suffix` reach needs the same wildcard
  in the slot. Before this, a wildcard reach was judged by its bare suffix, which answered for one
  host the plugin could reach and none of the others. `mega_session` lists
  `g.api.mega.co.nz`, `mega.nz` and `*.userstorage.mega.co.nz` — every host `plugins/mega` reaches,
  all of them MEGA's — and a slot's wildcard now admits sub-domains for sending, never the bare
  suffix. No slot declared a wildcard before, so nothing else moved.

Nor is it a new capability for MEGA specifically: `plugins/mega-auth` already held the master key
in its own sandbox while signing in (it computes the RSA operation there). What the rule adds is
that a *different* plugin — the file plugin, the crawler — may use it without holding it, and
only if it declares `key_derivation`, lists `mega_session` and reaches nowhere the session may
not go.

### Does the fuel accounting still mean anything?

Yes, for the same reason as before and one more. It is still **fuel-neutral**: an AES block costs
8 166 fuel, what it measured inside the sandbox, so the host is never cheaper than the guest's own
arithmetic. The 100 000-round floor was never about AES; it made each *password guess* cost a
full derivation. Over a sign-in key there are no guesses to price: the key is not chosen by the
guest and a decryption oracle does not converge on it however many calls it gets. So the cap
bounds work, as everywhere else, and the per-candidate argument of points 2 and 3 simply has no
candidates to apply to. One node key costs 2 + 2 × 8 166 fuel.

### Why not the other ways

- **Re-derive the master key from the password on every download.** Allowed by the old rule, and
  3.34 billion fuel and a password computation per file, with the password in use long after the
  sign-in. The session exists so that the password is needed only to sign in.
- **Hand the master key to the file plugin.** A credential in a sandbox is what this ADR exists
  to avoid.
- **Mark the origin in the contract.** Unnecessary — the host already knows it — and a WIT change
  stales every component.

### Consequences

- `store-token` for a provider with a flow slot keeps the session **beside** the person's
  credential instead of over it. Until RD-120-30, MEGA's session replaced the password, and the
  next sign-in computed PBKDF2 over a session identifier.
- `plugins/mega` owns `mega_password` (the person's) and `mega_session` (the sign-in's) and grants
  itself only the second; `plugins/mega-auth` grants the first.
- A crash point was not added: the session write is a variant of the OAuth token write beside the
  flow, with the same order — new entries first, the row in one statement, old entries last — and
  the recovery matrix is about a download's bytes. The pairing is tested in
  `signin_tests::signing_in_again_replaces_both_halves_and_drops_the_old_ones`.
