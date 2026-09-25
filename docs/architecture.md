# Architecture

The long half of `AGENTS.md`, moved here by RD-120-25 so that the rules an agent must
follow on every task are not carried behind six thousand words it needs on one task in
twenty. Nothing was shortened on the way: this is the same text, plus anchors.

Two parts. **The crates** is the crate-by-crate account with its job history, one section
per crate. **Why the build and the checks look the way they do** is the reasoning behind
the rules that stayed in `AGENTS.md` — the rules are there, in short form; the reasons,
the anecdotes and the incidents that produced them are here.

## The crates

`crates/rdownloader` is the service + CLI binary; it wires everything together. The layers underneath:

### rd-api

Axum REST + SSE, authentication, utoipa OpenAPI, the built-in MCP server (`/mcp`, rmcp), and the embedded web assets. All HTTP surface lives here.

**What the toolbox covers, and why it does not cover everything** (RD-120-29): `rd-api/src/mcp/coverage.rs` holds one row per *capability* — what a person can do, not what a route is — with a decision: covered, or left out with the reason. It is not documentation beside the code but a table the tests enforce. The module is `#[cfg(test)]`, because it decides nothing at run time; `coverage::tests` holds it against `crate::openapi_document()` in both directions, so a REST route that belongs to no capability fails `cargo nextest run -p rd-api --lib` rather than becoming an undecided gap, and `mcp::TOOL_POLICY` supplies the tool column, so "covered" cannot be claimed where no tool exists. The comparison in `docs/mcp-coverage.md` is generated from it by `scripts/mcp-coverage.sh` and compared by a test, the same arrangement `rd_core::failpoint` has with `docs/recovery-matrix.md`. Three rules decide the omissions: nothing that hands back or collects a secret, nothing destructive without a confirmation a person gives, nothing that is only meaningful as a screen. Deliberately *not* a rule: parity with REST. A toolbox that can do everything is harder for a model to use well than one that can do the right thing.

### rd-core

shared domain contracts (settings, job/package types) used across crates; changes here ripple everywhere.

### rd-db

SQLite (sqlx) persistence with a **serialized writer**: mutations go through writer commands, not ad-hoc queries. Migrations are numbered SQL files in `crates/rd-db/migrations/`; add new ones at the end, never edit applied ones. It also owns the event bus (`src/event_bus.rs`): the broadcast channel plus the bounded in-memory buffer both SSE streams resume from with `Last-Event-ID` (RD-110-23) — in memory on purpose, so a resume never survives a restart and nothing has to be migrated.

### rd-scheduler

persistent unified queue for all download types (HTTP, Usenet, media, torrent, …).

### rd-http

resumable HTTP download engine (parallel chunks, checkpoints). Since RD-110-33
it also owns the content transform: `src/transform.rs` holds the AES-128-CTR keystream and the
chunk-MAC accumulator, `engine.rs` applies them where it already writes at an absolute offset,
the finished chunk MACs travel in the checkpoint, and the provider's integrity value is
verified before the part file is promoted. Both are behind an `option`: a download with no
transform described runs exactly the code it ran before. A run whose chunk boundaries do not
line up with the provider's falls back to a single connection rather than condensing a value
it cannot compute.

### rd-usenet

NNTP transport, yEnc, PAR2-aware resume.

### rd-media / rd-stream / rd-torrent / rd-gallery

`ExternalRunner` implementations on the shared queue (yt-dlp/ffmpeg, streamlink, librqbit, gallery-dl).

### rd-ftp / rd-sftp

resumable FTP/FTPS and SFTP transfer runners. Both drive **rd-transfer-file**, which owns everything about such a transfer that is not the protocol: the `.rdownloader/<id>.part` staging, the resume validation, the throttled progress write, the exact-length guard and the sync before the rename. It is its own crate because it needs `rd-db`, `rd-scheduler` and `rd-limits`, all of which depend on `rd-files` — so the obvious home is a cycle Cargo refuses. **rd-webdav** handles hardened WebDAV discovery and `PROPFIND`, while uploads are a storage plugin concern.

### rd-collector

the LinkGrabber: link intake, online checks, packages, categories, routing
rules, NZB import. Since RD-110-18 it also owns **mirror groups** (`src/mirrors.rs`): which
links of one package point at the same file, from the three sources in the order they are
trusted — what the page declared, a name and a size that agree, a name alone as a proposal.
It is pure, reads no state and writes none; `rd-db`'s `collector_mirrors.rs` is the half
that persists it (migration `0080`) and recomputes it at intake, on regroup and on a move.
A mirror is **not** a duplicate: the `Duplicate` state is neither an input nor an output of
the grouping, and the one place they meet is the rule that two links with the same address
are never mirrors of each other.

### rd-hotfolder / rd-subscription

watched-file intake and scheduled polling of channels, playlists, galleries, feeds and indexers into the LinkGrabber. Since RD-130-19 also the administrator's own scripts: `rd-subscription/src/script_adapter.rs` turns the lines a script prints into items and runs nothing itself -- the process is started behind the `ScriptRunner` trait by `rd-api`'s `SandboxScriptRunner`, through `rd_extract::ExtractionService::run_output_script`, the same sandbox post-processing scripts use. The script's name is stored as a `script:<name>` address in `url`. A subscription may carry a cron expression (`schedule`, migration `0096`, parsed with `croner` in `rd-subscription/src/schedule.rs`, POSIX weekdays, the service's local zone) that replaces the interval; the poller reads the time from an injected clock (`SubscriptionService::start_with_clock`), arms a scheduled row that has never been timed instead of running it, and never jitters a scheduled time. Creating, changing, switching or running a script subscription costs `api:admin` inside the handler; MCP, the area bundle and its import refuse the kind outright.

### rd-plugin-api / rd-plugin-host

versioned WIT contract `rdownloader:plugin` and the Wasmtime host: signature verification (Ed25519), atomic install, conformance, fuel/memory/domain limits and twelve plugin worlds — the eleventh, `remote-job`, carries work that runs at a provider and outlives the call that started it (RD-107-06, `docs/adr/0003-*`), and the twelfth, `stream-transform`, carries an address plus a declarative description of how its bytes become a file, for a provider that encrypts on the client (RD-110-33, `docs/adr/0011-*`). Its **selection** side (`StreamTransformProviders`) lives in `rd-plugin-host/src/extension/stream_transform.rs` beside the provider it selects and is re-exported from `rd-plugin-ext`, because `rd-scheduler` needs it too and may not depend on `rd-plugin-ext` (RD-103-02); `rd-scheduler`'s worker asks it wherever the resolver chain said nothing, and migration `0086` keeps the finished chunk MACs of a transformed stream across a restart together with the fingerprint of the description that wrote them. MEGA is its first real provider: `plugins/mega` (file addresses), `plugins/mega-crawler` (folders) and the `rlib` `plugins/mega-common`. **A link's key survives intake only because it is declared:** `rd_core::candidate_url` drops every fragment before a candidate row is written (RD-109-32) and MEGA's key *is* the fragment, so a plugin names the hosts whose fragment is key material in its manifest (`secret_fragment_domains`, RD-110-38). For those the intake vaults the fragment through `rd-secrets` and keeps a `vault://` reference in `link_candidates.secret_fragment_ref` (migration `0087`); the reference moves to `downloads.secret_fragment_ref` when the link is enqueued and is removed from the vault with whichever row owns it, and `rd-scheduler`'s worker restores the fragment onto the address one call before it asks the plugin. Without the declaration nothing changes. `rd_provider_registry::fragment_is_secret` is the only place that answers the question, and it answers from installed manifests alone — there is no host list in the code. The canary is `crates/rd-db/tests/secret_fragment.rs`; the end-to-end proof is `rd-plugin-ext::mega_contract a_vaulted_fragment_reaches_the_resolver_and_yields_the_same_key`. **The host computes over a credential the guest never sees (RD-120-20, `docs/adr/0020-*`):** `interface key-derivation` -- contract `rdownloader:plugin@0.7.0` -- takes a *handle*, one of the references the plugin's own manifest lists under `capabilities.secrets`, plus a chain of steps (`pbkdf2-hmac-sha512`, `aes-ecb-decrypt`, `take`), runs them over the credential and answers with the last step's output alone. It exists because `{{secret:…}}` substitutes on the way *out*, so a guest can send a password but not compute with one, and MEGA's `us` wants the second half of PBKDF2(password, salt, 100 000) rather than the password. `crates/rd-plugin-host/src/keyderive.rs` holds the shape rules, the arithmetic and the price; the function is hand-wired with `func_wrap_async` rather than generated, because the charge has to be made *before* the work against the store's fuel and a generated host function is handed only the store's data. The price is the **measured guest price** of the same computation (33 383 fuel per PBKDF2 round per hash block, 8 166 per AES block, RD-120-11), so the primitive is fuel-neutral and is never a way around the cap. The first step follows the credential's **origin** (RD-120-30, ADR 0020 addendum), which the host decides from where it reads the value, never from the guest: over a typed credential (`accounts.secret_ref`) a chain begins with PBKDF2 at 100 000 rounds or more, or the interface would be an oracle cheaper than the guest's own arithmetic; over the key material a sign-in stored (`auth_flows.key_ref`, migration `0091`, written only by `store-token`) it begins with `aes-ecb-decrypt` keyed by all sixteen bytes; a `take` first is refused for both. The rule is `keyderive/origin.rs`, the origin decision and the session write are `native/signin.rs`, and `session.rs` reads the `{"token", "key"}` shape a sign-in stores and splits it -- token to `access_ref`, key to `key_ref`. A provider with a `filled_by = "flow"` slot keeps its sign-in's session beside the person's credential instead of over it. `GrantedHost` refuses a plugin whose `net_http` reach goes beyond the domains the credential itself could be sent to, compared as patterns (`rd_provider_registry::secret_reach_allowed`): a `*.suffix` reach needs the same wildcard in the slot. `plugins/mega-auth` is the first user, and `plugins/mega` now carries the `[provider]` section that gives MEGA an account row at all -- the one extension type allowed to, because a stream-transform plugin is a resolver in everything but the world it exports. It sits there and not on the sign-in because `PluginManifest::message_slug` is the provider slug when a manifest has one, and `mega.*` already has thirteen codes. **The host also builds the one credential a guest could not send (RD-120-04):** `{{basic:<reference>}}` expands, in `crates/rd-plugin-host/src/native/expand.rs`, to base64 of `username:secret` -- no header name, no scheme word -- for a provider whose whole API is HTTP Basic, which Seedr's REST v1 says of itself. It is not a new capability and not a WIT change: `value_template` already carried it, so no component went stale. What makes it safe is that it goes through the *same* finder as `{{secret:…}}`, so the credential-mode gate and the secret-domain gate run for it unchanged, and it also trips the username gate, because half of what it expands to is the username; a missing half refuses under `plugin.secret_missing` or `plugin.username_missing` rather than sending `Basic base64(":password")`, and a colon in the user name under `plugin.basic_username_invalid`. `plugins/seedr` and `plugins/seedr-jobs` (RD-120-04) are its users, and they are the sixth remote-job provider -- the one whose poll is the account's *folder listing* rather than a transfer endpoint, because a finished Seedr transfer stops being a transfer and becomes a folder, so the endpoint named after polling can only answer "not here", which is also what a deleted transfer answers. **rd-plugin-transfer** adapts transfer plugins to the queue; **rd-plugin-ext** adapts auth, enricher, intake, notifier, postprocess, remote-job, storage and stream-transform plugins to their domain services. `remote-job`'s adapter is `rd-plugin-ext/src/remote_job.rs`, and its durable half is not in the plugin layer at all: migration `0065_remote_jobs.sql` holds the row — with `remote_jobs_content_idx`, the duplicate guard that has to be made *before* any network call because a provider's submit is not idempotent — `rd-db/src/remote_job_store.rs` persists it and `rd-api/src/remote_job_service.rs` sweeps it. `plugins/realdebrid-torrents/` is the reference guest and `rd-plugin-ext/tests/remote_job_contract.rs` the contract test to copy; `plugins/putio-transfers/` (RD-120-03) is the third, and the one to read when the provider offers *less* than Real-Debrid rather than more -- Put.io offers no file selection at all, so `poll` never answers `awaiting-choice` and the tree is chosen in the LinkGrabber instead, which is what the state machine looks like when the provider has no opinion about which files it fetches. Since RD-120-20 `job-source` carries a third case, `address(string)`, for the providers whose `src` takes an ordinary HTTP link or an `.nzb` as readily as a magnet; it travels the same row (`source_kind = "address"`, no migration) and the content key stays the guest's to derive. The state machine is therefore the *baseline* a new remote-download provider builds on, not something each one brings (corrected 2026-09-22; this sentence claimed the opposite and cost RD-120-00 a planning round). **rd-provider-registry** is the provider table, filled solely from installed plugin manifests — nothing is compiled in, so a provider exists exactly while its plugin does; the highest installed SemVer of a provider wins.

### rd-tools

managed external tools: the signed tool manifest, the verified download, the
`<data>/tools/<name>/<version>/` store, the `active.json` pointer that activation and rollback
rewrite, and the leases that keep a running job's binary from being removed. A leaf crate like
`rd-authn`, because `rd-core` cannot grow a dependency on reqwest and `rd-sign`; it plugs into
`rd_core::locate_tool` as a registered resolver instead. It also owns tool *versions*
(RD-102-03): `version` parses and caches what a binary reports about itself, `compat` holds
the four verdicts and the rules that ride in the same signed manifest, and the rule set in
force is process-wide state that degrades to the compiled-in base on any error. `rd-media`,
`rd-gallery` and `rd-stream` depend on this crate to gate their own capabilities; the matrix
and the platform differences are published in `docs/external-tools.md`.

### rd-captcha

broker for configured solver services and manual image-captcha answers requested by sandboxed resolvers.

### rd-extract / rd-postprocess

SABnzbd-style post-processing (PAR2 repair, unpack, cleanup, user scripts). `script_job.rs` is also the one place a script subscription's script runs (RD-130-19): `execute_for_output` reads the whole standard output, refuses more than the log limit instead of cutting it, kills a script that keeps printing, and turns a non-zero exit into a failure with the end of standard error as its reason.

### rd-files

shared safe path, file-name, capacity, checksum, staging and storage-root handling.

### rd-limits / rd-power

scoped bandwidth budgets and schedules, platform power/network context, quiet hours and queue-completion actions.

### rd-notify / rd-automation

notification delivery/retry plus versioned event-triggered conditions, actions and idempotent runs.

### rd-diagnostics

the structured log store's capture side and the diagnostic bundle
(RD-110-02): a `tracing` layer that redacts every record through `rd_core::redact_text`
*before* it leaves the layer and hands it to a bounded channel with a non-blocking send, the
sink that writes batches into `rd-db` and prunes in bounded steps, and the bundle's
deterministic inventory, manifest and archive. A leaf crate (only `rd-core`, `rd-db`,
`tracing-subscriber`, `zip`), because the binary installs the layer before it knows whether
the database opens. The system checks the bundle carries live in
`rd-api/src/diagnostics_checks.rs`, which `rdownloader doctor` prints too; the REST surface
is `rd-api/src/diagnostics_handlers.rs`. `docs/diagnostics.md` is the user-facing page.

### rd-authn

the authentication decisions `rd-api` makes: login rate limiting that cannot
become a self-lockout, CIDR matching, and which forwarded address may be believed. A leaf
crate so its tests run in under a second instead of behind the rd-api link cost.

### rd-secrets

encrypted, reference-based credential storage (providers, proxies, NNTP), and since RD-110-38 the home of a link fragment that is key material. `rd-db`'s `Database` holds one, installed after the store is opened (`install_secret_vault`) rather than passed to `open`, because the binary needs the store before the vault's master key comes out of the keyring; without one the intake drops the fragment exactly as it did before, which is what `rdownloader doctor` and the plugin CLI run with.

### rd-sign

the signature, digest, trust and freshness primitives everything verified shares:
the frozen length-prefixed digest framing, the `SignedDocument` envelope with per-document-kind
domain separation, the trust store (key *and* content revocation), the replay/staleness rule and
the compiled-in trust roots per role. A leaf crate on purpose — the updater must verify before
the rest of the application is known to be healthy, so it cannot link Wasmtime.
`rd-plugin-host` re-exports what it used to own; the digest framing must never change, because
every `.rdplug` already in the field is signed with it.

### rd-siterules

the rules that recognise release pages (RD-110-04): the rule format, the
signed pack under `Role::SiteRules` with its domain string `rdownloader.site-rules.v1`, the
format-version and freshness checks, and the catalogue the selection consults.
It also holds the **executor** (RD-110-05, `src/exec/`): the seven step kinds, the three bolts
and the stable refusal codes. A leaf like `rd-sign` — `serde`, `regex`, `url`, `base64`, `hex`,
`percent-encoding` and a clock, no `rd-db`, no `reqwest`, no `rd-captcha`, no Wasmtime — and the
executor keeps that by taking four small traits from its caller (`exec::ports`): a `Fetcher`
(one request, **redirects reported and not followed**, so every hop is checked here, and it
**connects to the addresses the executor already checked** instead of resolving the name a
second time — otherwise a record with a time-to-live of zero decides which address is
actually dialled), a `HostResolver` (the ban on private ranges is checked against the
*resolved* address, never the name, and an IPv4 address hidden in an IPv6 one — mapped,
compatible, 6to4, NAT64, ISATAP — is decoded and judged as the IPv4 address it is), an
optional `CaptchaSolver` and a `Clock`. The `reqwest` and `rd-captcha` adapters belong
to the calling crate; the job file's planning line naming those two as
dependencies of this crate was overturned there, with the reason. Since RD-110-06 they exist:
`crates/rd-plugin-host/src/siterules.rs` holds all four (`RuleNetwork`/`RuleFetcher`,
`RuleResolver`, `RuleCaptcha`), and `crates/rd-plugin-ext/src/siterules.rs` puts them together
with the catalogue as a source in the crawler selection — **the order is plugins that name a
service, then rules (a person's own before the shipped ones), then the generic crawlers**, and
it is `FolderCrawlers::expand` that holds it. Since RD-130-07 the shipped side of
`Catalogue` is empty in production: no pack is compiled in any more, and the order is kept for
a pack delivered separately later. The fetcher is built per run rather than taken
from `rd_http::ClientPool`, because a client pinned to one host's checked addresses is of no
use to any other request; both obligations of the port — no redirect policy, and connect to
`FetchRequest::addresses` instead of resolving again — are tested in
`crates/rd-plugin-host/src/siterules_tests.rs`. **There is no JavaScript
interpreter and there will not be one** — `exec::decode` covers base64, hex, rot13,
percent-encoding and concatenated JS string literals, and what is genuinely a program ends as
`site_rules.decode_failed`. **Since RD-130-07 no rule is compiled in.** The project's rules
are `crates/rd-siterules/resources/site-rules.json`, signed locally with `rdownloader site-rules
sign` and committed, and every release carries the file as the artifact
`rdownloader-site-rules.json` (`release.yml` verifies it with `rdownloader site-rules verify`
first); `tests/release_pack.rs` holds it to the compiled-in root, so editing it without
re-signing fails the tests. An installation takes the rules through the settings page's
import, which verifies the file from its raw bytes and stores each rule as the person's own,
switched off. Rules live in `rd-db` as opaque JSON (`site_rules`, migration `0076`) on
purpose, so a change to the executor does not rebuild the database crate. Since RD-110-08 the
*assembly* has one home, `rd-api/src/site_rules_service.rs`: it folds the stored rules and the
group switches of `site_rule_switches` (migration `0082`) into the catalogue in force. `serve`, `rdownloader
doctor site-rules` and every write from the settings page call it, and
`site_rules_cli::load_catalogue` is only the name the binary knows it by — do not grow a
second copy of that rule. Where a switch lives is deliberate and not to be unified: a rule's
is `site_rules.enabled`, a group's is the switch table, because a group is not a rule. The
table's `rule` scope held the compiled-in rules' switches until migration `0095` removed them
with the pack (RD-130-07), which also merged the groups `comics` and `magazines` into `ebooks`
(`graphics` stays its own).

### rd-capture / rd-autostart

the separate desktop agent binary (Click'n'Load, clipboard,
`.nzb` association) and per-user autostart. Its desktop half — the tray icon (`tao`,
`tray-icon`, `image`, `open`) — is gated to `cfg(any(windows, target_os = "macos"))` in
`Cargo.toml`, because **the Linux build links no window toolkit, deliberately**. Where that
line runs exactly, since it is regularly remembered wrong: `arboard` is ungated and pulls
`x11rb` and `wl-clipboard-rs` behind it, so the headless Linux agent does link X11 and Wayland
*client* libraries — and that is right. A clipboard client is not a window toolkit: no GTK, no
WebKitGTK, and with no display server reachable it returns an error instead of crashing.
Clipboard watching is wanted on Linux. What must not cross the line is a windowing or
rendering stack. Keep that split when adding to it: the rules a surface obeys belong in an
ungated module so they compile and are tested here, and only the surface itself goes behind
the gate. `crates/rd-capture/src/platform_gate/` holds the line (RD-109-14): it parses
`Cargo.toml` and evaluates every `[target.…]` cfg against a Linux configuration instead of
matching strings, and it asks for a written reason for **every** dependency a Linux build
reads, so a new crate is refused until somebody gives one. A cfg it cannot decide counts as
reaching Linux. The other half of the guard is `scripts/check-capture-linux-tree.sh`
(RD-109-37): it holds the *resolved* Linux tree — what a dependency, a feature or a `[patch]`
pulls in behind an innocent name — against a named list of window, widget and rendering
stacks, and CI runs it on the Linux leg of every push. **What neither half sees**, plainly:
window code written straight into an ungated source file rather than declared as a `mod` —
the Linux compiler catches that, and CI compiles this crate on Linux, but the gate does not;
a native library linked by `#[link]` or a build script without a crate to name it, which
shows up at link time rather than in a tree; and a forbidden crate that arrives under a name
neither list knows, since both lists are hand-maintained. There was a second
gated surface, the captcha WebView (`wry`, RD-107-03); RD-109-11 removed it and `wry` with it,
because a widget captcha is answered in the person's own browser through the extension and a
second, measurably broken path made the working one unreachable.
`main.rs` is the entry point, `dispatch`, the two exit-code error markers and the agent loop
and nothing else (RD-109-12); each subject sits in its own module next to its own tests:
`cli.rs` the clap types and the defaults a bare invocation runs with, `commands.rs` the
one-shot subcommands, `clipboard.rs` the clipboard loop and its retry rule, `supervision.rs`
the background-task notices and the Click'n'Load binding, `status.rs` the tray's server-state
rules beside `activity.rs`, `icon.rs` the geometry of its activity badge, `tray_state.rs` the
tray's state machine, `tray.rs` the desktop half.
`status.rs`, `icon.rs` and `tray_state.rs` carry `cfg(any(windows, target_os = "macos", test))`
on their module declarations, which is what keeps those rules tested on Linux without a tray
(RD-109-13, RD-110-24). The cut runs where the gated crates begin: `icon.rs` paints a raw RGBA
buffer while `tray.rs` does the PNG decoding and the `Icon::from_rgba` around it; `status.rs`
classifies a health request from its status code and owns the `ever_answered` latch while
`tray.rs` only makes the request and sends the result into the event loop; and `tray_state.rs`
decides which mark is up, whether "Open" is enabled and what the status item and tooltip say
— a `Surface` for the build, an `Update` per event naming only the parts to redraw — while
`tray.rs` holds the `MenuItem` and `TrayIcon` handles and does nothing but write those
updates onto them. Put a new tray decision on the ungated side of that line, in `tray_state`
if it is about what the tray shows. And remember that **no Linux check compiles `tray.rs` at
all** — after editing it, verify with
`cargo xwin check --target x86_64-pc-windows-msvc -p rd-capture --all-targets`, which is the
only thing on this machine that reads the file.

Web UI (`web/src/`): Vue 3 + Nuxt UI v4 + Tailwind 4, Pinia stores in `stores/`, views/components, vue-i18n. The API client is generated: after changing REST endpoints or DTOs, run `scripts/api-contract.sh`, which regenerates both `web/openapi.json` and `web/src/api/schema.d.ts`. `npm run generate:api --prefix web` alone only regenerates the TypeScript types from the existing OpenAPI document.

## Web UI



## The rules in long form

Verbatim from `AGENTS.md` as it stood at RD-120-25, so that shortening the working card could
not quietly drop a rule, a command or a number. `AGENTS.md` carries the same rules in short
form; this is the text they were shortened from. The script table is the one thing not repeated
here in full, including the script table that `AGENTS.md` now carries only in part.

### Build

**Use `scripts/` rather than retyping these commands** — see `scripts/README.md`. The scripts
carry the flags this machine needs, above all a capped job count: unbounded `cargo` parallelism
has exhausted memory and taken WSL down more than once, and
`cargo clippy --workspace --all-targets --all-features` does so even at `-j 8`, so
`scripts/check.sh` lints only the crates you name unless you ask for the full sweep.

**Never run multiple build, test, Clippy, packaging or plugin-build commands concurrently on this
machine.** Check for an already running heavy job before starting another one, keep the scripts'
default `JOBS=4` cap (full workspace Clippy uses 2), and lower `JOBS` when other work is active.
Too many simultaneous Rust or Node processes push WSL into swap and can make it stop responding.

**Never stop a process with `pkill -f`.** The pattern is matched against every command line,
and the shell running the `pkill` contains the pattern too — so the agent kills its own tool
invocation and loses the rest of the command. This has happened repeatedly. Use the exact
process name instead, which matches the executable rather than the text:

```bash
pgrep -x rdownloader | while read -r pid; do kill "$pid"; done
```

The same trap applies to `pgrep -f` in a condition: it reports the checking shell as a match and
the check always succeeds.

**Never build every `rd-api` test binary at once.** `cargo nextest run -p rd-api`,
`cargo check -p rd-api --tests` and `cargo clippy -p rd-api --all-targets` each build all 55
integration test binaries, and every one of them links the whole dependency graph. That is the
memory hog, not the job count: it has OOM-killed WSL even at `JOBS=2`, because lowering the job
count does not make a single link cheaper. Run them in small explicit batches instead, and start
with the library:

```bash
cargo nextest run -j 2 -p rd-api --lib                       # the unit tests, cheap
cargo nextest run -j 2 -p rd-api --test mirrors --test dlc   # three or four binaries at a time
```

`cargo check -p rd-api` on its own (library only) is cheap and catches most breakage, but it does
**not** compile the `#[cfg(test)]` fixtures — a new field on a shared struct will pass it and fail
the `--lib` run, so run that too before assuming a change is clean.

| Task | Script |
| --- | --- |
| Run locally | `scripts/dev.sh` |
| Core local checks | `scripts/check.sh --clippy <crates>` |
| Linux release package | `scripts/package-linux.sh` |
| Windows release package | `scripts/package-windows.sh` |
| Build and sign plugins | `scripts/build-plugins.sh` |
| Check a component's imports | `scripts/check-plugin-imports.sh <component.wasm>` |
| Price a MEGA sign-in in guest fuel | `scripts/measure-mega-login-fuel.sh` (a measurement, not a gate) |
| Check the headless Linux tree of the capture agent | `scripts/check-capture-linux-tree.sh` |
| Build and run the container image | `scripts/docker.sh build\|run\|stop` |
| Read or set the version | `scripts/set-version.sh [X.Y.Z]` |
| Whole release chain | `scripts/release.sh [X.Y.Z]` |
| Same chain, unattended, to the tag | `scripts/release-pipeline.sh <X.Y.Z>` |
| Smoke-test a built binary | `scripts/release-smoke.sh <X.Y.Z>` |
| Tag a release commit | `scripts/tag-release.sh` (never pushes; push the tag yourself — every release is tagged) |
| Regenerate the API contract | `scripts/api-contract.sh` after any REST/DTO change |
| Test and build the browser extensions | `scripts/build-extension.sh` |
| Feature worktree | `scripts/worktree.sh new\|check\|finish <branch>` |
| Add a translation key | `scripts/i18n-key.sh <catalogue> <key> <de> <en> <es> <fr>` |
| Regenerate application and extension icons | `scripts/generate-icons.sh` |

`release-pipeline.sh` is the one to reach for when a release runs without somebody reading the
output. It appends every step's raw output to `artifacts/release-evidence-<version>.log` and
refuses to tag unless each earlier step left a record from that same run which exited zero *and*
produced output — a step that passed silently, or a green carried over from an earlier attempt,
counts as missing evidence. It stops at the tag; publishing stays a decision.

The underlying commands are documented below, for the cases the scripts do not cover.

The project requires Rust 1.98 with Edition 2024; CI pins Rust 1.98.0. The web build uses Node 24 and npm 11. `rust-toolchain.toml` installs the Rust components and compilation targets. Some checks and release tasks additionally need `cargo-nextest`, `sqlx-cli` 0.8.6, `cargo-component` 0.21.1, `wasm-tools`, `cargo-deny`, or `cargo-xwin` 0.23.1; use the versions and installation commands in `docs/development.md` and `.github/workflows/` rather than guessing.

The web UI is embedded into the binary at compile time via `rust-embed` from `web/dist` — **the frontend must be built before the first `cargo build`**, otherwise `rd-api` fails to compile:

```bash
npm ci --prefix web
npm run build --prefix web
cargo build -p rdownloader            # dev build of the service
cargo build --locked --release -p rdownloader -p rd-capture   # release
```

Run locally: `cargo run -p rdownloader -- serve --database data/rdownloader.sqlite3 --downloads downloads --listen 127.0.0.1:8710`. `--plugin-development-mode` allows unsigned plugins (local dev only).

#### Windows Cross-Builds

Build Windows artifacts with native `cargo xwin` in WSL, into `artifacts/windows/`:

```bash
cargo xwin build --locked --release --target x86_64-pc-windows-msvc -p rdownloader -p rd-capture
```

**Not the Docker path.** `docker/Dockerfile.xwin` exists and produces a reproducible build, but
under WSL it is slower and breaks on missing `COPY` paths and credential-helper issues. Reach for
it only when reproducibility is the point, and expect to fight it.

Plugins are built with `cargo-component`, deliberately **without WASI**:

```bash
cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-<name>
```

A plugin must import nothing outside `rdownloader:plugin`. `scripts/check-plugin-imports.sh`
enforces it by reading the import section with `wasm-tools`; `build-plugins.sh` calls it after
every build, and CI runs the same script. It falls back to a byte scan, loudly, when
`wasm-tools` is missing — that scan cannot tell an import from a string, so a plugin whose data
section merely contained `wasi:` used to be rejected as importing it.

**Build the components you need, not all of them.** `scripts/build-plugins.sh` builds, signs and
packages every bundled plugin; that is what a *release package* needs, and it is the wrong reach
for one or two. `scripts/build-plugins.sh --components-only [names]` builds and stamps without
signing — without names, exactly what is stale or missing — and leaves signing and packaging to
the release. The contract tests load components out of `target/wasm32-unknown-unknown/release/`,
and since RD-108-16 a **missing** one fails the suite loudly rather than skipping it.
`--list-stale` and `--list-missing` answer which ones in a second, without building anything.

**Staleness by content (RD-120-58).** Until then a component counted as stale when any of its
sources had a newer modification time. Every `git checkout` resets those, so a fresh worktree
named components stale that nobody had changed: on 2026-09-24 the first check stopped at "plugin
components against their sources" in five of eight branches, and the agents rebuilt between 7
and 72 components for no plugin change before starting again. Now every build through
`build-plugins.sh` writes a stamp beside the component, `rd_plugin_<name>.wasm.src-sha256`, with
two hashes: the **source hash** — SHA-256 over a `sha256sum` listing of the plugin crate, the
shared plugin libraries it depends on transitively and the WIT, sorted relative paths and
contents, no file time and no checkout location — and the SHA-256 of the component itself. A
component is current when the stamp exists, describes exactly its bytes and records exactly the
current source hash. `scripts/build-plugins.sh` (`stale()`) and `rd_plugin_host::artifact`
implement the same definition, and a unit test compares `--source-hash` with the Rust value.
The component hash is what keeps a stamp honest: a bare `cargo component build` writes none, so
the component it leaves — possibly built in another worktree sharing `target/` — no longer
matches and counts as stale. A stamp never vouches for a build it did not see. For the same
reason the script touches a plugin's own sources and its shared libraries before building it:
cargo fingerprints workspace crates by file time, and without the touch a build could find a
newer fingerprint from another checkout, call the crate fresh and leave that checkout's
component in place under this checkout's stamp. The WIT is not touched; `rd-plugin-api` generates
its host bindings from it, and touching it would rebuild half the workspace. Measured in a
throwaway worktree against stamps written from another checkout: after touching every file under
`plugins/` the old rule named 72 components, the new one none; a changed plugin source named
that plugin, a changed `xfs-common` its five users, a changed `guest` its 27, and the WIT all 72.

**Two levels (RD-120-58).** Of the same eight branch checks, seven ran all fourteen `rd-api`
integration batches, because nearly every change touches `crates/rd-api/` or `rd-core` — and every
full run after the merges was green, so the wide branch runs found nothing the full run would
not have found. The owner's decision the same day: optimise, never drop a test. So
`scripts/check.sh` without `--full` runs at **branch level** — clippy (all targets) and every test
of the touched crates, the library and binary tests of one level of reverse dependencies,
`rd-api --lib`, and only the `rd-api` integration binaries `scripts/lib/rd-api-tests.map`
selects — and `--full` runs everything, **once per wave on `development` after the merges and in
the release chain**. The map is a table of extended regexes over changed paths: a changed test
file selects itself, `tests/common/` selects all, each source area selects the binaries whose
routes it serves (derived from the `/api/v1/<area>` requests each binary makes), and handler or
route changes additionally select `scope_matrix` and `mcp`, which walk every route and drive every
handler through the tool catalogue. A path under `crates/rd-api/`, `crates/rd-core/` or a
migration that no row maps selects every binary: where the mapping is not clear the answer is the
wide one, never a guess. The run refuses a row naming a missing binary and a binary no row names.
Other crates select no binary, as before; their own tests and their reverse dependencies cover
them at branch level. The crash matrix, sqlx, web and extension keep their triggers, and a
manifest change (`Cargo.toml`, `Cargo.lock`, toolchain, nextest, deny) still runs the whole
workspace. Every run prints the time per stage and what it left out, and a branch-level run ends
by saying `--full` is still due.

The gates follow the levels. `worktree.sh finish` takes a branch green, as before. A tag and a
Windows package for the owner need a `--full` green: `check.sh --full` records one per half
(`rust`, `web`) under `<target>/.rd-verified-full/`, keyed by the **tree** of the working state
rather than by commit, because the release pipeline tests the tree after its version bump and
tags a merge commit on `main` afterwards — three commits, one content. `tag-release.sh` and
`package-windows.sh` refuse a tree without both halves; a difference in documentation only
(`docs/`, `*.md`, except `docs/recovery-matrix.md`) is accepted, so the verification note and the
release commit's changelog do not demand another full run. `package-windows.sh` has
`RD_UNVERIFIED_PACKAGE=1` for testing the cross-build itself, loudly, with `UNVERIFIED.txt` inside
the package; `tag-release.sh` has no override. `release.sh` and `release-pipeline.sh` run
`--full`.

**Rules for agents (RD-120-58).** A report is at most 30 lines: the caller relays it, and the
long form belongs in the job file. Mutation probes — breaking the code on purpose to prove a test
catches it — only for security fixes, where a test that cannot fail is a hole; elsewhere they
cost a build per probe. Small fixes go bundled to one agent: each agent pays for its own
worktree, its own component check and its own verification run, and a one-line fix does not
earn that.


### Test & lint

**A documentation-only change gets no build, no test and no lint run.** Roadmap entries, job
files under `docs/roadmap/jobs/`, `CHANGELOG.md`, `README.md`, `AGENTS.md`, `design.md` and the
rest of `docs/` change no behaviour, so compiling or testing them proves nothing and costs this
machine several minutes it needs for real work. `git diff --check` for stray whitespace is the
whole verification. This holds even when the text quotes code or file paths: check those by
reading the files, not by running a build. Only start a build when the same change also touches
code, a script, a workflow or a manifest.

Start with the smallest check that directly covers the change; do not run the whole workspace,
the complete web suite or every CI-equivalent check for an isolated edit. Expand the scope only
when a result indicates broader impact, the change crosses subsystem boundaries, it affects
release/build infrastructure, or the user explicitly requests broader verification. Examples:

```bash
cargo nextest run -p <crate> <test_name_filter>  # e.g. -p rd-usenet worker
npm run test --prefix web -- <file>              # one relevant Vitest file
scripts/build-extension.sh --test-only           # extension-only change
git diff --check                                 # documentation-only change
```

**Crash and restart cases need their feature — and it is the *owning* crate's feature.** The
RD-140-04 matrix is compiled out unless the feature is on, so a normal run silently skips every
case. Enabling `rd-core/failpoints` alone is not enough and is the trap this command used to
fall into: each crash-test file is gated on its own crate's `failpoints`, which
`rd-core/failpoints` does not turn on. Name every owning crate:

```bash
cargo nextest run --features rd-http/failpoints,rd-scheduler/failpoints,rd-usenet/failpoints \
    -j 2 -p rd-core -p rd-http -p rd-scheduler -p rd-usenet
```

The count is the check: without the owning crates' features `rd-http` runs 65 tests,
`rd-scheduler` 71 and `rd-usenet` 40; with them, 70, 76 and 42. A crash-test binary that
compiles to nothing reports success. These numbers and the ones in `docs/recovery-matrix.md`
had drifted apart and from the suite; both documents now carry the same measured figures, last
taken on 2026-09-22 (RD-110-20, which added `scheduler.before_mirror_promoted` and the mirror
handover's own tests; RD-110-33 before it added `http.after_chunk_mac` and the fifth rd-http
crash test). Measured per crate, one `cargo nextest run -p <crate>` with and without its
feature — **not** by counting lines out of a combined run, which is how the `rd-scheduler`
figure came to be five short the last time. Re-measure rather than adjusting by hand; that is
how these drifted in the first place.

Touching a persistence path means registering a crash point in
`rd_core::failpoint::CRASH_POINTS`, adding its row to `docs/recovery-matrix.md` (a test
compares the two) and covering it with a case. Axis B of that matrix — cases that really
`SIGKILL` a spawned `rdownloader serve` — is `#[ignore]` and **CI-only**; do not run it here.

Run `scripts/check.sh` when a change needs the complete capped local check across Rust and web.
At branch level it lints the touched crates itself (all targets; `rd-api` only as far as its
integration binaries are selected); `--clippy <crate> [<crate> ...]` names the crates instead, and
`--full` leaves Clippy to `--clippy`/`--clippy-all` as before, since the release pipeline runs its
own workspace Clippy. It also falls back from Nextest to `cargo test` and skips the SQLx offline
check when the corresponding tools are unavailable. Use `scripts/check.sh --clippy-all` only when a full workspace lint was explicitly
requested and nothing else memory-heavy is running; it caps that job at 2.

Vitest matches by file path or test name. Clippy config: `unsafe_code` and `dbg!`/`todo!` are denied,
`unwrap_used` warns — avoid `.unwrap()` in non-test code.

**Nothing local lints the Windows half of the workspace.** Code behind `cfg(windows)` — the
`rd-capture` tray, the platform branches in `rd-files`, `rd-power` and `rd-autostart` — is the
code no Linux build compiles, so every check above reads straight past it. That is how `rd-files`
carried two dead functions through three jobs, each of which recorded them as pre-existing:
`cargo xwin clippy` failed before it reached anything else, and a lint that cannot start reports
nothing. `cargo xwin` runs it from here without a Windows machine. The crates the Windows package
delivers are everything under `crates/`, and each has to be named, because `--` arguments reach
the selected packages only and never their dependencies:

```bash
cargo xwin clippy --target x86_64-pc-windows-msvc -j 2 $(ls crates | sed 's/^/-p /') --all-targets --all-features -- -D warnings
```

Run it when a change touches platform-gated code or a crate the Windows package ships. Until
RD-120-67 this line left `--all-targets` off, on the reasoning that it builds every `rd-api` test
binary; but clippy links nothing, and the whole workspace with every target took 4m40s at `-j 2`
under WSL on 2026-09-25. Without it, test code that compiles differently on Windows went
unchecked: unused imports in `rd-extract`'s script tests and Linux-only log helpers in
`rd-postprocess` failed the first real CI run on GitHub, and this line with `--all-targets`
reproduces both. It is the local equivalent of CI's `windows-2025` leg, not an extra gate — that
job already lints the same ground natively (RD-109-40).

For an isolated browser-extension logic change, use `scripts/build-extension.sh --test-only`.
Run the full wrapper when build/manifest/package behaviour changed or distributable artifacts are
needed; its Chrome and Firefox bundles and ZIP files live under `artifacts/browser-extensions/`.

The local script covers the core format, Rust test, SQLx, web and browser-extension checks, but not every CI job. `.github/workflows/ci.yml` additionally validates all plugin components and their WASI boundary, plugin packages and conformance, SDK contract copies and an out-of-workspace scaffold, platform launchers, Docker images and `cargo deny`. Consult that workflow when changing those areas.


### Verification Rules

- **Never pipe test output through `grep` or `head`.** The pipeline's exit code is the last
  command's, so a failing suite reads as success and gets reported as "all green". Run the suite
  plainly and read its summary.
- **A backgrounded check's reported exit code is not evidence.** A long run that the harness moves
  to the background has twice reported "exit code 0" for a run that actually failed — once with
  `scripts/check.sh` aborting on four red Vitest cases, once with a genuine exit 101. Same damage
  as the pipe above, and harder to see because nothing looks like a pipeline. Capture the code
  inside the command — `cmd > log 2>&1; echo "REAL EXIT: $?"` — and read the script's own closing
  line (`==> all requested checks passed`). Absence of that line is a failure even when the
  notification says otherwise.
- **A shared `CARGO_TARGET_DIR` lets one checkout link another's rlib.** Feature worktrees share
  this checkout's `target/`, and cargo fingerprints a workspace crate per source path. The symptom
  is a compile error against code that is demonstrably present — "no `MirrorHint` in the root"
  while `rd-core/src/collector.rs` defines and exports it — so the hunt starts in the wrong place.
  It cost three false reds on 2026-09-21 alone. `scripts/check.sh` now stamps the sources itself;
  a direct `cargo` invocation does not, so run `find crates -name '*.rs' -exec touch {} +` inside
  the same `flock`. Deliberately `crates/` only: `build-plugins.sh` touches a plugin's sources
  itself, right before it builds that component (the staleness check reads contents since
  RD-120-58, so the touch costs it nothing).
- **After merging any branch, check for duplicate migration numbers before running tests**:
  `ls crates/rd-db/migrations | sort`. Two branches that both added a migration will both have
  taken the next free number, and the result still compiles.
- **After merging a branch that touched a plugin, rebuild its component before believing the
  contract tests**: `scripts/build-plugins.sh --list-stale` names the ones that are behind.
  `target/wasm32-unknown-unknown/release/` is per checkout and `cargo test` never fills it, so
  the main checkout keeps the component from before the merge and the contract tests run the old
  guest code against the new expectations. That has twice looked like a defect in the code — a
  missing WIT export, a client identifier that had already been removed — and twice cost a full
  `scripts/check.sh`. The tests now name the state themselves and `scripts/check.sh` stops on it
  right after `cargo fmt`; the fix is always `scripts/build-plugins.sh --components-only
  <name>`, which stamps what it builds (a bare `cargo component build` does not, and its
  component counts as stale).
- **A fresh worktree has no components at all, and that now fails rather than passing.** Until
  RD-108-16 a missing component made every contract test return early and count as passed: a
  run reported `370 tests run: 370 passed` in three seconds having loaded no component
  whatsoever — the same number the real run produces in two minutes. A missing artefact is now
  a failure naming the build command, `scripts/build-plugins.sh --list-missing` names them all,
  and `scripts/check.sh` stops on it before it spends anything. Build what the tests need
  before you believe a contract-test number in a new checkout. A checkout that *cannot* build
  components — no wasm target, no `cargo-component` — leaves those tests out on purpose with
  `cargo nextest run -P no-components` (`.config/nextest.toml`), which counts them as skipped
  rather than passed; CI's `rust` job runs exactly that, and the `components` job runs them for
  real.
- **Do not claim a task is complete without the actual passing test count or a live smoke check.**
  "Tests pass" is not evidence; "330 tests run: 330 passed" is.

### Documentation at task completion

Before declaring any task complete, review every documentation source below and update all affected
ones in the same change. Documentation, implementation and planning status must never knowingly
drift apart:

- `CHANGELOG.md` for user- or developer-visible behaviour, fixes and workflow changes.
- `README.md` for the public overview: what rDownloader is, sources, quick start, platforms.
- `docs/development.md` for setup, build, operation and supported integrations in detail.
- `docs/roadmap.md` for strategic status, priorities, milestone scope and sequencing.
- The matching files in `docs/roadmap/jobs/`, including their status, acceptance checkboxes,
  verification notes and the jobs index when its inventory or summary changes.
- Topical documentation under `docs/`, plus `docker/README.md`, `extension/README.md`,
  `sdk/README.md` and `scripts/README.md` when their subject is affected.
- `docs/README.md`, the index of every document, whenever one is added, renamed or removed.
- `AGENTS.md` whenever repository structure, workflows, constraints or conventions change.
- `design.md` (repository root) whenever a user-facing pattern is introduced or changed — it is
  where the visual language, the interaction rules and the row/list conventions live.

Do not add meaningless placeholder edits to unaffected documents, but explicitly verify that each
source still tells the truth. Never mark a roadmap item or job complete unless its implementation
and required verification are complete; record partial, blocked or deferred work honestly.

### Scope: current versions, no installed base

Two decisions of the project owner, taken on 2026-09-20. They remove work rather than adding it,
so they are worth knowing before a job is cut.

**Only the current browser and runtime versions are supported.** No job carries a finding about
older Firefox or Chrome builds, and none should acquire one. RD-109-21 asked whether Firefox
rejects a port in a match-pattern host; the answer was measured on the shipped 1.0.9 extension —
it does not — and the remainder of that job, *older* Firefox and Chrome, is out of scope by
decision rather than unproven. `strict_min_version` stays in the manifest as a floor, not as a
promise of testing below it.

**There is no public release, so there is no installed base to protect.** Nothing has to be
migrated, cleaned up or kept compatible for the sake of an older installation: no data migration
for rows written by an earlier build, no removal of directories an earlier build left behind, no
compatibility shim. Where a finding is only a burden on an existing install — a stale profile
folder, a value already in a database — say so and let it stand; do not spend a release on it.
This will change the day the project publishes, and this section is where that change gets
written down.

### Conventions

- Treat `web/openapi.json`, `web/src/api/schema.d.ts`, `web/components.d.ts` and
  `web/auto-imports.d.ts` as generated files. Use the documented generators; do not hand-edit them.
  Do not run a web build in a feature worktree that uses symlinked `node_modules`; if it happens,
  use `scripts/worktree.sh check <branch>` to catch declarations rewritten with paths from the
  wrong checkout.
- A change to `crates/rd-plugin-api/wit/rdownloader.wit` must update every
  `sdk/templates/*/wit/rdownloader.wit` copy in the same change. CI requires byte-for-byte parity.
- **Three signing keys, one per role, all in `~/.config/rdownloader/`** — and picking the wrong one
  is the easy mistake, because the names are close and the failure is a signature check, not a
  compile error. `rdownloader-plugin.key` signs `.rdplug` packages, `rdownloader-siterules.key`
  signs the shipped site-rule pack (`rdownloader site-rules sign`), `rdownloader-tools.key` signs
  the external-tool manifest.
- `web/public/favicon.svg` is the icon source of truth. Run `scripts/generate-icons.sh` after it
  changes instead of editing extension, PWA or Windows icon outputs separately.
- **Locales**: every UI string exists in all four languages — `web/src/locales/{de,en,es,fr}/`. Adding or changing a key means updating all four.
- REST errors carry stable `code` values that the frontend translates; don't return free-text-only errors from new endpoints.
- External helper binaries (`yt-dlp`, `ffmpeg`, `unrar`, `7z`, …) are resolved via the vendor-folder lookup, never hard-coded paths.
- Release builds use `--locked`; keep `Cargo.lock` committed and consistent.

#### User interface patterns

`design.md` at the repository root is the source for how the interface behaves: the visual
language, the interaction rules, and the row and list conventions. Check a new pattern against it
before inventing one, and add the pattern there if it is genuinely new — the subscriptions list
drifted into five text buttons and an unconfirmed delete precisely because nothing said where
such a decision belonged.

#### Language and lint guards

Rust sources are English-only, and `crates/rdownloader/tests/no_german.rs` enforces it: any
umlaut or sharp s in a `.rs` file under `crates/` or `plugins/` fails, outside a short
allow-list. Comments, identifiers, test strings and commit messages are English throughout.

User-facing text never lives in Rust as prose — it lives as a stable code, translated in
`web/src/locales/` and the plugin catalogues.

This applies to code, not to planning documents: `docs/roadmap/jobs/*.md` are written in German
by convention and stay that way.

#### Removing a dependency

Before removing a package, grep for **every** surface it exports — composables, components,
auto-imports, config keys, build plugins — not just the one symbol that led you there. An
auto-import or a Vite plugin leaves no visible import to find. Verify with a full build before
committing, not with a typecheck.

Runtime dependencies are never installed as `devDependencies`.

### References

These are read for behavioural reference — how an established tool solved a problem rDownloader
also has to solve — not for code. Each project carries its own licence; do not copy source from
them.

- <https://github.com/mycodedoesnotcompile2/jdownloader_mirror/> — source mirror of JDownloader,
  the prior art for LinkGrabber behaviour: link decryption, package and container handling, hoster
  accounts, and the naming conventions users already expect from `rd-collector`.
- <https://github.com/pyload/pyload/tree/develop/src/pyload> — pyLoad's source, a second reading
  of the same ground: plugin-per-hoster structure, account and captcha handling, link extraction.
- <https://github.com/sabnzbd/sabnzbd> — SABnzbd's source, the reference for the Usenet path that
  `rd-usenet`, `rd-extract` and `rd-postprocess` mirror: NZB handling, PAR2 repair, unpack, the
  `{{password}}` file-name convention, post-processing categories and user scripts.
- <https://newznab.readthedocs.io/en/latest/misc/api.html> — the normative Newznab API
  specification: the `t=` functions, the `extended=1` attribute block and the `t=caps` document
  that `crates/rd-subscription/` implements against indexers.
