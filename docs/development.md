# Development and operation notes

The long half of what the repository's `README.md` carried until 1.3, when the README became
the public front page: the features in detail, running rDownloader from source, building it for
every platform, the plugins and the provider accounts they need, portable start and autostart,
the capture agent, `rdownloader://` links, the browser extension, the command line client, the
automation-client adapters, the MCP server, metrics, Docker and the quality checks.
[`README.md`](../README.md) is the overview; [`docs/README.md`](README.md) lists every other
document.

## Features in detail

The feature list the README carried until 1.3. [`feature-list.md`](feature-list.md) is the
complete inventory; this is the longer account of selected features, kept here so that nothing
of it was lost with the move.

- **HTTP/hoster downloads** — persistent download core with parallel files and chunks, range/resume, crash-safe checkpoints, configurable retries and a bandwidth limit adjustable straight from the Downloads toolbar. How many connections one host may see at the same time is capped across every running transfer (Settings → General, six by default, `0` lifts it), so adding a handful of links at once does not open dozens of connections against the same CDN. A server that answers a range request with a plain `200` is judged by what it actually sent rather than by the status: the whole file in one chunk is written from the start, a stale resume validator is reported as a changed remote, and anything else is retried automatically with that host planned as a single connection.
- **Transfer rate and time remaining** — the service measures a smoothed rate for every transfer and for the queue, and derives how long is left at that speed. Each running row and each package carries its estimate, the footer and the queue statistics carry the queue's. The figure is measured in one place, so the web interface, the desktop tray, MCP and automations all read the same number instead of each computing their own. The browser tab carries the same figure beside the number of active transfers while anything is running, and the application name alone at rest, so a window in the background can be read without bringing it forward; it can be switched off. It stays blank where it would be invented: no known size, nothing moving, or a rate of zero shows nothing rather than a placeholder or an infinity. Waiting, verifying, repairing, unpacking and seeding entries are outstanding but not being fetched, so they stay out of the estimate — the "remaining" byte figure beside it says that it counts them.
- **Free hoster downloads and CAPTCHA** — all seven bundled hosters support their anonymous countdown/download flow. Image challenges can be answered in the app, and so can click-point challenges, by clicking the spot in the picture; CutCaptcha is answered by a solver service alone; reCAPTCHA, hCaptcha and Turnstile are answered only in your own browser through the extension, which opens the hoster's page in a tab and hands the token back, or by an optional 2captcha-compatible solver whose API key stays in the encrypted vault. Free slots and IP-limit backoff are isolated per hoster.
- **JDownloader-style LinkGrabber** — links are grouped into packages on intake, with online check, variant selection, per-package category, priority and archive password, renaming, sorting and drag & drop. Each package downloads into its own folder `<category dir>/<package name>/`.
- **One release, many mirrors** — five hoster links to the same file are one entry with five mirrors, not five candidates of which four get deleted by hand. A mirror is not a duplicate: a duplicate is the same address a second time and is set aside, a mirror is a different address for the same bytes and is kept, because it is what remains when the chosen one goes offline. The group comes from a site rule that says its page is one release, from a file name and a size that agree after the online check, or from a shared file name alone — the last offered as a proposal and marked as one. Quality, language and hoster are shown per mirror where the source or the release name states them, and the group is one row in the LinkGrabber with the rest behind a chevron. Those same three are a standing preference rather than a filter that is forgotten: inside a group they choose the mirror the queue will fetch, outside one they hide what cannot match, and they still apply to the package that arrives tomorrow. If the chosen mirror fails at the hoster — offline, at its limit, or serving a page instead of the file — the next one takes over on its own and the partial data of the one given up is discarded; a full disk or an unwritable destination never burns a mirror, because the next one would write to the same place. Any group can be overruled by hand, and that choice survives a regroup. Hosters you do not want can be hidden from the LinkGrabber, several at once, from a chip row with each hoster's link count or from a link's menu; the choice is stored with the preference, a line says how many links it holds back, and what is hidden is neither queued nor checked — except as the fallback mirror of a link that is shown.
- **Lists that stay usable at a few thousand rows** — the LinkGrabber and the download queue render only the rows near the viewport instead of all of them, through one shared building block. A package and its files are one flat stream of rows with stable keys, so collapsing, reordering and selecting keep working across the part that is not in the document: the row holding keyboard focus is never taken out, an arrow-key reorder keeps its handle past the edge of the window, shift picks a range, and there is a way back to a selected row that opens its package first. A screen reader is told the real length of the list rather than the size of the window. Below sixty rows nothing is held back and the list looks exactly as it did.
- **Link containers** — `.dlc`, `.ccf`, `.rsdf` and plain `.txt` link lists are imported by upload, drag & drop or hotfolder and become reviewable LinkGrabber packages with the names, sizes, package name and archive password they carry. RSDF is decrypted on this machine, because its key is published, and a text list needs no key at all: one link per line, with `[A Name]` opening a package and `;` or `#` starting a comment. DLC and CCF keep their links behind a key only an online service can unwrap, so those two are off by default; once enabled under Settings → Collector it sends nothing but that key to the configured service, and the links stay here. Watched folders take `.ccf` and `.rsdf` as well, but deliberately not `.txt`, which is also somewhere people keep notes. A script or an assistant does not have to build a file upload: the same import routes — containers, `.torrent` and `.nzb` — also take the file as base64 in a JSON body, up to 48 MiB, and answer exactly as the upload does (RD-120-31).
- **Rename a package's folder, not only its label** — editing a package's name in the download list can rename the folder its files live in as well; the switch sits in the same dialog and is off until the name actually changes. The row is written before the disk moves and remembers where the data was, so an interruption leaves the package findable and the move finishes by itself on the next pass. Every absolute path the package has stored — its post-processing steps, its assembled Usenet files — moves in the same transaction, because a post-processing step is identified by its source path: left behind, a second "post-process anyway" run would not recognise its own earlier work. A folder name that is already taken is refused rather than sidestepped with a `(1)` suffix, and a package with a file still transferring is refused too. The name goes through the same rules a file rename uses, so it cannot walk the package out of its category directory.

- **Reset a download** — a finished, failed, stopped or paused job can be discarded and started over from zero: partial data, checkpoints, progress, retry budget and the package's post-processing steps. Available per row, over a whole selection, through the REST API and over MCP. A finished file is kept unless the confirmation explicitly asks for it to go, so a reset never destroys the only copy by accident.
- **Mirrors** — when a package holds several links to the same file, one is downloaded and the rest wait as its fallback rather than fetching the same bytes several times over. If the active link gives up for good, another takes over; a link with an account wins the turn, because it is not subject to the free-download limits the others would queue behind. Archive volumes, Usenet files and torrent files are members of one download rather than routes to it, so none of those are grouped. Can be switched off.
- **Reconnect** — hosters limit anonymous downloads per address, so a script you write can ask the router for a new one when free downloads are stuck behind such a limit. It runs in the same sandbox as post-processing scripts, inside the times you allow, keeps its distance from the previous attempt and by default waits until nothing is transferring. Off by default: the address checks that confirm the change are the one part that talks to somebody outside, and which addresses are asked is configurable.
- **Unattended housekeeping** — finished packages can leave the queue by themselves after a set number of hours, keeping anything still unpacking, still seeding or still holding a file that did not finish. Standby can be held off while a download, repair or unpack is actually running — not while the queue is merely waiting — through a helper process that releases the machine even if the service is killed outright.
- **Usenet/NZB** — NNTP server pool with parallel segments, CRC-verified resume, PAR2 repair, hotfolders (polled every 5 to 3600 seconds, a setting running watchers take over live) and `.nzb` file association; one unified queue for HTTP and Usenet alike. Archive passwords from the `{{password}}` file-name convention, the intake package or `<head><meta type="password">` inside the NZB reach the package and are visible in its editor. An NZB file is routed like any other intake: the watched folder's category if it has one, otherwise a matching routing rule, otherwise the default category — the same order a pasted link follows, and the same for an upload or a SABnzbd `addfile`. An NZB a watched folder cannot read is not lost either: it is recorded as a failed import and shown in the LinkGrabber with the reason, so a drop nobody made by hand still reaches somebody.
- **BitTorrent** — embedded engine for magnets and `.torrent` files, complete pre-download file-tree selection, priorities and exclusion patterns, tracker/peer/piece statistics and controls, interface/VPN binding with kill switch, IP blocklists, SOCKS5 peer proxying, UPnP and inheritable seeding policies. Sharing data with other peers is off by default — a new installation uploads nothing, not even while a torrent is still downloading — and must be switched on deliberately, which is noted next to the switch because a client that only ever takes gets banned from some private trackers. Unsupported engine capabilities are reported explicitly instead of being accepted silently.
- **FTP, SFTP and WebDAV** — FTP, explicit and implicit FTPS, and SFTP over SSH (password, private key or SSH agent) are native transfer sources; WebDAV shares resolve via `PROPFIND` and then download over the ordinary HTTP engine, inheriting chunking, resume, authentication profiles, proxy and the speed limit. Directories are reviewed as a file tree before queueing and each selected file becomes its own queue row. Resume is validated rather than assumed: if the file's size or timestamp moved, the partial file is kept and the transfer refused instead of corrupted. An SFTP server is used only after you confirm its host key, and a key that later changes is always blocked — a rebuilt server and an intercepted connection look identical from here. Credentials pasted in an `ftp://user:pass@host/…` link are stored encrypted and stripped from the link before it reaches the queue or any log.
- **Media pages** — YouTube, Vimeo, Twitch, SoundCloud, the public broadcasters' media libraries and around fifty other sites are fetched via external `yt-dlp` and `ffmpeg`, with playlist expansion in the LinkGrabber. Beyond the resolution presets and Audio (MP3), each link opens a format selector for container, video and audio codec, HDR mode, frame rate, bitrate and audio language, showing the estimated size and explaining a filter combination that matches nothing. What is stored is the selection *criteria*, not the extractor's volatile format id, so the choice is re-resolved at download time and survives the site rotating its ids. Additional audio languages and subtitles can be selected per link — as a sidecar file, embedded, or both — with auto-generated captions kept apart from authored ones and off by default. Thumbnail, chapters, metadata and the description can be embedded individually, and SponsorBlock segments marked as chapters or removed — both off by default, and a signed source URL is never written into the file. Output templates such as `{uploader}/{upload_year}/{title}` arrange the files, previewed against the real metadata through the same evaluator the download uses. Private and age-restricted pages can be fetched with an approved browser session, chosen per link before it is queued; only cookies for that page's own host are handed to the tool, and the file holding them is owner-only and deleted when the download ends. Without ffmpeg only formats that already carry video and audio are offered, instead of failing later. Direct `.m3u8` and `.mpd` addresses are recognised from their content rather than their extension and feed the same selector; a live one is routed to the recorder instead of the downloader, and a DRM-protected one is refused with a reason rather than attempted. The host list is editable in Settings.
- **Scheduled recordings** — livestream channels can carry recurring weekly windows or a one-off event time, with a pre-roll and post-roll for broadcasts that start early or overrun. A scheduled channel is watched only inside its window. The time zone is an IANA name rather than an offset, so a weekly slot stays put across daylight saving, and the two awkward hours — the one that happens twice and the one that never happens — are decided explicitly instead of producing a duplicate or a recording at the wrong time. A window that passed without the channel going live shows up as a missed recording rather than as nothing.
- **Robust recordings** — a livestream recording is a sequence of segments, so a dropped connection costs the reconnect and not the hours already on disk, and the segment history says where the gaps are. Long recordings can be split by time or size, joined into MKV or MP4 afterwards as a resumable step, and accompanied by metadata and thumbnail sidecars; what a provider does not offer is reported rather than silently skipped.
- **Subscriptions** — channels, playlists and gallery profiles are polled on their own schedule, and what they find enters the LinkGrabber through the ordinary intake, so routing rules, categories and review still apply. Every item is acted on exactly once, guaranteed by the archive rather than by luck, so a reordered feed or a restart mid-poll changes nothing. Switching a subscription on does not import its history: the first check records what already exists as seen unless you deliberately ask to review the backlog. Items can be filtered by title, duration, date, language and resolution, and one that is skipped says which rule skipped it. A skipped hit is kept rather than dropped, because an unwritten one would be rediscovered on every poll. The archive therefore starts with all open hits, pages them 50 at a time on the server, and offers queued, dismissed, skipped and all as counted filters; settled hits and check records can be cleared per subscription without deleting open decisions. Newznab and Torznab checks fetch up to five pages of 100 results before applying the exact local title filter, stopping at a short page and warning only when all 500 available positions were exhausted. The filter is not sent as `q`, because an indexer's tokenizer does not have the same semantics as a local substring or regular expression. Checking by hand reports start and completion over the event stream. RSS, Atom and podcast feeds work the same way, fetched conditionally so an unchanged feed costs a `304`; podcast episodes are downloaded by their enclosure, not by the show-notes page. Hits held for review wait in the LinkGrabber grouped by subscription. The main section initially opens only when decisions exist, each subscription starts closed, and its confirmed “queue all” and “dismiss all” actions cover every open page while reporting partial failures. Categories can be fetched while the subscription is still being written and requested categories are sent to the indexer. A hit shows IMDb score, genre and language under its title, with plot, season and episode, resolution, codecs, grabs and size in the details. Its cover opens immediately in a large viewport-bounded popover on hover, keyboard focus or tap and closes with `Escape`; disabling external images disables both thumbnail and enlargement. A real archive password is shown with a key and travels with that specific hit through the LinkGrabber to extraction, while Newznab's password-protection flag remains only a lock. Each indexer subscription chooses how its hits appear in the LinkGrabber: the list, which is the default, or a **card slider** — one card per hit with its cover or a coloured initials tile, size, release group and chips, paged by arrows, arrow keys, swipe or page dots, with the same actions and confirmations as the list and one details panel under the slider. Cards can optionally turn on their own every six seconds; that pauses under the pointer, under keyboard focus, while details are open and while the tab is hidden, has its own pause button, and does not run at all when the system asks for reduced motion. The cards' picture takes an aspect ratio chosen per subscription — 1:1, 3:2, 16:9, 4:3 or the default 2:1 — so square covers show square and banners wide. A **release or series page** can be watched too: the address is a listing -- a series, category or tag page -- fetched conditionally at most every half hour, and the links on it that a site rule reads become the items. Each one enters the LinkGrabber the ordinary way, so the rule produces the hoster links and a release with five mirrors is one row rather than five. Such a subscription recognises an episode by its *release name* -- series, season and episode -- rather than by its address, so the same episode posted again by another group, in another quality or three weeks later counts as already had; quality and language come from the same closed token lists the mirror grouping uses and feed the resolution, language and exclusion filters. *Keep every release* is the explicit counter-choice for somebody who collects versions. Subscriptions, stream schedules and automations can each be written to a file and read back on another installation; the file references categories and targets by name and carries no credentials.
- **Log viewer and diagnostic bundle** — every log line the service lets through its filter is also kept in the database, redacted before it is stored: credentials, signed-link parameters and authorization headers never reach the store, the API or the *Logs* page. The viewer filters by level, component, stable code, correlation id and text; retention is two weeks or twenty thousand records by default and configurable under Settings → System, and the sweep deletes in bounded steps so the queue never waits for it. The same section empties the store outright, after a confirmation that says how many records will go — for starting a test run from nothing rather than reading the interesting part out of what earlier runs left behind. For a support request, the same page builds a diagnostic bundle — versions, configuration metadata without its credentials, system checks, the `doctor` output and the most recent errors — written on this machine only, and only after you have seen its inventory and approved it. That inventory is shown in your own language, down to the lines saying what was redacted; the archive itself stays English, so a support engineer can read `manifest.json` without this installation in front of them. Details in [docs/diagnostics.md](diagnostics.md).
- **Vendor folder for the helper binaries** — `yt-dlp`, `ffmpeg`, `ffprobe`, `unrar`, `7z` and `apprise` are looked up in a configurable vendor directory, in `vendor/` next to the executable and in `vendor/` inside the data directory before falling back to `PATH`. Dropping the binaries into one folder is enough; no system-wide installation and no per-tool paths are needed. Settings and `rdownloader doctor` show for each tool where it was found. Note that `yt-dlp` needs `ffprobe` next to `ffmpeg` to merge video and audio.
- **About rDownloader** — *Settings → About rDownloader* names the running build (version, commit, build time, plugin contract, platform), the project's addresses, the author, and the licenses of everything the packages ship: rDownloader itself, the helper tools in `vendor/` with their license texts under `vendor/licenses/`, and every Rust crate and npm package. Commit and build time are the same values `VERSION.txt` carries; the dependency list is generated by `scripts/licenses.sh` and a test fails when a dependency arrives without a license entry. Before the first public release the repository, website and handbook are shown as "not yet published" rather than as links.
- **Tool versions are checked, and only what actually breaks is blocked** — `yt-dlp`, `gallery-dl`, `streamlink`, `ffmpeg` and `ffprobe` are compared against the versions this release is tested against. A version below the floor, or one listed as broken in the signed manifest, blocks exactly the capabilities the rule names — media downloads, merging, MP3 conversion, galleries or recordings — and nothing else; a version that cannot be read (an FFmpeg git build, say) warns and blocks nothing at all. Settings and `rdownloader doctor` show the version, the verdict and the upgrade path, and a job that was stopped says which capability it lost. Rules that cannot be read fall back to the ones compiled into the release. The matrix, the shipped floors and the platform differences are in [docs/external-tools.md](external-tools.md).
- **Managed tool versions, if you want them** — optional and off by default, rDownloader can download, verify, activate and roll back `yt-dlp`, `gallery-dl`, `streamlink`, `ffmpeg` and `ffprobe` itself. Every build comes from a signed manifest with its SHA-256, is checked before it is activated, and lands in `<data>/tools/<name>/<version>/`; activating one takes effect for the next job while a download already running keeps the version it started with. A tool path you configured yourself still wins over anything managed. The release ships builds for `yt-dlp` (x86-64 and ARM64, Linux and Windows) and for `ffmpeg`/`ffprobe` (x86-64, Linux and Windows), the latter from BtbN's builds — a third party, because the FFmpeg project publishes no binaries itself — pinned to a dated release by hash. `gallery-dl` and `streamlink` publish nothing that can be pinned this way and stay vendor/`PATH` tools; [docs/external-tools.md](external-tools.md) lists every source and both gaps.
- **SABnzbd-style post-processing** — levels None / +Repair / +Unpack / +Delete per package or category, SFV checksum verification before unpacking, a RAR integrity test when nothing else verified the payload, cleanup extension list and sample removal, user scripts with SABnzbd-compatible arguments and environment, live extraction progress and a post-processing queue in the Downloads view. A failed verification blocks unpacking only while "post-process only verified packages" is on, and a per-package "post-process anyway" action overrides it once — a damaged recovery set beside intact archives no longer locks a package away. PAR2 recovery volumes are held back when an NZB is queued — or, for an obfuscated post whose subjects announce nothing, the moment the main index lands with its real name — and only fetched if a repair turns out to need them, by the block count their names announce and no more; the package returns to downloading while they arrive and the pipeline runs again afterwards, and "Download all PAR2 volumes" restores the older behaviour. A finished file whose header says PAR2 is repair data whatever it is called. Whether a file with a missing segment is lost is decided when the package settles rather than when that file happens to finish: until nothing of the set is queued, downloading, verifying, repairing or waiting for a retry, the row waits in `Verifying` and says what it is waiting for, and then the set decides — with PAR2 the hole goes to the repair, without it the file fails with the same message as before. See [docs/postprocessing.md](postprocessing.md), which also lists what was and was not taken over from SABnzbd.
- **Planned load** — reusable bandwidth profiles activated by a weekly schedule, with limits per protocol, host, account or category on top of the global one, and daily or monthly traffic budgets. The strictest applicable limit wins and the UI names which one it is; what a transport cannot enforce is reported rather than silently ignored.
- **Unattended operation** — quiet hours postpone repair, unpacking and uploads without touching downloads; once the queue and post-processing have drained, a script, standby or shutdown runs exactly once per cycle of work, with power actions gated behind a local approval and a cancellable countdown. Battery and metered operation can hold the queue.
- **Storage capacity** — one free-space policy for every transport, with a configurable threshold per storage root. A root that runs low blocks only its own downloads and refuses only its own intake; it is released automatically or by hand, and the block survives a restart. A root pointing at a path that is not on a mounted volume — in a container, the writable layer — is flagged in the routing view and in the readiness card, because everything written there is lost when the container is recreated. Exactly one root is always the default, so a download without a category has a defined destination.
- **Notifications** — server-side delivery to signed webhooks, e-mail and Apprise-compatible services (Telegram, Discord, Slack, Matrix, ntfy, Gotify, Pushover, Home Assistant), filtered by event, category and severity, with retries and a delivery history that can be cleared. Target credentials stay in the encrypted vault.
- **Web UI in English, German, French and Spanish** — auto-detected and switchable, with light/dark/system theme; REST errors carry stable `code` values that the UI translates.
- **Plugins** — hoster/multihoster resolvers and transfer backends as signed WebAssembly components (`.rdplug`), WIT interface `rdownloader:plugin@0.9.0`. Every permission a plugin has is one line in its manifest and one interface in the host, so it reaches exactly what it declares and the plugin manager shows you the list before you install it. See [docs/plugins.md](plugins.md).
- **Link capture** — desktop capture agent for Click'n'Load 2, clipboard monitoring and `.nzb` double-click, plus a browser extension for NAS/Docker setups that also intercepts regular browser downloads and hands them over with the metadata needed to repeat the request. The agent raises a native desktop notification when links reach the LinkGrabber, so a handoff is visible without a browser tab open; the web interface raises its own notification for the same event, so the agent is not required for it.
- **Authenticated downloads** — reusable per-domain authentication profiles (browser session, HTTP Basic, Bearer, optional client certificate) with credentials kept in an encrypted vault. A download that would send credentials — a form `POST`, a request body or a signed address — is shown for approval first, stating which category of credential goes to which host, and is confined to the addresses you approve.
- **Settings with a structure** — twenty-three settings pages in six rubrics (General, Downloads, Sources & protocols, Integrations, Network & security, Administration), the same in the sidebar and on the entry page at `/settings`, where one card per page says what is found there. Every page opens with the same header; the older addresses still work.
- **Switchable services** — BitTorrent, Usenet, media downloads, galleries, stream recordings and remote file transfer can each be switched off in Settings → Services. A switched-off service refuses matching links at intake and blocks what it already had queued with that reason, rather than leaving entries waiting for a runner that never comes; switching it back on returns them to the queue.
- **Metrics and statistics** — `GET /api/v1/metrics` exposes queue, throughput, retry, provider, storage and runner metrics in the Prometheus text format behind a scope of its own (`api:metrics`), and a Statistics page shows what was transferred per hour or day, by kind and provider, from figures that survive a restart and are thinned after a configurable retention. Settings → System also empties them, buckets and all-time totals together, after a confirmation naming the number of rows. See [`docs/observability.md`](observability.md).
- **Audit log and optional tracing** — the security-relevant things that happen leave a record the action itself waits for: sign-ins accepted and refused, sign-outs, changes of the administrator password, tokens used, created, re-scoped and revoked, settings written and reset, plugin trust decisions, and the destructive actions on downloads, packages, categories, storage roots and the configuration backup. Each record names who acted, from where, on what and how it ended — and never a password, a token, a digest or a signed link. The log is append-only: no route writes, edits or deletes a single record, the database refuses an `UPDATE` on the table outright, deleting a download leaves its record behind, and restoring a backup cannot erase the record of itself. Nothing removes a single record — retention removes whole rows oldest first, and the one deliberate removal is emptying the log from Settings → System, which cannot pick its rows and writes itself into the emptied log as that log's first entry: when, by which account, and how many records went. An audit log that is empty with nothing in it saying why has lost the one trace that explains it. Read it on the *Audit log* page, filter it and export the filter as NDJSON. A trace id runs through the API, the scheduler, the resolver and post-processing, so one id reads a piece of work end to end in the log viewer; finished spans can optionally be exported over OTLP, off by default, and a collector that is down never delays a download. See [`docs/observability.md`](observability.md).
- **Local-first** — the server binds to `127.0.0.1:8710` by default; the port is configurable in Settings (applied on the next start, `--listen`/`RDOWNLOADER_LISTEN` take precedence) and the admin login set up on first run can be changed later under Settings → Security, with the current password, or disabled entirely for trusted networks.

## Run locally under WSL2

Prerequisites are Rust 1.98, Node 24 and npm 11.

```bash
npm ci --prefix web
npm run build --prefix web
cargo run -p rdownloader -- serve \
  --database data/rdownloader.sqlite3 \
  --downloads downloads \
  --listen 127.0.0.1:8710
```

On Windows 11 the UI is then reachable at `http://localhost:8710`. On first access the local admin password is set up.

Signed `.rdplug` packages can be installed via the CLI or the Plugins view. Additional trusted release keys are passed at service start (repeatable) as `--trusted-plugin-key KEY_ID=BASE64_PUBLIC_KEY`. `--plugin-development-mode` allows unsigned packages and is intended exclusively for local development; it relaxes nothing else. Letting installed transfer backends dial loopback and private-network addresses is a second flag, `--plugin-allow-local-targets`, which is likewise for local development only and has to be asked for separately. On startup, installed signatures are re-verified and the components are preloaded with manifest domain checks as well as fuel, time, memory and response limits. The highest installed SemVer version of a provider wins for new jobs; a running service keeps its loaded instances until restart. The version an upgrade replaced stays on disk, because a job already under way keeps the one that started it: the Plugins view lists it under the plugin it belongs to rather than as an entry of its own — so the count there counts plugins — and offers to remove it, which is refused while unfinished work is still bound to that exact version. A package this version cannot run — one built for an older contract, or asking for a permission it does not know — is listed in the Plugins view with the reason rather than silently ignored, and every invocation is recorded there with its outcome.

The provider components are deliberately built without WASI, see [Plugin components](#plugin-components-webassembly).

## Build

### Prerequisites

| Tool | Version | Notes |
| --- | --- | --- |
| Rust | 1.98 (stable) | `rust-toolchain.toml` installs Rustfmt, Clippy, `llvm-tools` and the targets `wasm32-unknown-unknown`, `wasm32-wasip1`, `wasm32-wasip2` and `x86_64-pc-windows-msvc` automatically on the first `cargo` invocation. |
| Node / npm | 24 / 11 | Only needed for the web frontend. |
| C toolchain | platform-specific | See the respective sections below. |

The UI is embedded into the binary at compile time via `rust-embed` from `web/dist`. **The frontend therefore has to be built on every platform before the first `cargo build`**, otherwise the build of `rd-api` fails:

```bash
npm ci --prefix web
npm run build --prefix web
```

All release builds use `--locked` so that exactly the versions from `Cargo.lock` are used. The release profile is configured with thin LTO, `codegen-units = 1` and symbol stripping. Results end up under `target/release/`, or `target/<TARGET>/release/` when `--target` is passed explicitly.

### Linux (native)

Requires a C compiler and linker, e.g. `build-essential` on Debian/Ubuntu. No further system libraries are needed: TLS uses `rustls`, SQLite is bundled.

```bash
npm ci --prefix web && npm run build --prefix web
cargo build --locked --release -p rdownloader -p rd-capture
./target/release/rdownloader --version
```

The official Linux release is built for `x86_64-unknown-linux-gnu` and linked against the glibc of the build system.

### macOS

Requires the Xcode Command Line Tools (`xcode-select --install`). The release artifacts are built separately for Intel (`x86_64-apple-darwin`) and Apple Silicon (`aarch64-apple-darwin`); locally the host target is sufficient:

```bash
npm ci --prefix web && npm run build --prefix web
cargo build --locked --release -p rdownloader -p rd-capture
```

A build for the other target is possible with `rustup target add <TARGET>` and `cargo build --locked --release --target <TARGET> -p rdownloader -p rd-capture`.

### Windows (native)

Requires the Visual Studio Build Tools with the "Desktop development with C++" workload (MSVC compiler, Windows SDK including `rc.exe`) and Rust via `rustup` with the `stable-x86_64-pc-windows-msvc` toolchain.

```powershell
npm ci --prefix web
npm run build --prefix web
cargo build --locked --release --target x86_64-pc-windows-msvc -p rdownloader -p rd-capture
& target/x86_64-pc-windows-msvc/release/rdownloader.exe --version
& target/x86_64-pc-windows-msvc/release/rdownloader-capture.exe --version
```

`.cargo/config.toml` sets `+crt-static` for this target, so the MSVC runtime is linked statically and the portable EXE files do not need a separate Visual C++ redistributable. Both binaries receive a `longPathAware` manifest as a native `RT_MANIFEST` resource via `embed-resource`; the Windows SDK resource compiler must be on the PATH for this. Registry, autostart, file association and keyring integration are tested on the respective native CI runners.

### Windows cross-build from Linux/WSL2

The fast cross-build needs `cargo-xwin`, LLVM/Clang, LLD, NASM, CMake and Ninja. `cargo-xwin` downloads the MSVC CRT and the Windows SDK itself on first invocation.

```bash
sudo apt-get install -y clang cmake lld llvm nasm ninja-build
cargo install cargo-xwin --version 0.23.1 --locked
rustup target add x86_64-pc-windows-msvc
npm ci --prefix web && npm run build --prefix web
cargo xwin build --locked --release --target x86_64-pc-windows-msvc \
  -p rdownloader -p rd-capture
```

`rdownloader doctor` names missing cross-build tools concretely. If `clang-cl` or `lld-link` are missing in WSL and no sudo access is available, the reproducible Docker build produces both EXE files in the (gitignored) folder `artifacts/windows/`:

```bash
docker build --file docker/Dockerfile.xwin --target artifacts \
  --output type=local,dest=artifacts/windows .
```

From Windows the files are reachable via `\\wsl.localhost\<Distro>\home\<user>\projects\rDownloader\artifacts\windows`. Exporting to `/mnt/c/...` does not work when the drive is mounted read-only in WSL.

### Docker (linux/amd64, linux/arm64)

`docker/Dockerfile` is a multi-stage build: frontend in `node:24-bookworm-slim`, Rust build in `rust:1.98-bookworm`, runtime in `debian:bookworm-slim`. Besides `rdownloader` the runtime carries the external helpers the service shells out to — ffmpeg, yt-dlp, streamlink, gallery-dl, apprise, 7-Zip and par2 — so media, streams, unpacking, PAR2 repair and Apprise notifications work without further setup. The desktop capture agent is not included.

```bash
scripts/build-plugins.sh     # otherwise the image ships no bundled plugins
scripts/docker.sh build      # caps cargo, works around the Docker Desktop credential helper
```

[`docker/README.md`](../docker/README.md) is the deployment guide: volumes and why every storage root needs one, PUID/PGID, time zone, the bundled tools, a Synology NAS walkthrough and troubleshooting.

### Plugin components (WebAssembly)

The provider plugins are built with `cargo-component` as WASM components without WASI. `cargo-component` additionally needs the `wasm32-wasip1` target for its own componentisation; the finished plugin nevertheless imports only the WIT host contract.

```bash
cargo install cargo-component --version 0.21.1 --locked
cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-ddownload
cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-premiumize
```

The results are located under `target/wasm32-unknown-unknown/release/*.wasm`. A plugin must import nothing outside `rdownloader:plugin` — no `wasi:` interface above all. `scripts/check-plugin-imports.sh` enforces that by reading the component's import section with [`wasm-tools`](https://github.com/bytecodealliance/wasm-tools) (`cargo install wasm-tools --locked`); `build-plugins.sh` runs it after every build and CI runs the same script. Without `wasm-tools` installed the script falls back to a byte scan and says so — that scan cannot tell an import from a string literal, which is precisely why it is no longer the check.

### Release pipeline

A tag of the form `v*.*.*` triggers `.github/workflows/release.yml`. The pipeline builds `rdownloader` and `rdownloader-capture` for Linux x86_64, Windows x86_64 as well as macOS x86_64 and aarch64, packs them with the matching portable launchers, `LICENSE` and `README.md`, and adds the NZB helper app to macOS archives. It also produces an SPDX SBOM and `SHA256SUMS`, signs the checksums with Sigstore and publishes a GitHub release. In parallel, the multi-arch container image is pushed to `ghcr.io/<owner>/rdownloader` with the tags `v<version>` — the tag's own name, e.g. `ghcr.io/degoya/rdownloader:v1.3.0` — and `latest`, equipped with provenance and SBOM, and signed with `cosign`.

## Plugins

Twenty-seven hoster/multihoster resolvers — DDownload, Premiumize, Rapidgator, Nitroflare, KatFile, 1fichier, Keep2Share, FileJoker, MediaFire, KrakenFiles, Turbobit, HitFile, Pixeldrain, AllDebrid, Debrid-Link, LinkSnappy, Real-Debrid, TorBox, Offcloud and Seedr — are built as signed `.rdplug` components and ship in the release next to the EXE under `plugins/`; `serve` installs newer versions automatically. Each hoster's protocol logic exists once. Fifteen of them are additionally compiled into the application as a fallback, so for those the two builds cannot behave differently; the newer resolvers — Real-Debrid, TorBox, Put.io, Offcloud, Pixeldrain and the cloud drives — ship as components only.

Most resolvers take an API key. **Real-Debrid takes an application of your own.** Register one at [real-debrid.com](https://real-debrid.com/) under *My account → API*, then add the account here with its **client ID as the username** and its **client secret as the credential**, and start the sign-in: the interface shows a short code to confirm at real-debrid.com, and the token that follows is renewed without anybody being asked again — you are never asked for a code twice.

The registration is per installation on purpose. An OAuth client secret shipped in an open-source repository would not be secret — it would stand in the git history, in every signed `.rdplug` and in every release artefact — and Real-Debrid's rate limits are per application, so one shipped registration would put every installation in the world into one bucket. Your own means your own limits and your own revocation.

**Pixeldrain needs no account at all, and takes one if you have it.** Public files go through the provider's own API, and the address a job carries is the file's identifier rather than a signed link, so a download that waits in the queue still works when its turn comes. A list address (`/l/…`) is unpacked into its files by a second plugin. Without an account the provider's limits are reported honestly instead of worked around. To lift them, add a Pixeldrain account and paste the **API key** from your Pixeldrain account settings — no user name: Pixeldrain takes the key as the HTTP Basic password, and rDownloader sends it to `pixeldrain.com` only, on the API calls and on the download itself.

Turbobit and HitFile — one operator, two brands — download without an account through the site's own Turnstile and countdown, one file per guest window; with an account they take the premium mirror. The password only ever reaches `app.turbobit.net` / `app.hitfile.net`.

**MEGA: public file links, public folder links, account sign-in and your own account's files work.** Paste a
`mega.nz/file/…#key` address and the file downloads, decrypted, under its real name. Paste a
`mega.nz/folder/…#key` address and the LinkGrabber lists what is behind it — names, sizes and
the folder structure — so you can pick before anything is queued. One folder link brings back at
most a thousand files; a larger folder is refused with a message saying so, rather than handed
over as a package quietly missing its end.

MEGA encrypts every file in the browser, so the key is the part of the link after the `#` and
the provider never has it. rDownloader decrypts while it writes, which means no second pass over
the file and no second copy on disk, and a link whose key does not fit its file is refused before
the first byte is fetched instead of after the last. When the download finishes it is checked
against the value MEGA published for that file. **The key is never written down in the open**:
it goes into the encrypted store the moment the link is pasted, the address kept in the queue is
the short one without it, and it appears in no row, no log line and nothing the interface shows.

**Signing in to a MEGA account works, and your password still never enters the plugin.** MEGA's
sign-in is unusual: it does not take the password but a value derived from it, and the half that
is not sent unwraps your account's master key. Plugins in rDownloader never see a credential —
the application substitutes them on the way out, so a plugin can send one and never read one —
which for a long time meant MEGA accounts simply could not be built. They now work the other way
round: the plugin *names* the credential and the steps, the application does the arithmetic, and
only the value MEGA is meant to receive comes back. The half that opens your master key is never
handed over, and the work is charged against the same budget the plugin would have spent doing it
itself, so nothing is gained by asking.

**Files in your own MEGA account download too.** Paste `mega.nz/fm/<handle>` for a folder of the
signed-in account and the LinkGrabber lists it like a shared folder; each file resolves, decrypts
and is checked the same way. Every file key in an account is locked under the account's master
key, and that key stays inside the application: a plugin hands over the locked file key and gets
back that one file's key, never the master key, and your password stays in the account settings
rather than being replaced by the session at sign-in. This has been tested against recorded and
constructed answers, **not yet against a real MEGA account**. What remains out: files that reached
your account through somebody else's share, MEGA accounts old enough to still use the legacy key
derivation, two-factor sign-in, and password-protected share links (`#P!`).

**MediaFire takes no account at all.** Public file links resolve through MediaFire's documented API and the file page's download button; folders and `/?key,key` lists are listed by the sibling `mediafire-crawler`. Private and password-protected files, the malware advisory and the per-IP download threshold are reported with their own codes rather than worked around, and a captcha — none was seen on the file page when this shipped — is handed to the configured solver or answered in the browser through the extension. Premium downloads are not offered: MediaFire's official route needs an application registered in your own account, and the project ships no key.

DDownload takes either: pick **Account credentials** and enter the username and password, and the application signs in and keeps the session itself, or pick **API key** and paste the key from Account → API. The credential never reaches the plugin — it is substituted inside the application and is pinned to the host its own mode talks to.

That session is the application's own, not the browser's: rDownloader signs in with its own HTTP
client and its own cookie jar, so a browser on the same machine stays signed out and that is not
a fault. *Test* on such an account asks the session it already holds before it makes a new one,
and signs in only when there is none — signing in over a standing session used to fetch the
login page *as a signed-in user* and report the missing sign-in form about a session that was
delivering a file at that moment. What the test reports about a subscription is only what it
read: an expiry from the provider's API makes it premium, while a sign-in or a pasted cookie
session proves the credentials and leaves the subscription unstated, which the result says in
as many words instead of assuming either answer (RD-109-34).

A plugin can also carry a **job that runs at the provider** (RD-107-06): a magnet or a `.torrent` handed to a debrid account, which runs there for minutes or hours, stops half-way to ask which files you want, and leaves something in the account that only an explicit delete removes. Such jobs are watched, answered and ended under **Remote jobs** in the sidebar, beside the subscriptions. `plugins/realdebrid-torrents` is the first one, a sibling of the Real-Debrid resolver that unrestricts the links it hands back; **`plugins/torbox-jobs` is the second, and it carries all three kinds at once** (RD-120-01) — a magnet or a `.torrent` becomes a TorBox torrent, an `.nzb` a Usenet download, an ordinary link a web download, behind three sets of endpoints and one state machine. TorBox asks nobody which files they want, so it never stops half-way: it fetches the whole job and every file arrives in the LinkGrabber as one package, which is where you pick. A TorBox account takes the API key from your settings page and nothing else; the address a finished file is fetched from is renewed by `plugins/torbox` on every attempt, so a download paused for an hour resumes instead of failing on a dead ticket. `plugins/premiumize-transfers` is a further one (RD-120-23), and the first that takes all three shapes a source can have: a magnet, a plain link, or a `.torrent`, `.nzb`, `.rsdf` or `.dlc` container uploaded as a file — because Premiumize's own `transfer/create` accepts `src` either way; Premiumize has no call that says which files of a transfer to keep, so it asks no question and hands every finished file over as one package. The form under **Remote jobs** offers only accounts whose service actually has such a plugin installed, read from the installed manifests; before 1.2.0 it offered all of them and refused after the button. Since 1.2.0 it also takes `.torrent` or `.nzb` files instead of a magnet, up to 16 MiB each (RD-120-31) — several at once, chosen together or dropped on the page, each handed over as its own job with its own state and, when it fails, its own reason (RD-120-51) — before that, no path in the application handed a plugin a container at all, however many of them accepted one. The contract keeps every call short and leaves the waiting, the remote identifier, the clock and your answer with the application, which writes them down so a restart never submits the same magnet twice — `docs/adr/0003-a-job-that-runs-at-the-provider.md` explains why that line is where it is. Since 1.0.8 the application drives these jobs (RD-108-03): a sweep submits, adopts and polls every row, a second paste of the same magnet is refused before anything leaves the machine, a restart adopts the torrent the provider already holds instead of creating a second one, and what the provider finished arrives in the LinkGrabber as one package. Since 1.0.8 such a job is also visible and steerable (RD-108-04): Settings → Accounts carries a card listing every job with the stage it is in and the progress the provider measured, one waiting for a selection shows the entries it offered and sends back only those, and each advance reaches the page over `remote_job.changed` rather than a poll. **Deleting at the provider is a separate, confirmed act**: it is the only path in the application that reaches a plugin's `discard`, the request has to carry the confirmation and not merely arrive, and afterwards the entry stays in the list — in `discarded`, still naming the job the provider knew — so an account that lost a torrent can be shown which request removed it. Removing an entry from the list is the other button and touches nothing at the provider. **What is still missing is a run against a real account.**

A plugin can also be a **transfer backend**: it carries the bytes of a protocol this application does not know, on the same queue and under the same speed limit. It never sees a path — it writes into the download's own staging file — and the application verifies the length itself before moving the finished file into place. `plugins/example-transfer` is the reference implementation.

**Google Drive** ships as three sibling plugins, and unlike the twelve above it exists only as plugins — there is no compiled-in fallback, which is the rule for everything added since the plugin platform: a provider exists exactly as long as its package is installed. It is also the shape the other cloud drives will follow. Only a resolver manifest may declare a provider — that is what creates the account and owns the vault reference — so `google-drive` resolves a file and owns the row, `google-drive-crawler` lists a folder or a shared drive, and `google-drive-oauth` signs the account in through Google's own endpoint with PKCE and the `drive.readonly` scope. Files, shared links, folders, shared drives and Workspace documents all work; a Doc, Sheet or Slide is exported per document type, and the name and extension it will arrive under are visible in the LinkGrabber before anything is queued.

**Setting Google Drive up takes one step this application cannot do for you: registering an OAuth client.** No client id ships in this repository, deliberately — Google counts its quotas per client, so a compiled-in one would put every installation in the world on a single shared allowance, and it would sit unrevocably in the git history and in every release artifact besides. Registering your own takes a few minutes and gives you a quota nobody else is spending:

1. In the [Google Cloud Console](https://console.cloud.google.com/), create a project.
2. Enable the **Google Drive API** for it.
3. Under **APIs & Services → OAuth consent screen**, add yourself as a test user (an unverified app may only be used by its test users, which is what you want here).
4. Under **Credentials**, create an **OAuth client ID** of type **Desktop app**, and add `http://127.0.0.1:8710/api/v1/oauth/callback` as an authorised redirect URI — adjust the port if you serve on another one; Google matches a loopback redirect on everything but the port.
5. In rDownloader, add a Google Drive account, paste the **client ID** into the *OAuth client ID* field, and press sign in. There is nothing to paste into the secret field: the sign-in fills it.

The only scope asked for is `https://www.googleapis.com/auth/drive.readonly` — read what the account can already see, and nothing else. It cannot delete, cannot share and cannot write. Until a client ID is entered, starting a sign-in refuses with `oauth.client_not_configured` and repeats these steps rather than sending you to a Google error page.

**OneDrive and SharePoint** ship the same way, as three sibling plugins following the interface Google Drive decided: `onedrive` resolves a file or sharing link and owns the provider row, `onedrive-crawler` lists a folder sharing link, and `onedrive-oauth` signs the account in at Microsoft. Only Microsoft Graph is used, and a sharing link is handed to it whole, encoded as `/shares/{id}` the way Microsoft documents it — OneDrive's short `1drv.ms` links, the long `onedrive.live.com` address, and every SharePoint tenant's `:f:`, `:w:`, `:x:`, `:b:` sharing links all work, whether they were shared with the account or belong to it. The type letter in the link decides what happens: `f` is a folder and is listed, everything else is a file; the long personal address says neither, so it is asked and answers with one file when that is what it is. The resolver always hands the transfer the stable `/content` route rather than the pre-authenticated download address every item also carries, so an address that expires after an hour is simply asked for again before a partial file continues — short-lived addresses are renewed without losing progress because nothing short-lived is ever stored.

**Setting OneDrive up takes the same one step: registering an application of your own.** No application id ships in this repository, for the reasons given above for Google — Microsoft throttles per application, and an id in a public repository is revocable by nobody:

1. In the [Microsoft Entra admin center](https://entra.microsoft.com/), under **App registrations**, create a new registration. Choose **Accounts in any organizational directory and personal Microsoft accounts**, so that a personal OneDrive and a work or school account can both sign in through the `common` endpoint.
2. Under **Authentication**, add the platform **Mobile and desktop applications** and enter `http://127.0.0.1:8710/api/v1/oauth/callback` as its redirect URI — adjust the port if you serve on another one, and enter exactly the address you serve on, because Microsoft matches it exactly.
3. On the same page, switch **Allow public client flows** on. That is what lets a headless installation sign in with a device code instead of a browser: the interface shows a short code to type at `microsoft.com/devicelogin` on any other screen.
4. Under **API permissions**, nothing has to be added by hand — the sign-in asks for `Files.Read.All` and `offline_access` as delegated Microsoft Graph permissions, and the person consents at sign-in. A tenant whose administrator requires consent for new applications will refuse until it is granted.
5. In rDownloader, add a OneDrive account, paste the **Application (client) ID** into the *Application (client) ID* field, and press sign in. There is nothing to paste into the secret field: the sign-in fills it. Where a browser can reach the application it opens Microsoft's sign-in page; otherwise a device code is shown.

The scope is `Files.Read.All` plus `offline_access` — read what the account can already see, and renew that without asking again. It is not `Files.Read`, and that is deliberate: a sharing link is reached through Graph's `/shares` route, which Microsoft grants to `Files.Read.All` and to nothing narrower, and sharing links are the whole point. It cannot delete, cannot share and cannot write. Until an application id is entered, starting a sign-in refuses with `oauth.client_not_configured` rather than sending you to a Microsoft error page.

A plugin can also be a **folder crawler**: it turns one address that stands for many files — a cloud folder, a directory share — into the files behind it, with their names, their sizes and the folder they sat in, which becomes the package suggestion. `plugins/premiumize-crawler` lists a Premiumize.me cloud folder this way, `plugins/google-drive-crawler` a Google Drive folder or shared drive, `plugins/onedrive-crawler` a OneDrive or SharePoint folder sharing link, and `plugins/dropbox-crawler` a Dropbox folder or shared folder link, `plugins/box-crawler` a Box folder or shared link, `plugins/pcloud-crawler` a pCloud folder or public link, `plugins/nextcloud-crawler` a public Nextcloud or ownCloud folder share, and `plugins/directory-index-crawler` an open directory index of Apache, nginx, Caddy or lighttpd, and `plugins/peeplink-crawler` the hoster links behind a PEEPLink or AlfaLink entry page. What a crawler returns is a proposal: it goes through the same review, the same domain blocklist and the same routing rules a pasted link does, and the walk bounds its own depth, breadth and cycles. A crawler whose service is self-hosted declares `*` in its manifest and is narrowed, for the duration of one crawl, to the host of the address that was pasted; a crawler that recognises an address by the shape of its path may say "not mine after all", and the link then goes to the next crawler instead of ending there. A password-protected Nextcloud or ownCloud share is opened by appending its password to the pasted address after a `#`, and its files are then also downloaded: the crawler puts the user name the public endpoint fixes into the addresses it hands back — never the password — and the application stores the password encrypted in the secret vault as an authentication profile scoped to that one share, which the queue applies to every file it fetches from it. If no crawler claims the address at all — a typo in the host name, or a share whose plugin is not installed — the fragment is dropped rather than kept, so a pasted password never reaches a LinkGrabber row, a listing or a log line; no link candidate is stored with a fragment.
A release page — a board, a paste service — is a different shape of problem from a folder: its structure changes with a theme update and its domains change every few months, and what has to be described is an address pattern, a container and two regular expressions, not code. So pages are described by **site rules** rather than plugins (RD-110-04): the project's rules are one signed file under a trust root of their own, refused as a whole when the signature does not hold, the format version is unknown or the file was withdrawn. **Since 1.3 no rule ships with the program** (RD-130-07): every release carries the file as `rdownloader-site-rules.json` beside its packages, and importing it under *Settings → Site rules* stores its rules as your own, switched off, beside any you write. [docs/site-rules.md](site-rules.md) is the format, with a commented example. Rules age, so every rule names a real address to probe and a self-test says how it fared: `rdownloader doctor site-rules` sorts each rule into `ok`, `structural`, `blocked` or `dead`, prints a table, exits non-zero on anything but `ok`, and a rule found dead is skipped by the recognition until a later run revives it — never deleted (RD-110-09). No rule and no protector plugin is implemented without the service having been measured first. The file carries **eight rules** (RD-110-11, RD-120-17): the release boards `scnlog.me`, `downmagaz.net` and Scene-RLS, GetComics, AvaxHome, CGPersia, the ControlC pastebin and the ViperGirls board in a group of its own, `adult` (five groups in all: `board`, `paste`, `ebooks`, `graphics`, `adult`) — each opened on its live service on 2026-09-22, with seventeen domains those services have left behind carried as `dead` so an old bookmark is rewritten instead of refused. Library Genesis and SatDL were in it and are not any more: the same check found libgen.bz no longer resolving the address its rule reads, and satdl.com was withdrawn. The rules are visible and editable under *Settings → Site rules* (RD-110-08): every rule with its hosts and self-test verdict, a switch per rule and per group that survives a restart, an editor whose seven step kinds are named fields rather than a JSON box, a trial run against a real address before saving, a duplicate that copies a working rule switched off and opens it in the editor (RD-130-07), and export and import — an imported rule arrives switched off, signed or not, and is activated one deliberate request at a time.

**Dropbox** follows the same shape: `dropbox` resolves a file and owns the provider row, `dropbox-crawler` lists a folder of your own Dropbox or a shared folder link, and `dropbox-oauth` signs the account in with PKCE. Shared file links (`/s/…`, `/scl/fi/…`), shared folder links (`/sh/…`, `/scl/fo/…`), folders of your own Dropbox (`https://www.dropbox.com/home/<folder>`) and files inside any of them (`?preview=<name>`, as Dropbox's web interface spells them) all work, and Dropbox's `content_hash` is verified after every download. A password-protected shared link is opened through the official `link_password` argument: add `?link_password=…` to the pasted address. Every call goes through the official API, so even a public shared link needs a signed-in Dropbox account.

**Setting Dropbox up is the same one step: registering an app of your own.** Dropbox rate-limits per app, so no app key ships in this repository.

1. In the [Dropbox App Console](https://www.dropbox.com/developers/apps), create an app with **Scoped access** and access to the **Full Dropbox** (an *App folder* app cannot open shared links or paths outside its folder).
2. Under **Permissions**, tick `account_info.read`, `files.metadata.read`, `files.content.read` and `sharing.read`, and submit.
3. Under **Settings → OAuth 2 → Redirect URIs**, add `http://127.0.0.1:8710/api/v1/oauth/callback` — Dropbox matches the redirect URI exactly, port included, and allows plain `http` for the loopback address only; adjust the port if you serve on another one.
4. In rDownloader, add a Dropbox account, paste the **App key** into the *App key (OAuth client ID)* field, and press sign in. No app secret is needed — the sign-in is a public client with PKCE — and there is nothing to paste into the secret field.

While the app is in development status Dropbox limits it to a handful of linked users, which is exactly right for a personal installation.

**Box** follows the same shape, with one credential more: `box` resolves a file and owns the provider row, `box-crawler` lists a folder of your own Box or a shared link, and `box-oauth` signs the account in. Folder addresses (`https://app.box.com/folder/<id>`), file addresses (`/file/<id>`), shared links (`/s/<name>`) and items reached through a shared link (`/s/<name>/file/<id>`, `/s/<name>/folder/<id>`) all work, on `app.box.com` and on an enterprise's own `<name>.app.box.com`. A bare `/s/<name>` does not say whether it points at a file or at a folder — Box spells both the same way — so it is asked, and a link that turns out to be one file comes back as that one file. A password-protected shared link is opened through the official `boxapi` header: add `?shared_link_password=…` to the pasted address, and the password reaches Box in that header and in no address the bytes come from. Every call goes through the official Content API, so even a public shared link needs a signed-in Box account.

The download address is Box's own `/2.0/files/<id>/content` **pinned to the file version it was resolved at**, never the temporary `dl.boxcloud.com` address Box redirects to. That is what makes a resume safe: the address still means the same bytes after a restart, so a file somebody replaced while the download was paused cannot be spliced onto what is already on disk. Box's SHA-1 is carried as the checksum and is verified after the download.

**Setting Box up is one step: registering an application of your own.** Box counts its rate limits per application, so no client id and no client secret ship in this repository — and unlike the first three drives, Box needs both.

1. In the [Box Developer Console](https://app.box.com/developers/console), create a **Custom App** with **User Authentication (OAuth 2.0)**.
2. Under **Configuration → Application Scopes**, tick **Read all files and folders stored in Box** and nothing that writes.
3. Under **Configuration → OAuth 2.0 Redirect URI**, add `http://127.0.0.1:8710/api/v1/oauth/callback` — Box matches the redirect URI exactly, port included; adjust the port if you serve on another one.
4. In rDownloader, add a Box account, paste the **Client ID** into the *Client ID (OAuth 2.0)* field and the **Client Secret** into the secret field, and press sign in. Box has no device code, so a browser this machine can reach is needed. The client secret stays stored: Box requires it for every renewal, and the access token the sign-in obtains is kept beside it rather than over it.

**pCloud** is the same three plugins again — `pcloud` resolves a file and owns the provider row, `pcloud-crawler` lists a folder of your own drive or a public link, and `pcloud-oauth` signs the account in — with one thing none of the others has: **pCloud is two separate installations.** A European account, its files and its link codes live at `eapi.pcloud.com`; an American one lives at `api.pcloud.com`, and neither knows anything about the other. Asking the wrong one gives you an answer that looks exactly like a bad password, which is why rDownloader never guesses twice: a pCloud address names its data centre in its host (`e.pcloud.com` and `e.pcloud.link` are European, `my.pcloud.com` and `u.pcloud.link` American), the account row shows which one answered, and the only two refusals that can mean "wrong data centre" — a refused token and a link code nobody issued — are retried once at the other and then settled. You do not have to know or configure which one you are on.

Public links in every spelling pCloud uses (`https://u.pcloud.link/publink/show?code=…`, `https://e.pcloud.link/…`, `https://my.pcloud.com/#page=publink&code=…`), folders and files of your own drive (`https://my.pcloud.com/#/filemanager?folder=…`) all work. A public link holding a single file arrives as that one file; one holding a folder is walked and arrives as a package. Checksums are taken from pCloud's `checksumfile` — SHA-256 in Europe, SHA-1 in both, MD5 in the United States — and verified after the download. pCloud's download links expire after a few hours, and that is handled by never keeping one: the queue remembers the file, not the ticket, and mints a fresh one for every attempt and every resume.

**Setting pCloud up is again one step: registering an application of your own.** pCloud rate-limits per application, so no application key ships in this repository.

1. In the [pCloud App Console](https://docs.pcloud.com/), create an application.
2. Add `http://127.0.0.1:8710/api/v1/oauth/callback` as a redirect URI — adjust the port if you serve on another one.
3. In rDownloader, add a pCloud account, paste the **App key** into the *App key (OAuth client ID)* field and the **App secret** into the secret field, and press sign in. Like Box, pCloud's token endpoint is a confidential client and offers no PKCE, so the secret really is needed; it is stored in the encrypted vault and the access token the sign-in obtains is stored beside it rather than over it.

pCloud issues no refresh token — its access token lasts until you revoke it — so nothing renews in the background, and if you ever revoke the application in pCloud, sign in again.

**Put.io** ships as three sibling plugins too, whose three are a resolver, a sign-in and a *remote job* — the same shape Real-Debrid and TorBox have: `putio` resolves a file in the account and owns the provider row, `putio-oauth` signs the account in at Put.io's own endpoint, and `putio-transfers` hands a magnet or a `.torrent` to the account and watches it run there. Paste a magnet or drop a `.torrent` on a Put.io account and it appears under **Remote jobs**; when Put.io has finished, every file it produced arrives in the LinkGrabber as one package, with its name, its size and the folder it sat in, and you pick what to fetch there.

Two things about Put.io are worth knowing before you use it. **Put.io downloads a torrent whole** — its API offers no way to leave a file out — so unlike Real-Debrid there is no selection *at the provider*; the file tree is shown in the LinkGrabber instead, before anything is fetched to this machine. And **a `.torrent` is handed over as the magnet it is equivalent to**: Put.io's `transfers/add` takes one address, so the file is read locally and its info hash, its name and every tracker it listed are put into a magnet. A torrent carrying no trackers and no peers reachable over DHT is the one case that cannot start this way.

What arrives in the queue is Put.io's stable per-file address, `https://api.put.io/v2/files/<id>/download`, and never the signed storage address Put.io would also hand out: that one expires, and a job that waited an hour for its turn would otherwise fail with a refusal nobody can act on. Put.io mints the short-lived address at the moment the bytes are fetched, and the account's token is attached by the application rather than written into the address. **Discarding a remote job cancels the transfer at Put.io and deletes no files**: what is already in the account stays there.

**Setting Put.io up is one step: registering an application of your own**, for the same reason as the others — Put.io counts its rate limits per application, and a client secret shipped in an open-source repository would not be secret.

1. At [app.put.io/oauth](https://app.put.io/oauth), create an application and add `http://127.0.0.1:8710/api/v1/oauth/callback` as its callback URL — adjust the port if you serve on another one.
2. In rDownloader, add a Put.io account, paste the **Client ID** into the *Client ID (OAuth 2.0)* field and the **Client Secret** into the secret field, and press sign in. A browser this machine can reach is needed: Put.io's out-of-band entrance for devices without one is not implemented here, because it is not stated in a published specification.
3. Put.io's tokens do not expire and it issues no refresh material, so nothing is renewed in the background and nothing asks you again — until you revoke the application at Put.io, which ends the sign-in at once.

**Seedr** ships as two sibling plugins and a shared crate: `seedr` resolves a file in the account and owns the provider row, `seedr-jobs` hands a magnet or a `.torrent` to the account and watches it run there. Paste a magnet or drop a `.torrent` on a Seedr account and it appears under **Remote jobs**; when Seedr has finished, every file it produced arrives in the LinkGrabber as one package, with its name, its size and the folder it sat in, and you pick what to fetch there. Like Put.io, Seedr downloads a torrent whole, so there is no selection at the provider, and a `.torrent` is handed over as the magnet it is equivalent to.

**Setting Seedr up takes one step.** Seedr's REST API is HTTP Basic and nothing else -- its own documentation says so -- and it is available only on a premium plan. Add a Seedr account and enter the **e-mail address** and **password** you sign in to Seedr with. The password is kept in the credential store and never appears in a log or an address; rDownloader pairs it with the e-mail address only at the moment a request leaves for `www.seedr.cc` -- the API calls and the downloads of the finished files alike, and never on a redirect to another host. No separate authentication profile is needed; one made for `www.seedr.cc` before 1.2 can be deleted.

Six more types plug into the rest of the application: **intake parsers** (bundled: Metalink and JDownloader `.crawljob`), **authentication providers**, **metadata enrichers** (bundled: SponsorBlock, and a film and series lookup that turns a release name into rating, year, genre and runtime — keyless, because an enricher has no account, and silent about anything that is not a release name), **notification destinations**, **post-processing steps** (bundled: SHA-256 and MD5 sidecar verification, and a file-name tidier that replaces spaces with dots) and **storage destinations**. Each is one service, one format or one destination per plugin, so they can be updated, versioned and switched off one at a time. An installed plugin can be deleted, or switched off without being removed — it stays listed so it can be switched back on, and takes effect on the next start.

Key generation, packaging (`rdownloader plugin keygen|package|verify|install`) and rotation are described in [docs/plugins.md](plugins.md). To write your own, `rdownloader plugin new --type resolver|transfer|intake|auth|oauth|crawler|enricher|notifier|postprocess|storage|remote-job --out DIR` scaffolds one that builds, packages and passes conformance before you change a line, and `rdownloader plugin conformance <package> --json` answers whether this application would run it. The templates and a reusable CI workflow are in [sdk/](../sdk/).

## Portable start and autostart

The release archives contain launchers next to the executables. They start both processes by default; `server` and `capture` select just one process. Output goes below `logs/` — appended by the Unix launchers, rewritten on each start by the Windows one — and the launchers keep their PID files below `run/`:

```bash
./start-rdownloader.sh                 # Linux: server + capture
./start-rdownloader.sh server          # Linux: server only
./stop-rdownloader.sh capture          # Linux: capture only
./start-rdownloader.command            # macOS: server + capture
./stop-rdownloader.command all         # macOS: both
```

On Windows use `start-rdownloader.bat [all|server|capture]` and `stop-rdownloader.bat [all|server|capture]`. To get the portable version running for the first time:

1. Extract the release archive.
2. Run `start-rdownloader.bat`.
3. Open [http://127.0.0.1:8710/](http://127.0.0.1:8710/) in a browser.
4. Complete the setup wizard to set the admin password, pair the capture client, choose the download folder and configure accounts.
5. Run `stop-rdownloader.bat`.
6. Run `start-rdownloader.bat` again.

Two outcomes are reported rather than treated as a failed start: the capture agent has not been paired yet, and something is already listening on the Click'n'Load port 9666 — a second capture agent, often from autostart, or JDownloader. In both cases there is nothing to fix. Anything else is a real failure, and `start-rdownloader.bat` then keeps its window open so the reason stays readable; set `RDOWNLOADER_NO_PAUSE=1` to suppress that for an unattended run.

The launchers are primarily intended for portable operation and testing. A normal per-user login start is installed independently for the service and capture agent:

```bash
rdownloader autostart install
rdownloader-capture autostart install
```

Use both commands for a local desktop. When the service runs on a NAS, configure only the desktop agent with the NAS URL and install only its autostart:

```bash
printf %s '<TOKEN>' | rdownloader-capture configure --service https://nas.example --token-stdin
rdownloader-capture autostart install
```

`autostart remove` on the respective executable disables future login starts without stopping the current process. Linux uses systemd user units, macOS uses LaunchAgents, and Windows uses a windowless per-user launcher. Re-running `rdownloader-capture.exe autostart install` replaces the legacy 0.1.0 Windows registry entry that opened a permanent console window. Stop the old running agent once with the portable stop script or sign out before testing the replacement.

## Capture agent

After pairing in the UI, the token shown once is stored in the operating-system keyring, with a user-private file fallback on Unix:

```powershell
'<TOKEN>' | rdownloader-capture.exe configure --token-stdin
rdownloader-capture.exe association install
rdownloader-capture.exe autostart install
rdownloader-capture.exe run
```

Drop the `.exe` suffix on Linux and macOS.

`--token-stdin` is the documented way to hand the token over. `--token <TOKEN>` still works, but an argument is readable by every local user through `ps` or the task manager and a shell keeps it in its history; standard input is neither.

A successful pairing puts the token in exactly one place: if the keyring takes it, an earlier `capture.token` file from a machine where the keyring had been unavailable is removed, so a revoked token cannot keep being presented. The plaintext fallback remains for a host with no working keyring, and is only consulted when the keyring holds nothing.

The paired address is stored in `capture.json` next to the keyring entry. It is written atomically and checked for being `http` or `https` in one place, so nothing that reads it — the HTTP client, the tray's **Open rDownloader** — can be pointed at something else by whoever can write that file; a configuration that cannot be read is reported rather than silently replaced by the loopback default.

A service address that is neither loopback nor `https` is refused at pairing time, because the capture token is a long-lived bearer credential that rides on every five-second poll and for the whole life of the event stream. `--allow-insecure-service` accepts it for a network you vouch for. Pairings made before this rule keep working — it is enforced when pairing, not when connecting — and the agent writes a warning naming the address on every start until you pair again with `https`. Linux installs the `.nzb` MIME handler through the freedesktop tools. The macOS release contains `rDownloader Capture.app`; keep it next to `rdownloader-capture`, run `association install`, and select it once through Finder's **Open With** dialog if another application is already the default.

`association remove` takes back exactly what `association install` wrote, including the registration that names the sender of the agent's desktop notifications. The one thing it deliberately leaves alone is the shared `SystemFileAssociations\.nzb` parent key, because other installed software registers its own verbs there; only rDownloader's own branch below it is removed.

`association install` also wires up an "Import into rDownloader" entry for right-clicking an `.nzb` file, regardless of which app is the default handler. On Windows 11 it's a classic verb, so it appears under **Show more options** (Shift+F10) rather than the top-level menu — top-level placement needs a signed IExplorerCommand handler, which is out of scope. On Linux it shows up in file managers' **Open With** list. On macOS there's no Finder context-menu item; use **Open With** and pick the helper app as above.

Clipboard monitoring reads at most 1 MiB of text; anything longer is left alone rather than hashed and scanned once a second, and the read itself runs off the agent's runtime so a program that is slow to hand the clipboard over cannot stall the rest of the agent. Clipboard monitoring uses the native desktop clipboard on Windows, macOS, X11 and supported Wayland compositors. Pure Wayland sessions need the data-control protocol; when the compositor does not provide it, Click'n'Load continues to work and the limitation is written to the capture log.

The status line also carries what the agent notices about itself: a background task that ended — clipboard monitoring, intake notifications, the transfer poll — and a Click'n'Load address it did not get. Neither puts itself right, so neither is left to the log alone. If another Click'n'Load listener holds only one of the two loopback addresses, the agent keeps running on the other and says which one; it exits with the "port busy" code only when not one address can be had.

On Windows and macOS `run` shows a tray respectively menu-bar icon with the paired service, **Open rDownloader** and **Quit**; on macOS the agent stays out of the Dock. Its status line and tooltip name what the queue is doing — `3 active · 2 queued · 47% · 12.4 MB/s · 1h 12m left` — with the rate and the remaining time taken from the service, so they are the same figures the web interface shows. Both stand only while something is actually running, and the remaining time is left out entirely wherever the service has nothing honest to say: an unknown size, a rate of zero, a paused transfer. The tooltip is cut to what Windows can hold; the menu entry is not. `--no-tray` (or `RDOWNLOADER_NO_TRAY=1`) forces the headless mode, and an icon that cannot be created — a remote session, for instance — is logged as a warning and leaves the agent running headless. Linux is always headless: no StatusNotifier/GTK stack is linked in deliberately, and the systemd user unit from `autostart install` provides the status instead. The line is drawn at the toolkit, not at the display server: clipboard watching is wanted on Linux, so `arboard` is ungated and the agent does link X11 and Wayland *client* libraries — no GTK, no WebKitGTK, and where no display server is reachable the clipboard reports an error instead of taking the agent down.

The agent also shows a desktop notification when links arrive in the LinkGrabber. Everything that arrives inside a three-second window is one toast naming the total, rather than one toast per import: a page handing links over in a loop fills the desktop otherwise, and the display used to sit in the middle of the stream reader, so each notification also slowed the reading. It reads a capture-scoped event stream that carries that event and a captcha signal — a bare count of waiting widgets, which the agent no longer acts on — and nothing else, so the token cannot see download paths or credential changes. Linux uses the freedesktop D-Bus interface, which needs no GTK and so leaves the headless decision above intact; on Windows the sender is registered by `association install`, without which the toast would arrive attributed to PowerShell. A desktop with no notification daemon is a warning in the log and nothing more. `--no-notifications` (or `RDOWNLOADER_NO_NOTIFICATIONS=1`) turns it off. The text is English, like the tray menu — the agent carries no translation catalogue.

A dropped connection no longer loses what arrived in the meantime. The agent reconnects with the id of the last event it saw (`Last-Event-ID`), and the service replays what came after it from an in-memory buffer of its last 4096 events — through the same narrow filter, so the token still sees only the intake and the captcha count. The service also tells its clients how soon to knock again (`retry: 5000`), so an agent waiting out an outage tries every five seconds instead of backing off to a minute. **A resume does not survive a restart of the service**: the buffer lives in the process, so after a restart the service answers every held id with `stream.expired`, the web interface re-reads its state as it always did on a reconnect, and the agent writes a warning that links which arrived during the outage were not announced. The same holds for the web interface's own stream under `/api/v1/events`, which resumes the same way.

The agent does **not** answer widget captchas. It used to: RD-107-03 opened the hoster's own page in a system WebView on the tray's event loop, and RD-109-11 took that window out again. A run against DDownload on 2026-09-20 settled it — the window opened instead of the extension's tab, the challenge could not be solved in it across several attempts, and full credentials typed into the sign-in page it showed went nowhere, because the plugin signs in over its own connection. Cloudflare Turnstile refuses an embedded WebView by design: the engine announces itself as `Microsoft Edge WebView2` in `navigator.userAgentData.brands`, a client hint that cannot be set, and hiding it would be defeating a bot check rather than answering one. Keeping a second, broken path next to the working one made the working one unreachable, because the watcher started on every desktop run and got there first. Widget captchas — reCAPTCHA v2, hCaptcha and Turnstile alike — are now answered in your real browser through the extension (below), or by a solver service; `wry` is gone from the agent, and so is the `captcha-webview` profile folder it kept (delete it by hand if an earlier version left one behind).

The agent listens exclusively on `127.0.0.1:9666` and `[::1]:9666`. It hands Click'n'Load submissions, HTTP(S) links from the clipboard and `.nzb` double-clicks over to the service's LinkGrabber.

Loopback is not a boundary against a browser: any page you have open can address that port. The listener therefore draws the line per route rather than handing the same wildcard to everybody.

- **`/flash`, `/jdcheck.js`, `/flash/add`, `/flash/addcrypted2` — JDownloader parity, cross-origin on purpose.** These carry `Access-Control-Allow-Origin: *` and answer `GET` as well as `POST`, because a Click'n'Load button is a request a hoster page makes from its own origin; an origin allowlist would lock out exactly the pages the mechanism exists for. The consequence is stated rather than hidden: while the agent runs, a page you visit can push links, a package name and an unpack password into the LinkGrabber without asking. They arrive in the LinkGrabber for review like any other capture, and they start no download by themselves — but if that is not a trade you want, run the agent with the port free of Click'n'Load by not starting it, or use the browser extension instead.
- **`/rdownloader/nzb` — the agent's own route, not reachable from a page.** It carries no `Access-Control-Allow-*` header and refuses any request that arrives with an `Origin` or a `Referer` header. Its only caller is this binary's own `open` subcommand behind the `.nzb` file association, which sends neither.
- **A refusal is a code, not prose.** The body of a refused request is one of `cnl_invalid_payload`, `cnl_invalid_key`, `cnl_no_links`, `cnl_payload_too_large`, `cnl_service_unavailable` or `cnl_foreign_origin`. The detail goes to the capture log — a readable error text would have been a padding oracle against the encrypted Click'n'Load path and a window into the service's state for whichever page made the call.
- **Nothing else answers.** The Flash-era `/crossdomain.xml` is gone, and a preflight is answered only for a path that exists.
- **Each route carries its own size limit,** refused before the body is decoded: 20 MiB for `/flash/addcrypted2`, 1 MiB for `/flash/add`, 64 MiB plus framing for `/rdownloader/nzb`. An encrypted payload whose key is a script is evaluated in a sandbox with an instruction budget, on a thread of the agent's own and at most two at a time, so a page cannot use the key step to stall clipboard monitoring or notifications.

## `rdownloader://` links

The capture agent can register itself as the handler for `rdownloader://` addresses, so a page or a script can hand links over with one click:

```bash
rdownloader-capture scheme install   # Windows and Linux
rdownloader-capture scheme remove
```

Two forms are accepted: `rdownloader://add?url=<link>` (repeatable, magnets included) and `rdownloader://open?path=<absolute local path to a .nzb>`. Everything else is refused. That refusal is the point of the feature's design: a scheme handler is reachable from any web page, so the parser is an allowlist — no `file:`, `javascript:` or `data:` links, no relative paths or `..`, no network shares or device namespaces (`//host/share`, `\\host\share`, `\\?\…`, `\\.\…`, which on Windows would make the agent dial SMB to a host the page named), at most 50 links and 8192 characters — and what it accepts goes into the same LinkGrabber review that the clipboard and the file association use. No new way into the service is opened. `.torrent` is not accepted: the import path hands the file to the NZB endpoint, so such an address used to parse and then break; a real torrent import is its own piece of work. The file is read with the 64 MiB cap applied to the read itself, so a file that grows while it is being handed over cannot get past it.

macOS is not supported: a URL scheme there is claimed through `CFBundleURLTypes` in an application bundle, which a bare executable cannot do. `scheme install` says so instead of appearing to succeed.

## Browser extension

For NAS/Docker setups where the desktop capture agent cannot be installed, the browser extension in [`extension/`](../extension/README.md) is the alternative: a context-menu entry and a popup that send links to the service, interception of regular browser downloads, and an action that shares a site's session for a single domain. Paired via a capture token from the System view. A download the page started with a `POST` is not taken over — repeating it would need its request body, which the extension does not read — so the browser finishes it and says why; see the extension's own README for the full permission list.

The extension is also where a **widget captcha** is answered (RD-108-02). It polls the service every 30 seconds for waiting reCAPTCHA, hCaptcha and Turnstile challenges, announces one with a notification and a badge, and lists it in its popup. One click there asks the browser for the hoster's origin — that one origin, at that moment — opens the hoster's own page in a tab and injects a reader that watches the widget's answer field and changes nothing on the page. The token goes to the waiting download through the same capture route the desktop agent uses, the tab closes, and the permission is released again. Closing the tab without answering declines the captcha. The web interface's captcha prompt says whether an extension is connected, because the service knows when one last polled.

## Command line client

Beyond `serve`, `doctor`, `openapi`, `plugin` and `autostart`, the binary can drive a running service — the local one or a remote one:

```bash
rdownloader queue list
rdownloader queue add "https://example.com/file.bin"
rdownloader queue pause <id>
rdownloader queue remove <id> --yes
rdownloader links add - < links.txt
./daily-links.sh | rdownloader links add - --category Series --enqueue
rdownloader links list
rdownloader links enqueue <package-id>
```

`links add` hands links to the LinkGrabber; `--category <name or id>` files them under a category, `--package <name>` keeps them in one package, and `--enqueue` puts them straight into the download queue instead (with the `api:intake` scope `queue add` needs; a category *name* is looked up, which needs `api:config`). rDownloader can also run such a script itself on a schedule — see *script subscriptions* in the feature list.

Both commands talk HTTP to the REST API; `--server` (default `http://127.0.0.1:8710`, or `RDOWNLOADER_SERVER`) points them at another host and `--token` (`RDOWNLOADER_TOKEN`) authenticates. There is no second code path that opens the database directly — a running service already holds that file. Read-only commands work with an `api:read` token.

Output is a table by default and the server's unchanged JSON with `--json`. Removals require `--yes`, because these commands end up in scripts where a prompt either hangs or is answered by accident. Failures exit with a code that says which kind: `2` bad arguments, `3` server unreachable, `4` credential rejected, `5` not found, `1` anything else.

## Automation client compatibility

The full endpoint matrix, the deliberate departures and what has actually been verified are in [docs/compatibility.md](compatibility.md).

Tools written against SABnzbd — Sonarr, Radarr, Lidarr, Readarr and similar — can use rDownloader as their download client. The adapter answers at `/api` and `/sabnzbd/api`, kept strictly apart from the native `/api/v1`, and authenticates with the same `api:*` token you create under **Settings → API & MCP**: paste it as the API key. A read-only token is refused here, because this surface adds and deletes.

For torrents there is a qBittorrent Web API v2 subset at `/api/v2`. It follows qBittorrent's own login shape — `POST /api/v2/auth/login`, then a `SID` cookie — but the credential is the same API token: leave the username as anything and put the token in the password field. The cookie carries the token itself, so a restart does not log the client out and revoking the token takes effect immediately. Supported: `app/version`, `app/webapiVersion`, `app/preferences`, `torrents/info`, `properties`, `files`, `filePrio`, `add` (torrent upload or magnet), `delete`, `pause`/`stop`, `resume`/`start`, `categories`, `createCategory` and `setCategory`. Torrents are addressed by their real info hash — read from the stored metadata, or from the magnet's `xt=urn:btih:` while metadata is still resolving, so a torrent is findable the moment it is added. Only magnets are accepted in `urls`, for the same reason `addurl` is refused above, and `deleteFiles=true` does not delete finished files.

Supported modes: `version`, `auth`, `get_config`, `get_cats`, `fullstatus`, `queue` (including `delete`, `pause`, `resume` and `value=all`), `history` (including `delete`), `addfile`, and the global `pause`/`resume`. Two deliberate departures: `addurl` is refused with a message naming `addfile`, because fetching a caller-supplied URL server-side would turn the API key into a request-forgery primitive; and `del_files=1` removes the queue entry without deleting finished files, which stays a deliberate action inside the application. Any other mode answers in SABnzbd's own failure shape — `HTTP 200` with `{"status": false, "error": …}` — rather than an HTTP error, which such clients read as the server being down.

## MCP server

AI assistants (Claude Code, Claude Desktop and other MCP clients) can drive the service through a built-in [Model Context Protocol](https://modelcontextprotocol.io) endpoint at `/mcp` (streamable HTTP, same port as the web UI). Create an API token under **Settings → API & MCP** — the same place used by REST, CLI and compatible download clients. It is shown once and stored only as a SHA-256 digest; then register the server:

```bash
claude mcp add --transport http rdownloader http://127.0.0.1:8710/mcp \
  --header "Authorization: Bearer <TOKEN>"
```

Clients configured through a dialog rather than a command line — the ChatGPT desktop app among
them — ask for the header value itself, often by naming an environment variable that holds it.
That variable must contain the complete `Bearer <TOKEN>`, and the token panel offers it in that
form ready to copy. **Configure exactly one source for the `Authorization` header**: when a
client sends two, only the first is read, so a correct token in second place never arrives and
the refusal is indistinguishable from a wrong one.

A refused token is logged. The line names what arrived — no header, a non-bearer header, an
empty bearer (an environment variable that did not resolve), several `Authorization` headers, an
unknown digest, or a token holding no API scope — together with the first eight characters of
the token's SHA-256 digest, which is what the token store holds, so the line can be matched
against the token list. The token itself is never written to the log. Raise the level with
`RUST_LOG=rd_api=warn` or lower if the default filter is narrowed.

### ChatGPT Desktop on Windows

Create a Windows user environment variable:

- Name: `RDOWNLOADER_AUTH_HEADER`
- Value: `Bearer <NEW_TOKEN>`

Then open `%USERPROFILE%\.codex\config.toml` and add:

```toml
[mcp_servers.rdownloader]
url = "http://127.0.0.1:8710/mcp"
env_http_headers = { Authorization = "RDOWNLOADER_AUTH_HEADER" }
enabled = true
default_tools_approval_mode = "prompt"
```

The endpoint exposes 162 tools. Downloads and the queue: `add_downloads`, `list_downloads`, `get_download`, `control_downloads` (pause/resume/cancel/remove), `get_status_summary`, `list_packages`, `delete_packages`, `collect_links`, `check_links`, `list_collector`, `enqueue_collector`. Hoster links go through the LinkGrabber flow (`collect_links` → `check_links` → `enqueue_collector`); direct HTTP(S)/magnet URLs through `add_downloads`. A container file goes in as base64: `import_container` for a `.dlc`, `.ccf`, `.rsdf` or `.txt` link list, `import_torrent` for a `.torrent`, `import_nzb` for an `.nzb`, each at most 48 MiB. They call the import routes' own handlers, so they answer and refuse exactly as a browser upload does; an NZB lands in review mode, and `list_nzb_imports`, `get_nzb_import`, `update_nzb_import` and `enqueue_nzb_import` review and queue it from there.

The configuration is writable, not just readable: `get_settings` and `update_settings` for the settings document, `list_configuration` for the inventory, and create/update/delete tools for categories, routing rules, storage roots, watched folders, provider accounts, proxy profiles, NNTP servers, notification destinations and rules, subscriptions, livestream channels, automations and plugins. Supported providers stay read-only, because nothing writes them. Update tools merge onto the stored row, so changing one field leaves the rest alone, and a `clear` list names the fields to reset to their inherited default. Destructive tools are named individually — `delete_category`, `delete_account`, `uninstall_plugin_version` — and say what they destroy.

Since 1.2 the toolbox also reaches the surfaces that arrived with 1.1, and reaches jobs that run at a provider: `list_remote_jobs`, `submit_remote_job` (a magnet, an address, or a `.torrent`/`.nzb` as base64 up to 16 MiB), `choose_remote_job_entries` and `forget_remote_job` for remote jobs; `get_transfer_stats` for the statistics; `list_log_records` and `list_audit_records` for the log and audit stores, with the same filters the views offer; `list_site_rules`, `set_site_rule_enabled` and `set_site_rule_group_enabled` for the release-page rules. Since 1.2 it can also start a test run from nothing: `get_data_reset_preview` says how much each store holds, and `clear_log_records`, `clear_audit_records` and `clear_transfer_stats` empty one store each, every one of them refusing to act without `confirmed: true`.

Since RD-120-32 **everything the interface can do, the toolbox can do too — except the capabilities the owner deliberately keeps out** (signing in, secrets, consents and irreversible changes outside this machine), with a listing tool first wherever an action needs an id: the LinkGrabber link by link (`list_candidates`, `update_candidate`, `move_candidates`, `reorder_candidates`, `enqueue_candidate`, `get_candidate_details`, `set_candidate_plan`, `set_candidate_mirror`, …), LinkGrabber package editing and ordering, queue order (`reorder_packages`, `reorder_downloads`), renaming (`rename_download`, `update_package`, `rename_package_folder`), `clear_finished_packages`, `extract_packages`, a torrent's detail, trackers and seeding (`get_torrent_details`, `update_torrent_trackers`, `set_torrent_seeding`), the post-processing inventory, the managed external tools (`list_managed_tools`, `manage_tool`), `get_storage_capacity`, and writing a site rule (`create_site_rule`, `test_site_rule`, …). **Nine capabilities stay out by the owner's decision, for one reason**: a tool that hands out a secret, takes one in, gives a consent, or changes something outside this machine irreversibly is not offered — signing in and API tokens, provider OAuth, testing stored credentials, remote logins, captchas, consent to replay a paid link, whole-area import and export, plugin installation, and deleting a remote job at the provider. So `discard` a remote job at the provider is not a tool while `forget` it from this list is. Since RD-120-55 the rest is here too, checked against the same line first: `list_remote_job_providers`, `get_power_status` and `cancel_power_action` (it stops a countdown; nothing starts one), `list_plugin_executions`, `get_plugin_messages`, the automation history, vocabulary and dry run, `list_notification_deliveries`, the subscription review list and its polls (`list_subscription_items`, `review_subscription_item`, `poll_subscription`, …, every address masked, since an indexer's carries its key), recording schedules and `record_stream_now`, `preview_diagnostic_bundle`, `get_metrics`, `get_reconnect_status`, `list_account_hosters` and `test_category_regex`. Probing an indexer, approving a diagnostic bundle and reconnecting stay out on the owner's line. Every capability is measured against the OpenAPI document, covered or left out with its reason; the table is generated from `crates/rd-api/src/mcp/coverage.rs` into [docs/mcp-coverage.md](mcp-coverage.md), and the build fails if a REST route is added that no capability claims, so the answer cannot quietly go stale.

**No tool takes or returns a credential.** A password, API key or cookie jar is entered in the web interface; a tool call that carries one is refused with `request.credential_rejected`. A row is created over MCP and its secret filled in afterwards.

Every tool costs the permission its REST route costs, read from the same table — so a tool is never cheaper than the endpoint underneath it. `list_configuration` costs what the section it is asked for costs: `categories` is `api:config`, `accounts` is `api:secrets`, `plugins` is `api:admin`. A refused call carries the same stable `auth.scope_insufficient` code the REST layer returns. API tokens carry the `api:*` scope and are managed (created, listed, revoked) via `/api/v1/api-tokens`; capture tokens do not unlock the MCP endpoint and vice versa.

For dashboards and monitoring there is a second, narrower scope. Switch on **Read-only token** when creating one and the token receives `api:read` instead: it may `GET` the download list and summary, packages and their post-processing, the post-processing queue, storage capacity, the media tool status, and the `/api/v1/events` stream reduced to queue and progress events. Everything else — intake, queue control, configuration, credentials and `/mcp` — answers `403 auth.scope_insufficient`. The areas of an existing token can be changed afterwards, in the token list under **Settings → API & MCP**: the token value is not reissued, so nothing has to be reconnected, and the new areas are in force at the client's very next request rather than after a restart.

**Why that is allowed now, when it deliberately was not.** Until 1.0.6 a token kept the areas it was born with, and that was a real guarantee: a leaked bearer could never become more dangerous than it was on the day it leaked. Giving it up was a decision, not an oversight. The alternative in practice was never "narrow tokens forever" — widening a token meant minting a new one and reconnecting every client that used it, which is expensive enough that people grant `api:*` up front instead, and a token that was born too wide is the worse outcome for exactly the same leak. What replaces the old guarantee is a record: issuing, re-scoping and revoking each write a `capture_changed` event naming the areas involved and, for a change, the areas it replaced, so "when did this token gain that area" has an answer. Two limits keep the reversal from becoming a bridge. `capture:*` cannot be granted to an API token any more than it can be minted into one, and only a token the API token list already shows can be re-scoped at all — so a browser-capture token cannot be turned into an API token by naming its id.

## Metrics

`GET /api/v1/metrics` answers in the Prometheus text format (and as `application/openmetrics-text` when the scraper asks for it). It costs the `api:metrics` area and nothing else grants it: create a token under **Settings → API & MCP**, tick **Metrics** alone, and give Prometheus the bearer:

```yaml
scrape_configs:
  - job_name: rdownloader
    metrics_path: /api/v1/metrics
    authorization:
      credentials: <the token>
    static_configs:
      - targets: ['127.0.0.1:8710']
```

A token holding only `api:metrics` reaches this route and no other — not the queue, not the statistics, not `/mcp` — and a token holding `api:read` does not reach the metrics. The families, their labels and why the label set is closed are documented in [`docs/observability.md`](observability.md); the **Statistics** page in the web interface shows the persistent figures behind the counters, and **Settings → System** sets how long they are kept.

## Docker

```bash
docker compose -f docker/compose.yml up -d
```

The compose example publishes the service on the host loopback only, adopts the host user through `PUID`/`PGID`, and keeps `/config` and `/downloads` as persistent mounts. `/downloads` is only the fallback destination — every path used as a storage root has to be mounted as well, or its contents live in the container's writable layer and are lost when the container is recreated. The full guide, including a Synology NAS walkthrough, is [`docker/README.md`](../docker/README.md).

Remote access goes through a TLS reverse proxy. Tell rDownloader about it under **Settings → Security**: the address ranges whose `X-Forwarded-For` may be believed, the external URL — scheme, host and mount point in one setting, so they cannot disagree — and whether the session cookie is marked `Secure`. Nothing is trusted by default, which means an unconfigured deployment treats the address it can actually see as the client rather than one a caller can name. The application, its API, its event stream and its MCP endpoint all work under a non-empty base path. `rdownloader doctor` prints the resolved contract and warns about the half-configured combinations that produce a working service which misbehaves later; sample nginx, Caddy and Traefik configurations are in [`docs/reverse-proxy.md`](reverse-proxy.md).

Security reports go to a private vulnerability report on GitHub; [`SECURITY.md`](../SECURITY.md) has the details, the supported-version policy, and an explicit list of the things that look like findings but are documented behaviour.

That external URL is also what passkeys bind to. Sign-in accepts a passkey as an alternative to the password — the authenticator verifies a PIN or a fingerprint before signing, so one step carries both factors — and the credential is tied to the address the installation is reached at, never to a header the caller sends, which is what makes it unphishable. `localhost` works without any configuration, so a local install needs nothing; anything else needs the external URL set, and rDownloader says so rather than failing inside the browser. A passkey is an additional way in, not a replacement: the password keeps working, and enrolling a passkey does not switch on the authenticator-app prompt.

Configured NNTP endpoints can be tested in the Usenet view up to and including TCP/SOCKS5, TLS and authentication. Queued NZBs are processed by a persistent background worker: a prioritised connection pool keeps authenticated connections reusable, respects the server limits and downloads up to eight segments in parallel. Confirmed yEnc part ranges are CRC-checked again after a restart, unconfirmed trailing bytes are truncated and only missing segments are re-fetched. Finished files land atomically in the category/storage root; the target path is persisted before the rename and existing final files are validated against all segment CRCs. PAR2 and optional ZIP/7z/RAR steps have their own crash-recovery checkpoints, safe limits are configurable and source archives are preserved.

## Quality assurance

`scripts/check.sh` runs these for you and, since RD-120-25, runs only what the change touches —
it prints what it skipped and why, and `--full` is the complete run that a merge and the release
chain use. `scripts/README.md` describes the scoping, the deferral (`--defer`) and the lock the
heavy scripts take on `/tmp/rd-build.lock`. The individual commands:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace
SQLX_OFFLINE=true cargo sqlx prepare --check --workspace
npm run typecheck --prefix web          # incremental; typecheck:full is the CI one
npm run test --prefix web
npm run build --prefix web              # no longer type-checks; see typecheck:full
scripts/build-extension.sh
```

`cargo sqlx prepare --check` needs the SQLx CLI (`cargo install sqlx-cli --version 0.8.6 --locked --no-default-features --features sqlite-unbundled`), `cargo nextest` the Nextest CLI, and the plugin import guard `wasm-tools` (`cargo install wasm-tools --locked`). The supply-chain check runs with `cargo deny check` against `deny.toml`. `scripts/build-extension.sh` needs `zip` and `unzip`: the two archives are the extension's release artefact, so a run that cannot write them, or writes one without a `manifest.json` carrying the workspace version, fails instead of reporting success. Details on the platform builds are in the [Build](#build) section.

`cargo nextest run --workspace` needs the bundled plugin components to exist. They are built separately — `scripts/build-plugins.sh`, or `cargo component build --release --target wasm32-unknown-unknown -p rd-plugin-<name>` for one of them — because `cargo test` never builds for the wasm target, and since RD-108-16 a contract test whose component is missing fails with that command instead of returning early and counting as passed. `scripts/build-plugins.sh --list-missing` names the ones that are not there and `scripts/check.sh` asks before it spends anything. A checkout without the wasm toolchain leaves those tests out on purpose with `cargo nextest run -P no-components --workspace`, which counts them as *skipped* rather than passed.

Legal manual fixtures and the Free/Premium resolver test matrix are documented in
[`testfile/README.md`](../testfile/README.md).
