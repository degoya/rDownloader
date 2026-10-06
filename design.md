# rDownloader — Design and Architecture

> Status: September 2, 2026 · Project version `0.4.0`, including the current
> `Unreleased` changes. This document describes the system implemented in the repository.
> Planned features are not described here; [`ROADMAP.md`](ROADMAP.md) summarises them.

## Purpose and Design Goals

rDownloader is a local-first, cross-platform download manager. The system brings direct HTTP(S)
downloads, one-click hosters, Usenet/NZB, BitTorrent, media sites, image galleries, and livestream
recordings into one persistent package queue. A Rust service provides the scheduler, REST and MCP
interfaces, and the embedded Vue frontend. A desktop capture agent, browser extension, and
hotfolders provide additional intake channels.

| Attribute | Value |
| --- | --- |
| Product | rDownloader |
| Author | Alexander Herling |
| Repository | `https://github.com/degoya/rDownloader` |
| License | GNU GPL v3.0 or later |
| Backend | Rust 2024, minimum version 1.99 |
| Frontend toolchain | Node 24 and npm 11 |
| Target platforms | Windows, Linux, macOS, and Docker |

The architecture follows six guiding principles:

1. **Local first:** The service binds to `127.0.0.1:8710` by default. Data, credentials, and
   download state remain under the operator's control.
2. **One queue:** Every transfer type uses the same packages, priorities, states, controls, and
   status presentation.
3. **Recovery rather than restart:** Transfer, segment, verification, and post-processing progress
   is persisted so that a process failure repeats as little work as possible.
4. **Secure boundaries:** Storage destinations are allowlist-based, secrets live outside the
   domain database, and plugins run signed, without WASI, in a constrained sandbox.
5. **Replaceable integrations:** Protocol-specific runners and versioned resolver plugins sit
   behind stable domain contracts.
6. **Live without excessive traffic:** Persistent mutations emit domain events; high-frequency
   progress events are throttled before being distributed to the UI through Server-Sent Events.

## System Context

```mermaid
flowchart LR
    U[User] --> WEB[Vue web app]
    C[rdownloader-capture] --> CAP[Capture API]
    E[Browser extension] --> CAP
    H[Hotfolder] --> COL[LinkGrabber]
    EXT[REST or MCP client] --> API[Axum API]
    WEB --> API
    CAP --> COL
    API --> COL
    COL --> DB[(SQLite)]
    COL --> Q[Scheduler]
    Q --> HTTP[HTTP and hosters]
    Q --> NNTP[Usenet]
    Q --> TOR[BitTorrent]
    Q --> TOOLS[yt-dlp / gallery-dl / streamlink]
    HTTP --> PP[Post-processing]
    NNTP --> PP
    TOR --> PP
    TOOLS --> PP
    PP --> FS[Local destination / rclone]
    DB -. SSE .-> WEB
    PLUG[Signed .rdplug components] --> HTTP
```

### Runtime Topology

- `rdownloader serve` initializes paths, SQLite, the secret store, settings, plugins, transfer
  runners, the scheduler, post-processing, torrent recovery, hotfolders, and the API.
- The compiled SPA from `web/dist` is embedded into the same binary by `rd-api` and served through
  the API port.
- `rdownloader-capture` is a separate desktop process. It can also connect to an rDownloader
  service running on a NAS or in Docker.
- The browser client is same-origin. Capture endpoints have deliberately scoped CORS support; the
  MCP endpoint deliberately does not.
- During shutdown, the stream monitor, torrent service, hotfolders, link checker, post-processing,
  and scheduler stop in a controlled order. SQLite performs a WAL checkpoint.

## Components

| Component | Responsibility |
| --- | --- |
| `rdownloader` | CLI, service startup, configuration wiring, diagnostics, plugin commands, and autostart commands |
| `rd-api` | Axum router, REST, authentication, SSE, OpenAPI, MCP, and the embedded SPA |
| `rd-core` | Domain models, typed UUIDv7 IDs, states, failures, events, and shared settings |
| `rd-db` | SQLite migrations, reader pool, serialized writer, and atomic domain events |
| `rd-scheduler` | Prioritized queue, concurrency limits, retries, provider slots, and runner orchestration |
| `rd-http` | Probing, range/chunk engine, resume, cookies, proxy/TLS, and bandwidth limiting |
| `rd-captcha` | Solver service, manual image captchas, transient captcha queue, and live events |
| `rd-usenet` | NNTP pool, server fallback, yEnc/CRC, segment resume, and assembly |
| `rd-torrent` | Embedded `librqbit` session, torrent runner, and seeding supervision |
| `rd-media` | Metadata and media downloads through `yt-dlp`, `ffmpeg`, and `ffprobe` |
| `rd-gallery` | Gallery downloads through `gallery-dl` |
| `rd-stream` | Livestream probing and recording through `streamlink` |
| `rd-collector` | Link extraction, normalization, NZB parsing, and automatic package creation |
| `rd-hotfolder` | File-system watcher and safe intake of NZB and torrent files |
| `rd-extract` | Persistent post-processing pipeline, recovery, scripts, and uploads |
| `rd-postprocess` | Archive formats, multipart detection, passwords, PAR2, and extraction backends |
| `rd-files` | Safe paths, file names, part files, checksums, and storage roots |
| `rd-plugin-api` | WIT contract `rdownloader:resolver@0.5.0` and shared plugin types |
| `rd-plugin-host` | Native resolvers, Wasmtime runtime, package verification, and installation |
| `rd-provider-registry` | Central provider, domain, alias, and credential metadata |
| `rd-secrets` | Encrypted local secret store |
| `rd-capture` | Click'n'Load, clipboard, NZB association, pairing, and tray/menu-bar integration |
| `rd-autostart` | Platform-specific autostart registration |

Crates are separated by responsibility rather than by individual UI pages. `rd-core` contains no
infrastructure. `rd-api` coordinates application use cases, while concrete transfer behavior stays
inside the scheduler, runners, and protocol crates.

### Technology Stack

| Area | Technology |
| --- | --- |
| Async/server | Tokio 1, Axum 0.8, and Tower HTTP 0.6 |
| HTTP/TLS | Reqwest 0.13, rustls 0.23, and platform certificate verification |
| Persistence | SQLite with SQLx 0.8 and embedded migrations |
| Contracts | Serde, Utoipa/OpenAPI 3, and `openapi-typescript` |
| Live updates | Tokio broadcast channels and Server-Sent Events |
| MCP | `rmcp` 3.2 over Streamable HTTP |
| Plugins | Wasmtime 48, WebAssembly Component Model, WIT, and Ed25519 |
| Torrent | `librqbit` 9 |
| Frontend | Vue 3.5, TypeScript 5.9, Vite 8, and Nuxt UI 4.11 |
| UI state | Pinia 4 and VueUse 14 |
| Localization | Vue I18n 11 |
| Tests | Rust test/Nextest, Vitest 5, and Testing Library Vue |

## Primary Data and Control Flows

### From Intake to Download

```mermaid
sequenceDiagram
    participant I as Intake channel
    participant C as LinkGrabber
    participant D as SQLite
    participant P as Probe/resolver
    participant S as Scheduler
    participant R as Transfer runner
    participant X as Post-processing

    I->>C: Text, URL, magnet, NZB, or torrent
    C->>C: Extract, normalize, exclude, and group
    C->>D: Persist batch, packages, and candidates
    C->>P: Online check and metadata
    P->>D: Status, name, size, route, and variants
    I->>C: Confirm selection / add paused
    C->>D: Atomically create download package and files
    S->>D: Claim next file by priority and position
    S->>R: Start the matching engine
    R->>D: Synchronized checkpoints and states
    R->>X: Package complete
    X->>D: Steps, progress, and result
```

Intake sources are `manual`, `clipboard`, `click_and_load`, `api`, `nzb`, `hot_folder`, and
`browser_extension`. Links are checked before queueing; duplicates remain visible and may be
queued again after explicit confirmation. Categories are selected by prioritized first-match
rules.

### Scheduler and Transfers

- The queue sorts first by `high`, `normal`, and `low` priority, then by manual position.
- Regular files share the global active-file limit. Open-ended recordings and suitably declared
  external runners may use dedicated slots.
- Providers limit concurrent resolution/download work through semaphores. Free downloads have a
  separate capacity-one slot per hoster and do not consume premium slots.
- `ip_blocked` holds back only free links for the same normalized hoster; other hosters and premium
  downloads continue running.
- Retries use the greater of exponential backoff, capped at 300 seconds, and the server-provided
  `Retry-After`, plus up to one second of jitter. IP blocks without a provider-supplied delay use a
  default hold-off of 15 minutes.
- HTTP files are probed before transfer. Range support, size, ETag, and Last-Modified determine the
  chunk plan and whether resume is safe.
- A chunk checkpoint is written only after the corresponding file data has been synchronized.
  Final files are moved atomically into place and may be verified with checksums.

### Free Hoster Downloads and Captchas

Resolvers may reproduce a hoster's browser flow without an account: load the file page, submit the
download form, wait for the countdown through the `wait` host function, solve a captcha through
`solve-captcha`, and return the final URL. Waiting consumes neither plugin fuel nor compute timeout,
but remains bounded by `wait_budget_milliseconds`.

The captcha broker tries a configured 2captcha-compatible service first. Temporary solver errors
fall back to manual input for image and click-point captchas; an invalid key or exhausted balance
is treated as a permanent configuration failure. reCAPTCHA v2, hCaptcha, and Turnstile are bound
to the hoster's domain and are answered by a solver service or by the person in their own browser
through the extension; CutCaptcha only by a service, and without one it is refused at once with
`captcha.cutcaptcha_needs_solver`. A click-point answer is a coordinate, returned to the plugin
through `solve-challenge`; `solve-captcha` stays the token form. Manual captchas exist only in
memory and are distributed live through `captcha.changed`; they are not restored after a restart.

### Post-processing

The effective level is resolved in the order package → category → global setting. Levels are
cumulative: `none`, `repair`, `unpack`, and `delete`.

```text
PAR2 verify/repair
  → extract archives
  → delete source archives
  → apply cleanup rules
  → run user script
  → optionally upload through rclone
```

Each step and its progress is persisted. Extraction uses a staging area; only complete results are
moved into the package directory. Recursive extraction is optional, limited to three additional
passes, and disabled for torrents while they are seeding.

## Domain Model

The primary aggregates are:

- `DownloadPackage`: name, destination, category, priority, position, kind, password indicator,
  post-processing configuration, and package state.
- `DownloadFile`: source, file name, transfer state, byte counters, retry time, checksums,
  resolver/network route, and protocol-specific metadata.
- `CollectorBatch`, `CollectorPackage`, and `LinkCandidate`: the persistent check and review area
  before the queue.
- `StorageRootConfig`, `Category`, `CategoryRule`, and `HotFolderConfig`: controlled storage and
  automatic routing.
- `Account`, `ProxyProfile`, `UsenetServer`, and `StreamChannel`: external connections and runners.
- NZB imports, NZB files, and segments: Usenet metadata and resume checkpoints.
- Plugin trust, capture/API tokens, and settings documents: system configuration and security.

64-bit byte counts are transferred as decimal strings in the JSON contract. This keeps values
above JavaScript's safe integer range exact. IDs are typed UUIDv7 values so that aggregates cannot
accidentally be addressed with the wrong ID type.

### States

Files use `queued`, `resolving`, `downloading`, `paused`, `retry_wait`, `verifying`, `repairing`,
`extracting`, `seeding`, `blocked`, `failed`, `cancelled`, and `completed`. The domain type validates
allowed transitions; direct transitions such as `completed → downloading` are forbidden.

Packages summarize file state as `queued`, `downloading`, `postprocessing`, `completed`, or
`failed`. LinkGrabber candidates use `resolving`, `checking`, `online`, `offline`, `unsupported`,
`unresolvable`, `duplicate`, `enqueued`, and `error`. A candidate whose check left a message
carries a stable code beside it, resolved as `server.codes.<code>`, with the English text only as
the fallback — free prose printed verbatim is how one sentence came to stand for three different
situations.

Failures contain a category, redaction-safe English message, optional stable code, and string
parameters. Categories distinguish transient, permanent, offline, authentication, rate-limit,
captcha, IP-block, and unsupported cases. This lets the UI and MCP share application logic while
the UI localizes the user-facing message.

## Persistence and Consistency

- SQLite runs with foreign keys, WAL, and `synchronous=NORMAL`.
- A single writer actor processes mutations serially; up to four readers may query concurrently.
  This avoids competing SQLite writes.
- A domain mutation and its persistent event are committed in the same transaction.
- Progress events are throttled to approximately 750 ms per download. Purely transient state such
  as pending captchas is broadcast only.
- The schema evolves only through numbered SQLx migrations in `crates/rd-db/migrations`.
- Resume metadata includes HTTP validators and chunk offsets, Usenet segment ranges and CRCs,
  torrent sessions, and post-processing steps.
- Configuration imports are validated in full and replace the affected tables atomically.

## API and Event Design

### Interfaces

| Interface | Path/transport | Authentication | Use |
| --- | --- | --- | --- |
| REST | `/api/v1/*`, JSON | Session, except for public endpoints | Web app and integrations |
| SSE | `/api/v1/events` | Session | Live updates |
| MCP | `/mcp`, Streamable HTTP | Bearer token with `api:*` scope | AI assistants |
| Capture | `/api/v1/capture/*` | Bearer token with capture scope | Agent and browser extension |
| SPA | Fallback routing | In-app authentication gate | Web interface |

The health endpoint, authentication status, initial password setup, login, and OpenAPI document are
public. All regular management and download endpoints require an administrator session. API and
capture tokens belong to separate permission spaces and are not interchangeable.

The REST error format is `{ "error": string, "code": string?, "params": object? }`. OpenAPI is
generated from Rust handlers and checked into `web/openapi.json`; `web/src/api/schema.d.ts` is
generated from it. Contract changes therefore begin in Rust, not in generated TypeScript types.

Event families cover download progress/state, package state, collector, category, hotfolder,
capture, account, proxy, Usenet, plugin, stream, post-processing, captcha, and system changes.

## Plugin Design

Resolvers are distributed as signed `.rdplug` archives. A package contains a manifest, WebAssembly
component, signature, and optional translations. The current contract is
`rdownloader:resolver@0.5.0` and covers URL matching, account checks, resolution, link checks, the
hoster catalog, and host functions for HTTP, cookies, secret lookup, logging, waiting, and captcha
solving.

The trust chain consists of the embedded release key, additional configured keys, and explicitly
confirmed trust on first use. Signatures are checked during installation and startup. The highest
installed compatible SemVer version wins for new jobs; running or pinned jobs retain their
assigned version.

Sandbox boundaries:

- no WASI, and therefore no general file, process, or socket access;
- network access only through the host and manifest domain lists;
- secret access only for declared references and domains;
- limits for memory, fuel, compute time, response size, and a separate wait budget;
- incompatible or faulty plugins must not prevent the service from starting.

Bundled resolvers are DDownload, Rapidgator, Nitroflare, KatFile, 1fichier, Keep2Share, FileJoker,
Premiumize.me, AllDebrid, Debrid-Link, and LinkSnappy. Native resolvers act as fallbacks and share
provider metadata with the components.

Bundled plugins are installed by choice, not all at once (RD-160-05). The setup wizard's *Your
services* step, right before the accounts, offers the bundle by service with a category chip row
and a search. An installed service is shown ticked, and a note above the list says that the
services needing no account came with the first start and can be unticked here or switched off
and removed later under Settings → Plugins (RD-180-14). Unticking one removes it on *Continue*,
after one confirmation that names the services and says they stop at the next start; a service a
download still uses stays and is named. The plugin manager lists every uninstalled service under
*Available services* with a one-click install and stays add-only; removing there is the
installed plugin's own delete. An update never installs a service nobody chose, and never
reinstalls one that was removed.

## Frontend and UX Design

### Information Architecture

The SPA has these primary sections:

1. **Downloads:** Package/file list, states, progress, queue actions, statistics, and
   post-processing.
2. **LinkGrabber:** Link, NZB, and torrent review; checks; grouping; and queueing.
3. **Streams:** Immediate recording and persistent channel monitoring.
4. **Statistics & history:** One page, one sidebar entry and the digit `7`, with two tabs
   (RD-1101-05; the owner wanted fewer entries in the navigation). The tabs are the settings
   sub-tabs' shape: `UTabs` in `pill`, the tab in the address as `?tab=` (none for the first, so
   `/stats` stays plain), a push per change, an unknown value shows the first tab; `/history`
   redirects to `/stats?tab=history`. Each tab is a chunk loaded when first shown, and only the
   shown one is mounted. A tab's own controls stand in the tab, not in the navbar, so each tab
   is whole on its own.
   - *Statistics* (first tab): the persistent transfer figures over a chosen range — stat tiles,
     bytes per hour or day as bars, and the range by kind and by provider — plus the metrics
     endpoint. The range switch is a group of pressed buttons beside the tab's heading rather
     than a select: four values, all visible, one click each. Bars, not a line: a bucket is a sum
     over an hour or a day, and a line would invite reading a slope between two sums.
   - *History* (second tab): every package that completed or failed for good, kept after it
     left the queue (RD-1100-04). The audit log's row conventions — filters above, the count of
     the matches, details behind the chevron pair — with paging by "Load more", a row action
     that adds the entry's sources back into the LinkGrabber, *Refresh* beside the introduction,
     and the shared clear control with its count and confirmation beside the list title.
5. **Remote jobs:** The jobs running at a provider — submitted, watched, answered and ended
   here. Beside the subscriptions, not under the accounts: a job somebody has to watch and
   answer is not a setting (RD-110-29).
6. **Audit log:** The append-only record of the security-relevant actions — who acted, from
   where, on what, how it ended. A page of its own rather than a tab of the log viewer, because
   the two lists answer different questions, are kept for different lengths of time, and a
   security surface should not be one click deep inside a diagnostics page (RD-110-03). It
   follows the row conventions like every other list: filters above, "full page" said out loud,
   details behind the chevron pair. The page offers **no destructive control at all** — there is
   no route that could serve one, and a button that suggested otherwise would be a lie in the
   interface; the export sits beside the filters, because the file *is* the filter.
7. **Settings:** Twenty-five directly addressable pages in six rubrics, the same in the
   sidebar and on the entry page at `/settings`: General (General, Interface), Downloads in the
   order a download takes — in, to its folder, after it, at what pace (Hotfolders, LinkGrabber,
   Storage & rules, Post-processing, Bandwidth, Unattended operation), Sources & protocols
   (Services, Accounts, Captcha & solver, Site rules, Usenet, BitTorrent, Media,
   FTP/SFTP/WebDAV), Integrations (Plugins, Tools, Notifications, Clients & API), Network &
   security (Network, Security) and Administration (Backup & restore, System & maintenance, About
   rDownloader). The table is one module, `settingsSections.ts`, and a test holds it to the
   owner's decisions of 2026-09-22 and 2026-10-06 (RD-1120-23: *Services* heads Sources &
   protocols, because its switches turn those protocols on and off; *Desktop client* and *API &
   MCP* are one page, *Clients & API*, whose tabs are Desktop, Browser and API & MCP). *Site
   rules* sits under Sources & protocols by the rubric's own test: a rule decides which services
   rDownloader recognises, which is where the bytes come from, not what happens to the queue
   afterwards (RD-110-08). A page or sub-tab that moves keeps its old address as a redirect
   (`MOVED_SETTINGS_PAGES`) and its search anchors as aliases (`MOVED_SETTINGS_ANCHORS`).

A collapsible, resizable sidebar contains navigation and live badges. A persistent transfer rail
shows global queue state. The captcha dialog and file-drop overlay sit above individual routes
because both may become active on any page. The overlay is a picture of the drop over the whole
page, not a field to pick a file in — that is `UFileUpload` — so it holds no control and takes no
pointer events. The first element in the tab order on every page is a skip link to
`#main-content` (WCAG 2.4.1, `docs/accessibility.md`): a plain `<a href>` styled by `.skip-link`,
not a `ULink`, because it is an in-page jump the browser performs, focus included, and not a route.

At startup, the application moves through Loading → Setup Wizard or Login → Control Room. After a
session becomes ready, transfers, the collector, and captchas connect to the event stream; streams
and other initial data are loaded once.

### Visual Language

- **Typography:** IBM Plex Sans Variable for UI and prose, JetBrains Mono Variable for numbers,
  addresses, status details, and eyebrow labels. Below `text-xs` there is one size, `text-2xs`
  (11 px, the `--text-2xs` token in `web/src/assets/main.css`), for metadata lines and small
  badges; no template writes a pixel size of its own (RD-1120-14).
- **Primary color:** `signal`, a custom teal scale from `#edfffd` to `#052f30`.
- **Secondary color:** cyan; **neutral:** slate; **warning:** amber; **error:** a custom `coral`
  scale from `#fff2f1` to `#41110f`.
- **Shape:** A small global radius of `0.3rem`; functional, compact surfaces rather than large
  decorative card radii.
- **Cards are soft cards** (RD-1101-09; the owner: the cards had the page's own colour and only an
  outline). A card on a page — every settings card, the views' cards and framed forms, the
  overview's link cards — is a `UCard`, whose default variant the theme in
  `web/src/uiTheme.ts` sets to `soft`: it stands off the page by its `bg-elevated/50` ground, not
  by a border. A card section is `<UCard as="section">` with its anchor and test id on the root and
  its content in the default slot; layout classes for the content go to `:ui="{ body: … }"`,
  placement classes (`mt-6`, `xl:col-span-2`) stay on the root. A card nested in a card names
  `variant="outline"` (the installed plugins' `PluginCard.vue`), so it stands off its soft parent.
  A card never shrinks below its content: the theme gives its root `overflow-clip` instead of
  `overflow-hidden`, because a hidden overflow lets a card in the panel body's flex column
  collapse, clip what it holds and fall out of the scroll height (RD-1110-17, the audit and log
  filters in 1.10.1); `min-h-fit` does not hold it in Firefox. In a form
  and list layout either one card holds both columns (the settings cards) or each column is a card
  of its own (subscriptions, automations, site rules) — never one column a card and the other
  bare; the heading with its actions and count stands above the list's card, and an empty state
  goes inside it. The logs' and the audit log's entries sit in such a card too, rows divided by
  hairlines. Not cards, and so not soft: rows and the lists themselves (queue rows, table and log
  lists, the torrent file, peer and tracker lists), dashed empty states, inner boxes of a card or a modal
  (previews, code samples, the statistics chart), the row of hairline-divided fact tiles, and the
  sign-in and setup-wizard panels, which float over the signal grid with their own shadow.
- **Motif:** A subtle 28 px signal grid for loading and introductory surfaces; transfer stripes
  indicate activity without replacing text.
- **Themes:** Light, dark, and system. Components use semantic Nuxt UI tokens so that contrast and
  state meaning survive across themes.

Queue rows and the LinkGrabber's link rows share one responsive grid of nine named cells. Below
560 px the row is two lines, with the name on the first and the state on the second; from 560 px
it is one line, progress joins at 768 px, and size plus additional metadata at 1280 px. The widths
behind those numbers are measured and recorded under "What a queue row may spend its width on"
below, together with why the subscription rows wrap instead. Numbers use tabular figures to
prevent columns from shifting during live updates.

### Interaction Rules

- Primary actions are state-dependent; mutually invalid actions are not offered together.
- **A state with an end says when it ends, and one click ends it early** (RD-190-20). The timed
  queue pause replaces the global toggle with *Paused until 14:30* — the clock alone within the next
  day, the date as well beyond — the time left in the tooltip, and a click (or `P`) resumes. The
  durations sit in a menu beside the toggle rather than replacing it, so a plain pause stays one
  click. A bandwidth profile switched on by hand says the same way why it is active: *Chosen by the
  schedule*, or *Switched by hand, until …*, with *Back to the schedule* beside it.
- Destructive actions require explicit confirmation.
- Server failures appear localized while retaining stable error codes as the contract.
- Forms expose loading, success, and error states. Self-saving settings areas remain separate from
  the shared settings document. Areas that *fetch* expose the same three, under the rule below.
- Full-page refresh is not the normal update path; SSE or local store updates keep views current.
- Global keyboard shortcuts, numbered in sidebar order: `1` Downloads, `2` LinkGrabber,
  `3` Streams, `4` Subscriptions, `5` Remote jobs, `6` Automation, `7` Statistics & history
  (`H` its History tab), `8` Logs,
  `9` Audit log, `0` Settings, plus `B` sidebar, `N` import, `P` global start/pause (ending a timed pause while one holds), and `?` help. A new navigation entry takes the
  number of its place and renumbers what follows, so the keys keep reading top to bottom. With
  all ten digits taken, an entry added later takes a free letter of its name instead (`H`
  History, RD-1100-04): no view loses the digit people learned, and the catalogue still lists the
  keys in sidebar order. Since the history became the second tab under `7` (RD-1101-05) `H` keeps
  opening it and is listed right after `7`.
  A key that belongs to one page (`K` removes the completed packages, only on Downloads,
  RD-180-17) is still bound in the one catalogue — same guards, listed in the `?` help — and
  the page hands its action in while it is mounted; the key runs that action with its
  confirmation and shows as a `UKbd` hint on the menu item that offers it. Pressed again, the
  same key answers that confirmation (`ConfirmModal`'s `confirmKey`, shown as a `UKbd` on its
  button), so the action never needs the mouse or a Tab to the button. `F` focuses the search of
  the page the same way — the LinkGrabber's indexer search (RD-180-19), the download list's name
  search (RD-190-21): the panel or the list hands its focus in while it is mounted, and the field
  shows the key as a `UKbd` at its end; while the indexer field is disabled the key lands on the
  hint's link to the indexer settings, the one thing there is to do. The
  LinkGrabber hands in its navbar buttons the same way (`A` add links, `E` enqueue all, `W` add
  all paused, `R` delete all; RD-180-23), each with its `UKbd` on the button and doing nothing
  while that button is disabled.
- **The key that opens a question confirms it; `X` closes every dialog** (owner, 2026-10-02).
  Every confirmation a key can start carries that key as its `confirmKey` — always, not only when
  the key started it, because a click user is not disturbed by a hint. `X` is the one plain key
  that acts *in* a dialog: it sends the `Esc` Reka answers, so it closes the topmost layer that
  `Esc` would close and leaves a dialog that may not be dismissed (the captcha) alone; in a text
  field it is typed, as every plain key is. A dialog therefore never takes a confirm key `x`. The
  guard that holds plain keys back counts a dialog open when `useOverlay()` tracks one or the page
  shows `[role="dialog"][data-state="open"]` — the second covers the dialogs bound with
  `v-model:open`.
- **A list's filter and search live in the address** (RD-190-21). The download list keeps its
  state filter and its name search as `?filter=` and `?q=`: a link opens the list narrowed, a
  reload keeps it, and the default carries no query, so the plain address stays plain. A filter is
  replaced in the address, not pushed — narrowing a list is not a place back should step through
  key by key. A search over what the page already holds filters as you type, a moment after the
  last key; a filter that hides every row says so, with *Reset filter*, instead of the empty
  list's welcome text.
- **A search that costs the other side something is asked, never typed into** (RD-180-19). The
  indexer search sits at the top of the LinkGrabber, because what it finds is reviewed there,
  and it is always there (owner, 2026-10-01) — the one exception to the absent-section rule
  below, because the field is where somebody looks for the feature. Until an indexer is enabled
  it is disabled, with a short hint and a link to Settings › Usenet › Indexers that says an
  indexer subscription alone is not searched; the hint waits for the first answer, so it does
  not flash up while the list loads. One
  press of *Search* is one request per indexer, the next page is the next press, and nothing
  searches as you type, polls or retries: an indexer counts requests against a daily limit and
  may cache an answer for minutes. What the server would refuse (a term of one or two
  characters) is refused under the field before anything is sent. Results are a sortable
  `UTable` of title, size, age, category and the password flag — the indexer's name and its
  grab count stay out, the space goes to the title — with a checkbox per hit and an icon-only
  download button per row (`aria-label` and `title` name the hit; pending, done and failed are
  that row's own state). *Add selected* and the row button send through the upload's own
  import, so the hits arrive in the list below like a dropped `.nzb`, and an indexer that
  refused is named in its own warning while the others' hits still show. How a hit's row looks
  is its indexer's list style, chosen where the indexer is defined and never in the result list
  (RD-190-16): *compact*, one line, or *detailed*, which puts the `size-12` thumbnail (or the
  placeholder below) before the title and one cut line of the indexer's metadata under it,
  inside the title column — size, age, category and the button stay where they are in either
  style, and under *all indexers* each row follows its own indexer. A torrent hit (Torznab,
  RD-1100-03) carries an outlined *Torrent* badge and its swarm, seeders up and leechers down,
  in the same title cell. The search type (free, TV series, film) offers only what the chosen
  indexers say they answer: their `t=caps` are asked when the type menu first opens, never on
  mount, a type none of them answers is switched off once the answers are in, and of the chosen
  type's ids only those one of them takes are shown, in a row of their own under the form.
- **One search reaches every view and every setting** (RD-170-15): Nuxt UI's `UDashboardSearch`
  — the command palette in a modal — on Ctrl/Cmd+K from anywhere, text fields included, on `/`
  outside them, and on the search button at the top of the sidebar (an icon with a tooltip on
  the rail). Its groups are *Go to* (the main views, each with its digit), *Settings pages* and
  *Settings* (cards and fields, the page named beside each). It matches in the interface
  language on title, description, translated synonyms and names that are the same in every
  language (ntfy, rclone, S3). Choosing a setting opens its page — and sub-tab — scrolls the
  card or field to the middle and outlines it for two seconds, still under reduced motion; a
  field takes the focus, a card does not. What it finds is a declarative table beside the
  settings sections (`web/src/settingsSearch.ts`) and a `data-settings-anchor` on the element,
  never a scrape of the rendered page: a page not mounted has nothing to scrape. A new settings
  page fails a test until it has its row; a new card is found once it has an anchor and a row.
- **A settings page with more than five cards is split into sub-tabs** (RD-180-15; the owner:
  "put its parts into separate tabs so it's clearer … so everything stays clear even as settings
  grow"). Count what a reader scrolls past: every card once, cards side by side each
  once, a row of tiles once, and a form-and-list editor (`FormListLayout`) twice — two cards
  beside each other, one above the other on a narrow screen. Five or fewer stay one scroll; the
  sixth card is the moment to split, not the tenth. The tabs are topics a reader comes with —
  *what is installed*, *what can be added*, *whom this machine trusts* — never "more" or
  "advanced"; three to five of them, because a tab bar divides one line (the chip-row rule
  below), or two where the page has two subjects (*Usenet*: servers and indexers; *Accounts*:
  provider accounts and site logins; *Network*: proxies and reconnect, RD-1120-23), and every tab
  at least one real card. Cards move with their order kept and are not
  rewritten; the page header stays above the tabs and the navbar keeps the page's name. The
  shape is the routing page's since RD-170-15: `UTabs` in `pill` variant, `:unmount-on-hide="false"`
  so every tab's data loads once and the search's anchors exist, the tab's name in the address as
  `?tab=` (none for the first tab, so the plain address stays plain; an unknown name shows the
  first tab), a push per change so back and forward walk the tabs. A count that should not wait
  unseen — installed plugins, waiting updates, the routing lists — goes into the tab's badge.
  The save bar shows only under a tab that edits the settings document — and under every such
  tab: System had none until RD-180-15, and its fields were saved only by another page's button.
  Until the document is loaded, a tab that edits it or shows its values waits with the failure,
  *Retry* and the tab bar above; the page's self-saving tabs stay usable (RA-WEB-05). A tab — or a
  page without tabs — that saves its own lists and carries one card of the document (`documentCard`,
  RD-1120-21: the storage capacity beside the storage roots, the admin login beside the password,
  the NNTP limits under the Usenet servers, the limits beside the bandwidth status) does not wait:
  that card alone shows the document's state and *Retry* (`SettingsDocumentGate`), and the save
  bar is under it once the document is there. The tables are
  `SETTINGS_SUB_TABS` in `settingsSections.ts` and `useSettingsSubTab`; the search entries name
  their tab, and a test holds each entry to the tab slot its anchor is actually rendered in. As
  of 1.12: Storage & rules, Bandwidth, Accounts, Usenet, Plugins, Clients & API, Network, Security
  and System have tabs; Notifications is at five and splits with its next card.
- **Unsaved settings are not lost without a question** (RD-180-16; owner, 2026-09-30). A view
  with a save bar knows when what is on screen differs from what was last loaded or saved, and
  `useUnsavedGuard` asks before that is lost: leaving the route asks in the app's confirmation —
  *Discard* (destructive) leaves, *Cancel* stays with the edits — and closing or reloading the
  browser tab gets the browser's own question, the only one a page may raise there. It asks only
  where edits are really lost: the settings pages and their sub-tabs share one document in one
  mounted view, so moving between them asks nothing; a form a page holds for itself (the captcha
  card, the proxy form) asks when its page is left. A clean view, and a view just saved, never
  asks. Forms that save each entry themselves carry no save bar, and of them the ones where a
  half-typed entry is costly ask too: a new subscription and an open automation draft
  (`useFormBaseline` tells them what changed since the form was last filled; RD-1120-15).
- The UI is fully translated into English, German, French, and Spanish. Plugin messages are merged
  into the same locale namespace at runtime. Languages come from `web/src/locales/languages.json`;
  the picker names each in its own language (*Deutsch*, *Français*), and one still being
  translated is marked *(in progress)* in the reader's language and shows English for each
  string it lacks — never a raw key. Only a complete language is chosen from the browser's
  languages; an unfinished one has to be picked (RD-1100-09).
- **German says "du", French "vous", Spanish "tú"** — as the website does (owner, 2026-09-27):
  an open-source tool, not a business form. German is lower-case *du*, *dein*, imperatives in the
  du form (*Klicke*, *Prüfe*), never *Sie*/*Ihnen*/*Ihr*; "sie" in the third person stays. Tests
  over the web, extension and plugin catalogues fail on a formal address, with a short list of
  third-person sentence starts ("Sie laufen …"). French and Spanish hold to theirs throughout
  (owner, 2026-10-02): French *Saisissez*, *votre*, never *Saisis*, *ton*; Spanish *Introduce*,
  *tu*, never *Introduzca*, *usted*. Their sibling tests (`frenchSpanishAddress.test.ts` and its
  extension and plugin twins) catch the forms that only ever address the reader — French "tu"
  pronouns and hyphenated "tu" imperatives, Spanish *usted*, reflexive *-ese* imperatives and a
  list of formal imperatives at a clause start — and list a misread sentence by key and phrase.
- **A technical term stays English where the language's IT usage keeps it** (owner, 2026-10-02):
  *Stream*, *Plugin*, *Token*, *Proxy*, *Cookie*, *Hash*, *Seed/Seeding*, *Tracker*, *Hoster*,
  *Webhook*, *Endpoint*, *Header* — a German "Stromtransformation" for a stream transform reads
  as electrical current. The owner's choices: German *Backup* (never *Sicherung*; *Voll-Backup*,
  *Backup-Ziel*), *Mirror* (never *Spiegel*), *Tresor* (never *Vault* or *Secret-Store*, as in
  password managers), *Konto/Konten* (never *Account*; *Providerkonto*, *Premium-Konto*),
  *Passwort* (never *Kennwort*), *Speicherort* for a storage root (never *Storage-Root*;
  *Standard-Speicherort*) and *Gratis/Direkt* for a download without an account (1.9.0);
  Spanish *seeding* and *Seeders*, *suma de verificación* (never *suma de comprobación*) and
  *stream de eventos* where an event stream is named (French keeps *flux d'événements*). Ordinary words
  are translated, and a native word that is genuinely the standard one stays (de
  *Warteschlange*, fr *jeton*, *point de terminaison*, *indexeur*, es *indexador*). Whichever a
  language chose, it uses that one everywhere: one term per concept per language, never *Queue*
  in one view and *Warteschlange* in the next; a search keyword may still name the other word as
  a synonym. Protocol, product and format names (Newznab, SABnzbd, yt-dlp, PAR2, WebDAV, S3,
  NZB, JSON …) are never translated.
- Settings navigation follows stable task groups rather than an alphabetical order that changes
  with the selected language. Group labels are headings, never expandable detours: every settings
  page remains one click away after Settings is open, including in the collapsed sidebar popover.
- **A settings page sits where somebody looks for it, and the rubric says what that is.** The
  six rubrics are subjects, not containers of convenience: *Downloads* is what happens to the
  queue, *Sources & protocols* is where the bytes come from, *Integrations* is what the service
  talks to, *Network & security* is the way in and out. A card goes on the page whose subject it
  is, never on the page that happened to have room — tools were under Interface, quiet hours
  under Bandwidth, the captcha solver under Network, and each of them was found by scrolling
  rather than by looking (RD-110-29), and so were the LinkGrabber's blocklist under Storage &
  rules, the site logins under Network and two display switches beside the blocklist, until they
  went to *LinkGrabber*, *Accounts* and *Interface* (RD-1120-23). The same holds for a single
  field (RD-1120-21; owner, 2026-10-06): *General* is the queue and its retries — parallel files,
  chunks, connections per host, retries, automatic removal, SHA-256; a field whose topic has a
  page of its own goes there, beside what it overrides or is measured against — the admin login
  on *Security* beside the password, the UI port with the reverse proxy and external address,
  the global storage capacity beside the storage roots, the NNTP limits under the Usenet servers,
  the speed and upload limits on *Bandwidth* before the profiles that overlay them — and its
  search anchor stays an alias, so an old link still lands on it. A heading that says where a value
  is kept stands only over what is kept there: *This browser only* covers language, theme and
  browser notifications, not the display settings every browser shares, which are a card of
  their own. A new setting first asks which of the six it belongs to;
  a setting that fits none is a sign the rubric is missing, not that one should be stretched.
- **Settings that work together point at each other** (RD-1120-23). Where a setting on one page
  only makes sense with one on another — the torrent upload limit and the global one, a proxy
  picker and the proxy profiles, quiet hours and the bandwidth schedule, the backup destinations
  and the S3 profiles — each carries one line under it: *See also* and a link that names the page
  and the card, `Network › Proxy profiles`. It is `SettingsCrossLink`, and it names the target by
  its search anchor, never by a path: the anchor's row in `settingsSearch.ts` says where the card
  is, so a card that moves takes every link along, and a test fails on a link to an anchor that
  is gone. Following it opens the page and tab and outlines the target as the search does. Where
  one value was set in two places, it is set in one and the other place links there — every
  program path is on *Tools › Custom paths*, and the cards of the services that run them say
  *Program path under* that card.
- **A switched-off service says so on its own page** (RD-1120-23). While BitTorrent, Usenet,
  media, galleries, recordings or remote transfers are off under *Services*, the page that sets
  them up opens with a warning `UAlert` (`SettingsServiceOffAlert`) whose action leads to the
  switch; the page stays editable, because its settings apply once the service is back on.
- **The settings open on an overview, not on a page.** `/settings` shows one card per page under
  its rubric, in the sidebar's order, each carrying the page's own title and description — the
  same header the page then opens with — and each a link, so the keyboard reaches the cards the
  way it reaches the sidebar. The overview reads the same table the sidebar reads; there is no
  second list to drift. What it does not do: hold settings itself, or promote a page above the
  others. A redirect to one page, as `/settings` used to be, makes the way into the settings
  lead to a status page with sixteen siblings hidden behind a sidebar group.
- **Every settings page opens with the same header, and a test counts it.** One `SectionHeader`
  at `level="page"`, eyebrow, title and one sentence, before anything else; the sentence is the
  one the overview card shows. A page without it — the desktop page had a bare paragraph, the
  security page nothing — reads as a fragment of another page.
- **A measured figure that cannot be measured shows nothing.** Estimates — the remaining time is
  the first of them — print no placeholder, no `∞` and no "calculating…" when the inputs are
  missing. The field is simply empty, and any caption beside it says what the neighbouring figure
  does include rather than letting it count things silently. An estimate the interface cannot
  stand behind is worse than an absent one, because a reader cannot tell the two apart.
- **The browser tab is a status line, and exactly one place writes it.** While transfers are
  running the title reads `<rate> · <count> active · rDownloader`, rate first because a narrow
  tab shows only its first characters; at rest it is the application name alone. It is derived
  from the shared stores in a single composable called once from `App.vue`, never per view — two
  writers would fight on every route change — and the rule above applies to it: a rate that is
  not a usable figure is left out rather than printed as a placeholder. The whole behaviour is
  one switch in Interface settings, because a title that keeps changing is a distraction for
  some people and that is not arguable.
- **The tray line is the same status line under a different constraint.** The desktop agent's menu
  entry and tooltip read `<counts> · <progress> · <rate> · <remaining> left`, widest context
  first because the line is read whole rather than truncated from the left. Rate and remaining
  time come from the service, never from the agent's own arithmetic, and they stand or fall
  together: both only while something is actually running, because either one beside a resting
  queue reads as motion. The rule two bullets up applies unchanged — an estimate the service
  cannot stand behind leaves the segment out. Untranslated, because the agent carries no
  catalogue (RD-092-05). Where the surface has a hard buffer, as the Windows tooltip does at 128
  characters, the line is cut to fit rather than trusted to be short; the menu entry, which has
  no such buffer, keeps it whole.
- **The sidebar collapses to an icon rail, and the way back out stays on screen.** The switch is
  Nuxt UI's own `UDashboardSidebarCollapse` in the sidebar header, not a second mechanism beside
  it, and `0` does the same from the keyboard. The rail is 64 px wide and its header keeps
  32 px between the paddings: one control, which the 32 px logo would fill by itself. So on
  the rail the logo *is* the switch — the mark as the button's face, the panel icon hidden —
  rather than the wordmark yielding the place and leaving the rail without the application's
  own mark (RD-110-32). A control that hides itself when used leaves the reader with a strip
  of icons and no stated way back, and a logo that is a button states it: the tooltip and the
  accessible name say *Expand sidebar*. It is the same element in both states, so the focus of
  whoever pressed it stays on it. The switch is named for the state it would produce —
  *Collapse sidebar* while open, *Expand sidebar* while collapsed — in all four languages. The
  choice belongs to the session and survives every route change; below the large breakpoint
  the sidebar is a drawer and the switch is not shown (RD-109-31). At the foot of the rail,
  language and theme — two selects while expanded, one per row, because side by side each kept
  20–30 px for its value (RD-120-53) — are one menu behind a gear
  (`i-lucide-settings-2`, not the navigation's settings icon: this is not the settings page),
  named for both, *Language and theme*, never for one of them. The footer takes the sidebar's
  width in both states — its separator and selects run where the navigation's separator runs —
  and on the rail the connection dot and the sign-out button stack, since side by side they need
  56 of the 32 px between the paddings (RD-120-61). The dot says what is true: green and
  pulsing beside the endpoint while the service answers, red with *Connection to the service
  lost* in place of the endpoint once the event stream broke off or a request got no answer for
  1.5 s, with one warning toast for the outage that goes when the service is back; the same
  return clears a *service could not be reached* alert, and a view whose code could not be
  fetched meanwhile says so in a toast and is loaded once the service answers (1.9.0). A page
  whose build is not the service's version — one left open across an update — says so in one
  persistent info toast *New version available — reload* with a *Reload* button, checked on start
  and on every reopened event stream; it never reloads by itself, since a half-filled form would be
  lost (RD-1120-16). That toast is about the page, not a release: a newer version is announced in the same
  footer, above language and theme, and nowhere else: nothing while there is none, one soft
  button *Version X available* while there is (an icon with a tooltip on the rail), opening a
  dialog with the notes and the download or the package manager's command to copy — never a
  banner over the content, never a toast (RD-180-01). Where the installation installs itself,
  the dialog's *Download* fetches the version in the background with its progress in the dialog,
  *Install and restart* uses that file, and the browser's download is a small *Download manually*
  link; a failed install shows its reason with *Try again*, and a service that never comes back
  ends the wait with what to do — the dialog never waits without an end (RD-180-02, owner
  2026-10-01). The open sidebar is a share of
  the window (15 %, dragged between 14 and 21 %) with a floor of 15rem, 240 px: at 15 % of
  1280 px it was 192 px and cut the application's name, "Téléchargements" and "Entfernte
  Aufträge". Its header carries the logo and the name and nothing under them; the tagline that
  stood there needed up to 223 px of 117 and was removed by the owner's decision (RD-120-53).
  Navigation labels wrap rather than being cut, which only the settings pages' longer names ever
  need.
- **On a settings page the settings group is open and highlighted, and the navbar names the
  page.** The group opens whenever a settings route is entered — not once when the shell mounts,
  which happens before the first route resolves — and a reader who closes it keeps it closed.
  The entry is marked active on every settings page, because the pages are routes of their own
  and the router would not. The navbar title is the page's sidebar label, as every other view's
  title is its own; only the overview at `/settings` says *Settings* (RD-120-53).
- **What a choice governs stands after the choice.** A control that decides which fields exist,
  which of them are required, or what a field is called belongs directly above all of them — and
  it is already answered when the form opens, with the first option its subject offers, rather
  than starting blank. A form filled from the top down must never ask three questions before the
  one that decides whether those questions exist. The account form did exactly that: sign-in
  method came after display name, username and connection route, no method was selected at all,
  and the secret field beneath therefore read *DDownload password or API key* — one box for two
  answers. A label naming two possible answers is the symptom; the cure is the order and the
  preselection, not a longer label (RD-109-35).
- **A name the application chose may still improve; a name a person chose is final.** Intake has
  to name a package before anything is known about it, and for a single link whose address offers
  nothing it can only fall back to the hoster. Such a name is a placeholder, and the interface is
  allowed to replace it the moment something better arrives — the file name a resolver or a check
  reports — without asking, because nobody chose the name being replaced. The moment a person
  types one, or a container or a common stem supplies one, it stands: it is never overwritten by
  a later discovery, and changing it is a deliberate action with its own confirmation. The test
  is the origin of the current name, never its quality. Where such an automatic rename also moves
  data, it happens before the data exists rather than afterwards, and a destination that is
  already occupied stops the rename instead of merging into it (RD-109-45).
- **A click in a picture is an answer, and answering is a separate step from aiming.** A
  click-point captcha is answered by clicking the picture, and a wrong spot is a wrong answer
  that fails the download — so the first click only marks, a later click moves the mark, and
  the same *Send answer* button that sends typed text sends the mark. The interface says where
  the mark is, in the picture's own pixels, and the mark is drawn as a share of the picture so
  it stays put when the dialog scales the image. Nothing is ever sent on the click itself. The
  picture is a `<button>` with a name, because a click handler on a plain element is out of
  reach for a keyboard (`docs/accessibility.md`): the arrow keys move the mark one pixel, ten
  with `Shift`, starting from the centre of the picture, the position beneath it is a
  `role="status"` line so a screen reader hears where the mark went, and `Enter` or `Space`
  sends it — the same route the button offers, never a shortcut past it (RD-110-15).
- **A credential from the browser takes two consents, in two places, and the service names the
  site.** The service cannot read a browser's cookies, so an account's session at its provider
  comes from the extension (RD-120-45). The person asks for it at the account in the web
  interface (*Take over from browser*, shown only where an installed plugin declares a
  `cookie_scope`); the request waits there with a line that says where to answer it and a way
  to withdraw it. The extension then shows the request in its popup — site and account named —
  with *Hand over* and *Decline*, and the click on *Hand over* is the second consent and the
  gesture for the browser's own permission prompt for `cookies` and that one origin. The site is
  never typed, picked or read off a page: it is the plugin's scope, carried by the request. The
  outcome comes back to the row (arrived and checked, declined, expired), and the grant is given
  back in the browser. A capture token alone can therefore not start a handover, choose an
  account or widen a domain — it can only answer what a person asked for, once. A sign-in that
  cannot finish because the browser is signed in already says so and names this way out, rather
  than waiting out a timeout in silence.
- **A sign-in with a code stays on its account row until it ends.** The address (a link the
  person follows, never opened for them), the code, a live status saying that rDownloader keeps
  checking in the background, and the code's expiry stay visible while the service waits —
  through a list refresh, a read that did not answer and a page reload, which reads the running
  flow back. Only the end replaces them: signed in, refused, expired or cancelled. *Connect* is
  not offered while a sign-in runs, and pressing it on a stale page shows the running one: a new
  code would make the one being typed worthless (RD-150-09).

### Fetching, Empty, and Failed

An area that fetches its data is in exactly one of three states, and it says which one. Until
1.0.4 it said none of them: the list was empty while the request was in flight, so the empty state
rendered — "no accounts", "no downloads", "no subscriptions" — and if the request failed the list
stayed empty, so the same sentence rendered again. Both are false statements. The second is the
worse one, because it turns a server failure into "nothing there" and gives the reader nothing to
act on.

- **Loading.** While the first fetch is outstanding, the area shows the 28 px signal grid named
  under Visual Language, with pulse bars standing in for the rows that are coming, and the word
  for loading beneath them. The grid is the motif for loading surfaces; this is where it is used
  during a fetch rather than only on introductory screens. The **first** fetch is meant
  literally: a later refresh never re-enters this surface. Lists refresh on state events, poll
  timers and after every write, and an area that re-enters loading on each of them replaces its
  empty state with the skeleton for a few milliseconds at a time — a flicker the reader sees
  precisely because nothing renders here once there is content (RD-106-19). The flag a view
  binds therefore describes the first fetch; the per-refresh flag belongs on the buttons that
  triggered the work.
- **Failed.** A fetch that did not arrive shows its localized message in an error frame, with
  `role="alert"`. Where the view already raises the failure in an alert of its own — the queue,
  the collector, subscriptions and automations all read a store `error` — the empty state simply
  retracts and the alert is the single statement. Either way the failure never degrades into the
  empty state. Where nothing else on the page would send the same request again — a section that
  fetches only when it is opened, like one subscription's hits in the LinkGrabber's review box —
  the failure carries a single quiet button beside it that repeats exactly that read, labelled
  `common.actions.retry`. Not everywhere: an area that refreshes on events or a timer asks again
  by itself, and a button that duplicates that is one more thing to ignore.
- **Empty.** The empty state appears only once the fetch has settled and the result really is
  empty. `v-if="!items.length"` on its own is not enough, and is the shape this rule replaces.
- **Content.** With rows to show, none of the three renders.

**A lapsed session is not a failed fetch.** A request refused with `auth.session_required` means
the sign-in ran out — the idle limit, the maximum lifetime (RD-130-09), or a sign-out from another
browser — and no area can do anything about that. `web/src/api/client.ts` notices the code on
every response and the session store returns the whole interface to the sign-in screen, which
says in a warning frame that the sign-in expired. The area that asked never shows it as its own
failure. The code is the signal, not the status: a wrong password at the sign-in is a `401` too.

A secondary section for a subject that does not exist at all is a fourth case, and it is absent
rather than empty: no frame, no heading, no button. The three states above describe an area whose
subject the reader has — a queue, a link list, an account list — where "nothing here yet" is the
answer to a question they asked. A box for a feature nobody has set up asks the question for them
and then answers it, which is how the indexer box came to sit under every LinkGrabber list saying
that no indexer subscription was set up (RD-107-12). The condition is the subject's existence, not
the emptiness of its result, and it follows the loading rule above: absent while the first fetch is
outstanding, so the section does not appear and vanish again. The LinkGrabber's indexer search is the one
exception (owner, 2026-10-01): its field is where the feature is looked for, so it stays, disabled
with a hint that leads to the settings (RD-180-19).

Skeleton or indicator follows from whether the shape of the content is predictable. A list of rows
gets bars in the shape of rows; a single figure or a status line that has no predictable shape gets
the wording alone. Either way the surface occupies roughly the space the content will, so the page
does not jump when it arrives.

`DataState.vue` implements all three and wraps the place where the empty state used to sit
unconditionally; `useFetchState()` holds the `loading` / `loadError` pair behind it, with `loading`
starting `true` — a component that fetches on mount is loading from its first render, not from the
moment its handler runs. Where a store already carries the flag for its own fetch
(`transfers`, `collector`, `subscriptions`), the view reads that flag instead of keeping a second
copy. The reference implementations are `SettingsAuthProfilesCard` and
`SettingsRemoteCredentialsCard`, which held this shape in their composables before it had a name,
and `StreamsView`, whose comment spells out why a section waits for `!loading`.

**A picker whose options are on their way is not drawn.** An empty `USelect` reads as "there is
nothing to choose", which is the empty state's sentence spoken during the fetch. Until the options
have arrived — and until every list they are filtered from has, since the accounts and the
providers they are matched against are two requests — the place shows `DataState` loading with a
`label` that names what is being waited for ("reading which of your accounts can take a job"),
not the generic word: the reader is looking for a control, and the label says why it is not there
yet. The remote-job form is the reference (RD-120-51); its first open after a start used to show
the empty picker for as long as the server took to compile every plugin.

### Handing Over Several Files

Where a form takes files one at a time for the server, it takes several at once too — chosen with
`multiple` or dropped — and turns them into the same number of single requests, sent **one after
another** through the endpoint that took one file before. The endpoint's limits then apply to each
file with nothing kept in step, and the requests reach the account in the order the files are
listed. Each file is a row with its own state named in words (waiting, being handed over, handed
over, already running, failed, not sent), and a failure is written on the row that had it, with the
server's translated reason: the rest are sent regardless, and the summary afterwards counts them
("1 of 3 files could not be handed over; each says why"). A file the client can refuse without
reading it — wrong type, over the size ceiling — is listed as *not sent* with its reason rather
than vanishing into a toast, so the reader can see which of the dozen it was. Finished rows make
room for the next selection; failed ones stay and are sent again with the next press.

A page with such a form claims the window drop zone for as long as the form is shown
(`claimFileDrops` in `nzbImportRequest.ts`): the global overlay stays away, and a file dropped
anywhere on the page goes to the form instead of navigating to the LinkGrabber, which is the one
place the reader went to this page not to send it. `RemoteJobSubmitForm` is the reference.

### A Control Promises What Is Behind It

A control that reveals something — an accordion, a "show more", a tab, a link into a detail
view — is a promise that there is something there. Opening it is the reader's part of the
bargain; finding "nothing recorded yet" is the promise broken, and it costs more than the click.
It leaves the reader knowing *less* than before, because an empty panel cannot tell *never ran*
from *ran and was unremarkable*, and they now have to hold both possibilities open. The plugin
manager showed every plugin a diagnostics accordion whether or not that plugin had ever been
invoked (RD-120-28).

So: **a control that opens onto nothing is not rendered.** Not disabled, not greyed, not shown
with a zero — absent, exactly as a secondary section for a non-existent subject is absent above.
A disabled control still occupies the eye and still has to be read and dismissed.

This puts a requirement on the data, and it is the real cost of the rule: the page has to know
*in advance* whether there is anything behind the control, and it must learn that without
fetching the thing itself. Otherwise the rule quietly undoes every decision to load a panel's
content only when somebody opens it. The cheap answer is a **count carried by the list the page
already fetches** — a number, never the entries — which is how the plugin inventory came to
report `execution_count` per plugin. Where no such number exists and none can be added cheaply,
the honest move is to leave the control and say why in a comment, not to start loading
everything up front; abandoning load-on-demand is a decision that needs a measurement behind it.

**Where an empty view is right.** The rule is about controls, not about emptiness, and there are
three places where showing nothing is the answer rather than a failure of one:

- **A place the reader navigated to on purpose.** An empty queue, an empty link list, an empty
  account list — the reader asked "what is in here", and "nothing" is a true and useful answer
  to that question. The three states above govern it; it is not a broken promise, because no
  control claimed there was content.
- **A place that is about to have content.** A drop target, a staging area, a filter result —
  the emptiness is the current state of something the reader is actively changing, and hiding it
  would remove the very surface they need. A filter that matches nothing must say so, or the
  reader cannot tell a bad filter from an empty world.
- **A place where absence is itself the report.** "No failures in the last 24 hours", "no
  conflicts", "nothing quarantined" — here the emptiness *is* the finding, and it is worth a
  sentence of its own. The distinguishing question is whether the reader learns something by
  looking: in the diagnostics case they did not, because they could not tell "never ran" from
  "ran cleanly"; in these they do, because the scope is known and the answer is bounded.

### An Address That Leads Nowhere Yet

A link is the same kind of promise as a control: that there is a page behind it. An address that
exists only for the owner ends in a 404 or somebody else's sign-in. So an address that is not
published yet is
**written out as text, in the mono face, with a neutral "not yet published" badge beside it** —
never an anchor, never a disabled one. The reader still learns where the thing will be, and
nothing on the page can be clicked into a dead end. An address that is not even decided yet is the badge alone; a
guessed address would be a promise of its own.
Whether an address is published is the service's answer (`published` on each link of
`GET /api/v1/system/about`, one constant per address), not the page's guess, so publishing one
changes one line and every surface follows. An address
that is published is an ordinary link that opens in a new tab with `rel="noopener noreferrer"`.
`SettingsAboutTab` is the reference (RD-130-12).

### Editing a Row in Place

An area that creates entries shows the form beside the list it feeds, and pressing edit on a row
fills that form rather than opening a dialog. `FormListLayout` holds the two columns; the rules
the form obeys live in `useEditableList()`.

Nine components carried this shape before it had a name, and only the DTO and the two endpoint
paths differed between them. What the composable states once:

- **A save is an update when a row is being edited and a create otherwise**, and the saved row
  replaces the edited one in place or is appended. The list never reloads to find out what it
  already knows.
- **A save that failed keeps the form filled.** The entry the reader typed is not thrown away by
  a rejected request.
- **A delete stops the form pointing at the row.** This one is a correctness rule rather than a
  nicety: leave the form on a row the server no longer has and the next save is an update to a
  dead id, so the reader is told the request failed rather than that the thing they were editing
  is gone.
- **A delete is judged by the absence of an error, not by the presence of a response body.**
  `/api/v1/streams/channels/{id}` answers `204 No Content` while every other delete answers with
  a message, and a check for the body reports the first as a failure.

The caller keeps its own form and its own mapping from a row into it, because that is where the
DTO lives, and it keeps any message or per-row spinner of its own.

**The row being edited says so in one of three shapes**, all from `editingRowClass()` in
`utils/editingRow.ts` (RD-1120-15): a row in a box of its own turns its border primary, a row of
a hairline-divided list gets a primary bar on its left, and a row inside a card that already draws
its edges gets a primary outline. The subscriptions list keeps its own: its rows have no box, so
the edited one alone is boxed. **A form's outcome** — the refusal and the confirmation of its
last action — is `FormFeedback`: an error notice with `i-lucide-circle-alert`, a success notice
with `i-lucide-circle-check`, never one without its icon.

**A weekly time window** — from, to, the days as a checkbox group, Monday first — is
`WeekWindowRow` wherever one is edited (quiet hours, reconnect windows, the bandwidth schedule),
with the day names from `common.weekdays`.

**A duplicate is a create, and the copy opens in the form.** Somebody copies an entry because
they want one that is almost the same, so the copy is there to be changed: it is stored through
the list's ordinary create route under a free name — the original's name with `common.copy_suffix`,
counted up on a collision (`useCopyName()`) — and then fills the form exactly as pressing edit on
it would, badge and focus included (RD-150-12). One row button everywhere: `i-lucide-copy-plus`
with its label and a `title`, as the row conventions below ask of a duplicate. What the copy
takes is the entry's settings; what it leaves is what hangs on a relation — the default mark,
rules pointing at the original, counters, history — and every secret. The browser never holds a
secret, so a copy has an empty secret field and the form says it must be entered again. Where
the create route refuses the copy as it stands — a Usenet server whose username needs its
password, a hot folder on a path another one already watches — the form holds the copy as a new
entry that is not stored yet, with the field that has to change named, and the cross drops it.
Something copied to act the same way twice is stored switched off: an automation, a
subscription, a site rule. The original is never written.

### Forms Share One Shape

About sixty forms had grown up one at a time, and a reader moving between them had to look for
the save button: right-aligned in two, behind an *Active* switch in two more, "Cancel" spelled in
nine ways, seven of them not a `<form>` at all, so Enter sent nothing (RD-150-11). What most of
them already did is the standard, and a form that differs is drifting:

1. **The field that decides comes first.** A type, kind, method or mode stands directly above
   every field it governs, answered when the form opens (the interaction rule *What a choice
   governs stands after the choice*). In a form beside a list the type is the first field, before
   the name; a switch that locks a value — *unlimited* above the limit it disables — stands
   before that value, not after it.
2. **One action row, at the form's end, left-aligned.** `FormActions.vue` is the row: the primary
   action first, `type="submit"`, *Create <thing>* with `i-lucide-plus` (or its own icon) while
   creating and *Save* with `i-lucide-save` while editing; then, only while editing, *Cancel
   editing* as an **icon-only** `ghost neutral i-lucide-x` with `aria-label` and `title` — the
   cross is unambiguous and the label cost a line in a narrow column. Anything else a form offers
   (a test, a preview) follows those two. An *Active* switch is a field of the form, never a
   member of the row. Every form is a `<form>` with `@submit.prevent`, so Enter submits.
3. **Feedback above the form, required fields marked by the field.** A failure or a success is a
   `UAlert` between the section header and the `<form>`, in the form's own column — never at the
   end of the page, where the reader who pressed *Save* does not look. A field the server refuses
   empty carries `required` on its `UFormField` (Nuxt UI draws the mark) and on its control;
   optional fields carry nothing. One field per row, the hint in `description` rather than `help`,
   and the edit state follows *Editing a Row in Place*: badge, "Edit …" heading, focus.
4. **A dialog footer ends right-aligned.** The theme's `modal.slots.footer` is `justify-end`, so no
   modal names it; *Cancel* first as `color="neutral" variant="outline"` without an icon, then the
   primary action with its icon.

**Nuxt UI before anything of our own.** A control Nuxt UI offers is taken from Nuxt UI:
`UFormField` with `orientation="horizontal"` for a label-and-switch row rather than a hand-built
flex row, `USwitch label/description` where the switch stands alone, `UCheckboxGroup` for a set
of chips a person toggles (it carries the group semantics a row of buttons lacks),
`URadioGroup legend` rather than a `<p>` above it, `UAccordion` for groups that open, `UFileUpload`
for a file the person picks or drops (`accept` names the extension, not a MIME type a browser
leaves empty), `UColorPicker` in a `UPopover` for a colour with a `UInput` for the exact hex value
beside it (the picker has no keyboard operation), `UTable` for a
table of figures, and a `UButton` as the trigger of a `UCollapsible` (RD-191-02). The small parts
too (RD-1110-13): `UAvatar` with an `icon` for the tile beside a row's name, `UPageCard` with `to`
for a card that is a link, `ULink` for a link, `UBadge` for a label on a picture or a chip, a
`UButton variant="link"` for a name that opens its rename, and a `UFieldGroup` around an input
and the button that acts on it. No second UI
library, and no re-implementation of one of its components. Our own components exist only where
Nuxt UI has no counterpart and the markup would otherwise drift — `FormListLayout`,
`SectionHeader`, `FormActions`, `CopyField` — and they are compositions of Nuxt UI, not
replacements for it.

**Time, date and form validation (owner, 2026-10-06).** A time or a date is `UInputTime` /
`UInputDate`, never the browser's `<input type="time">` or `type="date"` (RD-1120-23). A new form is
a `UForm` with a schema, so its validation and field errors come from Nuxt UI; an existing form is
moved to `UForm` the next time it is changed for another reason, not in a sweep of its own.

**A value to copy is a `CopyField`.** A token shown once, a header, a command, an address to
register at a provider: the value sits in a read-only `UInput` with the copy button joined to it
in a `UFieldGroup`, never as a `<code>` beside a loose button. The field keeps the value
selectable where the browser refuses the clipboard; the button says "Copied" for two seconds
after a copy that worked, a refused one raises `useCopy`'s error toast, and a caller with more to
say — where the value goes next — adds its own toast on `copied` (RD-1110-13).
`web/src/nuxtUiFirst.test.ts` holds this as a ratchet (RD-1110-01): it counts each hand-built
pattern the audit of 2026-10-05 found — framed cards, number fields, dashed empty states, tinted
notices, chevron toggles, sub-section dividers, file inputs and drop zones, trees, icon tiles,
status dots, raw links and buttons, native controls, and since RD-1120-14 button rows that
mark a selection by colour and paragraphs standing in for a `DataState`'s empty state — in every
template and fails on one more than
its `MAX`, and on one fewer until `MAX` is lowered in the same commit. A hand-built control that
stays on purpose enters the test's `ALLOWED` list only with the passage of this document that says
why, by line range and a quoted phrase; where no passage says so yet, it is written here first, or
the control is converted.

### Section Headers

Every section of this interface opens the same way: a small monospaced eyebrow, a heading, and —
when the heading does not say enough on its own — one sentence under it. Until 1.0.9 that block
was written out by hand, 73 times across 49 files, and with nothing owning it, it drifted. The
heading had four spellings, and two of them had lost `font-semibold` while the other fifty-seven
kept it. The description had twelve, mixing `mt-1` with `mt-2`, `text-sm leading-6` with `text-xs
leading-5`, and `max-w-3xl` with `max-w-prose` with no rule behind any of it. This is the same
failure recorded above for the subscriptions list: a visual decision with no home does not stay
decided.

`SectionHeader.vue` owns it now, and offers three levels, which are a hierarchy rather than a
size picker:

- **`page`** — the header of a settings page or a view. An `h2` at `text-xl`, description at
  `text-sm` over `max-w-3xl`. Every settings page has exactly one, with a description, and
  `SettingsHeaders.test.ts` mounts each page to count it (RD-110-29).
- **`card`** — a framed card inside such a tab, and the default. An `h3` at `text-lg`, description
  at `text-sm` over `max-w-prose`.
- **`sub`** — a section inside a card. An `h3` at `text-base`, description at `text-xs` over
  `max-w-prose`.

The description width is fixed per level on purpose: line length is a readability rule, not a
per-site choice. A description that carries markup — a monospaced scope name, a link — goes in the
`description` slot instead of the prop.

Anything a section needs beside its heading — a switch, a badge, a count, a button — stays in the
caller's own flex row. The component is the text block, not the bar it may sit in.

Three shapes deliberately keep their own markup, because they are not section headers: the `h1`
splash heading of the login and wizard screens, the stat tile (eyebrow, a numeric figure, a hint)
— one component, `StatTiles`, for the queue summary, the statistics and the system facts
(RD-1120-15) — and the eyebrow used as a bare label for a value, as the regular-expression editor
does for the pattern it generates.

### Row and List Conventions

A list row is the most repeated piece of interface in this application, and until 1.0.1 each view
had invented its own version. These conventions are not new rules — they are what most of the code
already did, written down so the next view does not have to guess, and so a drifting one can be
recognised as drifting.

- **Row actions** are `size="xs" variant="ghost"`. Edit and delete are icon-only —
  `i-lucide-pencil` in neutral, `i-lucide-trash-2` in `color="error"` — and each carries both an
  `aria-label` and a `title`, since an icon alone names nothing. See `docs/accessibility.md`.
- **An action that is not self-evident gets a label beside its icon**: test, connect, enqueue,
  duplicate. Edit and delete do not need one; a test button does.
- **An action with a start-mode variant offers the variant as a second button, never as a
  hidden modifier.** "Add" and "Add paused" are the pair the LinkGrabber toolbar established;
  a row and a selection bar offering the same action repeat that pair verbatim — same icons
  (`i-lucide-arrow-down-to-line` and `i-lucide-pause`), same labels, the variant in
  `color="neutral" variant="outline"` beside the primary one, and a `title` saying what the
  variant does differently. Both are enabled by exactly the same condition: a control that is
  dead for one kind of row in the list it acts on reads as a broken feature, and that is how
  "add paused" stayed disabled for a LinkGrabber holding only NZBs (RD-107-09).
- **Expanding a row** uses the chevron pair `i-lucide-chevron-right` / `i-lucide-chevron-down`
  with `aria-expanded`, so the control looks like what it does. A row that opens something must
  not be a text button, and a text button must not open a row.
- **Destructive actions go through `useConfirm()`** with `destructive: true` and
  `confirmIcon: 'i-lucide-trash-2'`. This is the Interaction Rule above, stated concretely: the
  rule existed, and the subscriptions list still deleted without asking.
- **A destructive action that reaches past this machine says where it reaches, and is never the
  same button as the local one.** A remote job can end two ways — deleted in the person's account
  at the provider, or cleared out of the list here — and one "delete" doing both is how somebody
  loses a torrent they meant to keep. So they are two buttons: the local one is the ordinary
  icon-only `i-lucide-trash-2` in `color="error"`, and the remote one carries a label beside its
  own icon under the rule two bullets up, because it is not self-evident. Both confirm, and the
  two dialogs are written to be told apart: the remote one names the account it reaches, says the
  change happens outside this machine and cannot be undone, and says what stays behind
  afterwards; the local one says in one line that whatever the provider holds is untouched. The
  confirmation is also a value the request carries, not only a dialog — the server refuses an
  unconfirmed one — so a client that never drew the dialog still deletes nothing.
  `SettingsRemoteJobsCard.vue` is the implementation (RD-108-04).
- **What the interface calls a proposal carries the way out of itself, and only a proposal
  does.** A mirror group built on a shared file name alone already reads differently from one a
  page declared — `5 possible mirrors` rather than `5 mirrors`, a warning colour, a dashed edge,
  the evidence in the title — and a reading that cannot be acted on is a label, not a statement.
  So the proposal's row offers *Ungroup these links* and the other two do not offer it at all:
  absent rather than disabled, because a dead control reads as a broken feature, and because a
  contradiction against a declaration or a matching size is a finding about the *source* rather
  than something one package may click away. The server refuses those two as well, so a client
  that offers the action wrongly still changes nothing (RD-110-34).
- **A confirmation that empties a store says how many records go, and says it before it asks.**
  "41,208 log records will be removed for good" is a question somebody reads; "clear the logs?"
  is one they answer out of habit, and the difference is the whole value of the dialog. So the
  count is fetched before the button is drawn (`GET /api/v1/system/data-reset`), it stands in the
  question rather than only in the answer, and the question also names what *stays* — the other
  stores, the queue, the settings — because a clear button's real cost is the fear that it
  took more than it said. The button is dead while the store is already empty, under the rule
  above about controls that cannot do anything. The answer then reports the number that actually
  went, which is not always the number in the question. Like the remote-job rule above, the
  confirmation is also a value the request carries (`confirmed: true`) and the server refuses an
  unconfirmed one, which is what lets the same action be offered to an agent over MCP.
  `SettingsDataResetButton.vue` is the implementation (RD-120-34). The same button sits at the
  notification history (RD-130-08), where what stays includes the deliveries still queued or
  retrying — a list that is not empty afterwards must not read as a clear that failed. Cancelling
  those is a second button beside it with its own words ("Discard pending", RD-170-11), never an
  option of the clear: its question says the notifications will not be sent. It is absent rather
  than dead while nothing is pending, because in the usual state there is never anything to
  discard and a permanently disabled second button is noise.
- **A menu holds at most one red entry, and it stands apart.** Red marks the entry that throws
  away something nobody gets back by waiting; entries that only tidy up what has already stopped
  are neutral, however much they take. In *Clear list* on Downloads that is *Clear the entire
  list*, the one entry that also stops running work, in a group of its own so a slip from its
  neighbour lands on the separator; *Remove all stopped packages* beside it is not red. Its
  question follows the rule above — how many packages go, how many are still active, what stays
  (finished files) — and carries the second decision, partial files, as an unticked box; the
  request says it was confirmed and the server refuses it otherwise. `clearItems` in
  `DownloadsView.vue` and `ClearEverythingModal.vue` are the implementation (RD-180-21).
- **An action that deletes nothing but cannot be undone asks once, without the destructive
  styling.** `useConfirm()` with the action's own icon and no `destructive: true` — the red
  button and the bin say "this is gone", and saying that about a grouping that leaves every link
  in the list is a lie that teaches people to click past the real one. The question names what
  stays (the links) and what goes (the grouping), and says the decision holds afterwards.
- **A settings list of cards pins each card's actions to its footer.** Where the entries of a
  settings list are cards side by side — the installed plugins are the reference,
  `PluginCard.vue` (RD-180-22) — each is a `UCard`: who the entry is in `#header` (icon, name,
  badges in one row, one muted meta line), what it holds as labelled rows in the body (a small
  uppercase label in a fixed left column, the values beside it), and the actions on the whole
  entry in `#footer`, the destructive one left in `color="error"`, the others right with the
  most used last. The root is a flex column and the body takes the free height, so cards in a
  grid row are equally tall and every footer sits at the same place: buttons that move with the
  content above them are hunted for, not found. A per-row action inside the body (a superseded
  version) stays an icon-only row action under the rules above.
- **A switch is the control for a boolean that takes effect immediately** — enabling an account,
  a subscription, a plugin step. A coloured dot is a status, not a control; if the value can be
  changed, it is a switch.
- **Inside a dialog, a switch may instead arm something the Save then does** — "remove the stored
  password", "rename the folder too". Two rules keep that from reading as a setting: it is
  disabled while it would do nothing (the folder switch is dead until the name actually changes),
  and when it is on, the dialog states in one line what Save will do beyond writing a field. A
  switch that moves data and says nothing is how somebody finds their files somewhere else.
- **State is named, not implied by colour alone.** A badge carries text; colour reinforces it.
- **A status dot is a `UChip standalone`, and only ever the colour beside a word.** The row names
  the state — a "Disabled" badge, the switch, "Expired" under a token — and the dot repeats it:
  green for on, red for expired, none for off. A dot that would be the only carrier of a state is
  a `UBadge` with the word instead (RD-1110-13).
- **A state nothing can be done from says why, and is the only kind that reads as an error.**
  Every other unhappy candidate state means *nothing good is known yet* — the hoster reports the
  file as gone, no account could check it, the check itself failed — and all of them stay
  selectable, because the person may still be right. `unresolvable` is the one exception: the
  address was reached and answered with a page rather than a file, so starting it could only
  store somebody's error page under a file's name. It is the one candidate state that is not
  queueable, it wears the same colour as a confirmed failure rather than the amber of a caveat,
  and its badge carries the reason in its title — a greyed-out checkbox with no sentence beside
  it is a dead end the reader has to guess at (RD-110-07).
- **A state that is a relation names the other end of it.** `duplicate` said only `duplicate` for
  a year: two links of two different hosters carrying it read as a collision in the dedup key,
  and nothing on screen said the address had simply been added before. A badge whose word only
  makes sense against something else — a duplicate *of* what, blocked *by* what — spells that out
  in the label where it fits and in its title where it does not (RD-109-43).
- **What the application did not resolve itself is shown as a chip that names its source.** A
  field a plugin contributed — an enricher's rating, a look-up from a service outside this
  machine — sits in its own wrapping row below the row it belongs to, as a monospaced badge
  reading `<label>: <value>`, with a tooltip naming the full field, the plugin and the time it
  answered. The label is the field name's last segment, translated where the catalogue knows it
  (`common.enrichment.fields`) and printed as it is where it does not. Never merged into the
  row's own text: a value somebody else supplied has to stay recognisable as somebody else's
  answer, and a stale one as stale. The LinkGrabber candidate row and the download-list package
  header use the one `EnrichmentChips` component for exactly this, and each of the two views
  carries a "Show metadata" `USwitch` at the right end of its toolbar that hides the row in this
  browser without touching the stored fields (RD-150-19).
- **A list that shows part of itself says which part, and how much it is holding back.** Where
  the default is not "everything" — the subscription archive keeps the hits a filter rejected,
  because an unwritten one would be rediscovered on every poll, and defaults to the accepted
  ones — the control that changes it sits above the list and the number of hidden rows is
  stated beside it. A default that quietly narrows a list is how a working filter gets reported
  as broken. The same applies to a boundary the list cannot show past: a fetch that returned a
  full page says so, rather than letting "nothing matched" stand for "nothing was asked for".
- **A log row is read left to right in the order a person triages: when, how bad, where, what
  code, which job, what happened.** Time in tabular figures, the level as a named badge whose
  colour only reinforces the word, component and correlation id in the monospaced face, the
  stable code as an outlined chip, and the message last because it is the one cell that wraps.
  Anything the event carried beyond that — the other fields — sits behind the chevron pair with
  `aria-expanded`, never inline: a row that showed six key-value pairs beside its message would
  be six lines tall for the one record in fifty that anyone opens. The filter bar stands above
  the list, the retention in force is stated beside the heading, and a full page says so under
  the list with the control that fetches the page behind it (`LogsView.vue`, RD-110-02).
- **A count beside a heading counts the things the list is about, not the rows it happens to
  draw.** The plugin view counted installed version directories and reported 44 for a package of
  43 plugins, because an upgrade leaves the version it replaced in place and that leftover stood
  as a row of its own (RD-108-10). Where one thing can be present more than once, the extra
  copies belong under the thing they are copies of — a collapsed sub-entry inside its row, with
  the chevron pair and `aria-expanded` like any other expansion — and the count stays a count of
  things. The number is read against something outside the screen: how many plugins were
  installed, how many accounts exist. A count that answers a different question than the one it
  looks like it answers is a bug report waiting to be written.
- **A filter with more groups than a line holds is a wrapping chip row, not a tab bar.** A tab
  bar divides one line among its entries, so every entry added shortens every label — and the
  plugin manager's type bar reached twelve entries with the eleventh plugin world, `remote-job`,
  at which point ten of them read "Benachrich… 3" and "Ordner-Cr… 6" and nobody could tell what
  the groups were (RD-107-17). That is growth, not a defect, so the answer is not a wider row but
  a shape that grows with the number: a `URadioGroup variant="card" indicator="hidden"
  orientation="horizontal" size="xs"`, whose card fieldset wraps, one card per group carrying the
  full name and the group's count in a badge beside it (the `#label` slot), the chosen one tinted
  in `primary`. A settings page already scrolls, so height is free where
  width is not, and the row behaves the same at 400 px as at 1600 px — only the number of lines
  changes. The two obvious alternatives were weighed and rejected for stated reasons: **a select
  shows one count at a time**, and the counts are the whole reason the control is worth having —
  they answer "how many resolvers are installed" before the choice is made, not after it; **a
  side list costs a column** at exactly the widths where the cards it filters need two, and the
  split beside a list is already spoken for by the form rule below. The group has an
  `aria-label` and every card is a radio Reka checks, because a selection stated only by fill
  colour is the "state is named, not implied by colour alone" rule broken again. A tab bar
  remains right for a handful of stable sections — Routing's four, the wizard's steps; the chip
  row is for a set whose size follows what is installed. Reference implementation:
  `SettingsPluginsTab.vue`.
- **One value out of a handful is the same radio row, everywhere.** The torrent detail views,
  the media presets, the SponsorBlock and subtitle modes, the statistics ranges and the two
  category filters were seven rows of buttons in three looks, four of them telling the chosen
  value by colour alone (RD-1120-14). All seven are now the chip row above — a `URadioGroup`
  with the theme's compact card (`radioGroup` in `web/src/uiTheme.ts`) and an `aria-label` — so
  a screen reader hears "radio, checked" and every row looks alike. A value that cannot be
  chosen right now is a `disabled` item that keeps its place, its reason in a `title` on the
  label. The guard test counts a `UButton` whose `:variant` follows a comparison of two values
  as `toggle-group`; a button coloured for one fixed case is not one.
- **A persistent review archive pages and filters at its source.** The subscription archive may
  outlive the process and grow past what a browser should receive at once. It starts on the set
  requiring a decision, shows totals for every state, and asks the server for 50 stable rows at a
  time. A bulk decision names the total and acts on one server-side snapshot across every page;
  it is never implemented by looping only over the rows currently rendered.
- **A list of typed steps is rows of named fields, and where the order is meaning it can be
  changed.** An automation's actions established the row: a kind select on the left, the fields
  that kind needs beside it, one `i-lucide-x` in `color="error"` to remove it, and a
  `size="xs" variant="soft"` add button under the list. A site rule's steps take the same row
  and add the one thing actions do not need — the steps run in sequence and read each other's
  variables, so the row also carries `i-lucide-chevron-up` and `i-lucide-chevron-down`, each
  with an `aria-label` and a `title`, disabled at the ends rather than absent there. The
  position is printed in front of the kind in monospace, because "step 3" is how the refusal
  that follows will name it. Changing the kind clears the fields the old kind owned instead of
  carrying them over into a shape they do not fit; what survives is the variable the step
  writes into, which is the one field every kind has. There is **no free-text JSON box** as the
  only way in — that is the JDownloader rule editor this pattern was written against, and it is
  where people stop (RD-110-08).
- **Something with a shape a person is guessing at gets a trial run before it is saved.** A site
  rule, a regular expression, a connection: the control is a labelled `i-lucide-flask-conical`
  button under the form, enabled only once the form carries what the service would need, and its
  result appears beneath it rather than as a toast — a toast is gone before the third link has
  been read. The result states what was produced *and* what the application will make of it:
  the links, the name that was read, the cost in requests, and per row the same verdict the real
  path applies, named in text with colour behind it. It never saves anything as a side effect,
  and it says which address it actually reached, which is not always the one that was typed.
- **An image in a row is decoration, and behaves like it.** A thumbnail is `size-12`, square,
  `object-cover`, with `bg-elevated` standing in until it loads and `loading="lazy"` so a long
  list does not fetch what nobody scrolled to. Its `alt` is empty: the row's text already names
  the thing, and a screen reader repeating the title as an image label is noise. An `@error`
  handler hides it, because a dead address must leave a gap rather than a broken-image glyph.
- **An overlay that only one row may have does not fade out.** Where neighbouring rows each
  carry their own overlay and the open one is decided globally, the leaving row's content is
  still on screen while the arriving row's is drawn — and two enlarged pictures a thumbnail's
  height apart read as one picture that failed to change. The closed state gets no animation,
  so the content goes in the same tick the row loses the slot. The picture inside carries a
  key on its address as well: an `img` keeps the pixels it has until a new address decodes.
- **A row without its picture keeps the picture's place.** A list where some rows carry a
  cover and others do not starts its titles in two different columns, and that is what makes a
  long list read as restless. Where a thumbnail can appear, its absence is filled by a
  placeholder of the same `size-12` on `bg-elevated`, carrying the application's own mark
  (`favicon.svg`) as a background, `aria-hidden` and without an `alt`: it is decoration
  standing in for decoration, and it must never look like a picture of the thing itself. It is
  one component, `CoverPlaceholder.vue`, wherever covers are drawn — the subscription hits and
  the indexer search's detailed rows — and stands in as well for a cover the person's setting
  does not load or whose address failed.
- **A picture the row is a decision about is enlarged in the row, not in the detail below.**
  This reverses what this document said until 1.0.6 — *"a larger version belongs in the
  expanded detail, never in the row"* — and the reversal is deliberate rather than a drift.
  The old rule was written for decoration and still holds for decoration. A cover on an
  indexer hit is not decoration: it is the fastest answer to "is this the film I meant", and
  at `size-12` it answers nothing — so the picture stood twice, uselessly small where the
  choice is made and usefully large behind a chevron nobody opens to look at a picture. The
  large copy now hangs off the thumbnail itself in a portalled popover, so it neither moves the
  list nor gets clipped by an accordion. It opens without a deliberate delay, is bounded to about
  512 px wide and to the window's actual free height, and uses collision padding to remain
  visible. A bounded, visible server page may preload these thumbnails; a virtual list still
  loads distant decoration lazily. The copy in the detail is gone.
  **The popover stays anchored to its row; the height it may use is read from the popover, not
  guessed from the viewport.** A cover that overran two thirds of the window height regardless of
  how tall the window actually was was reported as leaving a third of it unused. The fix is not a
  larger fixed percentage - a static number is always either too cautious on a tall window or, for
  a row near the bottom, too generous for what the row's own position leaves free. Reka's popper
  already answers that question itself: `align: 'start'` extends the popover downward from a row
  near the top, but flips to `align: 'end'` and extends upward instead for a row near the bottom
  (`side` flips the same way if there is no room to the trigger's preferred side), and once that
  placement is settled it publishes the space actually left over as the
  `--reka-popover-content-available-height`/`-width` custom properties. The image's `max-height`
  is bound to that value (minus what the content wrapper's own `p-1` padding and border take back
  out of it), so it fills whatever the row's true position leaves, at both ends of the list,
  without the popover ever needing to detach from the row and re-center in the window. Detaching
  would have gained a little more image for the cost of the one thing this pattern exists for -
  the visible line back to the row the picture is answering "is this the one" for - and the
  dynamic bound reaches the same "as tall as the window allows" outcome without paying it. The
  width stays independently capped at `min(32rem, 100vw - 2rem)`, because a landscape cover has no
  reason to grow just because the popover's placement happens to leave more room to one side.
  **Hover is the pointer's way in and never the only one.** The trigger is a `<button>` with
  a name and `aria-expanded`; the pointer opens it, keyboard focus opens it, a tap opens and
  pins it, `Escape` closes it, and focus never leaves the button, so there is none to hand
  back. A touchscreen has no hover at all, and an enlargement hung on `:hover` alone would
  simply not exist for the people using one (`docs/accessibility.md`, WCAG 2.1.1 and 1.4.13).
  **Exactly one enlarged cover stands open, anywhere on screen.** Rows are drawn in two
  places and a windowed list creates and destroys them while scrolling, so no row and no
  ancestor can decide this: one owner outside the component tree holds which row is open, and
  a row that opens takes that slot from whoever had it. A row only ever gives the slot up
  while it still holds it, which is what makes the `mouseleave` that arrives after the next
  row's `mouseenter` harmless instead of a flicker.
  **A pin outlasts a pointer passing by; it is covered, not discarded.** Pinning is a stated
  decision and crossing the list with a pointer is not, but only one cover may show — so the
  two are kept apart rather than ranked. A pointer or keyboard focus borrows the single slot
  for exactly as long as it lasts, and when it ends the pinned cover is shown again. Only
  another deliberate act takes a pin away: pinning a different row, a second tap on the
  pinned one, `Escape`, or a click outside. The alternative — letting a pass-by clear the pin
  — is simpler to build and loses the person's decision to an accident of where the pointer
  travelled; freezing hover for as long as something is pinned keeps the decision but reads
  as a list that stopped responding.
  **The switch for third-party pictures governs the large one too:** with image loading off
  there is no thumbnail, so there is nothing to enlarge and nothing fetched.
- **A review list that grows on its own belongs in a drawer, not in the page.** The
  LinkGrabber's indexer hits are a second list sitting above the one the screen is named after.
  As a panel in the page it unfolded itself when there were hits and grew with every hit that
  arrived afterwards, so the rows underneath moved while somebody was reading them. It is now a
  one-line header — title, the count as a badge, the hint — with a button that opens a drawer in
  scale-background mode; the groups live in the drawer. Three rules come with that:
  - **Nothing opens itself.** Not on load, not when a poll finds something. The badge is what
    says there is work; opening it is the reader's decision. This reverses RD-106-18, which had
    the panel unfold on hits — right for a small box, wrong for something worth reading.
  - **Asking for a check opens it.** "Check all" is a question about what the indexers will
    find, and the answer is in the drawer, so that one button opens it and loads it. A page that
    merely loaded is not asking anything.
  - **The scale-background effect needs its wrapper.** `should-scale-background` only works if
    an ancestor carries `data-vaul-drawer-wrapper`; in this application that is the element
    around the whole control-room layout in `App.vue`. A drawer added elsewhere inherits it.

  Inside the drawer the older rules still hold: individual subscriptions start closed and fetch
  their first page on demand, the person's manual open/closed state wins over any event, and a
  zero total carries neither an undecided-work label nor a decorative zero badge.
- **A subscription's hits may be cards instead of rows, and the subscription decides
  (RD-120-37).** The review drawer draws each group the way its own subscription asks — `list`,
  the default and what every subscription showed before the choice existed, or `cards`, a slider.
  Checked against the rules above before it was built, and it keeps them rather than adding
  exceptions:
  - **Same facts, one reading.** Card, row and details take every value from
    `web/src/utils/subscriptionHit.ts`; a second derivation for the card would drift from the row
    the way the two hit rows once did (RD-101-17). What the indexer did not send is absent from
    the card, never a placeholder. The name is `tvtitle`/`imdbtitle`, artist and album, or the
    head of the release name up to its episode, season, year or resolution token; the release
    group is `team` or the scene suffix `-GROUP`, and a title with spaces has none.
  - **Same actions, same confirmations.** Queue and dismiss are one component
    (`SubscriptionItemActions.vue`) in both views, and the header with *Queue all* / *Dismiss all*
    and their confirmations is the group's, not the view's — one component
    (`IndexerReviewBulkActions.vue`) in the header and again in the footer under every open group
    with hits, whether or not it has pages (RD-1101-01). The card's *Details* opens **one**
    panel under the slider with the same `SubscriptionItemDetails` the row expands.
  - **One height.** Each band — release name, subtitle, chips, actions — keeps its size with or
    without content (the text part is a fixed `12rem`), so a card without cover, size or group
    does not make the slider jump.
  - **The release name is the heading, and nothing on the card says a thing twice (RD-130-13).**
    A short name used to stand above it — "The Big Bang Theory" over
    `The.Big.Bang.Theory.2007.S11E23…` — repeating the release name's own head and making every
    card a line taller. The name still labels the card for a screen reader and gives the tile its
    initials; on the card, the release name is it.
  - **The slider is the paging; it holds every hit (RD-130-13).** A pagination bar under a
    carousel is two ways of paging stacked on each other, so a card group has none — the list
    keeps its bar. The slider counts all the subscription's hits, reads them fifty at a time
    while the page shown or the next one reaches past what it has, and holds a pulsing place for
    a card still on its way. Only the current page is rendered, so a long archive costs a longer
    array, not more cards — which is also why it is no `UCarousel`, whose Embla track keeps every
    slide in the DOM. Past ten pages the dots give way to a "Page 3 of 40" counter. Cards are
    at least 15 rem wide: a narrow window shows fewer per page, never thinner ones.
  - **The slider ends where the hits end (RD-1101-01).** It never wraps, in either direction and
    by no input — arrows, arrow keys, swipe or autoplay: a slider that went on from the first hit
    after the last hid that the list was over. Previous is disabled on the first page, next on
    the last page of the subscription's whole total, not of the hits read so far, so the end is
    the true end; a hit that arrives later adds a page and enables next again.
  - **The picture has the subscription's ratio, not a height (RD-120-42).** Five ratios, chosen
    per subscription in the form beside *View* and *Autoplay* and offered only for cards: **1:1**
    (square covers), **3:2**, **16:9**, **4:3** and **2:1**, the default — the closest to the
    fixed picture height every card had before the choice existed, so an existing subscription
    looks as it did. The picture area is CSS `aspect-ratio` over the card's full width; the cover
    is positioned out of the flow with `object-cover`, so it fills and crops the area and a large
    picture can never stretch it, and the initials tile or symbol fills the same area. All cards
    of one slider share one width and one ratio, so they keep one height; a narrow window keeps
    the ratio and shows fewer cards rather than squashing them. The five spellings are the ratios
    themselves, identical in every language; only 2:1 carries a translated "(default)".
  - **A picture that is not there is a tile, not a gap.** Without a cover — or with third-party
    pictures switched off, which governs cards exactly as it governs rows — the picture band is a
    colour with up to three initials (`The Big Bang Theory` → `BBT`, a leading article skipped),
    or a symbol by category (a note for Newznab 3xxx, an arrow otherwise) when the name has no
    letter to take. The colour is FNV-1a over the name into a fixed palette of eight colours dark
    enough for white initials, so the same series is always the same colour.
  - **Operable without a pointer.** The track is focusable and turns pages with the arrow keys,
    with a visible focus ring; the arrows and the page dots are buttons with names ("Page 2 of 3"),
    the current dot carries `aria-current`; a horizontal swipe of 50 px turns a page (`touch-action:
    pan-y` keeps vertical scrolling). The slider is a labelled region with
    `aria-roledescription` "carousel", each card a "card".
  - **Autoplay never takes the page away (WCAG 2.2.2).** A second per-subscription option, off by
    default and offered in the form only for cards. One fixed interval of **6 seconds**
    (`CARD_AUTOPLAY_MS`), no setting for it. It stops on the last page. It has a
    visible pause/resume control, and it holds while the pointer is over the slider, while
    anything in it except that control has focus, while the details panel is open and while the
    tab is hidden; a page turned by hand restarts the interval. Under `prefers-reduced-motion:
    reduce` it does not run and the control is not shown. While it runs the track is
    `aria-live="off"`, otherwise `polite`.
- **An action whose result arrives later says so twice.** A row action the server accepts
  before it has run — "check now" on a subscription — marks its own control busy for the
  length of the request, and says in the view's notice line that it was started, in words that
  do not claim it finished. The end arrives through the event stream and replaces that line
  with what actually happened. The busy state is per row, never global: checking one
  subscription must not lock its neighbours' buttons. And it ends with the request rather than
  with the work, because a control that waits for an event which may never arrive is a control
  that never comes back — the notice, not the button, carries the rest of the story.
- **A long list windows itself, and says how long it is.** Past sixty rows the queue and the
  LinkGrabber render only what is near the viewport, through one shared block
  (`VirtualRowList.vue`). Three rules come with it, and they are the pattern rather than the
  optimisation:
  - **A tree is flattened before it is windowed.** Both lists are packages with children, and a
    window can only slide over a flat sequence. The view builds one stream of rows with stable
    keys — the package header, then its children while it is open — and collapsing is a filter
    on that stream, not a `v-if` inside a package. The key survives a reorder, which is what
    lets the focus and the selection survive it too.
  - **The frame belongs to the rows, not to a box around them.** A package that no longer owns
    its children cannot draw a border around them either: the header drops its bottom edge while
    it is open and each child row carries the sides and the separator onward. The gap between
    packages is padding on the row wrapper rather than a margin, so what is measured is what is
    there.
  - **A row with focus is never taken out, and the list can be asked for a row.** Removing the
    focused row hands the focus to the document body, which is where a keyboard reorder ends. So
    the focused row is pinned and stays rendered wherever the window is, a keyboard move puts
    the focus back on the same handle afterwards, and both views offer a way back to a selected
    row — opening its package first if it is collapsed. Length is announced rather than counted:
    `role="list"` with a name, `aria-setsize` and `aria-posinset` per row
    (`docs/accessibility.md`).

  Below the threshold none of this is switched on: the list renders whole and has no scroll
  viewport of its own, so an ordinary queue looks exactly as it did.

  **Why it is no `UScrollArea virtualize`** (checked against Nuxt UI 4.11, RD-1120-14). Nuxt UI's
  virtualizer is TanStack's, and the pinned rows would fit its `rangeExtractor`, but three of the
  rules above would not hold: it decides once, when the component is set up, whether it
  virtualizes at all, so a queue that grows past sixty rows could not switch the window on, nor a
  short one stay without a scroll viewport; it places each row absolutely by a transform, so the
  rows leave the `role="list"` flow the wrapper names with `aria-setsize` and `aria-posinset`; and
  in jsdom its viewport measures zero and it renders no row, where `useVirtualRows` falls back to a
  viewport height and the component tests see every row of a short list. The own block stays
  until Nuxt UI's can be switched on and off with the length.
- **A list with row checkboxes selects a range with Shift+click, like a file manager.** A plain
  or Ctrl/Cmd click toggles one row and sets the anchor; Shift+click — or Shift+Space on a
  focused checkbox — sets every row from the anchor to the clicked one to the state the clicked
  row now has, in the order the rows are on screen. A group row (package, folder) at either end
  brings its whole content along, as its own checkbox does; in the middle only while it is
  collapsed, because an open group's rows are in the range themselves. The anchor is a row key
  that is dropped when its row leaves the list, so a list that changed never selects the wrong
  rows. `UCheckbox` stays: it reports only its new value, so the list notes the modifier in the
  capture phase of click and keydown (`useRangeSelection`, RD-170-13). A new list with row
  checkboxes and multi-select uses the same composable.
- **The status bar says how much is selected.** While the LinkGrabber or the queue has rows
  ticked, the transfer rail shows the count and the summed size — `3 selected · 5.4 GiB` — and
  nothing when the selection is empty or the view is left. The sum runs over the selected
  files, links and NZB imports only; a package checkbox selects those rows, so a ticked
  package and its ticked children count once. A size nobody knows yet stays out of the sum,
  which is then a lower bound written `≥ 5.4 GiB`, with the number of unknown sizes in the
  tooltip; with no size known there is only the count. On a narrow rail the word goes and the
  count and size stay. The view keeps its selection and publishes only the summary
  (`usePublishedSelection`, RD-170-14); another list with sizes publishes the same way.
- **The status bar sets how many downloads run at once** (RD-1120-22; owner, 2026-10-06). Beside
  "N running" a compact `UInputNumber` (1–32, whole, plus and minus) behind a *max* badge holds
  `max_active_files`; *Apply* or Enter saves it through the same settings write as the speed limit
  beside it (`writeSettings` in `stores/transfersSpeedLimit.ts`), a refusal is an error toast with
  the service's message, and the field goes back to the stored value. The scheduler takes it on
  its next pass; a lowered value starts nothing new until fewer run, it stops none. The bar
  follows what the settings page loads and saves. Below a 56 rem rail the control folds away, as
  the version does, so the rail never takes a second line.
- **A list that mixes two kinds of row orders them in one sequence, or it does not order them
  at all.** The LinkGrabber shows collector packages beside NZB imports. Packages carried a
  manual position and a handle; imports carried neither and were interleaved by creation time,
  so one row could be dragged and the row beneath it could not, for a reason no one looking at
  the screen could see. Both kinds now draw their position from a single sequence, and a reorder
  names the rows it moves together with the row they are placed behind — so a drag costs the
  same whether it happens at the top of the list or four hundred rows down. Two independent
  position sequences cannot interleave; if a second kind ever joins such a list, it joins the
  sequence rather than getting one of its own.
- **A drag starts at the handle, and only there.** The handle is
  `i-lucide-grip-vertical` in `text-muted`, the first cell of the row, `cursor-grab select-none`,
  and it is a `<button>` carrying `draggable="true"` together with a `title` and an `aria-label`
  that name both the drag and the keys — one component, `DragHandle.vue`, for every list that
  reorders (RD-1120-14). The row itself is never `draggable`: a candidate row
  that was made it turned every drag across the file name into a reorder and left no way to
  select the text, while the grip beside it — the one thing that looks like an anchor — did
  nothing. The row keeps `@dragover.prevent` and `@drop.prevent` so it can be a target, and a
  nested row stops the drop from also reaching the package around it.
- **Every drag has a keyboard equivalent.** The handle is focusable and answers `ArrowUp` and
  `ArrowDown` by moving its row one step. A reordering reachable only with a pointer is not
  reachable at all for part of the audience; see `docs/accessibility.md`.
- **A refused drag says why, where the list already speaks.** Reordering is refused when the
  visible list is not the whole list — a filter is active — or when the target is out of bounds,
  such as another priority tier in the download queue. The answer is the view's notice line
  (`transfers.notice`, the LinkGrabber's `notice`), not a toast and never a silent `return`:
  a gesture that visibly does nothing reads as a broken feature.
- **Pictures from a third party are a switch, and the row works without them.** An address that
  came from an indexer or a hoster is loaded by the browser, which tells that server what is on
  somebody's screen. That is a decision worth being able to reverse, so it hangs off a setting;
  and everything the row says in words stays visible when the setting is off.

- **Links that are the same file are one row, and the row says how sure that is.** A release
  page offers one episode in three qualities at five hosters. As fifteen rows the review is
  fifteen decisions about one thing, and fourteen of them end in a delete. So a mirror group
  (RD-110-18) draws as **one row — the chosen mirror's ordinary link row, on the same grid,
  with the same enqueue and the same dots** — and the other members hang under it behind the
  chevron pair with `aria-expanded`, exactly as any other expansion. Three rules come with it
  (RD-110-19):
  - **The badge in the name cell names the group *and* how it was formed, in words.** The three
    sources are not equally strong and the row is where that difference has to survive: the
    page *declared* these five links one file, two links *agreed on name and size* after the
    check, or two links merely *share a name*. The third is a proposal, so its badge does not
    only change colour — it changes the noun: `5 mirrors` against `5 possible mirrors`, in
    `warning` with a dashed outline and `i-lucide-circle-help`, and its title says what the
    evidence actually was and that it may be wrong. Colour alone would make a guess look like a
    fact to anybody who does not see the difference, which is the "state is named, not implied
    by colour alone" rule applied to certainty rather than to state.
  - **A member row is not a second candidate.** It carries no checkbox, no handle and no
    enqueue: queueing, selecting and reordering belong to the group, which is the one thing that
    will be downloaded. What it carries is its own state, its quality, its language, its hoster,
    its size, and the one action the expansion exists for — **Use this mirror**, labelled,
    because it is not self-evident. Choosing is a *pin*: it survives a regroup and the standing
    preference no longer moves that group, and the group row says so with
    `i-lucide-pin`, so a choice somebody made is never silently revised. The pin is released from
    the same menu.
  - **A group whose mirrors are all offline is still one row, and still queueable.** It wears the
    `offline` state its chosen mirror wears and the group badge says how many of its mirrors are
    gone (`0 of 5 online`), because that is the number the reader is deciding on. It is not made
    unselectable: every unhappy candidate state except `unresolvable` means *nothing good is
    known yet*, the person may still be right, and RD-101-06 holds for a group exactly as it
    holds for a link.
- **Facets over such a list choose before they hide.** Quality, language and hoster sit in the
  LinkGrabber toolbar as three selects beside the state filter, combine freely, and act on the
  whole list — but "only 1080p" is not the same act on a group as on a lone link, and treating it
  as one is how a filter starts lying. Where there is a choice, the facet *makes* it: a group
  holding a 1080p mirror shows that mirror as its chosen one, and the group stays whole. Where
  there is no choice — a lone link, or a group in which nothing matches — the facet hides the
  row. A list narrowed this way says so under the existing rule ("a list that shows part of
  itself says which part"): the toolbar prints the visible count against the total.
  Two consequences are deliberate. The facets are **the standing preference**, not a view state:
  they are stored on the server, they decide the chosen mirror of the next package that arrives
  and of every package after a restart, and a pinned group is the per-package way out of them. And
  because they decide what the queue will fetch rather than what the screen shows, changing one is
  a request, not a `computed` — the row that moves is the row the download will use.
- **Hiding hosters is a chip row of switches, and the list says what it holds back.** The hoster
  facet shows *one* hoster; hiding takes several out, which a select cannot say. So the
  LinkGrabber carries the wrapping chip row of the plugin manager above its list — one `xs`
  button per hoster with its link count in a badge — but as switches rather than one choice:
  a shown hoster is outlined with `i-lucide-eye` and `aria-pressed="true"`, a hidden one is a
  ghost with `i-lucide-eye-off`, its name struck through and `aria-pressed="false"`, and each
  title says what a click will do. The row appears once a second hoster is in the list and stays
  while anything is hidden. Under it, only while something is hidden, one line counts the links
  and the hosters they belong to and offers **Show all** — the "a list that shows part of itself
  says which part" rule with its way back beside it. The same act sits in a link's menu as "Hide
  links of <hoster>", where the hoster is noticed. It is a standing decision like the facets,
  stored on the server with them (RD-130-21), and it follows their rule for groups: a group stays
  while any member is at a shown hoster, its hidden members are its fallbacks and go to the queue
  with it, and the chosen mirror is never a hidden one while a shown one exists. What is hidden is
  neither queued nor checked, and "Clear filters" leaves it alone — it has its own way back.
- **A form that creates entries stands beside the list it feeds, never above it.** Until 1.0.6
  every settings area stacked the two, and pressing a row's pencil filled a form whose heading
  had already scrolled away — the list looked unchanged and the edit went unnoticed. The form is
  the left column, one field per row; the list is the right column, with its count beside its
  own heading; below `lg` they stack again, form first. Three signals name the row being
  edited, because one heading somewhere else is not enough: the row is outlined in `primary` and
  carries a badge that says "Editing" (a colour alone names nothing, see above), the form heading
  reads "Edit …" rather than "New …", and the focus moves into the form's first field — on a
  narrow screen that is also what scrolls it into view. `FormListLayout.vue` holds the columns
  and the list heading; `useFormFocus()` is the focus move; the breakpoint lives in the layout
  and nowhere else, so an area that needs a different split changes it for all of them.

  **One field per row governs the inside of that column too, not only the split between the two
  columns.** This was the reading that had been left implicit, and the site-rule editor took the
  other one: it kept the column and then put three `sm:grid-cols-2` grids inside it, pairing
  identifier with name, group with revision, path patterns with former hosts and the probe
  address with a date (RD-120-21). Labels and their hints are not the same length, so paired
  fields sit at different heights, and the eye is asked to jump between two columns whose entries
  have nothing to do with each other — inside a column that is already half the screen wide. A
  grid in the form column needs a reason that is about the fields themselves, such as a row of
  values that are read together and are genuinely alike; "there is room" is not one.

  **It is not the form column that makes the rule, it is the fields.** The same pairing turned up
  where there is no form and no list: `settings/interface` put language, theme, byte scale and
  byte unit into one `sm:grid-cols-2` grid, and `settings/unattended` paired two quiet-hours
  switches, the completion action with its countdown and two power-context switches — with a
  `sm:col-span-2` on the approval switch, which is a grid admitting in its own markup that the
  thing did not fit (RD-120-27). So the rule is wider than the column it was written for: **one
  setting per row on a settings page**, whatever holds it, and a `col-span` whose only job is to
  escape a grid goes with the grid.

  **What justifies a two-column grid** is a relationship the reader already has in mind — never
  free width. Three cases earn it and are meant to stay. The grid splits the *page* into two
  subjects rather than a field list into pairs, as `settings/security` does. The cells are alike
  and read together, so the second column is the reading order and not a pairing of strangers:
  a set of generated recovery codes, a row of figures in the same unit and shape. Or the cells
  are not settings at all but a display of values, like the system page's figures row. A label
  with a hint above a control is none of the three: it is a setting, and it gets its own row.

#### What a queue row may spend its width on

The download queue's package header had accumulated, in one line: the package name, a password up
to 30 characters, two badges spelling out `Complete` and `Extracted`, a file counter, a progress
bar with `100%` printed beside it, a byte total, a category select, a spelled-out priority select
and seven loose icon buttons. In a 1280 px window the name column was left **68 px** and the name
rendered as `Lieblin…` — the one thing the row exists to say was the one thing that could not be
read, and the only way to learn it was to hover. Below 1189 px it was worse than illegible: the
grid overran its container and drew text over text.

That is not seven separate mistakes. It is the absence of a rule, so here is the rule: **the name
is the row's first obligation, and everything else has to earn the width it takes from it.** A
cell earns it by saying something the reader cannot get from the row's other cells, from the rows
underneath, or from a glyph. Concretely (RD-109-30):

- **A value that has stopped being useful leaves the row.** The archive password is worth 30
  characters exactly while it still opens something; once the unpack has succeeded it opens
  nothing and goes. While the unpack is outstanding or has failed it stays, because that is
  precisely when somebody reads it.
- **A state with one unambiguous meaning is a glyph, and the word it dropped becomes its
  accessible name.** `Complete` and `Extracted` are each one idea with one icon, so the badge
  carries `i-lucide-circle-check` / `i-lucide-package-open` with the word as `aria-label` and the
  fuller sentence as `title`. This does not weaken "state is named, not implied by colour alone"
  above — the name is still there, it is simply not rendered. A state that needs a qualifier
  (`Post-processing 40%`, `Extraction failed`) keeps its text, because there is no one glyph for
  it.
- **An ordered set of three or fewer levels is a glyph too.** Priority is an arrow up, a dash and
  an arrow down on a `size="xs" variant="ghost"` button whose name reads `Priority: <level>`; the
  levels live in a dropdown beside it and each keeps its own label. A select that spells out
  "Normal" spends 96 px stating the default.
- **A bar at 100% carries no number beside it.** The bar is already the statement; the `100%` is
  the same statement a second time, and it is the only width the bar can give back.
- **At most two controls stand outside the row's menu, and the second one has to be the control
  somebody reaches for while the work is running.** The file rows have had exactly one — the dots
  — since RD-106-12, and the package header now reads like its own children: the package's
  start/stop button, because it acts on every file at once and waiting for a menu to stop a
  running download is the wrong shape, and the dots. Copy path, extract, force-extract, segments,
  post-processing, edit and delete are deliberate acts that can afford a menu, and they keep the
  labels they carried as `aria-label`s. A menu item that had a tooltip keeps it as the item's
  `description`, which is what `UDropdownMenu`'s `descriptionKey` is for.
- **A panel above a list of rows may only show what the rows cannot.** The Usenet segment panel
  listed every NZB file's subject, segment tally and size, directly above the download rows for
  the same files — and the rows are the better copy, because they carry state and progress. It is
  not pure duplication, though: the raw subject (the poster's `[n/m]` ordering and the yEnc part
  spec) and the per-file segment tally including the segments that never arrived exist nowhere
  else, so the panel stays. Its size column does not, and the reason is sharper than
  repetitiveness: `NzbFileStatus.total_bytes` sums the `<segment bytes>` attributes, which is the
  *posted* article size including the yEnc overhead, while the row underneath shows the decoded
  file — so one file stood as `722 MiB` in the panel and `699 MiB` one line down, with nothing on
  screen accounting for the gap. Two unlabelled figures for the same thing are worse than one
  figure twice.

**The wrap width is measured, and this is the measurement.** The row sits in `UDashboardPanel`'s
body (`p-4 sm:p-6`) beside a sidebar that is a slideover below `lg` and 15% of the window from
`lg` up, so what the row actually gets is about `window − 50px` without the sidebar and
`0.85 × window − 50px` with it — which is why viewport media queries had been switching columns on
at widths the row did not have. Each tier costs a fixed number of pixels and may only switch on
where the remainder still leaves the name 200 px, about 28 characters:

| tier | costs | needs a row of | which arrives at | set to |
| --- | --- | --- | --- | --- |
| two lines: name, then state | 180 px | 380 px | — (the fallback) | — |
| + state on the same line | 316 px | 516 px | 556 px | **560 px** |
| + progress | 420 px | 620 px | 670 px | 768 px |
| + size and metadata | 756 px | 956 px | 1184 px | 1280 px |

Since RD-120-53 the size column is 144 px rather than 128 px — its longest automatic figure,
`1023 MiB / 99.9 GiB`, is 137 px of 12 px mono, and `585 MiB / 10.5 GiB` was already cut at
128 — and the open sidebar has a 240 px floor, so at 1280 px the row gets about 990 px, not the
`0.85 × window − 50px` the next paragraph assumes. Measured in Chromium at 1280 and 1440 px: no
row overflows, and the name cell holds 205 px at 1280 px with the sidebar dragged to its 21 %
ceiling.

Measured in Chromium against the shipped rules in `web/src/assets/main.css`, at every window width
from 320 px to 2560 px in 4 px steps, reading `scrollWidth − clientWidth` on the row and the
rendered width of the name cell. Before: the row overflowed its container at every width from
1024 px to 1188 px, by up to 142 px, and the name was under 200 px from 1024 px all the way to
1432 px. After: it overflows at none of the 561 widths sampled, and the name holds 296 px at
1280 px, 398 px at 1024 px and 104 px at 320 px, where the row is two lines. Re-measure rather
than adjusting these by hand.

The two-line fallback is why the cells are **named** (`queue-cell-handle`, `-select`, `-expand`,
`-name`, `-state`, `-progress`, `-size`, `-meta`, `-actions`) and placed through
`grid-template-areas` instead of being auto-placed in source order: auto-placement can only fill
one track list left to right, so it can shrink a row but never break it. `PackageGroup.vue`,
`TransferCard.vue` and `CollectorCandidateRow.vue` carry all nine, and a test in each asserts
it, because a row that quietly loses a cell stops wrapping and starts overlapping again. The
chevron in `-expand` is therefore a `UButton` of its own, not the trigger of a `UCollapsible`:
the collapsible's root would wrap the cell and the panel together and take both out of the grid.

**The data columns' widths are the viewer's; the name's floor is not** (RD-191-11). The download
list and the LinkGrabber carry a column header — `QueueColumnHeader.vue`, one more `.queue-row`
with the same nine cells, so a label sits over its cell at every tier and a hidden cell takes its
label along; below 560 px, where there are no columns, it is not drawn. The two lists share the
grid, not what is in it, so each list names its own cells (RD-1101-08): the LinkGrabber says *Link
state* and *Hoster · Variant* where the download list says *State* and *Category · Account*, and a
cell a list leaves empty — the LinkGrabber's progress — is drawn without a label or a handle, and
the list keeps no width for it. The start edge of each data column a list fills is a resize handle: a pointer drag with pointer capture, and for the
keyboard a focusable `role="separator"` with `aria-orientation="vertical"` and the column width as
`aria-valuenow`/`min`/`max`. The arrow keys move the edge the way a drag does — left widens, right
narrows — by 8 px, Shift by 32 px; Enter and a double click put the column back, and the header's
own menu (its actions cell, the one place both lists show at every width) resets them all. Nuxt UI
has no handle for a grid that is not a `UTable`; this one is the exception, and the menu and its
button are Nuxt UI. The widths reach the rows as `--queue-col-<column>` on the container around the
header and the list (`useQueueColumns.ts`, kept per view in `localStorage`, every access guarded),
and every row elsewhere falls back to the measured widths above. What may not move is the
accounting: a widened column is the track `minmax(default, chosen)` and the name's minimum is
`min(200px, what the defaults leave it)`, so the grid hands the name its floor first and a widened
column gives way before the name does — widening spends only name width above 200 px and never
pushes a row past its container. Each column has a floor of what its cell must still show (the
size's 137 px figure, a state badge, a shrunk category select) and 480 px as its ceiling. A
windowed list reserves its scrollbar gutter, and the header then reserves the same one, so the
edges line up with the cells under them.

**The LinkGrabber's link row is on the same grid, and the subscription rows are deliberately
not** (RD-110-27). The link row sits in the same panel body as the queue, so the measurement
above is its measurement too: handle, checkbox, chevron, name — the thumbnail, the file name, the
page title beneath it and the glyphs for media, torrent file count, replay approval and
missing account — then the state as a word in its own cell, the size from 1280 px, and in the
metadata cell either the hoster or, for a media link, the variant picker. Its progress cell is
empty, because a link under review has no progress; the cell stays so the grid keeps its shape.
Beside the row stand enqueue — the act the review exists for — and the dots; rename, the MP3
switch and delete are under them with their labels. The state stays a word rather than a glyph:
`check failed` and `not checked` need their qualifier, and a list that mixes glyph states with
worded ones reads as two systems. A size nobody measured prints nothing, not a dash.

**A mirror group's row is that same row and costs no extra tier** (RD-110-19). The group badge
goes into the name cell beside the media, torrent, consent and account glyphs — it is one more
shrink-0 badge on a cell that already holds four — and the mirror chevron takes the expand cell
the link row already has, because a link that is a mirror group and a link with a torrent tree
are never the same link. The member rows are the same grid with the handle, select and progress
cells empty and a left indent, so the name column of a member starts where the group's name
column starts and the list keeps one reading edge from top to bottom. Nothing in the group needs
a column of its own: quality and language are already what the member's name says, and the hoster
is already the metadata cell.

The subscription list and the hit rows under it cannot use `.queue-row`, and the reason is the
measurement itself: the grid switches its tiers on by *window* width, computed for a row that
spans the panel body, while `SubscriptionsView.vue` draws its rows in `FormListLayout`'s list
column — half the panel from `lg` — and `SubscriptionItemRow.vue` is drawn there and in the
LinkGrabber's review drawer. At a 1280 px window that column gives a row about 470 px, less than
the 516 px the second tier needs and half of what the last one needs; the grid would switch on
cells the row has no room for, which is the overflow RD-109-30 removed from the queue. Giving
these rows their own grid with its own tiers would be a second copy of the accounting. So they
keep their wrapping line and take the one figure from it that matters: **the name claims the
200 px the accounting reserves for a name** (`grow shrink basis-[200px] min-w-0`, so it may
still shrink within a line it shares), and whatever does not fit beside that wraps under it
instead of squeezing it — before this the title was the one item allowed to shrink to nothing.
Making `.queue-row` measure its container rather than the window would let every row share
it; that is the way to unify them, and it re-measures the queue, so it is its own job.

What the subscription row spends its line on: the chevron first, as every other expandable row;
the name; the kind and the mode as glyph badges with `role="img"` and the word as their name and
title — each is one idea with one icon; the enabled switch, which *is* the row's state (a
boolean that takes effect at once is a switch, and the `Disabled` badge that stood beside it said
the same thing a second time); then check now, which stays outside the dots because it has to
show its own busy state for the length of the request, and the dots with edit, duplicate and
delete. The hit row keeps `Queue` and `Dismiss` beside it with their labels — two controls,
which is the limit, and enqueue is on the list of actions that need a label — and says why a
hit was skipped in a line under the row rather than in it, exactly as the link row prints its
check error, because a sentence in the row is taken from the title.

**The two file trees are not a `UTree`** (RD-1110-12, checked on Reka's `TreeRoot`
2026-10-05). `RemoteFileTree` and `TorrentFileTree` build their rows themselves — indented by
depth, a chevron button per folder — because `UTree` cannot carry what a row holds. Its row is
one `treeitem` whose click selects and opens: a click on a checkbox or the priority select
inside it selects the row in the tree's own model as well, and on a folder the priority select
folds it away. That model is a list of selected items, while the server stores exclusions and
priorities per file; a range is Shift+Arrow from the focused item, not Shift+click or
Shift+Space from the anchor of `useRangeSelection`; and checkboxes and selects inside a
`treeitem` are controls nested in a control. The trees stay hand-built, with the range
selection, tri-state folders and per-file priorities of the rules above.

Reference implementations: `PackageGroup.vue` for expansion and row actions,
`NotificationRules.vue` and `AutomationView.vue` for confirmed deletion,
`SettingsRemoteJobsCard.vue` for a deletion that reaches past this machine,
`SettingsAccountsTab.vue` for a mixed row with both labelled and icon-only actions,
`SubscriptionItemRow.vue` for a row with a thumbnail and a shared detail block that wraps
under its title, `CollectorCandidateRow.vue` for a leaf row on the shared grid,
`QueueColumnHeader.vue` for a column header with resizable columns on that grid,
`RoutingCategories.vue` for a form beside its list with the edited row marked.

### Number Fields

Every number a person types is a `UInputNumber`, never a `UInput type="number"` (RD-1110-10). It
parses and formats in the interface language — German and French type and read `1,5` — holds
`min` and `max` itself, and hands its model a number rather than the text.

- **The kind of number sets the format** (`web/src/utils/numberInput.ts`): `WHOLE` for counts,
  durations and days, where a typed fraction rounds; `PLAIN` for ports, versions and priorities,
  whole and without a thousands separator; `DECIMAL` for sizes in MiB or GiB, two places; `RATIO`
  for seed ratios, three. A field with decimals sets `:step-snapping="false"`, or the default step
  of 1 rounds 1,5 to 2.
- **The unit is the field's `hint`** (`hint="MiB/s"` on its `UFormField`): the number field has no
  trailing slot. A field without a `UFormField` puts the unit beside it as an outline `UBadge` in a
  `UFieldGroup`.
- **Empty is decided per field.** The field reports an emptied value as `undefined`. An optional
  field — the DTO's `Option`, read as *inherit*, *unlimited* or *no limit* — sends `null`, through
  `orNull` or a byte model, never a key the body drops. An obligatory field carries `required`: in
  a `<form>` the browser holds the submit; the settings document's save button is disabled and
  says *A number field is empty* while one of its plain-number fields is (`emptyNumberFields`); a
  form whose fields sit outside a `<form>` checks with `isNumber` before it sends.
- **No plus and minus, except on a small count.** `uiTheme.ts` turns the stepper buttons off
  for every field: a number is typed, and the arrow keys and the wheel still step. A count of at
  most 32 steps between `min` and `max` — parallel downloads, connections, chunks — names
  `increment decrement` and shows them, because there a click or two is quicker than typing and
  the bound is in reach. On a port, a timeout in seconds or a retention of 500 000 records the
  buttons offer a step nobody takes and cost the field a third of its width at 390 px.

### Empty States, Notices, Opening and Dividers

The four building blocks the audit of 2026-10-05 found drawn by hand all over the interface are
Nuxt UI's (RD-1110-11):

- **Empty state.** A `UEmpty`. Its frame is the dashed outline the theme in `web/src/uiTheme.ts`
  gives every one (`border-dashed border-muted` on Nuxt UI's `naked` variant), and its padding is
  Nuxt UI's own — one spacing for all of them, where 24 hand-drawn boxes had four. The sentence is
  its `description`; a view's empty state adds an `icon` and a `title`, and the way out is a
  button in `actions`. `signal-grid` stays a class on the ones that carried it (the queue, the
  LinkGrabber, the Usenet servers and indexers); `DataState` frames its loading panel the same way,
  and what a caller hands it for "nothing here" is a `UEmpty` too, never a muted `<p>` — the guard
  test counts the paragraph as `empty-paragraph` (RD-1120-14).
- **No direction that only one layout keeps.** An empty state or a hint names no "above",
  "below", "left" or "right" that is true in one layout only: `FormListLayout` puts the form
  beside the list from `lg` and above it below `lg`, so it says "with the form"; something in
  another tab is named by its tab (RD-1120-03). A field of the same card, which stands above or
  below in every layout, may still be pointed at by its place.
- **Notice.** A box that tells the reader something — a warning to act on, a secret shown once, a
  state worth knowing — is a `UAlert`, `subtle`, in the colour of its meaning; `subtle` is the
  theme's default (`web/src/uiTheme.ts`), so only a `soft` or `outline` one names its variant. A heading is its
  `title`, the rest its `description` or, where it holds controls, its `#description` slot; an
  action goes into `#actions`. UAlert sets no role, so a box that announced itself keeps its role
  on the UAlert (`DataState`'s failure: `role="alert"`). A line that says what went wrong with
  one field or one row stays text beside it; the feedback of a form is a `UAlert` above it
  (*Forms Share One Shape*).
- **Opening and closing.** What opens and closes is a `UCollapsible` with a `UButton` as its
  trigger, never a `<details>`; the button carries `aria-expanded` itself as well (Reka sets it
  in the browser, a component test's stub does not). A toggle among a row's other cells — the
  history, audit and log entries, a subscription and its hits, a remote job, the licence lists —
  puts its `UCollapsible` into the row with `class="contents"`: the trigger keeps its place, and
  the panel is the row's last line, `basis-full`, with an `order-*` where the row reorders its
  cells, so it opens directly under the row it belongs to. Besides the queue's grid rows (above),
  two LinkGrabber groups keep a chevron `UButton` with `aria-expanded`: the NZB import and an
  indexer's review group, whose header carries the group's own actions and whose open part is a
  block of its own under it, with a footer, and for the NZB a second trigger on its failure
  badge; a collapsible would take the header's actions into its trigger or that block into the
  header.
- **Dividers.** A line between the sub-sections of a card is a `USeparator` — the theme draws it
  in `border-muted`, the hairline the hand-drawn `border-t` was — and the spacing the
  `border-t … pt-N` carried moves to its margins or is left to the parent's `gap` or `space-y`.
  The lines between a list's rows stay `divide-y`.

### Frontend Technology

Vue 3.5 and TypeScript 5.9 use the Composition API, Vue Router, Pinia, VueUse, Vue I18n, and
`openapi-fetch`, built with Vite 8. Nuxt UI 4.11 and Tailwind CSS 4.1 provide accessible components and
semantic design tokens. Vitest, Testing Library Vue, and jsdom test stores, composables, utilities,
and interactions.

## Security and Trust Boundaries

- The administrator password is hashed with Argon2id and must be at least ten characters long.
- Administrator sessions are stored as SHA-256 digests of their bearer and end after an idle limit
  (default 12 hours, 1 to 720) or a maximum lifetime (default 30 days, up to 90), whichever
  comes first; both are set under Settings → Security and checked on every request, so a shorter
  value ends older sessions at once (RD-130-09). The cookie is `HttpOnly` and `SameSite=Strict`,
  and its `Max-Age` is the maximum lifetime.
- API and capture tokens are random and revocable, and only their SHA-256 digests are stored in
  SQLite. Plaintext is shown only when a token is created.
- Provider, proxy, solver, and NNTP secrets are represented by vault references in SQLite. The
  vault uses XChaCha20-Poly1305; its master key comes from the OS keyring or a private Unix fallback
  file.
- HTTP TLS uses `rustls` and platform certificate verification. A custom CA is optional.
- File destinations must remain inside configured storage roots; canonical path and symlink checks
  prevent directory traversal.
- Remote operation expects a TLS reverse proxy. The product is not a public multi-user service and
  deliberately has no role management.

## File System and Deployment

| Content | Default path |
| --- | --- |
| SQLite | `data/rdownloader.sqlite3` |
| Downloads | `downloads/` |
| Installed plugins | `data/plugins/` |
| Encrypted secrets | `data/secrets/` |
| Password list | `data/passwords.txt` |
| Post-processing scripts | `data/scripts/` |
| Domain blocklist | `data/excluded_domains.txt` |
| Torrent metadata | `data/torrents/` |
| Torrent session | `data/torrent-session/` |
| Docker configuration | `/config` |
| Docker downloads | `/downloads` |

Official artifacts target Windows x86-64, Linux x86-64, macOS Intel/Apple Silicon, and Docker
`amd64`/`arm64`. The Docker image includes the service but not the desktop capture agent. The
frontend must be built before the Rust binary because `rd-api` embeds its files at compile time.

## Known Constraints and Explicit Non-Goals

- No multi-user/role model and no built-in public TLS endpoint.
- No arbitrary file-system access for plugins; extensions remain resolvers inside the declared
  sandbox.
- BEP 19 web seeds are not supported by the embedded torrent engine.
- Widget captchas cannot be rendered manually in the rDownloader UI because they are bound to the
  hoster's domain.
- External tools are optional; their corresponding functionality is unavailable or reduced when a
  tool is missing.
- Roadmap items such as additional protocols, a notification hub, automation engine, plugin store,
  and update system are not current product capabilities.

## Decision and Maintenance Conventions

- Domain invariants belong in `rd-core`; persistent mutations go through the `rd-db` writer.
- New transfer types implement `ExternalRunner` rather than duplicating scheduler state.
- New resolver capabilities require a versioned WIT change and an explicit compatibility or
  migration decision.
- API changes are modeled in Rust/OpenAPI first; generated web types are refreshed afterward.
- New visible messages require stable codes and translations for every required locale.
- New persisted fields require an additive migration and a recovery test.
- Security-sensitive defaults must remain safe for local operation; remote exposure is a deliberate
  operator decision.

Further references: [`README.md`](README.md) and the [handbook](https://github.com/degoya/rDownloader/wiki), above all its
[Plugin reference](https://github.com/degoya/rDownloader/wiki/plugin-reference) and [Post-processing](https://github.com/degoya/rDownloader/wiki/post-processing) pages.
