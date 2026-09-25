# rDownloader – Complete Feature and Technology Overview

> As of September 22, 2026 · Source version 1.2.0, including the work merged into `development`
> after the `v1.1.0` tag. This document describes only features implemented in the repository; ideas
> and roadmap items are not included.

This overview is intended as a reusable content base for the project website, the
repository, release notes, and app-store-style product pages. The first sections are
suitable for public product copy, while the later sections serve as a technical reference.

## At a Glance

rDownloader is an independent, local, cross-platform download manager. A shared, persistent
queue handles direct HTTP downloads, one-click hosters, Usenet/NZB, BitTorrent, video and audio
sites, image galleries, and livestream recordings. The application combines a server written
in Rust with an embedded Vue web interface, a desktop capture agent, a browser extension, signed
WebAssembly plugins, and an interface for AI assistants through the Model Context Protocol (MCP).

### Short Description for a Website or Repository

> **rDownloader brings HTTP, hosters, Usenet, torrents, media, galleries, and livestreams
> together in one local download hub.** Its persistent queue supports parallel and resumable
> transfers, a LinkGrabber, categories, rules, hotfolders, secure credentials, automatic
> post-processing, and a responsive web interface. Native builds for Windows, macOS, and Linux,
> along with a multi-architecture Docker image, make the app suitable for desktops, NAS devices,
> and servers.

### Key Facts

| Attribute | Value |
| --- | --- |
| Product | rDownloader |
| Category | Local-first download manager and download automation |
| Current project version | 1.2.0 |
| Author | Alexander Herling |
| License | GNU GPL v3.0 or later |
| Project site | `https://rdownloader.net` |
| Repository | `https://github.com/degoya/rDownloader` |
| Platforms | Windows x86-64, Linux x86-64, macOS Intel and Apple Silicon |
| Container | Linux `amd64` and `arm64` |
| User interface | Responsive web app embedded in the server binary |
| Languages | German, English, French, Spanish |
| Default address | `http://127.0.0.1:8710` |
| Default capture-agent port | `9666`, loopback only (IPv4 and IPv6) |
| API | REST under `/api/v1`, OpenAPI document, and Server-Sent Events |
| Sign-in | Password with an optional authenticator code, or a passkey |
| Machine credentials | Tokens scoped by six permission areas, plus a metrics-only scrape area |
| AI integration | MCP over Streamable HTTP under `/mcp` |
| Bundled plugins | 72 signed WebAssembly components across twelve plugin types, three of them SDK examples |

## Feature List

### 1. Unified, Persistent Download Queue

- One shared queue for HTTP/hoster downloads, Usenet files, media, galleries, livestream
  recordings, and torrents.
- All files are grouped into packages with a dedicated package directory:
  `<category-path>/<package-name>/`.
- Persistent states for queuing, resolving, downloading, pausing, retry waits, verification,
  repair, extraction, seeding, failure, cancellation, and completion.
- `High`, `Normal`, and `Low` priorities, with manual ordering within each priority. In the queue
  the priority is an arrow — up, dash, down — whose accessible name states the level.
- A queue row leads with the name and gives every other cell only the width it earns. States that
  mean one thing are glyphs carrying the word as their accessible name, a full progress bar prints
  no percentage beside itself, a stored archive password disappears once the archives are out, and
  the package's start/stop control and a dots menu are the only two controls beside the row. The
  row shows selection, name, and state at any width; progress joins from 768 px and size plus
  metadata from 1280 px, and below 560 px it breaks into two lines rather than overlapping.
- Packages and the files inside them are sorted by dragging their handle, or with `ArrowUp` and
  `ArrowDown` once the handle has focus, so the order is reachable without a pointer. The order is
  stored, survives a restart, and is written through one endpoint per list that takes a package's
  complete id list and refuses anything else. A move that cannot be carried out — a filtered list
  shows only part of a package, a package would leave its priority tier — says so instead of
  quietly doing nothing.
- Parallel processing of multiple files with a runtime-configurable upper limit.
- Global start/pause control plus controls at package and file level.
- Individual transfer services — BitTorrent, Usenet, media downloads, galleries, stream
  recordings, and remote file transfer (FTP, FTPS, SFTP, WebDAV) — can each be switched off.
  Everything is on by default. A switched-off service refuses matching links at intake, reporting
  how many a paste lost, and moves anything it already had queued to blocked with that reason
  rather than leaving it waiting for a runner that never comes; switching it back on returns
  those entries to the queue. Enforced when a job is dispatched, so a change takes effect at once
  rather than after a restart.
- Multi-selection actions for starting, pausing, cancelling, removing, extracting, renaming,
  assigning a category or priority, and changing the post-processing level.
- Resume failed or cancelled transfers from existing partial data.
- Reset a transfer to start it over from zero, per file or over a whole selection: partial data,
  checkpoints, progress, retry budget, recorded error and the package's post-processing steps are
  discarded. Available for every transfer kind and every state a job rests in, a finished one
  included. A finished file is only deleted when the confirmation explicitly asks for it;
  otherwise the new attempt lands beside it under a collision-free name.
- Links in one package that point at the same file are recognised as mirrors of each other: one
  downloads and the rest wait as its fallback, shown as "Mirror" rather than as an error. The
  group is the one the LinkGrabber worked out, carried into the queue rather than computed a
  second time there; Usenet and torrent rows are members of one download rather than routes to
  it, so they are never grouped. A waiting mirror can also be promoted by hand, and the
  recognition can be switched off.
- A mirror that fails at the hoster hands the download to the next member of its group by
  itself: offline, gone, a limit, a login refused, or a web page served instead of the file. The
  partial data of the one that was given up is discarded, so a file never contains bytes from two
  sources. A cause that lies on this machine — no space, no permission, a file that could not be
  put in its place — never moves the job to another mirror, because the next one would write to
  the same disk; and an ordinary server hiccup waits for its retry on the same mirror rather than
  spending one. Every mirror is tried at most once per attempt, which the queue remembers across
  a restart, and a group that has run out says so with the reason the chosen mirror gave rather
  than the reason of whichever was tried last.
- "Clear the list" — completed, failed, or everything finished — works on whole packages and is
  decided by the service in one request, not row by row in the browser. A package is removed
  whole or not at all, and one member that is still running, waiting, paused, seeding or being
  post-processed keeps the entire package, including the files in it that are already done. The
  answer says how many packages went and names the ones it left alone with the reason, so nothing
  is passed over in silence. It is the same predicate the timed removal below uses.
- Deleting a package outright cancels whatever is still running in it and discards their partial
  data, so it is refused unless the request says to force it. The confirmation names the cost
  before that flag is sent, and the automation adapters that can only say "delete this" force it
  by definition.
- Finished packages can leave the queue by themselves after a configurable number of hours.
  Anything still unpacking, still seeding, or — unless you say otherwise — holding a file that did
  not finish stays where it is. A package records when it finished, so renaming it or changing its
  priority does not restart the clock.
- Configurable retry count and retry timing with support for `Retry-After`.
- Live speed globally, per package, and per file, plus a two-minute throughput history. The rate
  is measured by the service, not by each client, so the web interface, the desktop tray, MCP and
  automations all read the same figure.
- Estimated time remaining per running entry, per package, and for the queue as a whole, at the
  current speed. Nothing is shown where a number would be invented: an unknown total size, a
  paused transfer or a rate of zero leaves the field empty rather than printing a placeholder.
  Entries that are waiting, verifying, repairing, unpacking or seeding are not being fetched and
  stay out of the estimate.
- Statistics for active, queued, completed, and failed downloads, transferred and remaining data,
  the queue's remaining time, and free space per storage destination. The remaining-data figure
  counts paused and post-processing entries as well, and says so.
- Destination paths are visible in the interface and can be copied to the clipboard.
- A package's folder can be renamed, not only its label. The switch sits in the package editor in
  the download list and is offered once the name actually changes. The row is written first and
  the folder follows, remembering where the data was, so an interruption leaves the package
  findable and the move completes on the next pass or after a restart. Every absolute path the
  package has stored moves in the same transaction — its post-processing steps and its assembled
  Usenet files — because a post-processing step is identified by its source path and would
  otherwise be duplicated by the next "post-process anyway" run. A name that is already taken is
  reported as an error rather than sidestepped with a numbered suffix, a package with a file
  still transferring is refused, and the name goes through the same rules a file rename uses, so
  it cannot leave the category directory.
- The queue and the LinkGrabber stay usable at a few thousand rows: both render only the rows near
  the viewport, through the same building block. Package and files are one flat stream of rows with
  stable keys, so a collapsed package, a reorder or a selection still means the same thing for the
  part that is not in the document. The row holding keyboard focus is never removed, an arrow-key
  reorder keeps its handle past the edge of the window, shift selects a range along the visible
  order, and "show in list" jumps back to a selected row — opening its package if it is collapsed.
  A screen reader is told how long the list really is. Below sixty rows nothing is held back.
- Browser notifications when links reach the LinkGrabber, when the queue completes, and when
  downloads fail. The intake notification comes from the web interface itself and needs no
  desktop agent.
- Crash-safe recovery of transfers, post-processing, and torrent seeding after restarts.

### 2. Direct HTTP and HTTPS Downloads

- Parallel downloads across multiple files and multiple byte-range chunks per file.
- Automatic detection of file size, range support, `ETag`, and `Last-Modified`.
- Safe resume using persisted chunk positions and remote validators.
- Periodic checkpoints are stored only after data has been synchronized to the file system.
- Protection against writing changed remote files or ignored range responses over existing
  partial data.
- A shared limit on simultaneous connections per host, counted across every running transfer
  rather than per file (default 6, configurable, `0` lifts it).
- A server that answers a range request with `200` is judged by `Content-Range` and the response
  headers instead of the status alone: a complete body on a single whole-file chunk is written
  from the start, a resume that receives the entity from byte zero is treated as a changed remote,
  and a page instead of payload is retried rather than refused for good. A host that genuinely
  ignores ranges is planned as a single connection for the rest of the session.
- Fallback from `HEAD` to a one-byte range request when metadata is missing.
- Atomic completion in the destination directory and collision-free file names.
- Optional SHA-256 generation and validation of expected MD5, SHA-1, SHA-256, and CRC32 values.
- Global bandwidth limit that can be set directly in the download toolbar and changed live.
- Global, account-specific, and job-specific proxy selection.
- HTTP, HTTPS, and SOCKS5 proxy profiles, optional proxy authentication, and custom CA certificates.
- Approved browser downloads started by a form are repeated as a `POST` with their original body
  and user agent. Such a download is never split into parallel chunks, because no specification
  defines range semantics for a `POST` and repeating it could trigger the site's side effect more
  than once.
- Expiring and signed addresses are renewed before an interrupted download reuses its partial
  file: either through the resolver that owns the link, or through one plain re-request of the
  original address. Neither runs JavaScript. When nothing can renew the address the download is
  blocked with an explanation and the partial file is kept.
- A renewed address counts for that attempt and is not kept: no source says how long it is valid,
  and a stored single-use link would turn a cheap renewal into a failed download. What is stored
  is the link the person added, which every attempt starts from again.
- A repeated request only ever talks to the addresses that were approved for it; a redirect
  anywhere else is refused before a single header or byte reaches the other host.
- A `POST` the server will not resume is blocked rather than silently sent again; the queue
  row's reset action discards the partial and requests it from the beginning, deliberately.

### 3. JDownloader-Style LinkGrabber

- Accepts individual URLs, mixed text, and multiple links at once.
- Input from manual entry, the clipboard, Click'n'Load 2, REST/MCP, hotfolders, NZB imports,
  DLC containers, the browser extension, and intercepted browser downloads.
- Persistent intake batches, link candidates, and LinkGrabber packages.
- Automatic package creation based on explicit package names, shared name stems, and multipart
  archives such as `.part1.rar`, `.r00`, or `.7z.001`.
- A single link whose address offers no name at all can only be named after its hoster at intake.
  It does not stay that way: the moment a resolver reports the real file name, a package that is
  still called exactly after its hoster and holds that one file takes the release behind it, and
  its folder is created under that name rather than renamed afterwards. A package anybody named
  keeps its name.
- Automatic online check after intake plus manually triggered rechecks.
- Direct links are checked through `HEAD`/range requests; hoster and multihoster links use their
  provider-specific checks; media pages are analyzed through `yt-dlp`.
- Displays status, file name, size, provider, duplicates, and media metadata before download.
- A check that could not confirm a link says which of three situations it hit, as a stable code
  translated into all four languages rather than English prose: no answer for this address, an
  answer of "cannot tell", or a hoster whose *check* needs an account although the *download* may
  not. A direct probe that timed out is reported as such.
- A provider that says it holds the file in its own cache is shown as such: a neutral
  `cached: <time>` chip beside the unchanged `online` state, stamped with the time of the check,
  because a cache expires without notice and is a measurement, not a promise (RD-120-36;
  Premiumize today; magnets and TorBox's cache checks are not wired yet).
- The duplicate mark names what it refers to: the address is already in the LinkGrabber or
  already enqueued, and may be added again deliberately.
- Links that point at the same file are recognised as mirrors of one another rather than as
  candidates to delete: one entry per file, with the other hosters kept beside it because they
  are what remains when the chosen one goes offline. A mirror is deliberately not a duplicate --
  a duplicate is the same address a second time. Three sources, in this order: a site rule that
  says its page is one release, then a file name and a size that agree after the online check,
  then a shared file name alone, which is offered as a proposal and marked as one. Each mirror
  carries the quality, the language and the hoster where they are known; the group lives inside
  one package and survives a restart.
- Such a group is **one row** in the LinkGrabber -- the chosen mirror's own row, with the other
  mirrors behind the chevron pair -- so a release page offering three qualities at five hosters
  is one decision rather than forty. The badge names the group and how it was formed, and a
  group built on a shared name alone reads as the proposal it is: it says "possible mirrors"
  rather than "mirrors", in a different colour and with a dashed edge, and its title spells out
  what the evidence was. A group whose mirrors are all reported as gone says so and can still be
  queued.
- A proposal can be contradicted: the row of a group built on a shared name alone offers
  *Ungroup these links*, asks once, and leaves its links standing on their own. What is stored
  is that those links are not the same file, so the decision survives the online check -- even
  where a matching size arrives that would otherwise have promoted the group -- an intake and a
  move to another package, and a link that turns up later with the same name does not revive it.
  A declared group and one a matching size corroborates are refused instead of asked about: a
  contradiction there is a finding about the source.
- Quality, language and hoster are a standing preference rather than a filter that is forgotten:
  inside a mirror group they decide which member the queue will fetch, outside one they hide
  what cannot match, and they are stored on the server, so they still decide the package that
  arrives tomorrow. Any group can be overruled with "Use this mirror"; that choice is kept apart
  from the derived one, so neither a later preference nor a regroup takes it back.
- Hosters can be hidden from the LinkGrabber, several at once and independently of the hoster
  preference, which shows one: a row of hoster chips above the list carries each hoster's link
  count and hides or shows it on a click, and a link's menu offers "Hide links of <hoster>". The
  choice is stored on the server with the preference, so it survives a reload and a restart. A
  line under the chips says how many links from how many hosters are hidden and brings them all
  back. Hidden links stay in the LinkGrabber; "Enqueue", "Enqueue all", the selection and the
  online check act only on what is shown. A hidden hoster's link that is a mirror of a shown one
  is never the chosen mirror while a shown one exists, and goes along as its fallback.
- Links from an intercepted browser download keep their request metadata — effective URL,
  method, referrer, user agent, content disposition, and permitted headers — which can be
  inspected per link before queueing.
- A download that would send credentials — a `POST`, a request body, or a signed address — has to
  be approved first. The dialog states the target host, the method, the addresses redirects may
  follow, the body's field *names* (never their values), and each category of credential paired
  with the host it would go to. A plain captured `GET` needs no approval.
- Approval is bound to the exact request it was given for, but ignores signature parameters, so a
  re-signed address for the same file does not ask again. A link that carries an approval says so
  in the LinkGrabber row, and the approval can be withdrawn again from its details panel — which
  matters because the enqueue asks only once.
- A capture that cannot be reproduced — a file upload, a multi-part form, an oversize or
  unreadable body — is still shown with the reason instead of disappearing.
- Site rules are self-testing: `rdownloader doctor site-rules` fetches the probe address every
  rule names and reports `ok`, `structural` (the page changed), `blocked` (it guards itself) or
  `dead` (nothing answers), stores the result per rule and exits non-zero on anything but `ok`.
  A rule found dead is skipped by the recognition instead of being deleted, and comes back as
  soon as a later run finds the service alive. The run is triggered, never scheduled.
- Site rules are visible and editable: *Settings → Site rules* lists every rule grouped by its
  group, with the hosts it claims and the self-test's verdict beside it. A switch per rule and a
  switch per group, both surviving a restart, so a rule that has broken is switched off rather
  than waited on. A rule is written in named fields — the seven step kinds each draw what they
  need, and their order can be changed — and can be run against a real address before it is
  saved, showing the links found, the package name and which addresses answer with a page rather
  than a file. Rules export to a file and import from one; an imported rule is stored switched
  off, signed or not, and switching it on is a separate request per rule.
- **No site rule ships with the program** (RD-130-07). A fresh installation recognises no
  release page by rule. The project's eight rules are one signed file, `rdownloader-site-rules.json`,
  that every release carries beside its packages; the import verifies it against the site-rules
  root from the bytes as they arrived, refuses an altered or foreign file whole with a stable
  code, and stores its rules as your own, switched off — so every one of them can be edited and
  removed like a rule you wrote. An installation from before 1.3 keeps nothing of the rules it
  had compiled in; importing the file brings them back.
- A site rule can be **duplicated** (RD-130-07): the copy keeps the original's body, gets a free
  id (`<id>-copy`) and a copy name, is stored switched off and opens in the editor, so a working
  rule is a template to learn from without being touched. The groups are five — `board`, `paste`,
  `ebooks`, `graphics`, `adult` — each named in all four languages; `comics` and `magazines` were
  merged into `ebooks`, stored rules and group switches included (migration `0095`).
- Every address a folder crawler or a site rule returns is judged before it becomes a candidate:
  one a resolver, a transfer backend or a recognised provider claims goes through unprobed, one
  nobody claims is kept only when a probe confirms the response is file content, and anything
  else is refused with a stable code instead of becoming a candidate. The intake reports how many
  addresses were found and how many were dropped, so a rule that reached too far does not read as
  an empty page.
- Status values for online, offline, unsupported, unresolvable, duplicate, failed, and already
  queued. `unresolvable` is the only one that cannot be queued: the address was reached and
  answered with a page rather than a file, and a queued page would be stored under the file's
  name. It is shown apart from *offline* (the hoster says the file is gone) and *not checkable*
  (nothing could check it), both of which stay queueable.
- Duplicates can deliberately be downloaded again after confirmation.
- Per-package selection of category, priority, archive password, post-processing level, and script.
- Rename individual links or complete packages.
- Move links between packages and create new packages from a selection.
- Sort by name, hoster, size, or intake time; reorder manually by dragging a row's handle or with
  the arrow keys while it has focus. Reordering under an active filter is refused with a reason,
  because the visible list is not the whole package.
- Regroup automatically named packages.
- “Download all,” selective queueing, and “Add paused” actions. “Add paused” covers every row
  type the list holds: collector links, NZB *candidates* in a package, and imported NZBs from a
  hotfolder, an upload or a drop. It is offered on the toolbar, on an NZB's own row and in the
  selection bar, and the package lands in the download list with every file paused.
- Shared view for collected links, NZB imports, and torrent metadata.
- App-wide drag and drop for `.nzb`, `.torrent`, `.dlc`, `.ccf`, `.rsdf` and `.txt` files.
- Four link-container formats, all through one endpoint rather than one per format: **DLC** and
  **CCF** are imported as reviewable packages with the file names, sizes, package name and archive
  password the container declares — neither can be decrypted locally, so both are off by default
  and only ever ask the decryption service configured in the settings for the container key, while
  the links themselves never leave the installation. **RSDF** is decrypted on this machine, because
  its key is published. A **`.txt`** link list needs no key at all: one link per line, with
  `[A Name]` on a line of its own opening a package and `;` or `#` starting a comment. The original
  DLC address still answers, so nothing that used it has to change.
- The same import routes take a JSON body as well as a file upload (RD-120-31): the file as
  base64 in `content`, beside the upload's own fields (`file_name`, `name`, `category_id`,
  `priority`, and `format` for the container route). It is the same route and the same code, so a
  script or an assistant gets exactly the result and the error codes a browser upload gets, at
  the same `api:intake` price. A JSON body carries a file of at most 48 MiB — its base64 is
  64 MiB, which is what the 65 MiB request limit leaves room for — and a larger one is refused
  under `container.too_large` rather than truncated; a body over the service-wide limit is
  answered with `request.body_too_large`. A larger NZB still arrives as an upload.
- Links whose check failed are queueable. A check that never reached a conclusion says something
  about the check — a hoster account whose sign-in is broken fails the whole batch — not about the
  file, so such links are counted and labelled as not checkable rather than as offline, and the
  package's "add to downloader" action stays available. The list of queueable states is derived
  once on the service side and mirrored once in the interface.
- Undecided indexer hits appear below the collected links, grouped into a collapsible section per
  subscription with its own name and count. Every hit of one subscription can be queued or
  dismissed at once, behind a confirmation naming the search and the number; dismissing deletes
  nothing, it only stops the hit being suggested again. There is deliberately no action spanning
  every subscription.
- Each indexer subscription chooses how its hits appear there (RD-120-37): the **list** — the
  default, and what every subscription showed before — or a **card slider**, one equally tall
  card per hit with its cover or a coloured initials tile, size, release group, the release name
  as its heading, episode and chips, and one details panel under the slider for the chosen card.
  Both views offer the same actions behind the same confirmations. The slider pages by its arrows,
  the arrow keys, a swipe and its page dots, shows fewer cards in a narrow window rather than
  thinner ones, and honours "external images off". It holds every open hit of the subscription
  with no pagination bar under it (RD-130-13) — only the list keeps one — and reads them fifty at
  a time as the reader nears the end of what it has; past ten pages the dots become a "Page 3 of
  40" counter. Autoplay is a second per-subscription option, off by
  default: a page every six seconds, wrapping at the end, with a pause control, holding while the
  pointer or keyboard focus is in the slider, while details are open and while the tab is hidden,
  and not running at all under reduced motion. The card picture has a per-subscription aspect
  ratio (RD-120-42) — 1:1, 3:2, 16:9, 4:3 or 2:1, the default — so square covers show square and
  banners wide; the cover fills it without distortion and every card of a slider stays equally
  tall. Set in the subscription form; over REST and MCP
  it is `view` (`list` | `cards`) and `autoplay`.
- A link is recognised for what it is even when the server does not say so: when the content type
  is unhelpful, the first bytes of the document decide — an NZB is XML with an `<nzb>` root, a
  torrent is bencode — and the decision is logged either way. An indexer hit keeps the title its
  feed gave it rather than being named after the `…/api` address every hit is fetched from, falling
  back to the `Content-Disposition` and `X-DNZB-Name` headers indexers answer with. A link that
  turns out to be a `.torrent` is read during the check: it is named after the torrent's own
  release name and carries its file tree for review, never the download token its address ends in.
  The file read there is the one the download uses, so an indexer counts one grab, not two; a
  copy older than a day, or one whose link was removed, is dropped and the address fetched again.
- NZB history with status, size, date, and deletion controls.
- Domain blocklist loaded from a text file, with support for comments and subdomains.
- Normalization of known short and alias domains, including `youtu.be` and `ddl.to`.
- Release pages are described by **rules**, not plugins (RD-110-04): a rule names the hosts and
  path patterns it claims, the steps that turn a page into links (`fetch`, `fetch-json`, `regex`,
  `decode`, `form`, `redirect`, `captcha`), where the package name comes from, the domains the
  service once had and that are rewritten to the living one, a real address for the self-test,
  and the date the service was last measured alive. The project's rules are one signed pack
  under its own trust root (`Role::SiteRules`) — since RD-130-07 a release file that the import
  verifies rather than a pack compiled into the binary; a pack with an unknown format version, a
  revoked digest or an invalid rule is refused as a whole with a stable code. Rules are stored in
  the database, survive a restart, and never share an id. The format and the executor are documented in `docs/site-rules.md`; the rules
  themselves (RD-110-10 onwards) follow.
- A rule is **executed** by rDownloader itself, not by a plugin (RD-110-05): the seven steps run
  in order, and the run is bolted shut in three places. A rule reaches only the hosts its `match`
  names plus the address it was given, every redirect is checked again at the hop rather than
  followed blindly, and no target may resolve into a private, loopback or link-local address —
  checked against what DNS answered, not against the name, and one non-public address among
  several is enough to refuse. Depth, number of requests, number of links, total run time and
  response size are each capped with their own error code, a run that returns to an address it
  already fetched ends as a cycle, and an empty result is a refusal with a code rather than an
  empty package. There is **no JavaScript interpreter**: base64, hex, rot13, percent-encoding and
  concatenated JavaScript string literals are decoded, and what is genuinely a program ends as an
  honest refusal rather than an HTML file in the queue.
- A rule is asked **in the same selection as the crawler plugins** (RD-110-06), in one fixed
  order: the plugins that name a service, then the rules, then the generic crawlers that
  recognise a shape rather than a service. Among the rules a person's own come before the
  shipped ones, so a shipped rule that has gone wrong can be bridged without waiting for a
  release. A rule that does not claim an address hands it to the next source; a rule that
  claimed it and found the page dead, guarded or changed says so with its own code in all four
  languages, instead of letting the next source answer about the same page. The package name a
  rule read names the package the links form, through the same `package_hint` a crawler plugin
  fills.
- **Eight services are recognised this way today** (RD-110-10, RD-110-12, RD-110-11, RD-120-17):
  the release boards `scnlog.me`, `downmagaz.net` and Scene-RLS (`scene-rls.com`,
  `scene-rls.net`), GetComics, AvaxHome, CGPersia, the ControlC pastebin, and the
  ViperGirls board in a group of its own, `adult`. A release address becomes one package named
  after the release with the hoster links in it, and none of the page's own links, because a
  rule reads the block that holds only the hoster links rather than the whole page. A ControlC
  paste becomes the addresses in its text block, named after the paste — which of them is a file
  the probe decides, not the rule, because a paste is running text. One of the rules walks a
  gateway on the service's own host before the address it hands over — AvaxHome's `/go/` token —
  and ends at a redirect the rule keeps without following it. All eight were opened on the live
  service on 2026-09-22 and each rule carries that date, which is what the settings page shows
  in place of the former "Not checked" badge (RD-120-17). Two rules left the pack in the same
  pass: `libgen`, because libgen.bz no longer resolves its links, and `satdl`, withdrawn.
  Seventeen domains those services have left behind are carried as `dead`, so an address on
  `scnlog.eu`, `getcomics.info` or `avaxhome.ws` is rewritten to the canonical host rather than
  refused. `docs/site-rules.md` carries the table.
- **Four candidates carry no rule, and the reason is a measurement** (RD-110-11). `hdencode.org`
  keeps no address in its HTML at all and hands the links out behind a reveal form with a
  Cloudflare Turnstile widget and an image-captcha fallback; the submission itself was not
  measured, and the admission rule refuses a rule written on an assumption.
  `metalarea.org` and `hi10anime.com` replace the link block with a registration notice for a
  guest, and no rule is written against a credential this project does not have. `13dl.to`, the
  service's only domain, now answers with a parking page. Of the eleven services the job put on
  the list, that is seven described and four refused, each with its date.
- **What a rule cannot describe, measured rather than assumed.** A service that keeps its
  releases behind an XHR list carries no rule today: a template expands to a list's first value,
  no step repeats itself once per element, and a search pattern is a literal rather than
  something a step can fill in. `serienjunkies.org` and its identical sister `dokujunkies.org`
  were measured on 2026-09-21 and are the recorded case (RD-110-13, ADR 0013); neither is
  defended, and both are still out of reach. The other measured limit is that a rule cannot say
  "this host, no" (RD-110-10) — what keeps a page's own links out of a package is the block a
  rule reads, not a filter. Both are written down in `docs/site-rules.md` so a new rule meets
  them before it is written.

### 4. Hoster and Multihoster Support

- Provider accounts with API key, username/password, or cookies, depending on the provider.
- Accounts can be enabled, disabled, tested, and assigned to a proxy profile. A newly saved
  account is checked on its own straight afterwards, without the dialog waiting for it — a check
  reaches into the provider's resolver, which can park on a captcha for as long as somebody takes
  to answer it — and the row carries the result, with the account switchable on and off from the
  list.
- Displays account status, premium status, label, and remaining traffic where available. The
  label is translated: a plugin sends codes with parameters, the interface renders them in its
  own language (RD-110-28).
- Automatic provider and account selection based on the URL and a multihoster's hoster catalogue.
- The account and resolver route used are stored for every download.
- A provider can offer more than one way to hold an account: a manifest declares one credential
  slot per mode, each with its own hosts, and an account records which mode it uses. That
  separation is enforced — an API key is never sent to a login form, and a password never to an
  API host — even though the same account field holds both. The account form asks which mode it
  is directly under the provider, before the fields that mode governs, and opens with the
  provider's first mode already chosen, so the secret field is labelled for one answer rather
  than for both.
- A provider can also declare that it takes no account at all, which is what a resolver spanning
  many independent installations of the same hosting script needs: an account at one clone is not
  an account at another. Such a provider resolves links and stays out of the accounts list.
- **A provider exists exactly while its plugin does.** The provider table is filled solely from
  installed plugin manifests; nothing is compiled in. It is rebuilt when a plugin is installed,
  removed or switched off, rather than only at startup, so the accounts list offers what this
  installation can actually resolve. An account whose plugin was uninstalled stays and stays
  editable so it can be switched off; only creating a new one for an unknown provider is refused.
- Where a plugin is offered for selection, the version behind it is visible, because installing a
  plugin never removes the older one and the highest wins at load time.
- Twenty-seven bundled resolvers are available as signed `.rdplug` WebAssembly components; fifteen of
  them are also compiled in as native fallbacks, from the same source as their packaged
  counterparts.

| Provider | Type | Supported credentials | Without an account |
| --- | --- | --- | --- |
| DDownload | Hoster | Username and password, or API key | Yes — countdown and widget captcha |
| Rapidgator | Hoster | Username and password | Yes — countdown and reCAPTCHA |
| Nitroflare | Hoster | Username and premium key | Yes — countdown and reCAPTCHA |
| KatFile | Hoster | API key or cookies | Yes — countdown and widget captcha |
| Turbobit | Hoster | E-mail address and password | Yes — Turnstile and countdown, one file per guest window |
| HitFile | Hoster | E-mail address and password | Yes — Turnstile and countdown, one file per guest window |
| 1fichier | Hoster | API key | Yes — countdown only, no captcha |
| Keep2Share | Hoster | Username and password | Yes — image captcha |
| FileJoker | Hoster | Cookies | Yes — countdown and widget captcha |
| KrakenFiles | Hoster | None — the resolver takes no account | Yes — Turnstile widget captcha, no countdown |
| MediaFire | Hoster | None — public files take no account | Yes — no countdown; a captcha is handed over when the site asks for one; folders and key lists through the sibling crawler |
| MEGA | Hoster (client-side encryption) | Username and password, or none for a public link | Public files and folders work: the plugins resolve a file and list a folder, the key travels through the vault rather than through any row (RD-110-38, RD-120-11), and the host decrypts the stream as it writes and verifies MEGA's own condensed value before promoting the file. **Account sign-in works since RD-120-20**: the host computes the key derivation over the password and the plugin only names it (`docs/adr/0020-*`). **Files of the signed-in account work since RD-120-30** (`mega.nz/fm/<handle>` lists a folder, `mega.nz/fm/file/<handle>` resolves a file): the host unwraps each node key under the master key it keeps beside the session and hands back that key alone; the password is kept beside the session instead of being replaced by it. Not yet run against a real account. Files from incoming shares, legacy (version 1) accounts, two-factor sign-in and `#P!` links are not built |
| Pixeldrain | Hoster | An API key, optional — sent as the HTTP Basic password under an empty user name, to `pixeldrain.com` only, on the API calls and on the transfer (RD-120-38); without an account everything runs as before | Yes — public files need nothing; a list address is unpacked by the sibling crawler. The provider's per-IP allowance, transfer volume, concurrency ceiling and captcha state each get their own code and are reported rather than worked around |
| Premiumize.me | Multihoster | API key | No, by design |
| Google Drive | Hoster | Google sign-in (OAuth) | No — every call goes through the account's own token |
| OneDrive / SharePoint | Hoster | Microsoft sign-in (OAuth, browser or device code) | No — every call goes through the account's own token |
| Dropbox | Hoster | Dropbox sign-in (OAuth) | No — every call goes through the account's own token, shared links included |
| Box | Hoster | Box sign-in (OAuth); the person registers their own application, so the account holds its client secret as well as the token | No — every call goes through the account's own token, shared links included |
| pCloud | Hoster | pCloud sign-in (OAuth, application key and secret) | No — every call goes through the account's own token; a public link needs none at all |
| AllDebrid | Multihoster | API key | No, by design |
| Debrid-Link | Multihoster | API key | No, by design |
| LinkSnappy | Multihoster | Username and password | No, by design |
| Real-Debrid | Multihoster | Device sign-in, renewed automatically | No, by design |
| TorBox | Hoster (jobs at the provider) | API key from the TorBox settings page | No, by design. The resolver claims only TorBox's own `requestdl` address; a magnet, an NZB or a link becomes a **remote job** rather than a resolve, because that is what TorBox does with one |
| Put.io | Hoster (cloud storage with its own torrent client) | Put.io sign-in (OAuth); the person registers their own application, so the account holds its client secret as well as the token | No — every call goes through the account's own token |
| Offcloud | Multihoster | API key | No, by design — and the cloud half of the account runs as a remote job rather than a resolve |
| Seedr | Hoster (cloud storage with its own torrent client) | The account's own e-mail address and password, sent as HTTP Basic -- Seedr's REST v1 has no other credential | No, by design. The resolver claims only Seedr's own per-file address; a magnet or a `.torrent` becomes a **remote job**, because that is what Seedr does with one |
| XFileSharing sites | Hoster | None — the resolver takes no account | Yes — countdown and captcha |

- **Turbobit and HitFile share one implementation.** They are one operator's two brands with one
  JSON API, so `plugins/turbobit-common` carries the flow and each plugin contributes its hosts,
  its id shape, its codes and its catalogue. The direct link is single-use and IP-bound, so it is
  resolved immediately before the transfer, never probed and never stored; a premium-only file is
  refused before a captcha is spent, and the guest window is an IP block the scheduler holds off
  on rather than a wait.
- **DDownload signs itself in.** An account holds either credentials or the API key from the
  provider's own settings page, for anyone who would rather not store a password; pasting a whole
  `Cookie:` header out of the browser developer tools is no longer the only route. Neither
  credential is ever visible to the plugin — both are substituted inside the application, each
  pinned to its own host, so the password only ever reaches the login form and the key only ever
  reaches the API. The sign-in answers the Cloudflare Turnstile widget the login form carries,
  through the same captcha broker the free download flow uses. The session lives for as long as
  the process.
- **Signed in in the browser is signed in here (RD-120-45).** Where a provider's plugin declares a
  `cookie_scope`, *Take over from browser* at the account asks the browser extension for that one
  site's session; the person confirms it in the extension's popup and in the browser's own
  permission prompt, and the cookies land in the vault exactly where cookies typed into the
  account go. A sign-in whose login page the browser skips because it is signed in already no
  longer waits out a timeout: the extension reports the page without a widget and the check ends
  with `captcha.page_without_widget`, naming the two ways out.
- **Google Drive is three plugins, not one.** A manifest carries exactly one plugin type, and
  only a resolver may declare the `[provider]` section that creates an account, owns the vault
  reference and makes `{{secret:…}}` resolve. So `google-drive` resolves a file and owns that row,
  `google-drive-crawler` lists a folder or shared drive, and `google-drive-oauth` signs the
  account in. All three go through the official Drive v3 API and Google's own authorization
  endpoint; none of them scrapes the download interstitial or guesses at a `confirm=` token. What
  passes between the packages is nothing but an address, so each works alone.
  - A **Workspace document** is not a file until somebody names the format it becomes, so that is
    decided before anything is queued and shown as the name it will arrive under: Doc → `.docx`,
    Sheet → `.xlsx`, Slides → `.pptx`, Drawing → `.png`, Apps Script → `.json`, overridable per
    address with Google's own `?format=`. A Form, a Jamboard and a Site export as nothing and are
    refused rather than queued as an empty file. An export is reported with no size and no
    checksum, because its bytes do not exist until the export runs.
  - **Every way Drive says no arrives as HTTP 403** and means something different each time, so
    each gets its own translatable code: a download quota that resets, a file Google could not
    scan for viruses, an owner who switched downloading off, an export too large, a rate limit
    carrying Google's `Retry-After`, a refused token, a file that is not there.
  - **A shared drive is reached only with all three of** `supportsAllDrives`,
    `includeItemsFromAllDrives` and `corpora=allDrives`. Without them Drive answers that the
    folder is empty, which is the kind of wrong answer nobody would think to check.
  - The sign-in is authorization code with PKCE, scoped `drive.readonly` because two other
    plugins spend the same token.
  - **The OAuth client is registered per installation, and no client id ships in this
    repository.** Providers count their quotas per client, so a compiled-in one would put every
    installation in the world on a single shared allowance — and it would sit unrevocably in the
    git history and in every release artifact besides. The account's *OAuth client ID* field
    takes your own; the field's hint and `docs/development.md` carry the five steps. Until it is filled in,
    starting a sign-in refuses with `oauth.client_not_configured` and repeats those steps rather
    than sending anybody to a Google error page. A client *secret* is not supported: an account
    carries one secret reference and the access token takes it, so this is written against a
    public client with PKCE.
- **OneDrive and SharePoint follow the same shape**, as the second cloud drive and the first to
  inherit the interface Google Drive decided: `onedrive` resolves a file or a sharing link and
  owns the provider row, `onedrive-crawler` lists a folder sharing link, and `onedrive-oauth`
  signs the account in at Microsoft. All three go through Microsoft Graph and Microsoft's own
  identity platform; the sharing link is handed to Graph whole, encoded as `/shares/{id}`, and
  never taken apart for the `authkey` a personal link carries.
  - **The type letter in a sharing link decides who claims it.** `1drv.ms/f/…` and
    `…sharepoint.com/:f:/…` are folders and the crawler's; every other letter (`u`, `w`, `x`,
    `b`, `p`, `i`, `v`, `t`) is a file and the resolver's. The long `onedrive.live.com` address
    says neither, so the crawler takes it, asks Graph, and answers with one file when that is
    what it finds. The canonical address the crawler hands back keeps the share the item was
    found through, because a link shared with the account grants access through the share.
  - **The download address is the stable `/content` route**, never the pre-authenticated
    `@microsoft.graph.downloadUrl` every item also carries. An address whose only identity is an
    expiring signature cannot be asked for again after a restart; the `/content` route can, and
    the scheduler asks the resolver again before continuing a partial file. The bearer token
    goes to `graph.microsoft.com` only and is dropped on the redirect Graph answers with.
  - **Every way Graph says no arrives as its own code**, and the two that both arrive as HTTP
    403 are told apart by Graph's error code: a share made for another account or tenant, a
    SharePoint policy that allows viewing but not downloading, a file Microsoft's scan flagged,
    an item that is gone, a link Graph cannot decode, a throttle carrying Microsoft's own
    `Retry-After`, a refused token. `message` and `innerError` are never read.
  - **Both ways in.** The browser redirect with PKCE, and the device code typed at
    `microsoft.com/devicelogin` on any other screen — the first shipped plugin to use the device
    entrance with a renewal behind it. Scoped `Files.Read.All` and `offline_access`, and not
    `Files.Read`, because a sharing link goes through `/shares`, which Graph grants to nothing
    narrower. The application id is registered per installation in the Microsoft Entra admin
    center and travels as `{{client_id}}`; the account's hint carries the registration steps.
  - SHA-1 and SHA-256 are carried as checksums where Graph states them; `quickXorHash` is
    Microsoft's own and is not handed on. `eTag` and `cTag` are read by nothing: the download
    contract has no validator field, and the transfer validates against the HTTP `ETag` of the
    `/content` answer instead.
- **Dropbox is the same three plugins.** `dropbox` resolves a file and owns the provider row,
  `dropbox-crawler` lists a folder of the account's own Dropbox or a shared folder link, and
  `dropbox-oauth` signs the account in through Dropbox's own endpoint with PKCE and
  `token_access_type=offline`. Only the official API v2: metadata through `files/get_metadata` and
  `sharing/get_shared_link_metadata`, listings through `files/list_folder` and its cursor, bytes
  from the content endpoints — no `?dl=1` redirect chasing and no `get_temporary_link`.
  - The resolver answers with the **stable** content address and names the file in the
    `Dropbox-API-Arg` header, pinned to the revision it just described; the scheduler carries the
    header onto the transfer and attaches the account's token. Dropbox's `content_hash` is
    verified after the download under its own algorithm, `dropbox_content_hash`.
  - A **password-protected shared link** is opened through the official `link_password`
    argument, with the password taken from `?link_password=` on the pasted address and carried on
    into every file the crawler finds behind the link. A file inside a folder is spelled
    `?preview=<name>`, Dropbox's own spelling, which is what keeps the crawler's and the
    resolver's claims disjoint.
  - The **cursor lives in the walk's queue**, not in a loop: a folder with more pages comes back
    as the next thing to read, carrying the cursor its last page ended on. A rate limit holds
    every Dropbox link for as long as `Retry-After` asked, and nothing else.
- **Box is the same three plugins, with one credential more.** `box` resolves a file and owns
  the provider row, `box-crawler` lists a folder of the account's own Box or a shared link, and
  `box-oauth` signs the account in through Box's own authorization endpoint. Only the official
  Content API: `/2.0/files/<id>` and `/2.0/folders/<id>` for what an item is, `/2.0/shared_items`
  for what a link points at, `/2.0/folders/<id>/items` for a listing, `/2.0/files/<id>/content`
  for the bytes — no scraping of the web application and no reading of an address Box handed out
  for a browser.
  - The resolver answers with the **stable** API route **pinned to a version**,
    `/2.0/files/<id>/content?version=<file_version.id>`, rather than the pre-authenticated
    `dl.boxcloud.com` address Box redirects to. The stable half is what lets a resume ask again
    after a restart; the version pin is what keeps that answer meaning one set of bytes, so a
    transfer that outlived an edit cannot splice two files together. Box's SHA-1 is carried as
    the checksum and belongs to the same version.
  - A **password-protected shared link** is opened through the official `boxapi` header, with the
    password taken from `?shared_link_password=` or `?password=` on the pasted address. It never
    reaches the address the bytes come from. It does travel on the one address that passes from
    the crawler to the resolver — as at Dropbox, because nothing else passes between two sibling
    packages — and `shared_link_password` is in `rd_core::SIGNED_QUERY_SECRETS`, so it is struck
    out of every log line and every message.
  - **Two credential slots, because Box leaves no choice.** Its token endpoint requires a
    `client_secret` on the exchange and on every renewal and accepts no PKCE, so the arrangement
    RD-106-03 built for Real-Debrid applies here too: `box_client_secret` is what the person
    registered, `box_access_token` is what the sign-in obtained, and the second is never written
    over the first. Both leave the plugins as markers. Box has no device flow, and the manifest
    says so rather than offering one that would only be refused.
  - **Offset pagination, ten pages per folder**, with the same depth, breadth and cycle limits
    every other crawler uses — what somebody gets back from pasting a folder should not depend on
    which cloud it was in. A folder that pages further says which limit stopped it.
  - A refusal **behind a shared link** is reported as a shared-link refusal. Box answers a wrong
    password with the same `forbidden` it uses for a file somebody may not read, deliberately, so
    that a link cannot be probed for whether its password is the only thing in the way; the
    plugin keeps that ambiguity rather than inventing a distinction Box does not make.
- **pCloud is the same three plugins, and one problem none of the others has.** `pcloud` resolves
  a file and owns the provider row, `pcloud-crawler` lists a folder of the account's own drive or
  a public link, and `pcloud-oauth` signs the account in. Only the official HTTP JSON API:
  `stat`, `checksumfile`, `listfolder`, `showpublink`, `userinfo`, `getfilelink` and
  `getpublinkdownload`. No page is scraped and no undocumented endpoint is called.
  - **pCloud runs two installations**, `api.pcloud.com` and `eapi.pcloud.com`, and an account, a
    `fileid` and a public link code exist in exactly one of them. The other answers `result: 2094`
    to a token and the 7xxx family to a link code, both under HTTP 200 — which reads exactly like
    a bad credential or a dead link. So the region is never guessed twice: a pCloud address names
    its installation in its host, and **only those two refusals** are retried, **once**, at the
    other one; a missing file, a denied operation and a rate limit are settled where they were
    asked. The installation that answered is then pinned for the rest of the invocation, carried
    into every address the crawler hands the resolver, and shown on the account row as
    `pcloud.account_region`.
  - **The download ticket is never written down.** `getfilelink` hands back content servers, a
    path and an `expires`, and pCloud offers no stable byte endpoint at all. What makes that safe
    is that the durable address of a download is the canonical `fileid` or link code, which
    `rd_scheduler::worker::run` re-resolves from on every attempt and
    `rd_scheduler::replay::before_resume` before every resume — so a ticket lives for one attempt
    and is minted again for the next, and the size and checksum read afresh with it are what say
    it is the same bytes. No token reaches the content host either: the ticket is already
    authorised, and pCloud's content servers are picked per request, so they are deliberately not
    among the provider's `secret_domains` and `provider_download_bearer` never fires.
  - **Checksums depend on the installation**, because pCloud documents that they do: `sha1`
    everywhere, `sha256` in Europe only, `md5` in the United States only. The strongest on offer
    travels; a public link states none and none is invented.
  - **`fileid` is the line between the two plugins.** A pCloud link code is opaque, so no address
    can say whether a link points at a file or a folder, and `claims-url` and `match-url` must be
    decided from the address alone. So a bare public link is the crawler's — even when it holds a
    single file, which then comes back as that one file spelled with its `fileid` — and an address
    carrying a `fileid` is the resolver's.
  - **pCloud refuses with a number, not a sentence.** `error` is an English sentence written for a
    developer and is never read; what travels as a parameter is `result`, a decimal integer that
    cannot carry a token, a file name or a path. There is deliberately **no page cap** in the walk
    either: `listfolder` takes no cursor and `showpublink` answers a public folder link with its
    whole tree, so a page limit would be a limit on something that never happens.
- **One generic resolver claims 140 XFileSharing sites and 211 domains.** The domain list is
  derived rather than guessed: JDownloader models 315 of its hoster plugins as stock XFileSharing
  installations naming 682 domains; the dead ones and those already served by this application's
  own resolvers were removed, and each of the remainder was asked whether it still serves the
  XFileSharing login form. What is left is what answered. The manifest states how far each site
  deviates from the base flow rather than implying they all behave identically — a site nobody
  claims is a site nobody can report a failure for. Video and image hosts among them take nothing
  away from the media and gallery pipelines, which link intake asks first.

#### Free Downloads Without an Account

- All seven named hosters and the XFileSharing sites can download without credentials, following
  the hoster's own free flow:
  waiting out the countdown, answering the captcha, and posting the download form.
- One free download at a time per hoster. Free flows share the anonymous HTTP client and its
  cookie jar, and hosters allow one free download per IP anyway, so a second attempt would
  break the first one's session and earn both an IP block.
- A hoster that reports an IP block holds back every free link of that host until the block
  expires (fifteen minutes when the hoster names no duration). Premium downloads are unaffected.
- A hoster landing page is never stored as the finished file: a plugin-resolved link whose
  response is markup fails with `download.not_a_file`, quoting the hoster's own wording. The same
  rule is applied to the responses of the transfer itself, where a page arriving instead of the
  payload is a retryable failure rather than a finished download.
- Nor is anything else that contradicts the size the hoster announced. Where the resolver read a
  size off the link page, or the online check recorded one, it is compared against the length the
  server offers before a single byte is written, and a disagreement fails with
  `download.size_mismatch`. The rule needs both numbers and never judges one on its own: a
  percent of slack absorbs a rounded announcement, and a 4 KiB floor puts legitimately small
  files out of its reach, because "the response is small" is not evidence of anything.
- Multihosters need an account by design and are excluded from the free path.

#### Captcha Solving

- Captchas that a sandboxed resolver cannot answer are handed to the application, which either
  buys an answer from a solver service or asks the user.
- Image captchas appear as a prompt anywhere in the web interface, with the picture inline, a
  countdown to the deadline, and the choice to answer or decline.
- Click-point captchas — a picture answered by clicking one spot in it — appear the same way
  (RD-110-15). A click marks the spot, a second click moves the mark, and only the button sends
  it; the answer reaches the plugin as a coordinate in the image's own pixels, whatever size the
  dialog rendered it at. A solver service answers one as a coordinate task. Text is refused for
  it, and a click is refused for an image captcha, with `captcha.answer_shape` — the challenge
  keeps waiting for the answer it takes.
- CutCaptcha is answered by a solver service alone: no browser rDownloader drives can read its
  token. Without a configured service the download fails at once with
  `captcha.cutcaptcha_needs_solver` rather than waiting out the manual timeout, and the challenge
  is neither shown to a person nor offered to the extension.
- Widget captchas (reCAPTCHA v2, hCaptcha, Turnstile) are bound to the hoster's own domain and
  are never rendered inside the web interface — measured against DDownload's real site key,
  a page served from `localhost` earns `110200 - domain not allowed` and renders nothing. In
  the web interface they are therefore shown as a hint naming the hoster and the captcha type,
  with a link to the solver settings.
- Two things can answer one: a configured solver service, or the user in their own browser,
  through the rDownloader browser extension (see section 26). There is no third way — the
  desktop capture agent used to open a window for it and no longer does (RD-109-11), so a
  person with neither the extension nor a solver service is told so rather than left waiting.
- A typed answer stays refused for a widget whatever it says, because nothing typed outside
  that page is accepted. Declining stays possible, and still reports `captcha.skipped`.
- Optional solver service with a 2captcha-compatible `createTask`/`getTaskResult` API
  (2captcha, CapMonster, CapSolver, and others), configured with an https endpoint and an API key.
- The stored key can be checked against the service, which reports the remaining balance or a
  translated reason for the refusal, before any download depends on it.
- A rejected key or an empty balance stops the attempt instead of quietly asking the user
  instead; a service that is merely busy falls back to asking, for image captchas.
- The answer timeout is configurable between 15 and 600 seconds; the waiting time is reserved
  from the resolver's own budget, so a generous timeout is never cut short mid-answer.

### 5. Signed WebAssembly Plugin System

- Resolver plugins packaged as `.rdplug` archives with a manifest, WebAssembly component, and
  Ed25519 signature.
- Versioned WIT interface `rdownloader:plugin@0.9.0` with manifest version 3.
- Cache check for sources no resolver reaches (RD-130-11): a `remote-job` plugin names the kinds
  its provider's cache answers for (`cache-kinds`) and answers a batch of magnets, NZB links and
  hoster links read-only (`check-cached`). TorBox asks its three `checkcached` endpoints,
  Premiumize's transfers plugin asks `cache/check` about magnets; the LinkGrabber's cache chip
  names the provider that answered in its tooltip.
- Plugin functions for URL matching, account checks, resolution, link checks, and hoster catalogues.
- Capability-based permissions: a manifest declares the plugin type, the ABI version it speaks and
  every grant it asks for — web requests with their domain allowlist, account cookies, CAPTCHA
  solving, and the credentials it may expand.
- One grant is one WIT interface. A plugin that was not granted a capability cannot link the
  interface for it, so the permission is enforced before the plugin runs rather than at the call.
  The file system and other processes have no interface at all.
- The plugin manager shows the type, ABI version and granted permissions of every installed
  package.
- Twelve plugin types: hoster resolvers; transfer backends that carry a protocol's bytes on the
  shared queue with host-owned sockets, a host-owned destination and opaque resume checkpoints
  pinned to the version that wrote them; and the ten extension types — intake parsers, sign-in
  providers, OAuth providers, folder crawlers, metadata enrichers, notification destinations,
  post-processing steps, upload destinations, remote jobs and stream transforms — most of them
  described in their own section below.
- Streams the host has to transform (RD-110-33, ADR 0011). A provider that encrypts every file
  on the client and keeps the key out of its own reach cannot be carried by a resolver: a
  resolved download is an address and headers, with no field key material could travel in. The
  twelfth world carries the address **and a declarative description of the transform** — a named
  primitive with its parameters — and the bytes never leave the host: the cipher runs on
  `rd-http`'s own write path, where the buffer is already allocated at an offset that is already
  known, so there is no second pass over the file and no second copy on disk. The key goes
  straight into the vault and appears in no log line, no event and no error message; the
  provider's integrity value is verified before the part file is promoted, and a mismatch is a
  failed attempt that keeps the partial file rather than a wrong file presented as complete. A
  continuation resumes only what the same description wrote. Parallel connections survive where
  their boundaries line up with the provider's chunk boundaries and fall back to a single
  connection where they do not. Two primitives, the ones one provider needs: AES-128-CTR with the
  counter built from a nonce and a big-endian block index, and a per-chunk CBC-MAC chain
  condensed into one integrity value.
- Seventy-two plugins ship with the release (counted from `plugins/*/manifest.toml` on
  2026-09-23): twenty-seven resolvers — twenty-one hosters and cloud drives, six multihosters —,
  the reference transfer backend, two stream-transform plugins, two intake parsers, three
  notification destinations, three post-processing steps (two checksum steps and a file-name
  tidier), one upload destination, two metadata enrichers, five sign-in providers, twelve folder
  crawlers, eight OAuth providers and six remote-job plugins. Twelve of them are the four cloud
  drives — Google Drive, OneDrive/SharePoint, Dropbox and Real-Debrid's sign-in beside its
  resolver — because a manifest carries exactly one plugin type and only a resolver may declare
  the provider row the others hang off. One
  service, format or destination per plugin, so each can be updated, versioned and switched off on
  its own. Two bundled plugins can never share an id — a test fails if they do, because the loader
  keeps only the highest version per id and the other one would be skipped silently.
- Automatic installation of newer bundled plugins when the server starts. A build that shipped
  without bundled plugins says so at startup rather than presenting an installation with no hoster
  resolvers and nothing explaining why.
- Manual installation through the web interface or CLI.
- Every installed plugin can be deleted behind a confirmation, or switched off: it stays installed
  and listed so it can be switched back on, but it is no longer loaded, compiled or executed, and
  its provider is no longer offered when creating an account.
- The plugin manager groups the list by the plugin types actually installed, with a count each, and
  marks which of two installed versions is the one being loaded. The groups are a wrapping row of
  chips rather than a tab bar, so an eleventh or twelfth plugin type adds a chip instead of
  shortening every label (RD-107-17).
- The list is keyed by plugin rather than by installed version directory: the version that loads is
  the card, superseded versions sit under it in a collapsed sub-entry, and every count is a count
  of plugins — the number to compare with the packages a release ships (RD-108-10).
- A superseded version can be removed behind the usual confirmation. The service refuses with
  `plugin.version_in_use` while unfinished work is bound to that exact version — a resolver pin a
  job claimed, or a transfer checkpoint only that version can read — so cleaning up cannot break a
  job that is still running, paused or waiting on a retry (RD-108-10).
- The highest installed SemVer version of a provider wins for new jobs.
- Third parties can publish standalone resolvers for new hosters and multihosters without changing
  the core repository; provider metadata and translations are supplied by the plugin package.
- Third-party developers can generate their own Ed25519 key pairs and sign `.rdplug` packages with
  their private key.
- Trust on first use for third-party keys: the Plugins UI or CLI displays the fingerprint and asks
  the user to approve the plugin's public key before it is trusted persistently.
- Embedded release key plus configurable additional trusted keys, revocation, and key rotation.
- Trust roots are a table with a role — application update, plugin, tool manifest, repository, site
  rules — and
  an optional expiry, so rotating a key is an entry with an overlap window rather than a build that
  stops trusting everything signed with the old one. An individual artefact can be withdrawn by its
  digest without revoking the key that signed it, since revoking the key would take down every
  other artefact the same author signed.
- The signature, digest, trust and freshness primitives live in a crate of their own, shared by
  plugin packages, the managed tool manifest and application updates. The length-prefixed digest
  framing is pinned by a byte-literal test and never changes, so every `.rdplug` already published
  keeps verifying.
- CLI for key generation, packaging, verification, installation, and trust management.
- Separate development mode for deliberately unsigned local plugins.
- Sanitised execution history per plugin — which version, which entry point, which failure class,
  with a quotable correlation id and no credentials — shown in the plugin manager and served by
  `GET /api/v1/plugins/{id}/executions`. The manager offers the accordion only for a plugin that
  has actually recorded something: the inventory carries the number of invocations per plugin, so
  the card can leave the control out without fetching a single entry, and the entries themselves
  are still read only when somebody opens the panel.
- Plugin SDK: `rdownloader plugin new --type` scaffolds eleven of the twelve plugin worlds —
  resolver, transfer, intake, auth, oauth, crawler, enricher, notifier, postprocess, storage,
  remote-job — into a plugin
  that builds, packages and passes conformance outside this repository, and
  `rdownloader plugin conformance` reports machine-readably whether a package would run. The
  `oauth` scaffold is a complete authorization-code flow with PKCE *and* a device-code flow, with
  its response reading and its crypto outside the component so `cargo test` runs in a fresh
  scaffold; the `crawler`
  scaffold is a breadth-first folder walk with its own depth, breadth and cycle limits, tested
  the same way; the `remote-job` scaffold (RD-108-05) is the seven calls of a provider-side job
  with the BitTorrent content key derived locally — SHA-1 and bencode written out so the only
  dependency stays `wit-bindgen` — and an unknown provider state that waits rather than failing
  a job that is going perfectly well.
- Folder crawlers (`crawler-plugin`) turn one address that stands for several files into those
  files. `claims-url` decides narrowly and reaches nothing; `crawl` walks the folder under the
  plugin's own limits and hands back names, sizes and the folder each file sat in, which becomes
  the package suggestion. An empty, missing or unreachable folder reports a stable code in the
  person's own language rather than producing an empty package. `plugins/premiumize-crawler`
  lists Premiumize.me cloud folders and items through `folder/list` and `item/details`;
  `plugins/google-drive-crawler` lists a Google Drive folder or shared drive through `files.list`,
  following the page token Drive answers with up to a cap of its own — the limit a paginating API
  needs and a whole-answer one does not; `plugins/onedrive-crawler` lists a OneDrive or SharePoint
  folder sharing link through `/shares/{id}/driveItem/children`, following Graph's
  `@odata.nextLink` as given but only while it still points at Graph.
  needs and a whole-answer one does not; `plugins/dropbox-crawler` lists a Dropbox folder or shared
  folder link through `files/list_folder`, carrying the cursor in the walk's own queue so a folder
  with more pages is continued from exactly where its last page ended.
- Pixeldrain, with or without an account (RD-120-07, RD-120-38). `plugins/pixeldrain` resolves `/u/` and
  the API's own file addresses through the provider's documented
  `GET /api/file/{id}/info`, and hands the queue `pixeldrain.com/api/file/<id>?download` — an
  address that carries no signature and no deadline, so a job that waits an hour for its turn
  still has a working one. `plugins/pixeldrain-crawler` unpacks a `/l/` list through
  `GET /api/list/{id}` into one candidate per file, under the list's own title. The account is
  optional and holds an API key, which Pixeldrain takes as the HTTP Basic password under an empty
  user name: the requests write `{{basic:pixeldrain_api_key}}` and the host builds the pair —
  an empty name is allowed because this row does not require one — and the download engine
  attaches the same pair to the transfer (`transfer_auth = "basic"`), to `pixeldrain.com` only.
  A key Pixeldrain refuses has its own code. Without a key the provider's
  limits are reported rather than lifted: the per-IP allowance, the transfer
  volume, the concurrency ceiling and the captcha state each carry a stable code and a
  translation in all four languages, and `GET /api/misc/rate_limits` is read before a download
  is handed over so a spent allowance becomes a scheduled wait instead of a 429 mid-transfer.
- Folder crawlers for services nobody hosts centrally (RD-107-05). `plugins/nextcloud-crawler`
  lists a public Nextcloud or ownCloud folder share through the public DAV endpoint — the modern
  `/public.php/dav/files/<token>` from Nextcloud 29 on, `/public.php/webdav/` before that and on
  ownCloud — with names, sizes, subfolder structure and the shared folder's name as the package
  suggestion; a password-protected share is opened by appending the password to the address after
  a `#`, and without one the refusal says so instead of reporting an empty folder. Such a share
  is also *loaded*, not only listed (RD-108-07): the crawler puts the user name the public
  endpoint fixes — `anonymous`, or the share token on the old endpoint — into the addresses it
  hands back, never the password; the host lifts it out, drops any address that carries a
  password, and turns the password from the fragment into a `Basic` auth profile in `rd-secrets`
  scoped to exactly the path all the found files share. The queue picks it up by scope, so the
  download sends the same `Authorization` the listing did, and a wrong password ends with
  `nextcloud_crawler.password_wrong` rather than as an empty folder. An address that *no* crawler
  claims — a typo in the host name, a share whose plugin is missing — keeps no fragment either:
  no link candidate is ever stored with one, so a pasted share password cannot end up in a row,
  in a listing or in a log line (RD-109-32).
  `plugins/directory-index-crawler` lists an open directory index of Apache, nginx, Caddy or
  lighttpd, keeping only the links that resolve strictly one level below the address it was
  given. Three host rules make them possible and apply to any third-party crawler of the same
  shape: `PROPFIND` is a reading method a crawler may send without being handed `PUT`, `DELETE`
  or `MKCOL`; a crawler may declare `*` for its domains and is narrowed, for the duration of one
  crawl, to the host of the address that was pasted, with a redirect off it still refused; and a
  crawler that claimed an address wrongly refuses with `unsupported`, after which the selection
  asks the next crawler instead of ending the link. Crawlers that claim by the shape of a path
  declare `generic = true` and are asked after every crawler that names a service.
- A folder crawler for a link protector (RD-110-17). `plugins/peeplink-crawler` turns a
  `peeplink.in` or `alfalink.to` entry address into the hoster links behind it: one `GET`, the
  links in clear text inside the page's `<article>`, nothing outside it read and nothing on it
  followed. It is the one of eight measured link-protection services that is still resolvable —
  five are dead or parked, one answers every path with a maintenance page and one stands behind
  a Cloudflare managed challenge — and it needs no captcha: the reCAPTCHA, hCaptcha and QapTcha
  markers on those pages belong to the login and register popups, so the plugin declares neither
  `captcha` nor `cookies`. The two domains write their links two ways, as `<a href>` and as bare
  text, and both are read. Four refusals are told apart rather than lumped together: an unknown
  identifier answers `404` and an already deleted one answers `200` at the front page, so the
  address the answer came from is checked as well as its status. The access-password branch
  exists and is **untested**: no protected entry could be found to record.
- Remote jobs (`remote-job-plugin`, RD-107-06) carry work that runs at somebody else's provider
  and outlives the call that started it: a magnet handed to a debrid account runs there for
  hours, stops half-way to ask which files are wanted, and leaves something behind that only an
  explicit delete removes. Seven short calls — `claims`, `identify`, `submit`, `adopt`, `poll`,
  `choose`, `discard` — and none of them waits; the row, the remote identifier, the clock, the
  person's answer and the restart belong to the host. `identify` derives a content key without a
  request, the key is written down *before* the provider is asked for anything, and a unique
  index on it makes a second submit of the same content impossible before any network call
  happens — which matters because submitting is not idempotent at any of these providers.
  `adopt` closes the one window the row cannot: a crash between the request going out and the
  identifier coming back. `plugins/realdebrid-torrents` is the first implementation, taking
  magnets and `.torrent` files onto a Real-Debrid account; `plugins/torbox-jobs` is the second
  (RD-120-01), running torrents, Usenet downloads and web downloads on one state machine; `plugins/putio-transfers`
  is the third (RD-120-03), handing a magnet or a `.torrent` to a Put.io account; and
  `plugins/offcloud-cloud` is the fourth (RD-120-02), taking magnets and ordinary web addresses
  onto an Offcloud account. `plugins/premiumize-transfers` (RD-120-23) is the fifth: it claims
  all three shapes of source and is the first that needs the multipart half of a provider's
  submit to do it, because Premiumize's `transfer/create` takes `src` as a URI or as a file
  upload -- a magnet, a plain `http(s)` link, or a `.torrent`, `.nzb`, `.rsdf` or `.dlc`
  container whose format is read from its bytes. `plugins/seedr-jobs` is the sixth (RD-120-04),
  running magnets and `.torrent` files on a Seedr account -- and the first whose poll is the
  account's *folder listing* rather than a transfer endpoint, because a finished Seedr transfer
  stops being a transfer and becomes a folder, so the endpoint named after polling can only
  answer "not here", which is what a deleted transfer answers too. The five that followed are what showed which parts of the contract
  were Real-Debrid's habits rather than the world's: not one of them has a file-selection step,
  so none of them asks a person anything and `choose` is refused under a stable code rather than
  reporting a selection nothing acted on; Offcloud's sources need two content-key spaces rather
  than one, because a web address carries no info hash; and its folder structure comes from a
  second call, because the status call knows no paths. The reasoning, and the four designs
  that were rejected, are in `docs/adr/0003-a-job-that-runs-at-the-provider.md`. Since 1.0.8
  (RD-108-03) a sweep in the service drives every row - submit, adopt, poll or give up, as
  `submit_step` decides, with the plugin's suggested waits clamped into the host's bounds and a
  job waiting for a choice not polled at all - and what the provider finished goes to the
  LinkGrabber as one package of http(s) addresses, labelled with the plugin's name. Contract
  tests drive the built component against a mock of the provider with sanitised fixtures.
  Since 1.0.8 (RD-108-04) such a job is also visible and steerable: five endpoints under
  `/api/v1/remote-jobs`, a card beside the accounts they run on showing the stage and the
  measured progress, the entries a job in `awaiting_choice` is offering, and `remote_job.changed`
  carrying every advance to the interface instead of into an empty bus. Since 1.2.0 (RD-120-23)
  the form offers only accounts whose service has a `remote-job` plugin installed, read from
  `GET /api/v1/remote-jobs/providers` and therefore from the installed manifests rather than from
  any list in the code; with no fitting account it says so under a stable code rather than
  showing an empty picker. The refusal on submit, `remote_job.no_plugin`, stays for the case a
  plugin disappears between the form being drawn and the button being pressed. Since 1.2.0
  (RD-120-31) a job can also be started from a file: the form takes a `.torrent` or `.nzb`, and
  `POST /api/v1/accounts/{id}/remote-jobs` a `container` field with its bytes as base64, at most
  16 MiB — until then no path constructed a container source at all, so the plugins that accept
  one never received one. Since 1.2.0 (RD-120-51) it takes several files at once, chosen or
  dropped anywhere on the page, and hands each over as its own job, one request after another,
  with a state and — on failure — a reason per file; the providers list is read from the
  signature-checked manifests without compiling a component, and until it and the accounts have
  arrived the form shows why it waits instead of an empty picker. Deleting at the provider
  and removing an entry from the list are two separate acts on purpose: the first is the only
  path in the application that reaches a plugin's `discard`, needs `confirmed: true` in the
  request as well as a confirmation dialog, and leaves the row behind in `discarded` still naming
  the job the provider knew; the second forgets the row and sends nothing anywhere.
  **Not yet delivered:** a run against a real account.
- **Put.io is the third remote-job provider (RD-120-03)**, and the one that offers *less* than
  Real-Debrid rather than more. `plugins/putio-transfers` takes magnets and `.torrent` files onto a Put.io
  account; `plugins/putio` carries the `putio` provider row and resolves what the account holds;
  `plugins/putio-oauth` signs it in. Three answers differ from the reference implementation and
  each is a property of the provider rather than of this code. **There is no selection at the
  provider**: Put.io fetches a torrent whole and its files exist only once it has finished, so
  `poll` never answers `awaiting_choice` — it answers `ready` with the complete tree, each file
  carrying its name, its size and the folder it sat in, and the choice is made in the LinkGrabber
  before anything reaches this machine. Expressing a selection by deleting the unwanted files at
  Put.io was considered and rejected: that is exactly the implicit remote deletion ADR 0003
  forbids. **A `.torrent` is handed over as the magnet it is equivalent to**, because
  `transfers/add` takes one address and Put.io's only way to accept container bytes is a
  resumable upload session on a second host; the content key is unchanged by that, so a magnet
  and the matching file are still one job. **What goes into the queue is the stable per-file
  address** `api.put.io/v2/files/<id>/download`, never the signed storage address Put.io would
  also hand out — that one expires, and a job that waited an hour for its turn would fail with a
  refusal nobody can act on. The account's token is attached by the host, because the `putio`
  provider row declares `api.put.io` among its secret domains. Discarding cancels the transfer
  and deletes no files. **Not yet delivered:** a run against a real account, and Put.io's
  out-of-band sign-in entrance for machines without a browser, which is not implemented because
  it could not be confirmed against a published specification.
- **Seedr is the sixth remote-job provider (RD-120-04)**, and the one whose credential the
  plugin contract could not send. Its REST v1 states its own limit -- "only available to use with
  HTTP basic auth" -- and neither `{{username}}` nor `{{secret:<reference>}}` can build a Basic
  blob, because a guest holds neither half: `{{secret:...}}` substitutes on the way *out*. So the
  host gained `{{basic:<reference>}}`, which expands to base64 of `username:secret` and nothing
  else, through the same finder the secret marker uses, so the credential-mode gate and the
  domain gate run for it unchanged. The WIT contract is untouched. Three further answers are
  Seedr's rather than this code's. **The poll is the account's root folder listing**, not
  `GET /rest/transfer/{id}`: a finished transfer stops being a transfer and becomes a folder, so
  the transfer endpoint could only answer "not here" -- and so could it for a transfer somebody
  deleted. One listing shows both, in one request. **The REST API is a premium feature** by
  Seedr's own documentation, so a plan that does not reach it is refused under its own code.
  **There is no selection at the provider**, so `choose` refuses under a stable code and the
  whole finished tree arrives in the LinkGrabber; deleting the unwanted files at Seedr to express
  a selection was considered and rejected for the reason Put.io rejected it. What goes into the
  queue is the stable per-file address `www.seedr.cc/rest/file/<id>`, which does not expire.
  The credential reaches the *transfer* too (RD-120-38): the `seedr` row declares
  `transfer_auth = "basic"`, so the download engine attaches the account's own Basic pair to
  `www.seedr.cc`, and to no host a redirect leads to — no second authentication profile.
  **Not yet delivered:** a run against a real premium account.
- Sandbox without WASI access, with domain allowlists, cookie/secret boundaries, memory limits,
  fuel budget, timeout, and maximum response size.
- The built-in fallback resolvers are the same code as their packaged counterparts: each hoster's
  protocol logic exists once and is compiled for both, reading one manifest and confined by the
  same permissions, so the two cannot behave differently.
- Incompatible plugins are skipped at startup without blocking the service, and are listed in the
  plugin manager with the reason and a way to remove them instead of disappearing silently.
- A log line about a plugin carries the plugin's name beside its id, because a UUID is the right
  thing to grep for and the wrong thing to read.
- Downloads pinned to a resolver version that is no longer installed are released at startup and
  resolve through the current version of the same plugin.

### 6. Intake Parser Plugins

- A signed plugin can turn text the built-in scanner does not understand into LinkGrabber
  candidates, and can canonicalise a URL.
- It is asked whether it claims an input before being shown it, so a parser for one format does not
  see every paste.
- What it returns are proposals: the same review, blocklist and routing rules apply as to a pasted
  link. A parser cannot queue anything and never supplies request metadata.
- A rewrite that changes the host or the scheme is discarded — a normalizer tidies, it does not
  redirect.
- A failing parser costs only its own feature; intake keeps working.
- Two parsers are bundled. **Metalink** reads RFC 5854 `.meta4` and the older Metalink 3.0
  `.metalink`, proposing each listed file with its name and size — the first mirror only, since the
  LinkGrabber lists things to download rather than ways to download them. **Crawljob** reads
  JDownloader's `.crawljob` files and turns `packageName` into a grouping hint; it reads only
  `text`, `packageName` and `filename`, because such a file is written for another application and
  may also say where to download to and what to run afterwards.

### 7. Authentication Provider Plugins

- An account at a supported provider can be signed in through the provider itself instead of an
  API key being pasted in: the interface shows an address and a code, and the service polls
  until it is confirmed.
- The plugin never sees a credential and never names one. What it obtains is written to the
  vault by the host, which decides where that belongs; there is no call that reads one back.
- **The OAuth client itself is configuration, not a credential, and it is registered per
  installation (RD-106-04).** A plugin writes the marker `{{client_id}}`; the host substitutes
  what this installation registered, both in outbound requests and in the authorization URL the
  plugin returns — the one place the host expands anything into a string it did not send itself,
  and a place no secret may ever be expanded into, because that string goes to a browser. It is
  stored in the clear as the account's username, because a client id identifies the application
  to the provider rather than the person to the application, and the provider publishes it in
  the address the person is sent to. No client id ships in the repository: quotas are counted
  per client, and a compiled-in one would put every installation on one shared allowance.
- A sign-in address must be on a domain the plugin's own manifest declares, and it is shown
  rather than opened. A plugin that could name any address would be a signed phishing page.
- The flow lives in the database, so closing the browser or restarting the service in the
  middle of a sign-in loses nothing; the waiting between polls is the host's.
- Three sign-in plugins are bundled, one provider each: **Debrid-Link** and **Premiumize** (OAuth device flow)
  and **AllDebrid** (PIN flow). All three report a refusal under the code that fits it —
  consent refused, the code expired, an unreadable answer — and repeat nothing the provider
  wrote: an error value that is not code-shaped is dropped whole rather than filtered, because
  filtering keeps the digits of a leaked credential.
- A second plugin type covers the providers whose token expires. It offers two ways in and one
  way on: a redirect — authorization code with PKCE, answered at one fixed callback address the
  provider echoes a state back to, where a callback quoting a state no flow claims belongs to
  nobody and a state is dropped the moment it is answered so a code cannot be presented twice —
  and a device code, where the person types a short code on another screen and nothing redirects
  anywhere. Both end at the same renewal, which is the point: whichever way somebody signed in,
  the token that follows is kept alive without them.
- **Real-Debrid** is the first provider to arrive this way: a device code, and a token that
  dies in an hour and is renewed without anybody being asked again. Its resolver beside it reads
  the same stored token, so signing in is the only thing a person does for it — there is no key
  to paste and no session to keep. A torrent added to a Real-Debrid account is deliberately
  **not** part of this: it has to be uploaded, waited for and have its files chosen before any
  address exists, which is a job with a state machine behind it and not a link that resolves.
- Which ways in a plugin serves is stated in its manifest, in the order it prefers them, so the
  host never calls an entrance nobody implemented and a provider offering both is one plugin
  rather than two. A manifest that says nothing offers the redirect, which is what every such
  manifest meant before the device path existed. A device flow's address is confined by the same
  manifest rule as an authorization address, because both are put in front of a person with an
  invitation to sign in there.
- "Not confirmed yet" and "slow down" are waiting, not failure: they hold the sign-in open and
  the host waits as long as it was asked to, so a code is never invalidated while the person is
  still walking to the other screen.
- An access token that is about to expire is renewed from stored refresh material a minute
  ahead of time, without the person being asked again. A provider that refuses outright ends
  the sign-in and says so; a provider that cannot be reached does not, because a call that never
  completed says nothing about the credential — the token is kept and tried again later.
- A provider that no installed plugin claims is a third case, and the only final one of the
  three: no amount of waiting installs a plugin, so the renewal is recorded as failed on the
  first attempt instead of being repeated every five minutes. A sign-in in the same position
  ends the same way, and both say which provider has nothing to run it.

### 8. Metadata Enricher Plugins

- A signed plugin can add fields to a link that has just resolved, shown beside the core
  details with the plugin that supplied them and when.
- It adds and never replaces: a field whose name collides with something the application
  resolved itself is dropped, so an enricher cannot rewrite a file name, a size or a provider.
- **Off by default.** An enricher reaches a service outside this machine, so nothing is asked
  until enrichment is switched on, and only links a plugin claims are sent at all.
- **What the indexer already said reaches the plugin.** A link a subscription submitted carries
  the retained `<newznab:attr>` block — `imdb`, `imdbscore`, `imdbplot`, `coverurl` and the rest
  — inside the JSON the contract already passes, under an `indexer` key, so a plugin does not
  have to guess a title back out of a file name. It passes the same gate the archive does:
  credentials dropped, passkeys inside values redacted, cover addresses limited to absolute
  `http`/`https`, Newznab's `password` reduced to its flag. A link with no subscription behind
  it is asked exactly as before.
- **The fields survive being queued.** What an enricher found is carried onto the package and
  onto each queue row when the link is enqueued, and shown in the download list with the same
  chips the LinkGrabber row uses. Before that, it hung on the candidate alone — which for an
  auto-queueing subscription meant it was visible for a few seconds and gone afterwards.
- **Regardless of which finishes first.** The enricher's answer and the promotion an auto-queue
  subscription triggers are independent writes, and the promotion does not wait for a service
  outside this machine. An answer that arrives once the links are already on their way into the
  queue is carried onto the rows they became, so a release keeps its rating whether the fields
  were there before the enqueue, during it, or after it.
- **SponsorBlock** is bundled: how much of a YouTube video is sponsor, self-promotion or
  intro, before the download starts. It sends a video id and nothing else.
- **Film and series metadata** is bundled too: a release name from an indexer or a torrent
  gets rating, year, genre and runtime on the row, and an episode carries its series, season
  and number instead of a film title. An episode number without a season is still an episode,
  and the season is then left unsaid rather than guessed. The source is keyless, because an
  enricher has no account to authenticate with.
- **What stands beside a hit belongs to that hit.** The name the source answers with is
  compared to the name that was searched for, and a hit that does not read like it produces no
  field at all — the source's search is fuzzy, and a wrong rating beside a release is worse
  than none.
- That plugin claims no domain, because an indexer hit's URL points at the indexer and the
  content is in the file name. It is therefore asked about every link — and answers the
  commonest case, "this is not a film", without a single outward request. A silent, slow or
  nonsensical answer leaves the row untouched and the check successful.

### 9. Notification Destination Plugins

- A notification target can be a signed plugin. It delivers one message and reports the
  outcome; retries, backoff and quiet hours stay with the notification hub, so one destination
  cannot set a retry policy that affects the others.
- The token never reaches the plugin. It writes a reference-less secret marker into a header, a
  query value, the body or the address, and the host substitutes the one secret that delivery
  was granted — a marker written without a grant is refused rather than sent empty.
- Three are bundled, one service each so they can be updated and switched off separately:
  **ntfy** (topic, optional bearer token; `ntfy.sh` for a bare topic, or a self-hosted server
  given as its full address — https, or http inside the own network — with the delivery and
  its token narrowed to that one host, RD-130-15), **Discord** (webhook, with the token kept in the
  vault even though it is part of the address) and **Telegram** (bot token plus chat id).
- A target naming a plugin that is not installed is refused when it is saved, not when an event
  arrives.

### 10. Post-Processing Step Plugins

- A signed plugin can contribute one more step to the pipeline. It runs after cleanup and
  before a user script, and is switched on globally or per category — installing one changes
  nothing until somebody enables it.
- The list is ordered, so the order steps run in is the order they were switched on. A
  category's empty list means "none here" and switches a globally enabled step off.
- A step is given a package handle and the list of files it may read; it cannot name a path.
  Stopping mid-step stores a checkpoint, so a restart resumes rather than starting over, and
  "nothing to do" is reported as skipped rather than as a failure.
- A step may also rename the package's files, still by name rather than by path: a new name
  carrying a separator or a `..`, or one already taken, is refused by the host.
- Three are bundled: **SHA-256** and **MD5** sidecar verification, one format each so they can
  be updated and switched off separately, and **Tidy file names**, which replaces spaces with
  dots and collapses runs of separators. It leaves extensions and hidden files' leading dots
  alone, and reports "skipped" when a package is already tidy.

### 11. Upload Destination Plugins

- The post-processing upload target may be `plugin:<plugin-id>/<destination>`, sending the
  finished package through an installed destination instead of rclone. Everything after the
  first slash belongs to the plugin's own vocabulary.
- **Commit before delete**: the destination is asked, in a call of its own, whether it really
  holds each uploaded file, and only then is a local copy removed under `move`. A server that
  accepts an upload and stores nothing cannot take the only copy with it.
- A storage plugin declares a wildcard domain because a server's address is not knowable in
  advance; the grant that applies is the host of the destination the upload is for. It is also
  the only plugin type allowed the HTTP methods that write.
- Credentials come from the stored remote logins the FTP, SFTP and WebDAV transports use,
  matched on host and port; the plugin gets the user name and never the password.
- **WebDAV** is bundled: it creates a folder per package, uploads each file and confirms each
  one with a `PROPFIND` before anything local is deleted.
- Live progress while a plugin uploads: what the destination reports moves the upload step's
  own bar, counted across the whole package rather than the file being sent, so it does not
  fall back to zero at every file boundary.

### 12. Usenet and NZB

- Upload and parse `.nzb` files up to 64 MiB.
- Import dialog with package name, category, priority, password, and review before queueing.
- An import waiting in the LinkGrabber can be taken over paused, from the toolbar, from its own
  row or from the selection bar; it then sits in the download list in `Paused` and is started
  like any other job.
- Configurable history; deleted entries can be imported again.
- NNTP server pool with individually enabled servers, priority/fallback order, and a configurable
  connection count per server.
- Parallel segments over every connection the enabled servers allow, and two articles in
  flight per connection — the next `BODY` is on its way while the previous body is still
  arriving, so a connection never idles for a round trip between articles (SABnzbd's
  pipelining depth). Every `222` answer is checked against the message-id of the command it
  is read for: a server that answers a pipelined pair out of step, or names no id, breaks that
  connection, has both articles fetched again and is dropped to one command per connection for
  the rest of the process, with one warning; a `430` read beside another request is asked
  again alone before it is believed. An optional cap per NZB file; `0`, the default, follows
  the servers, and the General tab says whether a cap actually binds against the servers'
  current total.
- No forced disk write per article: an assembled article is checkpointed, not synced, and the
  file is synced once before it is renamed into place. A restart trusts no checkpoint — every
  range the database calls complete is CRC-checked against the disk on its own, what passes is
  kept where it lies and the rest is fetched again — so an unflushed article costs its own
  re-fetch, never the file, and a part file with a hole in the middle resumes as one.
- Articles are written at the offset they name, not in the order they arrive, so an article
  waiting for a second attempt delays nothing but itself. The connection pool belongs to the
  service, not to the file: a file change reuses the authenticated connections instead of
  opening them again, two NZB files download at once so the end of one overlaps the start of
  the next, and the pool is rebuilt only when the server settings actually change.
- A refusal is read for what it is about: `430` and `423` mean this server does not have the
  article, and only they leave a hole for PAR2 to repair. Every other status, a connection that
  breaks mid-article and a body whose checksum does not match mean the server could not answer,
  so the article is asked for again — three times, on a fresh connection each time, with a
  short growing wait. Among several servers, one that could not answer outweighs every one that
  said no. If none can deliver, the file returns to the queue as a retryable failure with its
  part file and checkpoints intact, instead of being completed with zeros in place of the
  articles that never arrived.
- TCP, TLS, authentication, and SOCKS5 proxy support.
- Connection test from the interface covering proxy, TLS, and login.
- Reusable authenticated NNTP connections.
- yEnc decoding and CRC validation of segments and complete files.
- Segment-level resume: confirmed ranges are rechecked, unconfirmed tails are truncated, and only
  missing segments are downloaded again.
- Robust handling of duplicate or zero-based segment numbers.
- Persistent destination files, atomic completion, and renewed CRC validation of existing files.
- File-name reconstruction from yEnc and Subject for obfuscated posts. Every quoted group of
  the subject is a candidate and the last one that is a file name wins, so a poster who quotes
  the release name before the file name still gets the file name.
- Content-based PAR2 detection instead of relying only on file extensions — on disk in the
  post-processor, and on the queue row the moment an assembled file lands: a file whose header
  says PAR2 is marked as repair data whatever it is called.
- PAR2 verification and repair with dedicated recovery checkpoints.
- Repair data is marked as such on the download row when the NZB is queued and again when the
  real name is known, so a recovery volume that expired on the servers is told apart from a lost
  payload file: it gets its own failure code, and in a package whose payload is complete it
  counts neither as an error nor against the progress, and raises no notification. A missing
  payload file, and a package that really lacks repair blocks, report unchanged. When the main
  index of a set arrives with a name the subject did not announce, the set's volumes still
  waiting in the queue are postponed then; a volume already downloading or finished is not.
- Whether a file with a missing segment is lost is decided when the package settles, not when
  that file happens to finish. Until nothing of the set is queued, downloading, verifying,
  repairing or waiting for a retry, the row waits in `Verifying` and says what it is waiting
  for; then the set decides. A set with PAR2 sends it to repair, a set without fails it with
  the same message as before, and a package with nothing else running is decided at once. The
  assembled file survives a restart in that state instead of being fetched again.
- Hotfolders and operating-system file association for `.nzb`.
- Unified controls and statistics alongside all other download types.

### 13. BitTorrent

- Embedded BitTorrent engine based on `librqbit`; no external torrent client is required.
- Magnet links and uploads of `.torrent` metadata up to 16 MiB.
- `.torrent` files follow the same review and LinkGrabber workflow as NZBs.
- Torrent metadata is validated before queueing; the complete file tree, name and total size
  appear in the LinkGrabber.
- Full file tree with folder tri-state selection before queueing: individual files and whole
  folders can be excluded, the selected total size updates as you choose, and the selection is
  stored on the link, carried over to the queue and applied to the engine, so deselected files
  are neither requested nor preallocated.
- Magnet links resolve their metadata on demand without starting a transfer and are then
  reviewed exactly like an uploaded `.torrent`.
- The selection can still be changed on a running torrent and takes effect immediately.
- Per-file priority tiers (high, normal, low, skip) and comma-separated exclusion patterns
  (`*.nfo`, `extras/*`, `**/sample.mkv`); the saved plan comes back naming the pattern that
  dropped each file, and clearing the field restores the selection.
- Documented, deterministic rules where selection, patterns and priority disagree: an explicit
  per-file decision beats every pattern, patterns only touch files you never decided yourself,
  `skip` is equivalent to deselecting, and a pattern-excluded file keeps its priority so
  re-including it restores the previous state.
- Tracker list per torrent with BEP 12 tiers: view, add, remove and reorder, force a fresh
  announce (rate limited to once a minute), and refresh scrape counters over HTTP (BEP 48) or
  UDP (BEP 15). Counters carry the time they were fetched and are marked stale after fifteen
  minutes rather than being shown as current.
- Live statistics per torrent: share ratio, uploaded bytes, transfer rates, connected peers and
  piece progress pushed over SSE; a paginated peer list and bucketed piece availability pulled
  while the detail view is open. Values from a torrent that left the session are marked as not
  live instead of being presented as current measurements.
- Sharing data with other peers is a switch of its own and is **off by default**, at the engine:
  a new installation uploads nothing, not even while a torrent is still downloading. Seeding sits
  inside it, and category or per-torrent overrides cannot re-enable sharing that is globally off.
  Downloading without ever uploading is unfair to the swarm and gets clients banned from some
  private trackers, which is said next to the switch so it stays a decision.
- Shared session with trackers, DHT, Peer Exchange, fast resume, and persistent session data.
- Dedicated package directory for each torrent.
- A removed torrent leaves the engine too, whichever way it was removed — the list, a package,
  auto-remove, the SABnzbd and qBittorrent APIs or MCP — and also after a restart; at start,
  a persisted torrent whose queue entry is gone is dropped before the engine can recreate its
  files. Removing keeps what was already written, as for every other download.
- Seeding policy inherits global settings → category → torrent, with each of enabled, target
  ratio and seed time inheriting independently; the effective value and the level it came from
  are shown per torrent, and either override can be set or cleared on its own.
- Seed time is persisted, so it keeps counting across service restarts instead of resetting.
- Resume ongoing seeding after a service restart.
- All torrent sockets can be bound to one network interface (Linux and macOS), with an optional
  kill switch that pauses every torrent within ten seconds of that interface disappearing and
  resumes them when it returns — the combination that makes a VPN tunnel safe.
- IP blocklist from an http(s) URL, selectable peer transports (TCP, uTP or both), a peer limit
  per torrent, and global upload and download limits.
- Outgoing peer connections can be routed through an existing SOCKS5 proxy profile. Tracker and
  metadata traffic is not covered, and the setting says so.
- UPnP port forwarding for the listen port, with a separate announced port for routers that map
  to a different external one. A failed mapping never blocks outgoing downloads.
- Configurable incoming peer port and global upload limit; port, interface, proxy, blocklist and
  transport changes rebuild the engine session on save instead of waiting for a restart, and a
  failed rebuild keeps the previous session running.
- Optional automatic deletion of stored `.torrent` metadata after completion.
- Hotfolders for `.torrent` files in review or direct-queue mode.
- `GET /api/v1/torrents/capabilities` reports what the embedded engine can actually do, so the
  UI disables what it cannot and the API rejects it with a stable error code instead of
  accepting an option and ignoring it.
- Known technical limitations of the embedded engine, all reported through that endpoint: file
  priorities are emulated by opening one tier at a time rather than being native, so files
  within a tier still download concurrently; sequential download and first/last-piece
  prioritisation, protocol encryption (MSE/PE), separate proxies per traffic class, NAT-PMP and
  PCP are not available; BEP 19 web seeds are parsed and displayed but never fetched.

### 14. FTP, SFTP, and WebDAV

- Native FTP, explicit FTPS (`AUTH TLS`) and implicit FTPS (port 990); no external client is
  required.
- SFTP over SSH with password, private key (with optional passphrase) and SSH agent
  authentication.
- WebDAV shares resolve through `PROPFIND` and then download over the ordinary HTTP engine, so
  they inherit parallel chunks, checkpoints, `Range`/`ETag` resume, authentication profiles,
  proxy, custom CA and the bandwidth limits rather than reimplementing any of it.
- Links are recognised by their scheme: `ftp://`, `ftps://`, `sftp://`, and `webdav://`/`dav://`
  (plain HTTP) or `webdavs://`/`davs://` (HTTPS). The secure and insecure aliases are never
  silently swapped for one another.
- A pasted `ftp://user:password@host/…` link is accepted, because that is how such a location is
  normally shared. The credential is stored encrypted once and the link is rewritten to its bare
  form before it reaches the queue, an event or a log line; an existing login for the same server
  and user is never silently overwritten.
- Stored logins per server, port and user, with a live test action, kept in the encrypted secret
  store and never returned by any API.
- SSH host keys are confirmed once by a person and stored per server, port and algorithm. An
  unknown server is blocked with its `SHA256:` fingerprint to compare; a server whose key later
  changes is blocked with both fingerprints and is **never** accepted automatically, because a
  rebuilt server and an intercepted connection are indistinguishable from the client. Optional
  first-use trust exists for unattended setups, is off by default, and covers only a first
  sighting.
- Directory listings are reviewed as a file tree with folder tri-state selection before anything
  is queued. Each selected file becomes its own queue row, so it resumes and retries on its own.
- Resume is validated rather than assumed: size and modification time are recorded on the first
  attempt and rechecked before continuing. If either moved, the partial file is kept and the
  transfer refused instead of being corrupted. An FTP server that refuses `REST`, or a WebDAV
  server without `Accept-Ranges`, reports that a restart would be needed instead of silently
  appending to what is already on disk.
- A transfer that ends early fails instead of promoting a truncated file into the destination.
- FTPS trusts the platform certificate store augmented by the operator's custom CA; a CA bundle
  that parses to nothing is refused rather than falling back to the public roots.
- WebDAV parsing is hardened against hostile servers: a `DOCTYPE` is refused rather than expanded
  (no entity expansion, no external entities), nesting and body size are bounded, and every
  `href` must resolve inside the requested collection and on the same origin.
- Weak `ETag` validators (`W/"…"`) are treated as absent everywhere, since they permit the body
  to change while the tag stays the same; the resume check falls back to size and
  `Last-Modified`.
- Server replies never reach the queue verbatim — they routinely quote user names and full remote
  paths — so failures carry rDownloader's own text under stable `ftp.*`, `sftp.*` and `webdav.*`
  codes that the interface translates.
- Concurrent transfers and the connect/login/read timeout are configurable; both transports honour
  the scoped bandwidth limits.

### 15. Video and Audio Downloads

- Resolve media pages through `yt-dlp`; download and merge through `yt-dlp`, `ffmpeg`, and `ffprobe`.
- Metadata such as title, duration, uploader, and thumbnail in the LinkGrabber.
- Quality selection for best quality as well as 2160p, 1440p, 1080p, 720p, and 480p.
- Audio-only option with MP3 conversion.
- Advanced format selector per link, filtering on container, video codec, audio codec, HDR mode
  (SDR, HDR10, HDR10+, HLG, Dolby Vision), frame rate, bitrate, and audio language, with the
  estimated size and the resulting format shown live. The presets are expressed in the same
  vocabulary, so a preset and a hand-built selection follow the same path.
- A filter combination that matches nothing is explained rather than merely reported: the
  selector shows how many formats each criterion would keep on its own, which distinguishes
  "this page has no HDR" from "it has AV1 and HDR, but never in the same format".
- A selection is either preferred, in which case criteria are dropped in a fixed order until
  something matches and the relaxations are named, or required, in which case the download fails
  with a stable error instead of quietly substituting something else.
- The selection criteria are persisted, not the extractor's volatile format id, and are
  re-resolved at download time. The expression handed to `yt-dlp` pins the chosen ids first,
  expresses the same choice semantically second, and keeps a merge-free last resort third, so a
  rotated format id degrades to an equivalent format instead of failing.
- Multiple audio tracks per file where the container permits them, selected by language.
- Subtitles as a separate file, embedded in the container, or both, optionally converted to SRT,
  VTT, or ASS. Manual and auto-generated tracks are kept apart throughout; auto-generated ones are
  off by default and are never implied, because a speech-recognition guess embedded in a file is
  indistinguishable from an authored translation.
- Cover image, chapter markers, tag metadata, and the description with its source URL can each be
  embedded individually. A source URL that carries a signature or token is never written into the
  file, because the signature is the credential and the file goes on to be shared and uploaded.
- SponsorBlock segments can be marked as chapters or removed, off by default and limited to
  sponsor segments unless more categories are chosen. Removing re-encodes the file and says so;
  marking leaves the media untouched.
- Approved browser sessions for private and age-restricted pages, selected per link before it
  is queued. Only cookies matching the page's own host are passed to `yt-dlp`; rows for other
  domains that shared the browser jar are dropped rather than forwarded, and the rows that stay
  follow the same scope rule as every cookie import: no public-suffix row, a parent-domain row
  written for the profile's host, subdomains only when the profile includes them. The cookie file is
  created owner-only, carries no site name, and is removed when the download ends, whether it
  succeeded, failed or was cancelled. A pinned profile that is disabled, expired or scoped to
  another site fails the download instead of silently fetching the public version of the page.
- Direct HLS (`.m3u8`) and MPEG-DASH (`.mpd`) manifests as media sources, recognised from the
  document rather than the file extension, so a signed CDN address served as `text/plain` is
  still understood. Master variants, alternate audio languages and subtitles feed the ordinary
  format selector. A live manifest is routed to the recorder rather than the file downloader,
  relative segment paths resolve against the address the manifest actually came from, and a
  manifest's credentials are not sent on to a CDN on another origin. DRM-protected streams are
  detected and refused with a stated reason; ordinary AES-128 encryption is not DRM and works.
- Output templates with a live preview, globally and per link. The grammar is closed and
  unrelated to yt-dlp's own `-o` syntax: only an allowlisted field set (`title`, `uploader`,
  `upload_date`, `upload_year`, `extractor`, `id`, `resolution`, `ext`) is available, unknown
  fields are refused while typing, and the template is expanded to a literal path before anything
  reaches the tool. Absolute paths, drive letters, and `..` are refused both before and after
  expansion, and every segment passes the shared name sanitiser, so reserved device names and
  path-length limits are handled in exactly one place.
- Container limits and missing languages are reported as warnings before queueing rather than
  discovered afterwards.
- Expand playlists into individual LinkGrabber entries.
- Live progress during download and correct handling of separate video/audio streams.
- Without `ffmpeg` and `ffprobe`, only formats that already carry both video and audio are
  offered at all; a page with nothing progressive says so before anything is queued.
- Configurable default variant or full default criteria, maximum parallel media jobs, and probe
  timeouts.
- Editable host list with around 50 presets, including YouTube, Vimeo, Twitch, SoundCloud, TikTok,
  Instagram, Facebook, X/Twitter, Reddit, ARD Mediathek, ZDF, Arte, ORF, SRF, BBC, France.tv, Rai,
  RTVE, Archive.org, and others.
- File names and paths are sanitized for the current platform and shortened to permitted path lengths.

### 16. Subscriptions

- **Script subscriptions (RD-130-19).** The administrator's own script from the scripts folder
  -- `.sh` or `.py` on Linux, `.bat`, `.cmd` or `.ps1` on Windows -- run by rDownloader itself,
  without cron or the Task Scheduler. Every line it prints that is one link is taken, into the
  review list or straight into the queue, with the subscription's category; a link already seen
  is not taken again. The same sandbox as post-processing scripts: no shell, the scripts folder
  only, the script time limit, 64 KiB of output (more fails the run rather than cutting the
  list), and a non-zero exit fails the run with the end of the script's error output as the
  reason in the history. A script has no backlog: its first run is taken as it is. Only the
  administration scope creates, changes, switches on or off, or runs one; MCP, the area bundle and its import refuse the
  kind.
- **A cron schedule** for a script subscription instead of the interval: five fields in the
  service's local time (`0 6 * * *` is six every morning), POSIX weekdays, `@daily` and the other
  aliases. A new or re-scheduled subscription waits for its first time rather than running at
  once; "check now" runs it immediately and keeps the next scheduled time. A failed run is
  retried at the interval's backoff, never later than the next scheduled time.

- Channels, users, playlists and supported gallery profiles are polled on an independent
  schedule with backoff, jitter and a bounded retry delay. A failing source blocks no other
  one: each poll is isolated and its error is stored on its own row.
- A persistent archive keyed on the item's canonical identity guarantees that an item is acted
  on once. The key prefers the source's own id, falls back to a normalised address, and finally
  to a hash of title and publication date, so a re-shared or renamed item keeps its identity
  while two genuinely different ones stay apart.
- The first check of a new subscription does not import the backlog. Everything present at
  activation is recorded as seen unless the whole history is deliberately collected for review,
  which never queues regardless of the subscription's mode.
- Items are filtered by title inclusion and exclusion, duration bounds, publication date,
  language and minimum resolution. A skipped item records which rule skipped it, and absent
  metadata never rejects, so a source that reports no durations is not silently emptied.
- The archive tells open, queued, dismissed and skipped hits apart rather than listing them
  together. A hit a
  filter rejected is stored on purpose — one that were simply dropped would be rediscovered on
  every poll forever — so the list shows all open decisions by default and pages them on the
  server in groups of 50, with every other state and all hits one counted selection away. More
  than 200 open hits remain reachable without loading the entire archive into the browser.
  Settled hits and all check records can be cleared per subscription after confirmation; open
  hits remain, SQLite reuses the freed pages, and a deleted release may be discovered again.
  Without that separation
  a title filter that works reads as one that was ignored, which is how it was reported.
- The title filter is applied to what a query returned, and the list says when that mattered. An
  indexer page carries at most 100 hits, so a check follows sequential offsets through as many as
  five pages and stops at the first short one. Stored search and category parameters survive each
  page request, duplicates across pages are folded together, and a failed follow-up page fails the
  check rather than returning a partial archive. Only an exhausted 500-hit boundary warns that an
  older match may not have been fetched. The filter is deliberately not sent as the indexer's own `q`: a
  substring, let alone a regular expression, is not what a server's tokenizer would search for,
  and handing it over would drop hits the filter accepts — the same failure, one layer earlier.
- A title filter wrapped in slashes — `/^S0\d/` — is a regular expression; anything else is
  matched as plain text. The distinction is explicit rather than inferred, because most substrings
  people already wrote are also valid expressions with a different meaning. The regex editor from
  the routing rules is available on both fields.
- RSS, Atom and podcast feeds as a subscription kind, parsed in a single pass over both
  dialects. Podcast items are downloaded by their enclosure rather than their show-notes page,
  with season, episode, duration and publication date carried through. Feeds are polled
  conditionally with the stored `ETag`/`Last-Modified`, so an unchanged feed costs a `304`.
- Feed documents are treated as hostile input: a DOCTYPE with an internal subset or an entity
  declaration is refused rather than expanded, and a truncated document fails the poll instead
  of being reported as a feed that emptied itself.
- A release or series page as a subscription kind (RD-110-21). The address is a listing -- a
  series, category or tag page -- fetched conditionally like a feed; the links on it that a
  site rule claims become the items, and each enters the LinkGrabber through the ordinary
  intake, where the rule reads the release page and the mirrors of RD-110-18 arrive as one
  group rather than as forty rows.
- Recognition for such a subscription is by release name rather than by address: series,
  season and episode, in the three spellings that occur (`S01E02`, `1x02`, and `S01` for a
  season pack, with `S01E02E03` its own thing). The same episode posted again by another
  group, in another quality or weeks later carries the same key and is therefore already had.
  A name that spells out no episode has no such key and is identified by its address, because
  a wrong key would silence a genuinely new release for good.
- Quality and language are read with the closed token lists of RD-110-18 rather than guessed,
  so `Show.2160.mkv` is no 2160p release, and they feed the minimum-resolution, language and
  exclusion filters that already existed instead of a second set.
- "Keep every release" is the explicit counter-choice: identity is the address again, and
  every further version of an episode enters the LinkGrabber.
- A watched page is polled at most every half hour, a floor of its own kind rather than the
  five minutes an indexer may be asked for, and it passes the same per-host spacing every
  other poll does.
- Newznab and Torznab indexers as a subscription kind, reusing the feed parser because the
  protocols answer with RSS carrying `newznab:`/`torznab:` attributes. Saved-search parameters
  in the configured address are preserved; results carry size and category and are imported
  through the existing NZB and torrent paths.
- The indexer API key lives in the encrypted vault, is read only when a request is built, and
  is masked out of every address that reaches a log, an error or the UI. A key pasted into the
  address is replaced by the stored one rather than sent alongside it, and an indexer that
  refuses the query — answered as HTTP 200 with an error document — is reported as a failure
  instead of being mistaken for an indexer with nothing new.
- Indexer capability discovery through `t=caps`: the category tree, the supported search
  types and the result limit. The same request is the connection test, since it proves the
  address and key without pulling results. The categories can be fetched while the subscription
  is still being written — a second endpoint takes the address and the key in the request, uses
  them for that one call and stores neither — so an indexer no longer has to be saved and
  reopened before it can be configured.
- Which categories to fetch is its own field, sent as `cat`, so a subscription interested in one
  category no longer pulls the indexer's whole feed and discards most of it. It is deliberately
  not derived from the mapping: one can want a category fetched without redirecting it anywhere,
  and keep a mapping for a category not currently being fetched. An address that already names
  its categories keeps them, because a saved search pasted out of an indexer's own RSS button
  means what it says.
- Per-subscription mapping from an indexer's categories to rDownloader categories, picked
  from the discovered tree by name. An unmapped category falls back to the subscription's own,
  and the raw category is stored on each item so a mapping added later still applies. Both the
  mapping and the requested categories travel in the settings bundle.
- Review-first and automatic queueing per subscription, with a destination category and
  priority. Accepted items go through the ordinary LinkGrabber intake, so routing rules,
  grouping and the online check apply exactly as they do to a pasted link. Queue-all and
  dismiss-all operate on a server-side snapshot of every open hit, across pages; later arrivals
  stay open and intake failures are counted and left open for another decision.
- What an indexer says about a hit is kept and shown: the cover image, IMDb id, score and
  plot, season and episode, resolution, video and audio codec, genre, grab count and size.
  The attributes travel in the `extended=1` block of the search answer the subscription
  already makes, so nothing costs a second request. The row carries the thumbnail, size, IMDb
  score and language; the rest sits behind the chevron, and attributes this build does not know by name are
  listed rather than dropped, because indexers disagree about what they emit.
- The cover can be looked at properly, in the row. The thumbnail is a button: a pointer
  resting on it, keyboard focus reaching it, or a tap opens the picture immediately in a portal,
  up to 512 px and 70 percent of the viewport height without moving or escaping the viewport;
  `Escape` or the next tap closes it, and focus never leaves the button. The visible 50-hit page
  requests its thumbnails eagerly. It is shown exactly once — the second, larger copy that used to sit in the expanded
  detail is gone, so the useful size is no longer the one behind a chevron. With external
  image loading switched off there is no thumbnail and no enlargement either, and an address
  that does not load leaves a gap rather than a broken-image glyph.
- Attributes are filtered before they are stored: a value carrying the subscription's own API
  key is redacted, an attribute named like a credential is dropped whole, and a cover address
  that is not absolute `http`/`https` is discarded rather than handed to the browser. Loading
  the pictures is a setting — on by default, and switching it off leaves every written detail
  in place.
- "Check now" reports what it did. The button is busy for the length of the request and cannot
  be pressed again while it is, per subscription rather than globally, so several can be checked
  at once; the view states that a check was started, in words that do not claim it has finished,
  because the server answers before the poll runs. The end of the check arrives on the event
  stream — the event written with the run carries the subscription and its counts — and replaces
  that line with what the check found, or with why it failed. Nothing has to be reloaded.
- The indexer-subscription section in the LinkGrabber starts open exactly when its summary reports
  at least one open hit. Its subscription sections start closed and load their first page only
  when opened. A manual close is preserved when later poll events refresh the summary, and no
  undecided-hits wording or count is shown when the total is zero.
- The count beside a subscription and the hits behind it come from the same state. Both follow
  the event stream: where the count a change reports differs from the list already loaded, that
  list is read again, so an open group fills in place instead of waiting for a page reload. A
  read that started before the hits were written cannot overwrite the newer one that replaced
  it, which is what used to leave a group empty under a climbing number. A page read that fails
  is stated in the group as a failure, with a button to ask again, rather than looking like a
  subscription with nothing left to decide.
- The archive of an expanded subscription says which of the three states it is in: the loading
  surface while it is being read, the failure when the read did not arrive, and "nothing found
  yet" only once it has settled and really is empty.
- Requests to one indexer are serialized with a quiet time between them, whether the schedule
  or the "check now" button starts them. The poll schedule spreads subscriptions by their own
  id, which says nothing about the server at the other end, so several subscriptions to one
  indexer -- one per category, as they are usually configured -- would otherwise arrive as
  several clients at once. Different hosts never wait for each other.
- An archive password announced with a hit is serialized to authenticated clients, shown beside
  the hit with a key and carried for that specific link through its LinkGrabber and download
  packages to the extractor, which tries it before the
  shared password list. Two sources: the SABnzbd `{{secret}}` marker in the title, which now
  also applies to torrent hits, and an indexer that writes a real password where Newznab
  specifies its `0`/`1`/`2` flag. The flag itself is only ever shown as a lock; storing it as
  a password would make every unpack start with a wrong one. Concurrent hits with different
  passwords do not share a batch-wide fallback.

### 17. Image Galleries

- Download complete galleries through the external `gallery-dl` tool.
- One queue entry represents an entire gallery; files are stored in a subdirectory of the package
  destination.
- Repeated runs skip existing files, making retries efficient.
- Configurable path to `gallery-dl`, editable host list, and 1–8 parallel gallery jobs.
- Presets for Pixiv, DeviantArt, ArtStation, Flickr, Danbooru, Gelbooru, e621, Rule34, Kemono,
  Fanbox, Redgifs, and Imgbox.
- Authenticated galleries can use the standard `gallery-dl` configuration.

### 18. Livestream Recording

- Record livestreams through `streamlink` into `.ts` files.
- Immediate one-time recording of a URL (“Record now”).
- Persistent channel list with URL, display name, quality, destination category, and enabled state.
- Automatic monitoring of enabled channels and recording as soon as a channel goes live.
- At most one concurrent recording per channel.
- Timestamped package and file names.
- Quality selection such as `best`, `1080p`, `720p`, or a custom Streamlink selector.
- Configurable check interval and 1–8 parallel recordings.
- Error status and timestamp of the latest live detection in the interface.
- Backoff after repeated probe failures.
- Recordings use dedicated runner slots and do not block regular downloads.

- Recurring weekly windows and one-off event times per channel, with configurable pre-roll and
  post-roll. A channel that has a schedule is watched only inside it; one without keeps being
  polled continuously, and manual "record now" works alongside either.
- Windows are stored against an IANA time zone rather than a UTC offset, so a weekly slot keeps
  its local time across daylight saving. The repeated hour when clocks go back yields one
  occurrence, and the skipped hour when they go forward yields none rather than a shifted one.
- Each occurrence is planned once, enforced by a unique key, so restarts and repeated planning
  passes cannot produce a second recording of one broadcast. A window that closed without the
  channel going live — including while the service was stopped — is recorded as missed.
- Recording from the start of a stream where the provider exposes a replay window; what is
  stored is whether replay was actually used, not merely requested.

- A recording is written as a sequence of segments. A dropped connection ends one segment and
  the recording continues in the next; bytes already written are never discarded, and the
  history is persisted after every segment so a crash leaves an accurate record. Reconnects are
  bounded, because a stream that has genuinely ended looks the same as one that dropped.
- Long recordings can be split by elapsed time or by size. A deliberate split and a disconnect
  produce the same file boundary but are recorded differently: only a disconnect counts as a
  gap. Segment names are zero-padded so they sort in recording order.
- Completed recordings can be joined into MKV or MP4 as a persistent post-processing step,
  using a stream copy rather than a re-encode. It resumes after a restart, and the segments are
  removed only once the container exists and is non-empty.
- Metadata and thumbnail sidecars are named after the recording. A sidecar that was requested
  but is unavailable records *why*: subtitles and live chat are reported as unsupported,
  because streamlink passes the muxed stream through and chat needs a per-provider client.
- Optional VOD fallback when a recording covered far less than its scheduled window. An
  unscheduled recording never triggers it, since there is no expected length to fall short of.

### 19. Post-Processing, Repair, and Upload

- SABnzbd-style levels: `None`, `+Repair`, `+Unpack`, and `+Delete`.
- Level inheritance in the order package → category → global default.
- Pipeline order: PAR2 repair → SFV verification → RAR integrity test → extraction → archive
  deletion → PAR2 deletion → cleanup → user script → rclone upload.
- PAR2 repair for Usenet packages. A main index that cannot be parsed sends verification on to
  the next member of the same set — every `.vol` volume carries the same file descriptions —
  before the package is treated as unrepairable (SABnzbd's `promote_par2`).
- Postponed PAR2 recovery volumes (SABnzbd's `postpone_pars` / `get_extra_blocks`, RD-107-04).
  When an NZB is queued, its `vol` volumes enter the package as `skipped` rows while the main
  index comes down with the payload: a release that arrives intact never pays for recovery data
  it does not read. If the repair then reports missing blocks, the volumes are re-queued by the
  block count their own names announce (`release.vol031+16.par2` carries sixteen), smallest
  first, until the gap is covered and no further. The package goes back to `downloading` while
  they arrive and the whole pipeline runs again from the top afterwards, so a restart in between
  changes nothing. A set that still cannot cover the gap says so with the stable code
  `postprocess.par2_not_enough_blocks` and the two numbers that decide it. The "Download all
  PAR2 volumes" setting (`enable_all_par`, off by default) restores the older behaviour of
  fetching every volume. In the queue such a row reads "Postponed", not "Mirror" (RD-120-16): it
  shares the `skipped` state with a waiting mirror, but the two are distinguished by the group
  key -- a mirror always carries one, a postponed volume never does -- because a volume is a
  statement about when it will be fetched, not about a second source it could come from.
- RAR integrity test (`unrar t` / `7z t`) as a substitute check, run only when neither a PAR2
  set nor an `.sfv` index answered whether the payload arrived intact (SABnzbd's
  `try_rar_check`). A missing tool, or an archive whose password is not known yet, is recorded
  as skipped rather than as damage.
- "Post-process only verified packages" (SABnzbd's `safe_postproc`), on by default and
  overridable per category: it is the single place a failed PAR2, SFV or RAR check decides
  whether unpacking, cleanup and plugin steps run at all. Switched off, they run anyway.
- "Post-process anyway" as a per-package action (`POST /api/v1/packages/{id}/extract/force`):
  runs the pipeline once despite a failed verification without changing the setting — the
  answer to a damaged recovery set sitting beside intact archives. The failed step stays on the
  record either way, and a user script still receives status `3`.
- CRC32 verification of any `.sfv` index found in the package, before extraction and for every
  package kind. Enabled by default, switchable globally in post-processing settings and
  inheritable or overridable per category. A differing checksum or a missing listed file fails
  the step, skips extraction and cleanup, and fails the package; the step history names the
  affected files.
- Optional removal of the PAR2 recovery set once repair and extraction have both succeeded. Off by
  default, switchable globally and per category. It deliberately runs after unpacking rather than
  after the repair: PAR2 verification comes first, so discarding the data at repair time would
  leave a package whose extraction then failed with nothing to repair from. A set is the main
  index plus its volume siblings, matched on the shared stem, so a second release in the same
  folder keeps its own.
- ZIP, including ZipCrypto/AES, 7z, and RAR.
- Multipart RAR, 7z, and split-ZIP sets.
- Password attempts from the package password, `<head><meta type="password">` inside an NZB,
  the configurable password list, and `{{password}}` in the NZB file name. The explicit
  file-name or package value wins over embedded metadata.
- A password announced with an indexer hit is tried before the shared password list, whether it
  came from the SABnzbd `{{secret}}` marker in the title — which now applies to torrent hits too,
  not only where the title had already become an NZB file name — or from an indexer that writes a
  real password where Newznab specifies a `0`/`1`/`2` flag. The flag stays a flag: stored as a
  password it would make every unpack start with a wrong one. The marker never reaches the package
  name, so the password does not stand in plain text in the LinkGrabber and the queue.
- Safe extraction through a staging area; completed contents are then moved into the package
  directory.
- Live progress for ZIP, 7z, `unrar`, and external 7-Zip.
- Configurable safety limits for the number and total extracted size of archive entries.
- Optional recursive extraction of archives found inside extracted archives, capped at three
  additional passes and disabled by default. It is configurable globally in post-processing
  settings and can be inherited or overridden per category.
- Successfully extracted inner archives are always removed; recursive extraction is disabled for
  torrent packages to protect payloads that may still be seeding.
- Persistent step history and resume of interrupted post-processing.
- Visible extraction success/failure state per package.
- Cleanup of configurable file extensions and optional removal of small sample files.
- Category-specific overrides for post-processing level, script, cleanup list, SFV verification,
  "post-process only verified packages", PAR2 deletion, and upload target.
- Manual extraction of completed packages and archives.
- Option to stop starting new downloads during post-processing.
- User scripts with SABnzbd-compatible arguments and environment variables.
- Script support for shell, Python, PowerShell, Batch/CMD, and EXE, depending on the platform.
- Configurable script timeout; stdout and stderr are stored in the step history.
- Optional final upload through `rclone`, using either `copy` or `move`, globally or per category.

### 20. Storage Destinations, Categories, Rules, and Hotfolders

- Multiple allowlist-based storage roots, with exactly one of them always the default: the first
  root created is the default whatever the form submitted, the last default cannot be given up,
  deleting the default hands the flag to another root, and restoring a backup repairs a bundle
  that claims none or several. Without that guarantee a download with no category went to
  whichever root sorted first alphabetically.
- A storage root whose path will not survive the container is badged in the routing view, flagged
  in the readiness card and reported by the MCP configuration tool. A path outside a mounted
  volume is created inside the container's writable layer, so downloads run normally and
  everything is gone on the next `docker rm`. The check is deliberately quiet — only a known
  overlay filesystem carrying `/` counts, so a bare-metal install with a btrfs or zfs root is
  never accused; `tmpfs` and `ramfs` count anywhere.
- A destination that cannot be used says which part is wrong, as a stable error code with the path
  attached: the path is a file, no permission, a read-only filesystem, not creatable, not writable,
  not absolute. The check writes a probe file and removes it again, because creating a directory
  proves nothing about writing into one.
- Categories with a name, color, relative destination structure, and default category.
- Packages receive a dedicated subdirectory within the category.
- Category defaults for post-processing, scripts, cleanup, and rclone upload.
- Prioritized first-match rules for automatic categorization.
- Rule criteria: intake source, domain, protocol, file extension, MIME type, and name regex.
- Every way in is routed the same way, files included: an NZB from a watched folder, an upload or
  a SABnzbd client goes through the same rule evaluation and the same default-category fallback as
  a pasted link, so a rule for `source = hotfolder` with `extension = nzb` fires. A folder that
  names a category still outranks both, and the same file dropped again adopts the category it
  would be given now.
- Intercepted browser downloads count as their own intake source and can therefore be routed
  separately from links sent through the extension's context menu.
- Visual regex editor for creating filename-matching patterns used by routing rules.
- Full create, edit, and delete operations for storage roots, categories, and rules.
- Protection against deleting storage destinations or categories that are still referenced.
- Monitored hotfolders for `.nzb`, `.torrent`, `.dlc`, `.ccf`, and `.rsdf`, optionally recursive.
- One poll interval for every watched folder, 5 to 3600 seconds (default 30), set on the
  hotfolder tab and taken over by running watchers without a restart; a native file-system
  event still reaches a folder between two passes.
- Hotfolder modes for “review in LinkGrabber” and “queue directly.” A `.dlc` always stops in the
  LinkGrabber for review, since its links still have to be resolved before they can start.
- Separate destination directories for successfully processed and failed intake files.
- An `.nzb` a watched folder cannot take in is recorded as a failed import: the LinkGrabber shows
  the file with the reason it did not work, which is the only way a drop nobody made by hand
  reaches anybody. It holds no files, so it cannot be queued; deleting the row makes room for the
  same file to be imported again. A `.torrent` or a link container that fails keeps the log entry
  and the failed directory.
- Watchers restart automatically when configuration changes.
- Path validation and protection against escaping allowed directories through symlinks.
- Changing a package's category moves its data into the new `<category-path>/<package-name>/` and
  clears the directory it came from. Files still transferring keep their partial file and are
  promoted into the new folder when they finish, so the move never interrupts a running download;
  a directory is only removed while it is empty. A destination on another disk is handled by a
  copy-and-remove fallback rather than failing.
- One capacity policy for every transport: HTTP, Usenet, torrent, media, gallery, and stream
  jobs all pass the same free-space check before they start.
- A configurable minimum free space per storage root, with a global default; a transfer whose
  size no runner can state may start while a configurable multiple of that threshold is free.
- A root below its threshold is blocked on its own: its downloads stay queued and its intake is
  refused with `storage.capacity_blocked`, while every other destination keeps running.
- The block is persisted, so switching automatic resume off survives a restart instead of
  silently resuming; `POST /api/v1/storage/capacity/{target}/resume` releases one by hand and
  requeues exactly the downloads it held back.
- Preallocation separates a filesystem that cannot size a file up front (best effort, the file
  stays sparse) from actually running out of space, which surfaces as a real error.

### 21. Bandwidth Profiles, Budgets, and Quiet Hours

- Reusable profiles bundle a global download rate, an optional torrent upload rate, a
  parallelism cap and traffic budgets; a weekly schedule decides when each one applies.
- Limits per protocol, host, provider account and category on top of the global one. A transfer
  passes every bucket that applies to it, so the strictest applicable limit wins by
  construction, and the status names which one it was.
- The speed limit set by hand in the Downloads toolbar is a limit of its own: it survives every
  profile switch, and whichever of the two is stricter takes effect.
- Schedule windows are local times in a configurable IANA timezone and may wrap past midnight.
  Overlaps resolve by an explicit priority, then by the later start. Everything is evaluated
  from a UTC instant into local time, so a daylight-saving change neither skips a switch nor
  invents a second one.
- Daily and monthly traffic budgets, counted per local calendar period, so a restart resumes the
  same period rather than starting a new one. An exhausted budget holds back new starts only;
  transfers already running finish.
- What a limit cannot reach is stated rather than silently ignored: HTTP and Usenet are paced
  in-band, the torrent session takes the stricter of profile and setting, yt-dlp and gallery-dl
  are given `--limit-rate` for the whole job, and streamlink is reported as unenforceable in
  `GET /api/v1/bandwidth/capabilities`.
- Quiet hours postpone only the configured work — PAR2 repair, unpacking, uploads and
  notification grouping — and never touch downloads. A job already running is not interrupted;
  only the next one waits.
- A queue completion action runs exactly once per drained cycle of work, after post-processing
  has finished too: a configured script, standby, or shutdown. Standby and shutdown need an
  explicit local approval and then count down visibly, cancellable through
  `POST /api/v1/power/cancel`; adding new work or withdrawing the approval calls the countdown
  off. A completion script reuses the post-processing sandbox unchanged.
- Battery and metered operation can hold the queue with a visible reason. What a platform cannot
  report is declared unavailable in the power capability matrix instead of being guessed.
- The machine can be kept awake while it is actually working. Standby is held off while a
  download, a repair or an unpack is running — deliberately not while the queue is merely waiting,
  since a queue held back by a bandwidth schedule or an IP block is not a reason to keep a machine
  running for hours. The display is a separate switch. The hold runs through a helper process on
  every platform, so the machine is released even if the service is killed outright.
- **Reconnect**, for the hosters that limit free downloads per address. What a new address takes is
  specific to the router, so this runs a script you write, in the same sandbox post-processing
  scripts already run in. It acts only while free downloads are actually waiting on such a limit —
  never on a premium download, which is not subject to it — keeps its distance from the previous
  attempt, and by default waits until nothing is transferring rather than interrupting. Afterwards
  the address limits are forgotten and the waiting downloads are queued again. Off by default: the
  address checks that confirm the change are the one part that talks to somebody outside, and which
  addresses are asked is configurable.

### 22. Notifications and Webhooks

- Server-side delivery with a persistent history, so a finished download reaches you with the
  browser closed.
- Rules route events to targets, filtered by event, category and minimum severity. The
  idempotency key is derived from the rule and the event and is unique, so one event produces at
  most one delivery per rule even if the worker restarts mid-flight.
- Signed JSON webhooks: HMAC-SHA256 over the exact body in `X-RDownloader-Signature`, plus
  `X-RDownloader-Idempotency-Key` so a receiver can drop a repeat.
- SMTP with STARTTLS or implicit TLS and authentication.
- Apprise-compatible targets covering Telegram, Discord, Slack, Matrix, ntfy, Gotify, Pushover
  and Home Assistant through the external `apprise` CLI, looked up in the vendor folder set under
  Settings → Tools first, like every other helper, and shipped in the Docker image
  (RD-130-14). The target URL carries the service token and is therefore handed over in
  `APPRISE_URLS` in the child's environment, never as an argument that would stand in the
  process list; apprise's own error text is scrubbed of it before the history keeps it.
- Every target secret lives in the encrypted vault as a reference and is never returned by the
  API, written to a log, or placed in a backup exported without secrets. What a target answers
  is truncated and redacted before it is stored.
- Retries back off to an hourly cap and give up after six attempts; a non-retryable answer fails
  at once — including a failure a destination plugin reports as permanent, such as a rejected
  token. Each target is attempted by one task at a time, so a hanging endpoint delays neither
  the other targets nor any transfer.
- `budget_exhausted` fires once when the active bandwidth profile's daily or monthly traffic
  budget runs out, not on every sample while it stays used up.
- The history can be cleared from the list itself (RD-130-08), after a confirmation naming how
  many deliveries go. Deliveries still queued or retrying stay, because they are notifications
  not yet sent. `POST /api/v1/notifications/deliveries/clear` with `confirmed: true`, `api:admin`
  like the other clears, audited as `notifications_cleared`, and the MCP tool
  `clear_notification_deliveries`.
- Quiet hours group deliveries instead of dropping them, and every target has a test action that
  reports what actually came back.

### 23. Automation Engine

- Automations of the form "when this happens, if this holds, do that", configured in the application
  rather than compiled in.
- Twelve triggers: links arrived, a download resolved, started, finished or failed, a package finished
  or failed, an unpack, script or upload step finished, a storage threshold changed, a subscription
  accepted an item.
- Conditions reuse the routing-rule vocabulary (source, domain, extension, name, category, state,
  failure code, size, kind) with `all`, `any` and `not`; an invalid condition is refused while it is
  written, not at trigger time.
- Actions: call a configured webhook, run a script from the post-processing scripts directory, move
  the package to a category, pause or resume it. Up to ten per automation.
- Every save writes a new version, and a run holds the version it started under, so editing a rule
  never changes what an in-flight run is judged by.
- The same event starts the same automation at most once; a run interrupted by a restart is picked up
  again; failures back off and end as a recorded failure.
- **At-least-once**: the idempotency is on the run, not the individual action, so a webhook delivered
  but interrupted while answering is retried.
- The engine subscribes to the event bus instead of hooking into the queue, so a failing automation
  cannot affect the download that triggered it, and an action cannot name a URL, a path or a secret.
- A visual editor under **Automation**, built from the server's own vocabulary rather than a second
  hand-kept list, with nested `all`/`any`/`not` shown in reading order instead of on a canvas. The
  operator list narrows with the field, so a size compared with `contains` cannot be built at all.
- A dry run evaluates a trigger and a real package and reports, per automation, whether the trigger
  matches and whether the condition holds — without changing anything.

### 24. Desktop Capture Agent

- Separate lightweight `rdownloader-capture` binary for Windows, macOS, and Linux.
- Click'n'Load 2 server on `127.0.0.1:9666` and `[::1]:9666`.
- Pass package names and archive passwords from Click'n'Load to the LinkGrabber.
- Monitor the native clipboard for HTTP(S) links.
- Clipboard support on Windows, macOS, X11, and compatible Wayland compositors through the
  Data-Control protocol.
- Import `.nzb` files by double-clicking and through operating-system file associations.
- Context-menu/“Open with” integration for NZB files where supported by the platform.
- Secure pairing with a revocable capture token that is shown only once.
- Store connection details and the token in the operating-system keyring, with a private file
  fallback on Unix systems.
- Tray or menu-bar icon on Windows and macOS showing the version and server status, with “Open”
  and “Quit” actions; optional headless mode.
- The tray's status line and tooltip also name what the queue is doing: active, queued and failed
  counts, overall progress, the transfer rate and how long the queue still needs at that rate.
  Rate and remaining time come from the service — the same two figures the web interface shows,
  not a second calculation — and appear only while something is actually running. The remaining
  time is left out wherever no honest figure exists: an entry of unknown size, a rate of zero, a
  paused transfer. The tooltip is cut to the buffer Windows keeps for it; the menu entry is not.
  English, like the rest of the agent, which carries no translation catalogue.
- Deliberately headless on Linux, suitable for a systemd user service.
- Native desktop notification when links reach the LinkGrabber, one per import, on Windows, macOS
  and Linux — for installations that run the agent; the web interface raises its own notification
  independently. The agent reads a capture-scoped event stream carrying that event and a bare count
  of waiting widget captchas, and nothing else, so the restricted capture token gains no sight of
  download paths or credential changes.
  Linux uses the freedesktop D-Bus interface, which needs no desktop toolkit; a machine without a
  notification daemon logs a warning and keeps capturing. Switchable off.
- Answers no widget captchas. The agent opened the hoster's own page in a system WebView on the
  tray's event loop until RD-109-11 removed it, together with `wry`, the `captcha-webview`
  browsing profile, the second event-stream watcher and the environment/user-agent diagnostics
  built around it. A run against DDownload on 2026-09-20 decided it: the agent's window opened
  instead of the extension's tab, the challenge could not be solved in it across several
  attempts, and full sign-in credentials typed into the page it showed went nowhere. Cloudflare
  reads `navigator.userAgentData.brands`, WebView2 names itself there, and the client hints come
  from the runtime — so the window could not be fixed without defeating a bot check. Widget
  captchas are answered through the browser extension (section 26) or a solver service.
- The capture-scoped event stream still carries a bare count of waiting widgets; the agent
  reads the stream only for the LinkGrabber notification and acts on nothing else.
- The stream resumes after a dropped connection: the agent reconnects with the id of the last
  event it saw, the service replays what came after it, and an intake that arrived while the
  agent was disconnected is still announced. The service paces the reconnect with `retry:`.
  Not across a service restart — the buffer is in memory, and a held id is then answered with
  `stream.expired` and a warning in the agent's log.
- Dedicated autostart installation and removal independent of the server.
- Connect to either a local server or remote NAS/Docker installations.

### 25. `rdownloader://` URL Scheme

- The capture agent registers as the handler for `rdownloader://` on Windows and Linux, installable
  and removable per user.
- `rdownloader://add?url=…` hands links or magnets to the LinkGrabber; `rdownloader://open?path=…`
  imports a local `.nzb`. `.torrent` is refused rather than accepted and then broken at the NZB
  endpoint it would be handed to (RD-109-03).
- The parser is an allowlist, because a scheme handler is reachable from any web page: no `file:`,
  `javascript:` or `data:` links, no relative or traversing paths, no network shares or device
  namespaces, and bounded length and link count. The 64 MiB cap on an imported file is applied to
  the read itself, so a file that grows in between cannot get past it.
- Everything handed over goes through the same review the clipboard and the file association use;
  no additional way into the service exists.
- macOS is not supported — a URL scheme needs an application bundle there, and `scheme install`
  refuses rather than appearing to work.

### 26. Browser Extension

- Manifest V3 extension for Chrome/Edge and Firefox.
- The manifest carries the workspace version: extension and service are only useful as a pair, so
  one number answers which build is installed. `scripts/set-version.sh` writes it, a unit test
  fails when manifest and workspace drift apart, and the build refuses to report success without
  both archives (RD-109-17).
- Alternative to the desktop agent for NAS and Docker setups.
- Context-menu actions for a link, the current page, and selected text.
- Popup for sending the current page or all links from the clipboard.
- Package name derived from the page title when sending the current page.
- Interception of regular browser downloads: the download is handed to the LinkGrabber
  together with the metadata needed to repeat the request.
- The browser download is paused first and only removed once the server has accepted it;
  a failed handoff resumes it in the browser, so an unreachable server loses nothing.
- Interception can be switched off globally, and every single download can be kept in the
  browser through the notification action.
- Downloads started by rDownloader itself or from the server's own web interface are never
  intercepted.
- Credential-bearing headers such as cookies and `Authorization` never leave the browser.
- A download the page started with a `POST` is never taken over: repeating it would need its
  request body, and the extension reads no request bodies. Such a download stays with the
  browser, which finishes it, and the notification says why. The capture contract on the server
  still accepts a body — the extension no longer produces one (RD-109-20).
- A download the extension could not correlate is kept by the browser rather than handed over as
  a `GET`, but only where it could have observed the request in the first place; where it holds
  no host access for the address — every hoster in a default install — a plain download is
  handed over on what the download API reports (RD-109-18).
- NZB, torrent and ZIP downloads stay in the browser, because only its session can fetch an
  indexer's cart (RD-120-63) — except from a site the person allowed in the popup (RD-130-16,
  revocable there and in the options). Firefox hands over the bytes it received
  (`filterResponseData`, held back from the browser until rDownloader has them); Chrome, and
  Firefox when the copy breaks off, hands over the address with the cookies the browser would send
  to it, and rDownloader fetches it exactly once, sends the cookies only to that address's own
  origin and stores them nowhere. `POST /api/v1/capture/file` (capture scope) takes an NZB, a
  `.torrent` or a ZIP of NZBs — the first ZIP-of-NZBs import anywhere in rDownloader, one import
  per NZB, all or nothing, with member, total and entry-count limits against a decompression bomb.
- Sharing a site's session reads the cookies the browser itself would send to that page, so a
  login cookie set on the parent domain travels with it; `__Host-` and `__Secure-` cookies are
  included, and the subdomain option bounds both the read and the permission asked for
  (RD-109-22).
- Capture-token pairing, connection test, and freely configurable server address.
- Dynamically requested host permission for every server the manifest does not already cover —
  including a loopback address on a port other than 8710.
- Localization in German, English, French, and Spanish.
- Browser notification and badge on success or failure.
- Widget captchas (reCAPTCHA v2, hCaptcha, Turnstile) are answered in the person's real browser
  (RD-108-02): the extension polls the capture surface every 30 seconds, announces a waiting
  widget with a notification and a badge, and lists it in the popup with an answer and a decline
  button.
- Answering asks the browser for the hoster's origin at that moment — exactly that origin, through
  the optional host permission — opens the hoster's own page in a tab, and injects a reader that
  watches the widget's answer field and writes nothing. The token travels through the capture
  route to the waiting plugin; the tab closes and the permission is released again.
- Closing the tab without answering declines the captcha (`captcha.skipped`); a captcha that
  expires or is answered elsewhere closes its tab on the next poll.
- The web interface's captcha prompt reports whether an extension is connected, because the
  extension's poll names itself and the service remembers when it last did
  (`GET /api/v1/captcha-answerers`).

### 27. Web Interface and Usability

- Responsive single-page app with sections for Downloads, LinkGrabber, Streams, Subscriptions,
  Remote jobs, Automation, Statistics, Logs and Settings.
- Live updates through Server-Sent Events; manual refreshes are normally unnecessary.
- Navigation badges for active/total packages, LinkGrabber contents, and monitored streams.
- Light, dark, or system-dependent color scheme.
- Automatic language detection and manual language switching.
- Complete UI localization in German, English, French, and Spanish.
- Stable backend error codes with parameters; messages are translated on the client.
- App-wide drag and drop for NZB, torrent and container files.
- Global keyboard shortcuts, numbered in sidebar order: `1` Downloads, `2` LinkGrabber,
  `3` Streams, `4` Subscriptions, `5` Remote jobs, `6` Automation, `7` Settings, plus `0`
  collapse or expand the sidebar, `N` file import, `P` global start/pause action, and `?` help.
- The sidebar collapses to an icon rail by button or with `0`, and the choice holds for the rest
  of the session, across every page change.
- The LinkGrabber's indexer box has a "check all" button that asks every indexer subscription
  to check now, says for how many the check was started, and reports each result as it arrives.
- Every area that creates entries — categories, rules, hotfolders, storage roots, accounts,
  Usenet servers, bandwidth profiles and schedule, notification targets and rules, automations,
  subscriptions — shows the form beside the list it feeds, one field per row, and marks the row
  being edited with an outline and a badge while the focus moves into the form.
- Each settings page has its own address — `/settings/usenet` — and appears in an expandable
  sidebar rather than in a strip of truncated tabs. The twenty-three pages sit in six rubrics
  (RD-110-29), the same in the sidebar and on the entry page, and stable across the interface
  languages: *General* (General, Interface, Desktop client), *Downloads* (Storage & rules,
  Hotfolders, Bandwidth, Unattended operation, Post-processing), *Sources & protocols*
  (Accounts, Captcha & solver, Usenet, BitTorrent, Media, FTP/SFTP/WebDAV), *Integrations*
  (Services, Plugins, Tools, Notifications, API & MCP), *Network & security* (Network,
  Security) and *Administration* (Backup & restore, System). Only the page on screen is
  mounted, so a card on one page no longer keeps polling while another is open. Every earlier
  page address still works, the older `?tab=` links still redirect, and an address that names
  no page lands on the entry page.
- `/settings` is an entry page rather than a redirect: one card per page under its rubric,
  carrying the page's title and description and leading to it. Every card is a link, so the
  keyboard reaches it the way it reaches the sidebar.
- Every settings page opens with the same header — eyebrow, title, one sentence — and a test
  mounts each page and counts exactly one. Until 1.1.0 four pages lacked it: Desktop client had
  a bare paragraph, API & MCP a card heading without a description, Security no header at all
  and Accounts a header without a description.
- Remote jobs are a page of their own, `/remote-jobs`, beside the subscriptions in the sidebar:
  a job that runs at a provider is something to watch and answer, not a setting.
- The system settings state whether setup is finished: the four things the wizard makes
  mandatory — the login, a storage destination, a category and the setup having been completed —
  each shown as settled or outstanding with a way to the place that fixes it. Deliberately not
  the steps the wizard lets you skip, which would turn a correctly set up system into a page of
  warnings.
- Byte counts can be shown on the binary (KiB/MiB/GiB) or the decimal ladder, chosen once in
  Interface settings and applied to every figure at once without a reload. Binary stays the
  default, because changing it would make every size in every installation change overnight.
- Beside the ladder, the magnitude itself can be pinned: automatic scaling picks the step that
  fits each value, while byte, kilo, mega, giga, tera or peta prints every size in that one step
  so a column of them can be compared without arithmetic. The pinned figure keeps roughly three
  significant digits, and anything smaller than a thousandth of the unit reads `<0.001` rather
  than a `0.000` that would say nothing. Automatic stays the default.
- The browser tab reports the running state: the queue-wide rate and the number of active
  transfers while something is moving, the application name alone at rest. The title is derived
  in one place rather than per view, a rate that cannot be measured is left out rather than shown
  as a placeholder, and the whole behaviour is one switch in Interface settings for whom a
  changing title is a distraction. On by default.
- Time zones are picked from a searchable list fed by the browser's own zone data, wherever a
  schedule takes one, rather than typed as free text where a typo produced a schedule silently
  running in the wrong zone.
- Signing out from the sidebar footer, beside the connection indicator. A logout whose request
  does not reach the service still signs this browser out, because the alternative is somebody
  seeing an error and walking away from an open session.
- Captcha prompts appear over any section, so a download waiting for an answer is never missed.
- One shared event stream for the whole interface rather than one per store, since browsers allow
  roughly six connections per host over HTTP/1.1 and parked streams would otherwise crowd out
  ordinary requests.
- Row conventions are written down in `design.md` and followed throughout: icon-only edit and
  delete carrying both a label and a title, a chevron pair with `aria-expanded` for expanding, a
  switch for anything that takes effect immediately, and a confirmation for anything destructive.
- Five-step setup wizard for the administrator password, capture-agent/browser-extension pairing,
  storage destination, optional MCP access, and provider/Usenet configuration. It offers the
  directory the service actually writes to rather than a hardcoded path.
- Guided app tour introducing navigation, download controls, the status rail, LinkGrabber, and
  settings; it can be started when setup finishes.
- Both the setup wizard and app tour can be launched again from System settings.
- Runtime settings can be restored to backend-defined factory defaults without removing storage,
  routing, accounts, tokens, or downloads.
- The built frontend is embedded in the Rust binary; no separate web server is required.

### 28. Accessibility

- The interface targets WCAG 2.2 AA. A skip link is first in the tab order on every page, the
  sidebar and the routed view are landmarks, and every action is a real control rather than a
  click handler on plain markup.
- Queue changes are announced once, politely, as a summary — "3 of 9 packages downloading" —
  rather than one message per download, which would be unbearable during a busy queue.
- `prefers-reduced-motion` collapses every animation and transition; everything animated here
  is feedback that also exists as text or as a static state.
- An area that is fetching says so, in one of three states: loading, failed, or empty — and the
  empty one only once the fetch has settled. The loading surface carries `role="status"` and the
  failure `role="alert"`, so a screen reader hears the change rather than a list that silently
  fills. No list claims to be empty while it is still being fetched, and a failed fetch is never
  drawn as "nothing there".
- Accent contrast is computed from the palette and pinned by a test: the light-mode accents are
  5.27:1 and 6.29:1, both as text and as a button ground under white.
- `axe-core` runs over rendered components in the test suite. What it cannot answer — focus
  behaviour, whether an announcement is worth hearing, screen-reader use — is recorded in
  [docs/accessibility.md](accessibility.md), including what has *not* been verified.

### 29. Installable App and Sharing

- The web interface installs as a Progressive Web App: a manifest, icons and a standalone display
  mode, so it gets its own window and home-screen entry.
- The service worker is deliberately small. The API, the event stream and the compatibility adapters
  are never cached — a stored queue that shows a finished download as running is exactly what an
  installed app must not do — and navigation is network-first with the shell only as a fallback.
- A Web Share Target: sharing a link from a mobile browser opens the LinkGrabber with it. Declared as
  a GET target, so nothing has to accept a form post from an unauthenticated context; the link is
  taken over once and removed from the address, and a share without a link is ignored rather than
  producing an empty batch.

### 30. Command Line Client

- `rdownloader queue {list,summary,add,pause,resume,remove}` and
  `rdownloader links {list,add,enqueue,remove}` against a local or remote service.
- `links add` takes `--category <name or id>`, `--package <name>` and `--enqueue`, which puts
  the links straight into the queue instead of the LinkGrabber (RD-130-19). Each uses the route
  and the scope it would use from the interface: the LinkGrabber intake and its bulk edit, or
  `queue add`.
- One code path for both: the CLI talks HTTP to the REST API, and "local" is only the default
  address. `--server`/`RDOWNLOADER_SERVER` and `--token`/`RDOWNLOADER_TOKEN` point it elsewhere.
- Table output by default, the server's unchanged JSON with `--json`.
- Removals require `--yes`; failures exit with a distinct code per kind (arguments, unreachable,
  unauthorized, not found).

### 31. REST API, Events, and OpenAPI

- Versioned JSON REST API under `/api/v1`.
- Public health endpoint and authentication status.
- `GET /api/v1/metrics` in the Prometheus text format, behind the `api:metrics` area that reaches
  this one route and nothing else; `GET /api/v1/stats/transfers` serves the persistent transfer
  statistics over a day, week, month or year at `api:read`. See section 37.
- `GET /api/v1/audit/records` and `GET /api/v1/audit/export` read and export the append-only audit
  log at `api:admin`. The area has no write, edit or delete operation, and a test reads the
  OpenAPI document to keep it that way.
- Complete API for the queue, packages, LinkGrabber, NZB, torrents, link containers, streams,
  subscriptions, automations, post-processing, accounts, proxy profiles, Usenet servers, storage
  destinations, categories, rules, hotfolders, plugins, tokens, sessions, passkeys, captchas,
  managed external tools under `/api/v1/system/tools`, and settings.
- Every one of the API's 316 operations declares which of the six permission areas (or the metrics-only scrape area) it belongs to,
  and the declaration is checked against the OpenAPI document in both directions. See section 34.
- Captcha endpoints list the waiting challenges, accept or decline an answer, read and update the
  solver configuration, and test the solver against its service.
- A capture-scoped trio under `/api/v1/capture/captchas` serves the desktop agent: the waiting
  widgets narrowed to page and kind, the token back, and a decline. A capture token reaches
  nothing else of the captcha surface.
- Generated OpenAPI document under `/api/v1/openapi.json`, also exportable through the CLI.
- Frontend type generation directly from the OpenAPI description.
- Server-Sent Events under `/api/v1/events` for download, package, collector, post-processing,
  stream, and captcha changes, plus transient aggregate torrent counters (`torrent.stats`) that
  are broadcast but never persisted. `collector.intake` marks one import arriving, separate from
  `collector.changed`, which also fires for every later edit of a link.
- Both event streams resume: every frame carries a stable id, the service buffers its last 4096
  events (at most 8 MiB of payload) in memory, and a connection that sends `Last-Event-ID` is
  handed exactly the events after that id, through the same scope filter as the live stream.
  An id the buffer no longer holds is answered with the marker `stream.expired`; a lagging
  live subscriber with `stream.lagged`. Every stream opens with `retry: 5000`. The buffer does
  not survive a service restart, so a resume across one is always answered with the marker.
- Every torrent action the UI offers is available over REST: the file tree and its plan,
  magnet metadata resolution, the tracker list with reannounce and
  scrape, aggregate statistics, a paginated peer list, piece availability, the per-torrent and
  per-category seeding policy, the network status, the selectable interfaces, and the engine
  capability matrix.
- Downloads can be reset over the API: `POST /api/v1/downloads/{id}/reset` for one file and the
  `reset` / `reset_delete_files` bulk actions for a selection, with the same two variants exposed
  to the MCP tool.
- Separate CORS-enabled capture endpoints with a dedicated token scope.
- A capture-scoped event stream under `/api/v1/capture/events` for the desktop agent. It carries
  the intake event alone; the full bus stays behind session authentication, so a capture token
  never gains sight of download paths, accounts, proxies or credential changes.
- Versioned capture contract: intake accepts either free text or structured links with request
  metadata, and the ping endpoint announces the supported version so older clients keep working.
- Request metadata is validated at the boundary: only `GET` and `POST`, a fixed header allowlist,
  a bounded request body, and bounded field lengths are accepted. Structurally invalid input is
  rejected with a stable code; input that is merely impossible to reproduce is accepted and
  marked with a reason, so an intercepted download cannot vanish after the browser already
  cancelled it.
- Preview, approval, and withdrawal of a captured request — every step of the approval flow is
  available through the API, not only in the web interface.
- Unified error format `{ error, code, params }`. A refusal names the permission that was
  missing rather than reporting an authenticated caller as unauthenticated.

### 32. Download-Client Compatibility for Automation Tools

- A SABnzbd API subset at `/api` and `/sabnzbd/api`, isolated from the native `/api/v1` contract,
  so Sonarr, Radarr, Lidarr, Readarr and similar tools can use rDownloader as their download client.
- Authenticated with the existing `api:*` tokens as the API key; read-only tokens are refused.
- Queue and history listing, per-job and bulk delete, pause and resume, category listing, and NZB
  upload through `addfile`, all routed through the same handlers, validation and removal logic as
  the native API.
- Job ids are derived from package ids, so a client keeps addressing the same download across a
  restart.
- `addurl` is refused on purpose, and `del_files=1` never deletes finished files.
- A qBittorrent Web API v2 subset at `/api/v2` for the torrent side, with qBittorrent's login shape
  and the same API token as the credential: listing, properties, file list and selection, adding by
  torrent upload or magnet, pause, resume, delete and categories.
- Torrents are addressed by their real info hash, derived from the magnet while metadata is still
  resolving, so a client finds what it just added.
- The call sequences of both download clients are replayed as tests, and the endpoint matrix with its
  documented departures and known limits is published in `docs/compatibility.md`.

### 33. MCP Server for AI Assistants

- Built-in MCP server over Streamable HTTP at `/mcp` on the same port as the web app.
- Compatible with Claude Code, Claude Desktop, and other MCP clients.
- MCP access can be configured during initial setup or later under Settings → API & MCP, using the
  same scoped API tokens as other machine clients.
- 162 tools: everything the interface does except what is deliberately kept out (secrets, consents, irreversible remote changes), with a listing tool first wherever an action needs an
  id (RD-120-32, RD-120-55). Direct downloads and magnet links, container files, queue and package control
  including order, renaming, clearing finished work and unpacking on demand, status queries,
  the LinkGrabber down to the single link — rename, move, reorder, media variants, torrent and
  directory-listing plans, mirror groups — the NZB review and queueing, a torrent's detail,
  trackers and seeding, the post-processing inventory and queue, the managed external tools,
  storage capacity, the settings document, the whole configuration, jobs that run at a
  provider, the transfer statistics, the log and audit stores, and reading, switching and
  writing the release-page rules; since RD-120-55 also the histories and catalogues beside the
  editors (automation runs, versions, vocabulary and dry run, notification deliveries), a
  subscription's review list and polls, recording schedules and recording now, plugin runs,
  power and reconnect status, metrics, the diagnostic bundle's preview, an account's hosters and
  the routing regex tester.
- The toolbox is measured against the REST surface rather than assumed to match it: seventy
  capabilities, fifty-two covered and eighteen out, each omission carrying its reason.
  Twelve are on the owner's line of 2026-09-23 — his nine, and three parts RD-120-55 found to
  meet it (probing an indexer, approving a diagnostic bundle, reconnecting) — with one reason
  for all: a tool that hands out a secret, takes one in, gives a consent, or changes something
  outside this machine irreversibly is not offered — signing in and API tokens, provider OAuth, testing stored credentials,
  remote logins, captchas, consent to replay a paid link, whole-area import and export, plugin
  installation, and deleting a remote job at the provider. Removing a remote job from this
  installation's list is therefore a tool while deleting it at the provider is not.
- Every tool answer passes one mask on its way out (RD-120-57): a credential in an address — an
  indexer's `apikey`, a token, a passkey, a password in the userinfo — is replaced by
  `[redacted]` with its name kept, whichever tool returns it. The web interface's own REST
  answers keep the address as it is.
- Where one question has several routes, a tool takes a `view` or an `action` rather than
  becoming six tools — `get_torrent_details` reads the summary, peers, pieces, statistics,
  trackers or seeding policy — and every route it reaches costs exactly what the tool costs,
  held by a test.
- A container file is handed in with `import_container`, `import_torrent` or `import_nzb`, the
  file as base64 (RD-120-31). Each calls the import route's own handler with a JSON body, so it
  answers and refuses exactly as the upload does. An NZB lands in review mode as an upload does,
  and `enqueue_nzb_import` queues it from there.
- The measurement is a table the build enforces, not a list somebody keeps: a REST route that
  belongs to no capability fails the test suite, so the coverage answer cannot go stale between
  releases.
- Everything the interface configures can be configured here too: categories, routing rules,
  storage roots, watched folders, provider accounts, proxy profiles, NNTP servers, notification
  destinations and rules, subscriptions, livestream channels, automations and plugins each have
  create, update and delete tools. Supported providers stay read-only, because there is no write
  path behind them to mirror.
- Update tools merge onto the stored row rather than replacing it, so changing one field leaves
  the rest alone; a `clear` list names the fields to reset to their inherited default.
- No tool takes or returns a password, an API key or a cookie jar. A row is created over MCP and
  its credential is entered in the web interface, and a request body that names a credential
  field is refused with `request.credential_rejected`.
- Destructive tools are named individually — `delete_category`, `uninstall_plugin_version` — and
  say in their description what they destroy; there is no generic `delete`.
- MCP tools use the same application logic, validation, and error codes as REST.
- Each tool costs the same permission area as the REST route behind it, read from the same route
  table rather than written down twice — `list_downloads` is reading, `delete_packages` is queue
  control, `delete_category` is configuration, `delete_account` is credentials,
  `uninstall_plugin_version` is administration — checked at one point rather than in every
  tool. `list_configuration` costs what the section it is asked for costs, because reading the
  categories and reading the accounts are not the same request. A refused call carries the same
  stable `auth.scope_insufficient` code the REST layer returns. An assistant can therefore be
  given the queue without also being given every stored credential, and a read-only assistant is
  possible at all: the endpoint admits any token carrying an API permission, while a
  browser-capture token reaches none of it.
- Revocable tokens with a freely chosen label; the plaintext is shown only once. See
  [34. Authentication, Sessions, and Permissions](#34-authentication-sessions-and-permissions)
  for how a token is scoped.

### 34. Authentication, Sessions, and Permissions

#### Signing In

- An administrator password hashed with Argon2id, at least ten characters. Administrator login can
  be disabled for explicitly trusted local networks.
- **The password can be changed, under Settings > Security, by somebody who knows the current
  one** (RD-120-22). A wrong current password is refused exactly as a wrong sign-in is -- same
  status, same code, same body, counted in the same limiter -- and the policy check on the
  replacement runs *before* the current password is consulted, so `auth.password_too_short` can
  never mean "and your old one was right". The change ends every session, the caller's own
  included, and hands the caller a fresh one in the same response: a change that leaves the old
  sessions alive protects against nothing, and one that signs you out of the screen you made it on
  looks like a failure. It leaves an audit record naming who changed it and how many sessions it
  cost, and neither password is anywhere in it.
- The change is a **second door, not a widened first one**: setup still refuses its second call,
  and there is no recovery without the current password. API tokens keep working, because they do
  not hang off the password -- the card says so and points at the token list, since anyone who
  knew the old password could have minted some.
- Optional two-factor sign-in with an authenticator app (TOTP, RFC 6238). It is off until it is
  switched on, and the design is shaped by one fact: this is a service you host yourself, with one
  account and nobody to prove your identity to if you are locked out.
- Enrolment happens in two steps — scanning a code does not gate sign-in until a code from it has
  been accepted once, because otherwise scanning badly locks you out by the act of trying to be
  safer. Ten single-use recovery codes are issued at that same moment, while you are already
  looking at a screen you are meant to write things down from, and can be re-issued at any time.
- The enrolment shows a QR code, drawn in the browser from the `otpauth://` address rather than
  fetched as a picture, so the shared secret is not sent a second time. The key itself stays
  visible for an authenticator without a camera.
- The second factor is switched off with the password rather than a code, because a lost device
  must not be permanent on a service with no support desk.
- The prompt for a code appears only after the password was accepted, so a wrong password
  discloses nothing about the account. The seed lives in the encrypted secret store, never in the
  database, and never travels in a settings backup; a test fails if that changes.
- **Passkeys (WebAuthn)** as an alternative to the password rather than an addition to it: the
  authenticator verifies a PIN or a fingerprint before signing, so one step carries both factors.
  Several can be enrolled, named and revoked individually, and the sign-in screen offers them above
  the password field.
- Enrolling a passkey does not switch the code prompt on, and switching the code prompt off does
  not delete passkeys. They are independent ways in, and the password keeps working alongside both,
  so no single loss is unrecoverable. Both directions are held by tests.
- A passkey is bound to the external URL configured for the installation, never to a header the
  caller supplies — that binding is what makes it unphishable. `localhost` is accepted so a fresh
  install works unconfigured; an IP address or a domain with no external URL set is refused with
  the reason and a pointer to the setting, rather than being bound to whatever was claimed.
- The signature counter is written back after each sign-in, which is what lets a cloned hardware
  key be noticed; a challenge is answerable exactly once. The ceremony itself is `webauthn-rs`
  rather than hand-written code, deliberately unlike the TOTP implementation: RFC 6238 publishes
  reference values a hand-written implementation can be proven against, and WebAuthn publishes
  none.

#### Sessions

- Sessions survive a restart and are stored as a SHA-256 digest of the bearer, never the bearer
  itself, so a copied database or a backup hands over nothing usable.
- A session ends after an idle limit — hours without a request, default 12, 1 to 720 — or a
  maximum lifetime from sign-in, default 30 days and at most 90, whichever comes first
  (RD-130-09). The idle limit ends a session nobody is using; the maximum is the floor under how
  long a stolen cookie is worth anything, because a session in use would otherwise slide for
  ever. Both are set under Settings → Security, refused outside their ranges by the service, and
  changing either needs the administration scope and lands in the audit log. They are checked on
  every request rather than written into the session, so a shorter value ends the sessions
  already past it at once; a longer one applies from the next sign-in. The cookie is `HttpOnly`
  and `SameSite=Strict`, and its `Max-Age` is the maximum lifetime.
- A sign-in that runs out while the interface is open returns it to the sign-in screen with a
  notice that it expired, instead of every view failing on its own with a `401`.
- A session inventory under Settings → Security lists every open sign-in with its device, address,
  when it was last used and when it lapses, with the browser you are sitting at marked as such.
  Any of them can be ended, and "sign out everywhere else" ends the rest while deliberately keeping
  yours — an action that also signs you out is one nobody can use to check whether it worked.
- Signing out is offered in the sidebar footer beside the connection indicator.

#### Login Rate Limiting

- The lockout is per address rather than per account. This installation has one account, so
  counting failures and locking it would let anyone who can reach the login form lock you out of
  your own service by failing on purpose.
- A second, much gentler counter slows every attempt down while an attack is running, capped at
  two seconds and incapable of refusing anyone. Getting the password right clears both.
- A forwarded address is believed only from a proxy that has been named. With nothing configured,
  the address rDownloader can actually see is the one it uses and no header can change that —
  otherwise the limit would be bypassed by varying a string the client chooses.

#### Permission Areas

- Six areas, one per kind of delegation: `api:read` (queue, progress, post-processing and storage
  status), `api:intake` (adding links, files and containers), `api:queue` (controlling queued and
  running work), `api:config` (configuration that carries no credential), `api:secrets` (stored
  credentials and the resources built on them) and `api:admin` (plugins, setup, reconnect, power,
  whole-configuration import and export).
- Every one of the API's 316 operations declares its area, checked against the OpenAPI document in
  both directions, so a route cannot ship without one and a stale entry cannot linger.
- Two properties are enforced rather than intended. Nothing confers the credentials or the
  administration area — not even administration confers credentials. And everything that acts can
  also look, because controlling a queue you cannot see is useless and demanding a second
  permission for it would only teach people to grant everything.
- The read surface is kept as narrow as the hardcoded list it replaced, for the reason that list
  gave: a token pasted into a status page must not become a way to enumerate the installation. The
  LinkGrabber, the script and upload destination lists, run histories and the provider table stay
  out of it.
- A machine token is minted with the areas it needs, chosen from a list that shows how many
  operations each one reaches and flags the two — stored credentials and administration — that
  nothing else confers. The form opens on reading alone, because a form that opens on "everything"
  is a form whose default everybody keeps.
- Settings → API & MCP is the single place that creates and manages those machine tokens for REST,
  the CLI, SABnzbd/qBittorrent-compatible clients and MCP. A newly minted value is shown once as
  the raw token and as a complete `Authorization` header; the MCP command is an additional
  client-specific hand-off, not a separate kind of account.
- The numbers in that preview are derived from the same table that enforces the permissions, so
  the figure shown while granting cannot drift from the behaviour that later refuses. An unknown
  permission is refused rather than quietly dropped, and a capture token cannot be minted as an
  API token at all.
- A refusal names the permission that was missing. Presenting a browser-capture token on an API
  route answers "your credential does not cover this" rather than "please log in", which the
  holder has already done.
- The event stream applies the same areas: a monitoring token receives queue and progress events
  and never account, proxy, credential or plugin changes. A new kind of event is a compile error
  until somebody decides who may see it, rather than defaulting to everyone.
- The areas of an existing token can be changed afterwards, from the token list, without reissuing
  its value: the client keeps the bearer it has and reaches the new areas at its very next request.
  There is nothing to invalidate, because the scope lookup hashes the bearer and reads the row on
  every request rather than caching it — so a withdrawn area is refused at the next call, not at
  the next restart.
- That reverses a deliberate promise, and the reversal is argued rather than assumed. Fixed areas
  meant a leaked token could never grow more dangerous than it was on the day it leaked; what
  practice produced instead was `api:*` granted up front, because widening meant reconnecting
  every client. The replacement for the old guarantee is a record: issuing, re-scoping and revoking
  each write an event naming the areas involved and, for a change, the areas it replaced. Issuing
  wrote nothing before, which would have made the trail of a widened token start at the widening.
- Re-scoping is bounded by the same two isolations minting is. A permission that is not an API area
  is refused there as it is here, so `capture:*` is as ungrantable as it is unmintable; and only a
  token the API token list already shows can be re-scoped at all, so a browser-capture token cannot
  be turned into an API token by naming its id. The route costs the credentials area, the same one
  handing a credential out costs.
- `api:*` keeps meaning every area and `api:read` keeps meaning reading, so tokens issued before
  this split keep working unchanged.
- Capture tokens and API tokens are strictly separated and cannot be used interchangeably. Both are
  random, revocable and stored in SQLite only as SHA-256 digests.

#### Running Behind a Reverse Proxy

- One section under Settings → Security carries the three things a proxied deployment needs and
  none of which could previously be told to rDownloader: which hops may speak for a client, what
  the outside world calls this installation, and whether the session cookie may travel
  unencrypted. The external URL carries scheme, host and mount point together so they cannot
  disagree.
- The whole application works under a sub-path — `https://home.example.com/downloads` — API, event
  stream and MCP endpoint included, with the asset and manifest references rewritten as they are
  served.
- `rdownloader doctor` prints the resolved contract and warns about the half-configured
  combinations: an external URL without trusted proxies, which makes everyone share one rate limit
  and one address in the session list, or a forced-`Secure` cookie on a plain HTTP deployment,
  where the browser silently drops it and signing in appears to work while the next request is not
  authenticated.
- Sample nginx, Caddy and Traefik configurations are in [docs/reverse-proxy.md](reverse-proxy.md).

### 35. Settings Backup and Restore

- Export all service settings and configuration tables as a versioned JSON bundle.
- Includes storage roots, categories, rules, hotfolders, stream channels, proxy profiles,
  provider accounts, and Usenet servers.
- Export either without credentials or with encrypted credentials.
- Password-protected secrets using Argon2id key derivation and XChaCha20-Poly1305.
- Validate format version, IDs, references, and settings before importing.
- Atomically replace configuration tables so no partially imported configuration remains.
- Encrypted secrets are transferred into new vault references during import.
- Summary of imported objects and immediate application of settings that can change at runtime.
- Separately, one area at a time: subscriptions, stream channels with their schedules, and
  automations export to a bundle that merges by name rather than replacing, carries no
  credentials and therefore needs no passphrase, and references categories, notification targets
  and channels by name so it means the same thing on another installation. A subscription that
  needed an API key arrives switched off, and an automation whose action cannot be resolved is
  skipped whole rather than imported without it.
- A subscription can be duplicated as the starting point for a similar one; the copy arrives
  disabled and without the original's API key.

### 36. Security and Local-First Operation

- Binds exclusively to `127.0.0.1` by default.
- How people and machines prove who they are — the password, the optional authenticator code,
  passkeys, sessions, login rate limiting, the six permission areas and the reverse-proxy
  contract — is described in
  [34. Authentication, Sessions, and Permissions](#34-authentication-sessions-and-permissions).
- Provider, proxy, and NNTP secrets are not stored as plaintext in SQLite.
- The captcha solver API key is held the same way: the settings row keeps only a reference into
  the secret vault. The key is never returned by the API, never written to a log or an event, and
  never placed in a URL — it travels in the request body to the solver service and nowhere else.
- Solver endpoints must be absolute https URLs, so a typo cannot put the key on the wire in clear.
- Intercepted browser downloads carry no credential headers: cookies, `Authorization`, and
  comparable headers are removed in the extension and dropped again on the server, so they reach
  neither the database nor the API, events, or logs. Only an explicit allowlist of harmless
  headers is stored.
- The body of a form-based download is the one exception, because reproducing the download is
  impossible without it. It is encrypted in the secret vault the moment it arrives, is never
  returned by any API, and is not replayed until a person has approved that specific request.
  Only its field names and size are ever displayed. The browser extension no longer supplies
  one: reading every `POST` body in the browser is the widest permission it could hold, and the
  path was never reachable in a normal installation, so it was removed (RD-109-20). The server
  side of the contract is unchanged and still accepts a body from any other capture source.
- A repeated request is confined to the addresses that were approved for it. The approval can be
  narrowed but never widened by a client, and the transfer verifies the address it actually
  reached, so neither a credential header nor a body can arrive at a host nobody approved.
- Signed query parameters, `Authorization`, `Cookie`, and secret-vault references are redacted
  centrally — where a failure is stored and published, in REST error bodies, and in logs.
  Parameter *names* survive so support output stays readable, and the redaction is idempotent, so
  a value that passes several layers is not mangled further.
- Tracker URLs never leave the service in full. A private tracker's passkey — as userinfo, as a
  query parameter, or embedded as a path segment — is replaced before the URL reaches the API, an
  event or a log, and the UI addresses a tracker by a one-way id derived from it, so an entry can
  be kept, reordered or removed without ever seeing its credential. Web-seed and proxy URLs are
  redacted the same way, and a tracker's own error text is truncated before it is shown.
- A tracker URL comes out of a `.torrent` and is therefore attacker-controlled input, so a scrape
  resolves the host first and refuses loopback, link-local, unique-local, unspecified, broadcast
  and multicast targets, and follows no redirects — a hostile torrent cannot use rDownloader to
  reach a cloud metadata endpoint or a service bound locally. Private LAN ranges stay allowed, so
  a self-hosted tracker keeps working.
- Peer addresses are personal data of third parties: they are never written to a log and never
  put into a persisted event, and the API masks them to their network prefix — IPv4 keeps three
  octets, IPv6 the routing prefix, the port is dropped — unless an operator explicitly turns full
  addresses on.
- Local secret vault using XChaCha20-Poly1305; master key from the operating-system keyring or a
  private `0600` fallback file on Unix.
- TLS through `rustls` with platform certificate validation; optional custom CA.
- Signed resolver plugins, capability grants declared per plugin, a strict network allowlist, and
  a resource-limited Wasmtime sandbox.
- A plugin's request never follows a redirect outside the hosts it may reach: every hop is checked
  against the same list as the first address before it is followed, and a refused hop is never
  requested (RD-130-24).
- No WASI imports in distributed plugins.
- Restricted file-system destinations through configured storage roots and validated paths.
- A TLS reverse proxy is intended for remote access; the app deliberately provides no public
  multi-user or role management. The contract for running behind one is a setting rather than a
  guess, and is described in section 34.
- **A security policy, in [`SECURITY.md`](../SECURITY.md).** Where to report a vulnerability — a
  private vulnerability report on GitHub — what belongs in a report, and how long an
  acknowledgement realistically takes for a project with one maintainer and no on-call rotation.
  It says which version gets fixes (the most recent one; there are no backport branches, because a
  backport branch nobody tests is worse than an honest "upgrade"), and lists the things that look
  like findings and are not: the loopback-and-no-password first-run state, the administrator's
  ability to run scripts and install plugins, and a rate limiter deliberately incapable of locking
  the owner out of their own service.
- Being interrupted mid-write costs time and not data, and the tests do that rather than assuming
  it. The instant that matters — bytes on disk that the database has not recorded yet — lasts
  microseconds and cannot be hit by timing, so it is named as a crash point instead and a test
  stops the code exactly there. Three points in the HTTP engine are covered; each case asserts
  that no byte is counted as confirmed without having been fetched, that nothing before a
  checkpoint is rewritten, that the resumed file is byte-for-byte what an uninterrupted download
  produces, and that nothing is left behind. [docs/recovery-matrix.md](recovery-matrix.md) records
  what is covered and, deliberately, what is not.
- The migration chain is exercised with a queue in it, not only on an empty database: a database
  is built at the schema each past release shipped, seeded with a package, its download and a
  chunk checkpoint, and upgraded — and the bytes already on disk have to still be accounted for
  afterwards.

### 37. Operation, Diagnostics, and Distribution

- One server binary provides the API, web app, scheduler, and all internal runners.
- CLI commands: `serve`, `doctor`, `openapi`, `plugin`, `tools`, `autostart`, `queue`, and `links`.
- `rdownloader doctor` checks paths, SQLite, and external tools, including their location and version.
- *Settings → About rDownloader* (RD-130-12) names the running build — version, commit, build
  time, the plugin contract it links, platform — the project's addresses, the author, and the
  licenses of what ships: rDownloader itself (GPL-3.0-or-later), the seven helper tools with the
  file their text is in under `vendor/licenses/`, and every Rust crate and npm package, summarised
  by license and listed in full on demand. Commit and build time are compiled in from the same
  values `VERSION.txt` carries; the dependency list is generated by `scripts/licenses.sh`, and a
  test fails when a lockfile names a package it does not list or an entry has no license.
  The website, the repository, its changelog, its wiki (the handbook) and its private
  vulnerability reporting are links; an address that is not public yet would say "not yet
  published" instead. Behind the sign-in (`api:read`); MCP reads it as `get_about`.
- Structured log store: every `tracing` event that passes the log filter is kept in SQLite,
  redacted before storage (RD-110-02). The *Logs* page filters by level, component, stable code,
  correlation id and text; retention (records and days) is configurable and pruned in bounded
  batches that never block the queue. `GET /api/v1/diagnostics/logs` serves the same read.
- Diagnostic bundle: a local archive with versions, scrubbed configuration, system checks, the
  `doctor` output and the most recent errors, written only after its inventory was previewed and
  approved, with a deterministic manifest. Nothing is uploaded. See `docs/diagnostics.md`.
- Append-only audit log (RD-110-03): a separate table with its own retention, holding one record
  per security-relevant action — sign-ins accepted and refused, sign-outs, token use, creation,
  re-scoping and revocation, settings writes and resets, plugin trust decisions, and the
  destructive actions on downloads, packages, categories, storage roots and the configuration
  backup. Each record names the actor, the client address, the target, the outcome and the trace;
  none of them ever carries a password, a token, a digest or a signed URL. There is no route that
  writes, edits or deletes a single record and the database aborts any `UPDATE` on the table, so a
  domain delete cannot remove its own trail and a restore cannot erase the record of itself. An
  *Audit log* page filters it and exports the filter as NDJSON; both routes cost `api:admin`.
  See `docs/observability.md`.
- Starting a test run from nothing (RD-120-34): *Settings → System* empties the log store, the
  audit log and the transfer statistics **one at a time** — no collective action, because keeping
  the statistics while clearing the logs is the usual wish. Each goes through a confirmation that
  names how many records will disappear *before* it asks, the answer reports how many went, and
  the confirmation travels as `confirmed: true` in the request, so a client that drew no dialog
  clears nothing. Downloads, packages, categories, accounts and the settings are untouched.
  **Emptying the audit log writes itself into the emptied log as its first entry** — in the same
  transaction as the delete, with the moment, the account and the number of records removed.
  `POST /api/v1/diagnostics/logs/clear`, `/api/v1/audit/records/clear` and
  `/api/v1/stats/transfers/clear`, with `GET /api/v1/system/data-reset` for the three counts; all
  four cost `api:admin` and all four are MCP tools.
- Per-user autostart through systemd user units, macOS LaunchAgents, and Windows Run integration.
- Portable start/stop scripts for Windows, Linux, and macOS, covering the server, capture agent,
  or both processes.
- Multi-stage Docker build and Docker Compose example with persistent `/config` and `/downloads`
  volumes.
- Docker runs as an unprivileged user with `no-new-privileges` in the Compose example. `PUID` and
  `PGID` are honoured — the entrypoint adopts the ids given to it and drops privileges with
  `gosu` — so a bind mount owned by the host user is writable, which is the single most common
  reason a NAS setup fails. `/downloads` is deliberately not chowned recursively, because on a NAS
  share that would run for minutes on every start.
- The container image carries the external helpers at pinned versions visible under
  *Settings → Tools*: ffmpeg, yt-dlp, streamlink, gallery-dl, 7-Zip
  and par2. A newer binary can still be dropped into `/config/vendor`. `unrar` stays out: it is
  non-free, and Debian's `unrar-free` cannot read RAR5, which 7-Zip can.
- A container health check reports whether the service is actually answering, with a start period
  long enough for the plugin installation, and `TZ` defaults to UTC and is documented so schedules
  and quiet hours do not fire at the wrong time.
- [`docker/README.md`](../docker/README.md) covers volumes and why every storage root needs one,
  PUID/PGID, the bundled tools, building locally, troubleshooting, and a Synology NAS walkthrough.
- Configuration through the CLI, persisted settings, and environment variables.
- Structured logging through `tracing`, controlled with `RUST_LOG`.
- Prometheus/OpenMetrics exposition under `/api/v1/metrics`: queue depth by kind and state, a
  histogram of queue wait, the current rate and active runners by kind, completed and failed
  transfers, bytes, retries and turnaround by kind and provider, accounts by provider, blocked
  hosts, and free, total and blocked space per storage root. Label values are drawn from closed
  sets only — never a URL, a file name, an account label or a host — and a test proves the series
  set does not grow with the queue. The format is written by hand; there is no metrics crate.
- Persistent transfer statistics: every completion, retry and final failure is recorded in an
  hourly bucket and an all-time total inside the transaction that changes the download's state.
  Hourly buckets fold into daily ones and daily ones expire after two configurable periods
  (Settings → System), swept in bounded batches through the serialized writer so the queue is
  never held behind the sweep. The all-time totals are never thinned. A Statistics page draws the
  figures per range, by kind and by provider. See [`observability.md`](observability.md).
- Trace context through the API, the scheduler, the resolver and post-processing (RD-110-03): a
  request continues the caller's `traceparent` or starts one and answers with it; queued work
  derives its trace from the job, so one id covers the scheduler's attempt, the resolver call and
  the package's post-processing across restarts. The id rides as a span field and lands on every
  log record and every audit record written inside.
- Optional OpenTelemetry export, **off by default**: finished spans are posted to an OTLP/HTTP
  endpoint as OTLP/JSON in batches. A bounded channel that drops rather than waits, a failed
  batch dropped and never retried, and every attribute redacted before it is encoded — the
  request span carries the matched route pattern, never the URL. No OpenTelemetry SDK; the wire
  format is written directly, as the Prometheus exposition is.

## Architecture and Data Flow

```mermaid
flowchart LR
    A[Manual links] --> G[LinkGrabber]
    B[Capture agent / Click'n'Load] --> G
    C[Browser extension] --> G
    D[NZB / Torrent / Magnet] --> G
    E[Hotfolder] --> G
    F[REST / MCP / CLI] --> G
    N[Containers: DLC, CCF, RSDF, TXT] --> G
    O[Subscriptions: feeds, indexers, channels] --> G
    G --> Q[Persistent package queue]
    Q --> H[HTTP & hosters]
    Q --> U[Usenet]
    Q --> T[BitTorrent]
    Q --> R[FTP / SFTP / WebDAV]
    Q --> M[yt-dlp media]
    Q --> I[gallery-dl]
    Q --> S[streamlink]
    H --> P[Post-processing]
    U --> P
    T --> P
    R --> P
    M --> P
    I --> P
    S --> P
    P --> Z[Local destination / rclone / storage plugin]
    Q -. SSE .-> W[Vue web app]
```

### Internal Rust Components

| Component | Responsibility |
| --- | --- |
| `rdownloader` | Server CLI, startup configuration, diagnostics, plugin commands, and autostart commands |
| `rd-api` | Axum REST API, authentication, SSE, OpenAPI, MCP, and embedded SPA |
| `rd-authn` | Login rate limiting, CIDR matching, and which forwarded address may be believed |
| `rd-sign` | Digest framing, signature envelope, trust store, revocation, and freshness rules |
| `rd-core` | Shared domain models, IDs, states, events, and settings |
| `rd-db` | SQLite migrations, reader pool, serialized writer, and domain events |
| `rd-scheduler` | Prioritized queue, runtime limits, retry logic, and HTTP/runner orchestration |
| `rd-http` | HTTP probing, range/chunk engine, clients, cookies, and proxy |
| `rd-limits` | Bandwidth profiles, scoped limiters, weekly schedule, quiet hours, and traffic budgets |
| `rd-power` | Platform power and network context, quiet hours, and the queue completion cycle |
| `rd-notify` | Notification model, signed webhooks, SMTP, and Apprise delivery |
| `rd-usenet` | NNTP, pooling, server fallback, yEnc, segment downloads, and recovery |
| `rd-torrent` | Embedded librqbit session, torrent runner, and seeding supervision |
| `rd-ftp` | FTP/FTPS transport, directory listing, and the resumable transfer runner |
| `rd-sftp` | SFTP over SSH, the host-key trust decisions, and the transfer runner |
| `rd-webdav` | `PROPFIND` resolution and hardened multistatus parsing (no runner of its own) |
| `rd-media` | Metadata probing and downloads through yt-dlp/ffmpeg |
| `rd-tools` | Managed external tools: signed manifest, verified download, activation, rollback |
| `rd-gallery` | Gallery downloads through gallery-dl |
| `rd-stream` | Livestream probing and recording through streamlink |
| `rd-collector` | Link extraction, normalization, containers, and package creation |
| `rd-subscription` | Scheduled polling of feeds, channels, galleries, and Newznab/Torznab indexers |
| `rd-automation` | Versioned event-triggered conditions, actions, and idempotent runs |
| `rd-captcha` | Broker for solver services and manual image-captcha answers |
| `rd-hotfolder` | File-system watcher and safe handoff of NZB/torrent/DLC files |
| `rd-extract` | Persistent post-processing jobs, recovery, scripts, and uploads |
| `rd-postprocess` | Archives, multipart detection, passwords, and extraction backends |
| `rd-files` | Safe paths, file names, archive types, checksums, and storage roots |
| `rd-plugin-api` | WIT contract, shared plugin messages, and bundled manifest metadata |
| `rd-plugin-host` | Native resolvers, Wasmtime runtime, package verification, and installation |
| `rd-plugin-transfer` | Adapts transfer plugins to the shared queue |
| `rd-plugin-ext` | Adapts auth, enricher, intake, notifier, post-process, and storage plugins |
| `rd-provider-registry` | Provider, domain, credential and alias metadata, built solely from installed plugin manifests |
| `rd-secrets` | Encrypted local secret vault |
| `rd-capture` | Desktop agent, Click'n'Load, clipboard, NZB association, and tray |
| `rd-autostart` | Platform-specific autostart registration |

## Technology Stack

### Backend and Runtime

| Area | Technology |
| --- | --- |
| Language | Rust, Edition 2024, project Rust version 1.98 |
| Async runtime | Tokio 1 |
| HTTP server | Axum 0.8, Tower HTTP 0.6 |
| HTTP client | Reqwest 0.13 with HTTP/2, cookies, gzip, multipart, SOCKS, and streaming |
| TLS | rustls 0.23, rustls-platform-verifier |
| Persistence | SQLite with SQLx 0.8, embedded migrations, and bundled SQLite |
| API contract | Serde, Utoipa/OpenAPI 3, `openapi-typescript` |
| Real-time updates | Server-Sent Events through Tokio broadcast streams |
| MCP | Official Rust SDK `rmcp` 3.2, Streamable HTTP |
| Plugin runtime | Wasmtime 48, WebAssembly Component Model, and WIT |
| Plugin signatures | Ed25519 (`ed25519-dalek`) |
| Passkeys | `webauthn-rs` 0.6 |
| Second factor | TOTP (RFC 6238), implemented against the specification's reference values |
| Torrent | `librqbit` 9 |
| FTP/FTPS | `suppaftp` 11 with Tokio and rustls |
| SFTP | `russh` 0.63 and `russh-sftp` 2.4 (pure Rust, no libssh2) |
| WebDAV | `reqwest` plus `quick-xml` with entity expansion disabled |
| Archives | `zip`, `sevenz-rust2`, external `unrar` or 7-Zip |
| PAR2 | `rust-par2` |
| Cryptography | Argon2id, XChaCha20-Poly1305, SHA-2, and operating-system keyring |
| CLI | Clap 4 with environment-variable support |
| Logging | Structured `tracing` with filterable text output |

### Frontend

| Area | Technology |
| --- | --- |
| Framework | Vue 3.5 with Composition API |
| Language | TypeScript 5.9 |
| Build | Vite 7 and `vue-tsc` |
| UI components | Nuxt UI 4.11 |
| Styling | Tailwind CSS 4.1 with custom design tokens |
| State | Pinia 3 and Pinia Colada |
| Routing | Vue Router 4.6 |
| Internationalization | Vue I18n 11 |
| REST client | `openapi-fetch` with generated types |
| Utility composables | VueUse 14 |
| Icons | Lucide through Iconify |
| Fonts | IBM Plex Sans Variable and JetBrains Mono Variable |
| Tests | Vitest 4, Testing Library Vue, and jsdom |

### External Optional Tools

| Tool | Purpose |
| --- | --- |
| `yt-dlp` | Media metadata, video/audio downloads, and playlists |
| `ffmpeg` | Merging, conversion, and MP3 output |
| `ffprobe` | Media tool detection and support for yt-dlp/ffmpeg |
| `gallery-dl` | Complete image-gallery downloads |
| `streamlink` | Livestream probing and recording |
| `unrar` | RAR and multipart extraction |
| `7z` | 7z, RAR, and split-ZIP extraction as an alternative tool |
| `rclone` | Optional upload of completed packages to a remote |
| `par2` | PAR2 verification and repair as an external alternative |
| `apprise` | Optional delivery to Telegram, Discord, Slack, Matrix, ntfy, Gotify, Pushover, and Home Assistant; in the Docker image, not in the packages |

Tools are searched first in an explicitly configured vendor directory, then in the managed tool
store, then in `vendor/` next to the executable, in `vendor/` inside the data directory, in the
executable directory, and finally on `PATH`. The interface and `rdownloader doctor` show each
tool's path, source, and version.

### Managed Tool Versions

Optional and off by default: rDownloader can download, verify, activate and roll back yt-dlp,
gallery-dl, Streamlink, FFmpeg and ffprobe itself.

- A signed manifest names every installable build — URL, SHA-256, size, platform triple and the
  application versions it is declared to work with. It is verified against the compiled-in
  tool-manifest root and refused when its sequence is not above the highest one this
  installation has already accepted, which is what stops a genuine older manifest being served
  back indefinitely. The manifest compiled into the release is the offline-safe floor; an
  optional `https://` URL supplies newer ones.
- Bytes stream into a staging directory beside the version directory and are hashed as they
  arrive. The download is refused on a hash or size mismatch, and the staging directory is
  removed on every failure path, so a partially written version never becomes visible.
- Activation writes `active.json` beside the version directories and renames it into place — a
  pointer file rather than a symlink, because Windows only creates symlinks with a privilege
  or developer mode switched on. Rollback repoints it at the previously installed version.
- A running job holds a lease on the version it resolved. Activating another version takes
  effect for the next job immediately, but a leased version is never removed, so an update
  cannot pull the binary out from under a download in progress.
- Nothing outside `<data>/tools/**` is written to, and an explicitly configured tool path still
  outranks every managed version.
- Three archive formats are read: a raw binary, a ZIP and an xz-compressed tar. The xz decoder
  is pure Rust rather than a binding to liblzma. Only regular files are taken out of an
  archive, each member is flattened to its base name so none can be written outside the
  staging directory, and the bytes an archive expands to are counted as they are written
  rather than as its headers claim.
- The manifest shipped with the release names yt-dlp for x86-64 and ARM64 on Linux and
  Windows, and FFmpeg with ffprobe for x86-64 on Linux and Windows — the latter as two entries
  over one archive, each naming its own member. gallery-dl and Streamlink are managed names
  with no managed build: neither publishes a per-platform artefact this model can pin. The
  source table, the third-party FFmpeg origin and both gaps are in
  [external-tools.md](external-tools.md).

### Tool Compatibility Rules

Every helper binary is compared against the versions this release is tested against, and the
answer has four states rather than two.

- `supported`, `too_old`, `known_bad` and `unknown` are distinct. `unknown` — an unreadable
  version, or a tool no rule covers — warns and never blocks, because "we could not tell" is
  not evidence of a fault. FFmpeg git builds print a version with no number in it and are
  usually newer than every release.
- A rule names the capabilities it gates: `media_download`, `media_merge`, `audio_extraction`,
  `gallery_download` and `stream_recording`. Only those stop. Nothing here refuses work in
  general, and unrar, 7-Zip and rclone carry no rule at all.
- A stopped job fails with `media.tool_incompatible`, carrying the tool, the version, the
  minimum and the capability. The interface translates the capability name into all four
  languages.
- Rules ship inside the signed tool manifest, under the same signature, domain separator and
  replay floor as the builds they describe. Any failure to read a delivered set — bad
  signature, stale document, unknown tool, unparseable version, a rule that gates nothing —
  leaves the compiled-in base rules in force and records why.
- An override is an explicit list of tools in the settings. It keeps the verdict and the
  warning and drops only the block, and every skipped block is written to the log.
- The version is read by running the binary once and cached against its modification time and
  size, so the settings page no longer spawns a process per tool per load.

The matrix, the shipped floors and the platform differences are published in
`docs/external-tools.md`.

## Data Model and Persistence

### Main Persisted Objects

- **Downloads:** Packages, files, chunk checkpoints, remote validators, checksums, states, errors,
  retry times, account/proxy routes, and media selections.
- **LinkGrabber:** Intake batches, packages, candidates, ordering, online checks, duplicate state,
  category, priority, password state, media metadata, and the request metadata of intercepted
  browser downloads, which therefore survives a restart of the service.
- **Usenet:** Imports, NZB files, segments, yEnc/CRC data, output destinations, errors, and
  post-processing checkpoints.
- **Configuration:** Service settings, storage roots, categories, rules, hotfolders, accounts,
  proxy profiles, NNTP servers, and stream channels.
- **Security:** Password hash, token digests, and opaque vault references—no plaintext provider
  passwords in SQLite.
- **Live system:** Persistent domain events plus in-memory broadcasts for SSE clients.

### Storage Architecture

- SQLite in WAL mode with foreign keys enabled.
- A serialized writer actor prevents concurrent write conflicts.
- Small reader pool for parallel queries.
- Schema changes through versioned SQLx migrations.
- Domain changes and their corresponding events are written atomically.
- Controlled WAL checkpoint during clean shutdown.
- Byte values are transferred as decimal strings in the JSON API so JavaScript does not lose
  64-bit precision.

### Default Paths

| Content | Default |
| --- | --- |
| Database | `data/rdownloader.sqlite3` |
| Download root | `downloads/` |
| Installed plugins | `data/plugins/` |
| Encrypted secrets | `data/secrets/` |
| Password list | `passwords.txt` next to the database |
| Post-processing scripts | `scripts/` next to the database |
| Domain blocklist | `excluded_domains.txt` next to the database |
| Stored torrents | `torrents/` in the data directory |
| Torrent session | `torrent-session/` in the data directory |
| Docker configuration | `/config` |
| Docker downloads | `/downloads` |

All relevant paths can be customized through the CLI, environment variables, or app settings.

## Important Defaults and Limits

| Setting | Default / limit |
| --- | --- |
| Active files | 3 |
| HTTP chunks per file | 4 |
| HTTP connections per host | 6, configurable from 0 (unlimited) to 32 |
| NNTP connections per file | 0 (every connection the enabled servers allow), configurable up to 32 |
| Articles in flight per NNTP connection | 2 (fixed) |
| Retries per file | 8, configurable from 0–100 |
| Global download limit | Unlimited |
| SHA-256 generation | Enabled |
| Import history | Retained |
| Global post-processing level | `+Unpack` |
| Downloads during post-processing | Paused by default |
| Maximum archive entries | 20,000 |
| Maximum extracted archive size | 100 GiB |
| Sample threshold | 300 MiB |
| Script timeout | 3,600 seconds |
| Parallel media jobs | 2, configurable from 1–8 |
| Media probe timeout | 60 seconds, configurable from 5–600 |
| Parallel gallery jobs | 2, configurable from 1–8 |
| Stream check interval | 120 seconds, configurable from 60–3,600 |
| Parallel recordings | 2, configurable from 1–8 |
| Transfer services (torrent, Usenet, media, galleries, recordings, remote) | All enabled |
| Torrent data sharing | Disabled |
| Torrent seeding | Disabled |
| Torrent target ratio | 1.0 when seeding is enabled |
| Torrent peer port | Random |
| Torrent upload limit | Unlimited |
| Maximum NZB file | 64 MiB |
| Maximum `.torrent` file | 16 MiB |
| Administrator session | Ends after 12 hours without use or 30 days after sign-in, both configurable; persisted across restarts |
| Administrator password | At least 10 characters |
| Second factor (TOTP) | Disabled; ten single-use recovery codes at enrolment |
| Passkeys | None enrolled; independent of the second factor |
| Login rate limiting | Per address, with a global delay capped at 2 seconds |
| Trusted proxies | None; the peer address is the client |
| Managed tool versions | Disabled |
| Automatic removal of finished packages | Disabled |
| Mirror recognition | Enabled |
| Reconnect | Disabled |
| Keeping the machine awake | Disabled |
| Byte units | Binary (KiB/MiB/GiB) |
| Byte magnitude | Scaled automatically |
| State in the browser tab | Enabled |
| Schedule time zone | Europe/Berlin |
| Concurrent free downloads | 1 per hoster |
| Captcha answer timeout | 180 seconds, configurable from 15–600 |
| IP-block hold-off | 15 minutes when the hoster names no duration |
| Links per capture batch | 100 |
| Stored request headers | 32 per link, from a fixed allowlist |
| Captured header and text fields | 4,096 characters |

Waiting captchas and active IP blocks are held in memory only. Restarting the service therefore
drops any captcha still waiting for an answer; the downloads behind them return to the queue and
are resolved again, raising a fresh challenge. An IP block is likewise forgotten, so the first
free link of that hoster is tried once more after a restart and re-arms the block if the hoster
still refuses.

## Platforms and Distribution

| Platform | Artifact / notes |
| --- | --- |
| Windows x86-64 | Portable EXE files, static MSVC runtime, long-path manifest, tray, and file association |
| Linux x86-64 | Native GNU binaries, systemd user autostart, headless capture agent |
| macOS Intel | Native binaries, LaunchAgent, and menu-bar agent |
| macOS Apple Silicon | Native arm64 binaries, LaunchAgent, and menu-bar agent |
| Docker amd64/arm64 | Unprivileged Debian container with embedded web frontend |
| Browser extension | Chrome/Edge and Firefox from a shared Manifest V3 codebase |

The Docker container includes the server, not the desktop capture agent. The image ships FFmpeg,
7-Zip, PAR2, yt-dlp, streamlink and gallery-dl; unrar stays out on purpose (it is non-free, and
7-Zip reads RAR5), and rclone must be provided where it is wanted.

## Quality Assurance and Release Process

- Rust formatting check with `cargo fmt`.
- Clippy for the workspace, all targets, and all features, with warnings treated as errors.
- Rust test execution with `cargo nextest`.
- Frontend type checking, component/unit tests, and production build.
- SQLx offline metadata validation against the migration schema.
- Native CI on Linux, Windows, and macOS.
- Smoke tests for server binaries, capture agents, and portable launch scripts.
- WebAssembly builds of all bundled plugins (72 in 1.3) and CI checks for forbidden WASI imports.
- Packaging and signature smoke test for `.rdplug`.
- Docker build and health check of the amd64 image with persistent volumes; the multi-architecture
  image is built by the release workflow.
- Supply-chain validation with `cargo-deny`, and a `gitleaks` secret scan in CI.
- Every release is exported to the public repository `github.com/degoya/rDownloader` as one
  fresh commit without history, and GitHub's CI runs on the candidate before the tag (RD-130-23).
- Tag-based releases for Linux, Windows, macOS Intel, and macOS Apple Silicon.
- SPDX SBOM and SHA-256 checksums for release artifacts.
- Sigstore signature of the checksums.
- Container provenance, SBOM, and Cosign signature for the published multi-architecture image.

## Suggested Keywords

`download-manager`, `local-first`, `rust`, `vue`, `usenet`, `nzb`, `bittorrent`, `torrent`,
`linkgrabber`, `click-and-load`, `yt-dlp`, `gallery-dl`, `streamlink`, `webassembly`, `wasmtime`,
`mcp`, `docker`, `self-hosted`, `cross-platform`, `download-automation`, `passkeys`, `webauthn`,
`newznab`, `torznab`, `sabnzbd-compatible`, `reverse-proxy`
