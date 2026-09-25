# ADR 0011 — MEGA: the key travels through the contract, the bytes never leave the host

- **Status:** Accepted by the project owner on 2026-09-22
- **Date:** 2026-09-21
- **Job:** RD-103-02
- **Supersedes:** —

## Context

RD-103-02 asks for public and authenticated MEGA files and folders, with structure, resume and
"providerseitiger Entschlüsselung". The measurements behind this record — every command, status
code and recomputed value — are in `docs/roadmap/jobs/120-11-mega.md` (`RD-103-02` until
2026-09-22; the identifier in this record is deliberately left as it was), section "Messung und
Entwurf, 2026-09-21". In short, and all of it verified against a public file from somebody
else's README on 2026-09-21:

MEGA encrypts every file client-side. The key rides in the URL fragment and never reaches the
provider. `POST https://g.api.mega.co.nz/cs` with `[{"a":"g","g":1,"p":"<handle>"}]` answers
with the plaintext size, an encrypted attribute blob and a short-lived storage URL that is then
a plain ranged GET — `Range: bytes=1048576-1048591` answers `206` with the matching
`Content-Range`. Folder listing needs no session at all: `?n=<folder handle>` with
`[{"a":"f","c":1,"r":1,"ca":1}]` returns every node of the folder in one response, with no
cursor and no page token.

The crypto was not read off somebody's source, it was recomputed here and matches byte for byte.
The 43-character fragment key is eight `u32`; the file key is the XOR of its halves, the nonce
is words four and five, the expected meta-MAC is words six and seven. Attributes are AES-128-CBC
under a zero IV and decrypt to `MEGA{"c":…,"n":…}`, where `c` carries four sparse CRC32 values
over 8192 sampled plaintext bytes and the file's mtime as a little-endian integer. The payload
is **AES-128-CTR with the counter block `nonce(8) ‖ big-endian u64 block index`** — so any
16-byte boundary decrypts on its own, with no preceding byte. Integrity is a CBC-MAC per chunk
(128 KiB × i for i = 1…8, then 1 MiB, IV = nonce‖nonce), condensed by a second CBC-MAC into the
meta-MAC. Computing that over the measured file produced exactly the value the fragment carries.

So the file is streamable and seekable, and a decrypting download costs nothing beyond the XOR
and the MAC. JDownloader (`svn_trunk/src/jd/plugins/hoster/MegaConz.java`) and pyLoad
(`src/pyload/plugins/downloaders/MegaCoNz.py`) nevertheless both download an encrypted file to
disk and decrypt it in a second pass — `.encrypted` and `.crypted` respectively — which costs a
full extra read-and-write of the file and its size again on disk. Neither verifies the meta-MAC.

Against that stands the shape of the contract:

**`resolved-download` is a URL and headers, and a resolver never sees a byte.**
`crates/rd-plugin-api/wit/rdownloader.wit:46` gives it `url`, `file-name`, `size`, `headers`,
`checksum-algorithm`, `checksum-value` and `client`. There is no field in which key material
could travel, and no world in `rdownloader:plugin` in which a plugin could decrypt a stream
affordably. That is the open question, and it is not about MEGA alone: every provider with
client-side encryption arrives at the same wall.

## Options

### 1. A `transfer` backend that fetches and decrypts the stream itself — rejected

`transfer-plugin` already owns a long-lived `run`, imports `net`, `http` and `sink`, and
`sink::write-at` would happily take decrypted bytes. It is a real design, not a strawman, and it
is the first thing anybody will propose again.

It fails three times over, each on its own sufficient.

- **It routes by scheme, and MEGA is `https:`.** `[transfer] schemes`
  (`rd-plugin-host/src/manifest.rs:335`, `rd-plugin-transfer/src/lib.rs:76`) is what sends a link
  to a backend. Making MEGA one means inventing a pseudo-scheme and building something that
  rewrites links into it — a second routing mechanism, for one provider.
- **It would reimplement `rd-http` inside the sandbox.** A transfer backend gets sockets and a
  request function, not a resumable engine. Parallel chunks, range validation, checkpoints, the
  interplay with the bandwidth limiter, mirrors: all of it, per provider, in guest code that
  nobody can profile.
- **The fuel does not reach.** `DEFAULT_FUEL` is 2·10⁹ (`rd-plugin-host/src/lib.rs:75`), a
  manifest may ask for at most twenty times that (`manifest.rs:241`), and `runtime.rs:265` sets
  it **once per store** — `create_transfer_store` builds one store per transfer attempt, so a
  whole `run` shares one budget that is never refilled. The work is two AES-128 block operations
  per 16 plaintext bytes, 134 217 728 of them per GiB, on a target with no AES-NI and no
  `simd128`. At an optimistic 200 wasm instructions per block that is ≈2.7·10¹⁰ fuel per
  gigabyte: two thirds of the largest budget a manifest may declare, with the ceiling at about
  1.5 GiB. That per-block figure is an estimate, not a measurement — this phase built nothing —
  but a tenfold more favourable one only moves the wall to ~15 GiB and still doubles the CPU of
  every MEGA download.

Worth naming honestly, because the bestiary says otherwise: the transfer world is right that
somebody has to hold the stream. What it gets wrong is putting the stream inside the sandbox.

### 2. A resolver plus a post-processing step — rejected, as impossible rather than as expensive

The reference implementations' design: download the ciphertext, decrypt afterwards. Zero
contract change, or so it looks.

`postprocess::run(step-input) -> step-end` (`wit/rdownloader.wit:748`) receives a package handle,
a file list and a checkpoint. **There is no channel for key material**, so the resolver cannot
hand the step what it would need. And `source` offers reading and `rename`, not writing: a
post-processing plugin could not create the decrypted file even if it had the key. The option
is not costly, it is unavailable.

As a *native* step in `rd-postprocess` it would work — and it would be a core change all the
same, while additionally paying what JDownloader and pyLoad pay: a full second pass over the
file and its size a second time on disk, for every MEGA download, on a manager whose users pull
whole folders. It is the fallback if this record is rejected, not the answer.

### 3. A new field on `resolved-download` — rejected

The smallest-looking change: one `option<…>` beside `checksum-value`, and the existing resolver
world carries MEGA.

It changes the type of the resolver export, so **every `.rdplug` of type `resolver` already
signed and installed stops instantiating** — sixteen bundled ones and any third-party resolver
this project cannot rebuild. That is exactly the class ADR 0001 reserved a version bump for
("a changed signature, a removed field, a renamed record") and exactly the ground on which
ADR 0003 rejected its own option 3. Paying it to add one optional field is the worst available
trade.

The variant of smuggling the key into a `resolved-header` is worse still. Headers are logged and
replayed on every chunk request, and the job's acceptance criteria say in as many words that
decryption keys never appear in logs or UI URLs.

### 4. Decryption in core, fed by an additive twelfth world — chosen

A new interface and a new world in `rdownloader:plugin`, joining the eleven, at
`api_version = 0.6.0`. The plugin answers with the file's address **and a declarative
description of the transform**: named primitives with their parameters — AES-128-CTR under this
nonce with the counter starting at this block, chunk MACs at these boundaries, this expected
meta-MAC. The host holds the bytes and executes a fixed, reviewed set of primitives.

- **The transform has a home already.** `rd-http` writes at an absolute offset:
  `engine.rs:350` `self.part.write_at(position, bytes.to_vec())` into
  `rd-files/src/part_file.rs:91`. CTR is seekable, so decryption is a transform on a buffer that
  is already allocated at a position that is already known. Nothing about chunking, checkpoints,
  ranges or the limiter changes, there is no second pass, and no second copy on disk.
- **Parallel chunks survive.** The chunk MAC is sequential *within* a MEGA chunk and independent
  *between* chunks. Aligning the engine's chunk boundaries to the provider's and carrying the
  finished chunk MACs in the checkpoint keeps both the parallelism and the integrity check that
  neither reference implementation performs.
- **Refresh is already solved.** MEGA's storage URL is short-lived and differs on every `g` call.
  Rule 5 of the shared cloud-source interface applies unchanged:
  `crates/rd-scheduler/src/replay.rs:171 before_resume` re-asks the resolver before continuing
  and replaces URL and headers. It is the same call that would re-supply the key material.
- **The crypto ends up on the right side of the sandbox.** Sandboxing exists to contain a
  provider's parsing, not to hide a cipher. Key handling in reviewed core code, under the
  project's existing `aes` and `cbc` dependencies, is better than key handling in a signed blob
  nobody profiles.
- **It generalises without being generic.** Nothing in the interface says MEGA. A provider
  declares which named primitive applies from which offset under which key, and which integrity
  value is expected at the end. The next provider with client-side encryption is one more
  plugin, not one more crate and one more release — which is the rule `rd-provider-registry`
  already holds for providers.
- Costs an interface, a world, the byte-identical copies under `sdk/templates/*/wit/`, one
  `PluginType`, the transform and MAC accumulator in `rd-http`, and the chunk MACs in the
  checkpoint format.

## Decision

**Option 4.** A twelfth interface and world join `rdownloader:plugin`, carrying a declarative
description of a content transform from the plugin to the host. The host owns the byte stream,
executes the named primitives in `rd-http`, and verifies the provider's integrity value before
the part file is promoted.

Four properties are part of the decision rather than of the implementation:

- **The plugin describes, the host computes.** What crosses the boundary is parameters for
  primitives the host already implements, never code and never a callback per buffer. A
  primitive the host does not know is a refusal at instantiation time, not a fallback, so the
  set of ciphers in the product stays a decision of this repository.
- **Key material is a secret from the moment it arrives.** It goes to `rd-secrets`, not into a
  download row, and it is excluded from every surface that prints a URL or a header —
  `rd_core::redact_text` and the diagnostics layer included. The acceptance criterion
  "Entschlüsselungskeys erscheinen nie in Logs/UI-URLs" is a property of the contract, not of
  the plugin's good manners.
- **The integrity value is checked, and a failure keeps the partial file.** MEGA's meta-MAC is
  the only thing that distinguishes a correct download from a silently wrong key, and neither
  reference implementation checks it. The host does, before promotion, and treats a mismatch the
  way it treats a length mismatch: a failed attempt, not a truncated file presented as complete.
- **Resume validates the transform, not only the bytes.** The checkpoint carries the finished
  chunk MACs and which transform description wrote them. A continuation whose description
  differs starts over rather than resuming somebody else's state — the rule `transfer` already
  follows for its opaque checkpoints.

## `api_version` stays at `0.6.0`

The same answer ADR 0001 gave and ADRs 0002 and 0003 confirmed, for the same reason: this is
additive. A new interface and a new world take nothing away from an existing world, so every
plugin built against the contract without it still satisfies the world it declares. No exported
function's signature changes, no record loses a field, nothing is renamed — which is the list
ADR 0001 reserved a bump for, and the reason option 3 above was rejected rather than chosen.
An older host refuses a package of the new type outright: its `PluginType` does not deserialize
there, so the manifest is rejected as an unknown plugin type rather than half-loaded.

## Consequences

- MEGA becomes sibling packages in the shape rule 1 of the shared cloud-source interface set
  down (`docs/roadmap/jobs/106-04-google-drive.md`): the new type for file addresses and the key
  schedule, a `crawler` for folders, and an `rlib` `-common` crate like `mediafire-common`.
  In place of `oauth` it needs an `auth` package, because MEGA's `us0`/`us` login is not OAuth.
- Folder pagination is local. MEGA's `a=f` answers with every node of a folder in one response
  and offers no cursor, so rule 4's "at most 10 pages per folder" does not apply; the abort
  limit is the size of the single answer.
- `ctr` becomes a direct workspace dependency. `aes` and `cbc` already are
  (`Cargo.toml:113,122`), used by `rd-collector/src/dlc.rs`, `rd-collector/src/rsdf.rs` and
  `rd-capture/src/cnl.rs`, so the crypto family in the tree does not change.
- `rd-http` grows a transform and a MAC accumulator on its write path, and the checkpoint format
  grows the chunk MACs. Both are behind an `option`: a download with no transform described runs
  exactly the code it runs today.
- The MEGA API is official and documented, and its SDK (`meganz/sdk`, "Simplified (2-clause) BSD
  License") states that applications must present a valid application key and comply with the
  terms of service. The key is registered per installation and never compiled in, which is
  rule 8 of the shared cloud-source interface applied unchanged.
- Password-protected links (`#P!`) are out of this record. The form exists, neither reference
  implementation supports it, and its derivation was not measured. It is a later job or an
  explicit non-feature, not an assumption.

## Addendum, 2026-09-22: the sentence this record was accepted on was not true yet

This record was accepted on "the key travels in the link fragment the person already has".
Nobody checked whether the fragment still existed by the time a resolver was asked. It did not.

`rd_core::candidate_url` strips the fragment off **every** address before a candidate row is
written, and `rd-db`'s intake applies it on every path into the LinkGrabber — pasted text, a
container, an NZB import, a hotfolder, a subscription poll. That rule is RD-109-32's and it is
right: nothing distinguishes a share password from an anchor name, a fragment is the one part
of a URL never sent to a server, and one wrong letter in a host name was enough to write a
share password into `link_candidates.url` in clear text and print it in every LinkGrabber row.
So the address that reached the queue carried MEGA's handle and not its key, and neither a
file link nor the crawler's child links were resolvable. RD-103-02 phase 2 shipped unable to
work, and the collision was recorded as a failing-by-description test in `rd-core`.

Two accepted decisions, both right, pointing in opposite directions. The project owner decided
on 2026-09-22: **the fragment goes into the vault.** RD-110-38 implements it.

- A provider **declares** the hosts whose fragment is key material, in its manifest
  (`secret_fragment_domains`, `docs/plugins.md`). Without that declaration nothing changes.
  There is no host list in the code and no special case for one service.
- The intake puts the fragment away through `rd-secrets` before it shortens the address, and
  writes only a `vault://` reference into `link_candidates.secret_fragment_ref` (migration
  `0087`). The stored address is shortened exactly as every other one.
- At resolve time the scheduler reads the reference back and restores the fragment onto the
  address one call before the plugin is asked — the same shape as the `{{secret:…}}` expansion
  the host already does on an outgoing request. The resolver gets the key, not the row.
- Ownership moves with the link: the queue row inherits the reference when the candidate is
  enqueued, and deleting whichever row owns it removes the secret from the vault.

Two alternatives were put and rejected. *Leaving the fragment in place for a declaring
provider* is cheap, but it writes the decryption key exactly where RD-109-32 removed it and
contradicts RD-103-02's second acceptance criterion word for word. *Resolving MEGA at intake*
would mean the link never becomes a candidate — at the cost of the LinkGrabber step, review and
folder choice before queueing, and of making one provider a special case in the intake path.

What does not change: `resolved-download` still carries no key, the description still travels
beside the address rather than inside it, and `api_version` stays at `0.6.0`. Nothing in the
contract moved; what moved is where the key waits between intake and resolve.

## Addendum, 2026-09-22 (RD-120-11): the sign-in was priced, and the price is not the problem

This record left one number open and said so: what the account sign-in costs a guest. It is now
measured rather than estimated — `scripts/measure-mega-login-fuel.sh`, `plugins/mega-login-probe`
and `cargo run -p rd-plugin-host --example mega_login_fuel`, under the same Wasmtime
configuration a real plugin runs in. The full table is in the job file; the two figures that
decide anything are these:

- **The RSA-2048 private operation costs 189 444 831 fuel** — under a tenth of `DEFAULT_FUEL`
  and under half a percent of the largest budget a manifest may declare. The operation everyone
  expected to be the obstacle is the cheapest interesting thing in the sequence.
- **The password derivation MEGA mandates costs 3 338 300 549 fuel** — PBKDF2-HMAC-SHA512 at
  100 000 rounds, 17.6 times the RSA operation and 1.67 times what a plugin gets by default. A
  whole sign-in is 8.8 % of the ceiling, so a manifest that declared four to eight times the
  default would carry it, and it is allowed to.

**Fuel is therefore not a reason to leave the sign-in unbuilt.** What does stand in the way is
this contract's own rule: a plugin never sees a credential, and the host's `{{username}}` /
`{{secret:…}}` substitution happens on the way *out* of the guest. MEGA's `us` call does not want
the password; it wants a value derived from it, and the same derivation produces the key that
unwraps the master key, the RSA key and the session identifier. A guest that cannot hold the
password cannot produce any of them.

So the sign-in needs a decision rather than an implementation, and three are open: a host-side
key-derivation primitive (additive, provider-neutral, but it hands derived key material to a
guest), a native sign-in in the core the way FTP and SFTP are core, or no MEGA account at all.
**Nothing was built in the meantime.** A signed `mega-auth` package that cannot perform its own
task would be worse than none, and raising a fuel limit quietly would have answered a question
nobody was actually asking.

One correction to what this record assumed about its own implementation. It says the key goes to
the vault and the description keeps the reference; the reference was never written. The host
answers `key_reference: None` and leaves it to its caller, and no caller filled it in, so
`StreamTransform::new` refused every transformed download as `transform.key_missing`. Migration
`0089` adds `downloads.transform_key_ref`, `Database::adopt_transform_key` puts the key away
idempotently, and the scheduler calls it before it builds a transform. Idempotently matters:
the reference is part of the fingerprint, so a fresh one per attempt would have told every
continuation that its own chunk MACs belonged to somebody else.
