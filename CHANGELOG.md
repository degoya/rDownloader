# Changelog

All notable changes to rDownloader are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), the versioning follows [SemVer](https://semver.org/).


## [Unreleased]

## [1.3.0] - 2026-09-25

### Added

- **Contributor files for the public repository (RD-130-23).** `CONTRIBUTING.md` explains the
  export model — a pull request is a proposal the maintainer applies by hand, credits and closes
  with a pointer to its release — plus `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), GitHub
  issue forms for bugs and feature requests, and a pull request template.
- **Duplicate a site rule.** Each row has *Duplicate*: the copy keeps the rule's body, gets a
  free id (`<id>-copy`, `<id>-copy-2`, …) and a copy name, is stored switched off and opens in
  the editor, leaving the original untouched.
- **`rdownloader site-rules verify <file>`** runs the import's check over a signed rule file.
  `release.yml` runs it before attaching the committed file to a release, and
  `scripts/package-linux.sh` writes the verified file to `artifacts/rdownloader-site-rules.json`,
  which the release pipeline's artifact step now requires.
- **`scripts/update-website.sh <version>`** brings rdownloader.net to a release — version, date and
  the links derived from them, tested, generated and committed in the site's repository — and runs
  in the release pipeline's `publish-public` step after the repository and wiki exports.

### Changed

- **The README is the public front page; developer notes moved to `docs/development.md`**
  (RD-130-23). The README now says what rDownloader is, which sources it supports, how to start
  it and on which platforms, with two screenshots under `docs/images/readme/`. Its long feature
  list and everything it carried about running from source, building, provider-account setup,
  autostart, the capture agent, `rdownloader://` links, the CLI, the automation-client adapters,
  the MCP server, metrics, Docker and the quality checks are in `docs/development.md`,
  unchanged.
- **Post-processing, BitTorrent and media settings stand in one column (RD-130-13).** The five
  cards behind these pages — galleries and streams included — split their fields into two
  columns that paired unrelated settings; they now put one setting per row like every other
  settings page. The two archive ceilings stay side by side, as values read together.
- **A card subscription's slider holds every hit (RD-130-13).** The LinkGrabber review drawer no
  longer puts a pagination bar under a card slider: the slider counts all open hits and reads
  them fifty at a time as the reader nears the end of what it has, keeping what it read across
  decisions and checks. Past ten pages the dots become a "Page 3 of 40" counter. The list view
  keeps its pagination.
- **A hit card is headed by its release name (RD-130-13).** The short name above it repeated the
  release name's head; it is gone, and the card is 2 rem shorter.
- **The cache check reaches magnets, NZB links and TorBox (RD-130-11).** A `remote-job` plugin
  can now say whether its provider holds a source ready, for links no resolver of its own
  reaches: a magnet, a stored `.torrent` (by its info hash), an indexer's NZB link, or a hoster
  link another plugin resolves. The LinkGrabber asks one account per such provider before the
  online check and shows a hit with the `cached` chip RD-120-36 built; its tooltip now names the
  provider that answered. TorBox asks `torrents`, `usenet` and `webdl/checkcached`, Premiumize's
  transfers plugin asks `cache/check` about magnets. A cache answer only raises a link — it never
  turns one offline, never changes who resolves it and never queues anything — and a hoster link
  without an account stays "not checked" while still showing that a provider has it.
- **Plugin contract `rdownloader:plugin@0.9.0`.** `interface remote-job` gains `cache-kinds` and
  `check-cached`; every bundled plugin was rebuilt and raised its `version`, and a package built
  for `0.8.0` is refused as `plugin.capability_unknown` and stays listed in the plugin manager.
  Migration `0094` adds `link_candidates.cached_by`.
- **The notification history can be cleared (RD-130-08).** A button at the history list itself,
  in *Settings → Notifications*, empties it after a confirmation that names how many deliveries
  go before it asks, like the three clears of RD-120-34. A delivery still queued or retrying
  stays: it is a notification not yet sent, not a record of one — the same rule the history's
  own per-rule trim follows. `POST /api/v1/notifications/deliveries/clear` carries
  `confirmed: true`, costs `api:admin` like the other clears and is the MCP tool
  `clear_notification_deliveries`; `GET /api/v1/system/data-reset` gained the `notifications`
  count. The clear is audited as `notifications_cleared` with the number removed.
- **How long a sign-in lasts is a setting.** Settings → Security sets an idle limit — hours
  without a request, default 12, 1 to 720 — and a maximum lifetime from sign-in, default 30 days
  and at most 90 (RD-130-09). Until now a session lasted a fixed twelve hours from sign-in, which
  ended it in the middle of an evening's work however busy it was. Both limits are checked on
  every request, so a shorter value ends the sessions already past it at once; a longer one
  applies from the next sign-in. The service refuses values outside the ranges, changing either
  needs the administration scope and is in the audit log, and the session cookie is kept for the
  maximum lifetime.
- **An expired sign-in says so.** A request refused for want of a session now returns the whole
  interface to the sign-in screen with a notice that the sign-in expired, instead of each view
  failing on its own with an error — or with nothing at all.
- **ntfy on your own server.** An ntfy destination may now be the full address of a self-hosted
  server's topic, not only a topic on `ntfy.sh` (RD-130-15). Each delivery reaches exactly the
  one host its destination names — a bare topic still means `ntfy.sh` — and its token goes
  nowhere else — a redirect off it is not followed. `https` is required; `http://` is accepted only inside
  your own network (private and link-local addresses, `localhost`, `.lan`, `.local`, and a
  single-label service name such as `http://ntfy:2586`), because the token would otherwise
  cross the internet readable. A destination that would be refused is refused already when
  the target is saved.
- **Hide hosters in the LinkGrabber** (RD-130-21), after JDownloader's quick filter. A row of
  chips above the list names every hoster with its number of links; a click hides it or shows it
  again, several at once, and a link's menu offers "Hide links of <hoster>". The choice is part
  of the standing mirror preference (`hidden_hosters` on `/api/v1/collector/mirror-preference`
  and the MCP tool `set_mirror_preference`), so it survives a reload and a restart. A line says
  how many links from how many hosters are hidden and shows them all again in one click. Hidden
  links stay in the LinkGrabber and are neither queued nor checked; a hidden hoster's mirror of a
  shown link is never the chosen mirror while a shown one exists, and goes along as its fallback.
- **A public repository, exported per release.** Development stays in the private repository;
  `github.com/degoya/rDownloader` receives every release as one fresh commit without history
  (RD-130-23). `scripts/export-public.sh` builds it from the release tag minus the paths
  `scripts/public-exclude.txt` names — the planning, the working card for coding agents, the
  audits — scans it with gitleaks before committing, and pushes only with `--push`; the release
  pipeline runs it as its last step, `publish-public`, and before the tag, as `public-ci`, it
  pushes the candidate to the branch `ci/<version>` there and tags only once GitHub's CI on
  Linux, Windows and macOS is green. CI runs the same gitleaks scan, with the
  known test-fixture findings allowlisted by path, or path and value, in `.gitleaks.toml`.
- **The user handbook as the GitHub wiki** (<https://github.com/degoya/rDownloader/wiki>):
  `scripts/export-wiki.sh` converts the handbook to GitHub-wiki form and commits it as one
  snapshot per release, refusing duplicate page names and dead links; the release pipeline's
  `publish-public` step runs it after the repository export.
- **`ROADMAP.md`**: the milestones 1.3 to 1.8 in short, for readers of the public repository.
- **`docs/mcp-coverage.md`**: the comparison of what the interface and the MCP toolbox can do,
  generated by `scripts/mcp-coverage.sh`, now a public page instead of a table inside a planning
  file.
- **The marketing package moved to the website repository**, its only home now; the copy under
  `docs/` was identical to it and is gone.
- The documentation index, `design.md` and `docs/compatibility.md` no longer link to planning
  files the public repository does not carry.

### Fixed

- **Web tests have 20 s before they count as hung.** Vitest's 5 s default failed a test that renders
  every settings page in four languages on a GitHub runner.
- **Three tests no longer measure the runner.** The Click'n'Load tests that expect a `jk` script
  to finish now allow it 10 s instead of the agent's 250 ms (Boa's first run overran that on a
  loaded runner; the expiry keeps its own tests), and the metrics test that a large queue adds
  no series leaves out the per-kind rate gauge, which appears at the scheduler's first sampling
  pass whenever that falls.
- **The macOS CI run passes the tests it can run.** Two more `rd-plugin-host` test files load
  real components through their `support` module (`notifier_wire`, `notifier_foreign_text`) and
  were missing from the `no-components` profile; a new test,
  `crates/rdownloader/tests/no_components_profile.rs`, fails for any such file the profile leaves
  in. Three `destination` tests compared a canonical path with the macOS temp dir's symlinked
  one (`/var` → `/private/var`).
- **CI costs fewer GitHub minutes and cannot hang for hours.** Every job has a timeout, a newer
  push cancels the run it replaces, nextest ends a test that hangs for five minutes, and macOS is
  linted with clippy rather than tested — its minutes cost ten times Linux's. The account's
  Actions minutes ran out on 2026-09-25, after a keychain hang had held the macOS tests for an hour.
- **About rDownloader** (RD-130-12): a settings page under Administration that names the running
  build — version, commit, build time, plugin contract, platform — the project's addresses, the
  author, and the licenses of everything the packages ship: rDownloader itself, the seven helper
  tools and every Rust crate and npm package, summarised by license and listed in full on demand.
  Commit and build time are the values `VERSION.txt` carries: `rd_build_stamp` exports them
  before the build and `crates/rdownloader/build.rs` compiles them in. The dependency list is
  generated by `scripts/licenses.sh` from `cargo metadata` and `package-lock.json`, and a test
  fails when either lockfile names a package the list does not, or an entry has no license.
  The website, the repository, its changelog, its wiki (the handbook) and its private
  vulnerability reporting are links; an address that is not public yet would say "not yet
  published" instead. Behind the sign-in (`api:read`); the public health check still names only
  the version. MCP reads it as `get_about`.
- **The helper tools' license texts travel with the repository** (`resources/vendor-licenses/`)
  and both packages install them under `vendor/licenses/`. Streamlink's was missing, and the
  Windows package carried none at all; it now gets the texts of its own binaries, 7-Zip's from
  the Windows distribution rather than the Linux one. Docker images name their commit and build
  time too: `scripts/docker.sh` and the release workflow pass them in as build arguments.
- **The Windows package ships 7z.dll.** Its 7z.exe loads every archive format from the DLL beside
  it and opened no archive without it. The DLL comes from the same official 7-Zip 26.02
  installer as the shipped 7z.exe, and `scripts/package-windows.sh` now refuses a vendor folder
  that has 7z.exe without 7z.dll (RD-130-12).
- **Apprise ships in the Docker image** (RD-130-14), pinned and installed by pip like yt-dlp,
  streamlink and gallery-dl, so an Apprise notification target works without installing
  anything. The Windows and Linux packages still leave it to you: there it is looked up in the
  vendor folder and on `PATH`, as before. *Settings → Tools* and `rdownloader doctor` now list
  it with its path and version, and the CI image smoke test runs `apprise --version`.
- **Apprise notifications are delivered.** The target URL was written to apprise's stdin and a
  `-` passed where apprise expects a URL; the real CLI reads stdin as the message body and took
  `-` for an unsupported URL, so every Apprise delivery and every test of such a target failed
  with "You must specify at least one server URL". Only a shell stand-in in the tests had ever
  seen the URL. It now reaches apprise in `APPRISE_URLS` in the child's environment — still
  never as an argument, which the process list would show to every user — and the stand-in
  asserts the real contract: the URL in that variable, not in argv, and nothing on stdin. What
  apprise prints about a URL it cannot parse names the URL, token included; that is now
  replaced before the failure is kept in the delivery history.
- **A mirror group reaches the queue whole.** Its chosen link starts and the other mirrors wait as
  its fallbacks (RD-110-20), but two LinkGrabber paths left the fallbacks behind: "Enqueue
  selected" split a partly selected package and moved only the chosen link — selecting a group
  selects that one — and the state filter, which acts per link, hid a mirror in another state so
  that 1.2.4's "only what is shown" did not send it. The queued download then had nothing to
  fall back to. A shown or selected link now takes its mirrors along; a hidden link of its own
  still stays behind.
- **"Edit package" selects the name (RD-130-13).** The name field was focused with the caret at
  the end, so a pasted name landed beside the old one; the whole name is now selected on open.
- **A link that turns out to be a torrent is grabbed once** (RD-130-18). Since RD-120-68 the
  online check reads such a file to name the package after the release, and the download then
  fetched it again from the same address — two grabs at an indexer that counts or limits them.
  The check now keeps what it read, and queueing the link stores it the way an uploaded
  `.torrent` is stored, so the row's source is that file. The copy expires after a day
  (`rd_torrent::PREFETCH_TTL`) and when its link leaves the LinkGrabber; a restart does not
  expire it. Without a copy the address stays the source and the download fetches it as before,
  with a log line saying so. A torrent served without its content type is recognised by its
  first bytes as before, and the check now reads that same response to its end instead of
  asking for the file a second time.
- **No site rule ships with the program any more** (RD-130-07). A fresh installation
  recognises no release page by rule. The project's eight rules are a signed file every release
  carries beside its packages, `rdownloader-site-rules.json`; *Settings → Site rules → Import*
  now takes it, verifies it against the site-rules root from the bytes as they arrived, and
  stores its rules as your own, switched off — so each can be edited, duplicated and removed. A
  file altered after signing or signed by another key is refused whole (`site_rules.bad_signature`,
  `site_rules.untrusted`, …). **An installation from before 1.3 keeps nothing of the rules it
  had compiled in**, the owner's decision: import the file once after the upgrade. The rule list
  loses its "Shipped"/"Own" badge and the "Checked" state that trusted a compiled-in rule's own
  date; `site_rules.shipped_immutable` and `site_rules.id_taken` are gone from the REST surface,
  and `origin` from `SiteRuleResponse`.
- **Five site-rule groups instead of seven.** `comics` and `magazines` are merged into `ebooks`,
  `graphics` stays its own; migration `0095` moves stored rules — column and body — and their
  group switches (off wins), and removes the switches of the former compiled-in rules. Every group
  now has a label in all four languages, so the list no longer mixes `Boards` with `ebooks`.
- **Your own scripts deliver links (RD-130-19).** A new subscription kind, *script*, runs a
  script from the scripts folder on a schedule set in rDownloader — an interval, or a cron
  expression in the service's local time such as `0 6 * * *` — without cron or the Windows Task
  Scheduler. Every line the script prints that is one link goes into the review list or straight
  into the queue, with the subscription's category, and a link already seen is not taken again.
  It runs in the post-processing script sandbox (no shell, the time limit, 64 KiB of output), a
  non-zero exit stands in the run history with its reason, and "check now" runs it at once. Only
  the administration scope creates, changes, switches or runs one; MCP, the area bundle and its import refuse
  the kind. Migration `0096` adds the schedule column; the cron parser is `croner` (MIT).
- **`rdownloader links add --category … --package … --enqueue`** (RD-130-19): a category by name
  or id, one package for all links, and the queue instead of the LinkGrabber — for scripts that
  run from an external scheduler after all.
- **A browser download only the browser could load reaches rDownloader after all (RD-130-16).**
  An indexer's cart, a one-time link, a download behind a session: since RD-120-63 NZB, torrent and
  ZIP downloads stay in the browser. For a site you allow in the extension's popup — revocable
  there and in its options — they now go to rDownloader: Firefox hands over the bytes it received
  (`webRequest.filterResponseData`, only in the Firefox build), and Chrome, or Firefox when its copy
  breaks off, hands over the address with the cookies the browser would send there. rDownloader
  fetches it **once**, sends the cookies only to that address's own scheme, host and port, and
  keeps them nowhere. The new route `POST /api/v1/capture/file` (capture scope) takes the bytes
  (upload or base64) or the address and understands an NZB, a `.torrent` and — new anywhere in
  rDownloader — a ZIP of NZBs as NNTmux's cart hands it out, one import per NZB, refused whole if
  one NZB is broken. Anything else is refused and stays in the browser. Without a consent nothing
  changes. **Known limit:** Firefox may drop the copy of a response it turns into a download
  (Mozilla bug 1787119); the address hand-over then takes its place.

### Security

- **A plugin's request no longer follows a redirect outside its domains.** A plugin may reach
  only the hosts its manifest names, or the one host a destination names — but that held for
  the first address of a request only. A server the plugin was allowed to reach could answer
  `307` and have the request, body included, sent on to any other host; the host refused it
  only afterwards (RD-130-24). Every redirect hop is now checked before it is followed, and a
  refused one is never requested. Credentials were not affected: `Authorization` and cookies
  were already dropped on a change of host.
- **A configuration token could switch the administrator login off over MCP.** The MCP tool
  `update_settings` costs `api:config`, like `PUT /api/v1/settings`, but applied the settings
  without the check that keeps that scope off the administrator-only fields. A token holding
  only `api:config` could therefore switch the administrator login off — which hands every
  caller every scope — or change the trusted proxies, the executables the service runs and the
  session limits. Over REST the same change was refused. Both paths now go through one
  function that refuses the change without `api:admin`, audits the refusal, and records a
  successful change in the audit log, which a settings change over MCP had not done at all.

## [1.2.4] - 2026-09-25

### Fixed

- **Only the service's own vault uses the OS keyring.** The vault's master key sits in one
  keyring entry per user account, and every vault opened with it — every test's throw-away
  vault included — read that entry, or created it. On the macOS CI runner the locked keychain
  made that call wait for ever, so the captcha tests hung; on a developer's machine a test run
  read, or minted, the master key of the real installation. `SecretStore::open` now keeps the
  key only in the file beside the vault; the service and the CLI open theirs with
  `open_with_os_keyring`, exactly as before.
- **The GitHub workflows build what they test.** The plugin jobs of `ci.yml` and `release.yml`
  build the packager, and with it `rd-api`, without the web interface; `rust-embed` refuses that,
  so the jobs now create an empty `web/dist`. The `rust` job's `no-components` profile left out
  three `rd-plugin-host` test files that load real components (`foreign_address_entries`,
  `foreign_address_wire`, `krakenfiles_parity`); they run in the `components` job instead. The
  Click'n'Load body-limit test sends only the declared length, because on Windows a client still
  writing an oversized body sees a reset instead of the 413.
- **The LinkGrabber adds only what its filters show.** With the hoster facet on one hoster,
  "Enqueue all" and a package's own "Enqueue" sent the links of every other hoster along, because
  the server was handed whole packages; the state, quality and language filters leaked the same
  way. The LinkGrabber now names the links on screen (`candidate_ids` on
  `POST /api/v1/collector/packages/enqueue`), and the hidden ones stay behind in their package.
- **A CI test run shows every failure, not the first.** The test steps run with
  `--no-fail-fast`, and each group of the `rust` job runs even after an earlier one failed;
  the step fails at its end. On GitHub one run costs 45 minutes.
- **CI runs once per release push.** `ci.yml` ran for the branch and again for the tag, which
  names the same commit; it now runs for branches only, the tag has `release.yml`.
- **The transfer contract test stops once bytes have arrived, not after 60 ms.** On a GitHub
  runner the component had not written its first byte by then, so the stop kept nothing and
  `a_stopped_transfer_resumes_from_disk_after_a_restart` failed.

## [1.2.3] - 2026-09-25

### Fixed

- **The release pipeline stops when signing refuses a plugin.** In 1.2.2 the sign step ran
  `build-plugins.sh` without checking its exit: two notifiers changed under their signed version,
  were refused, and the older packages of that version still sat in `dist/plugins/`, so the count
  matched and the step passed. 1.2.2 therefore ships `ntfy-notifier` and `telegram-notifier` 0.9.2
  built before the `rd-plugin-api` change — same behaviour, older build. Both are raised to 0.9.3.
- **A deleted torrent stays deleted (RD-120-68).** Only the single `DELETE` of a download took
  the torrent out of the librqbit session; the list's bulk removal, deleting a package,
  auto-remove, the SABnzbd and qBittorrent APIs and MCP did not, so the torrent stayed in the
  persisted session and the engine created its folder and files again on every start. Every
  removal now goes through one path that forgets the torrent in the engine.
- **Forgetting a torrent works after a restart (RD-120-68).** The engine's own list of what it
  runs is empty after a start, so a torrent that had not run since was never found. It is now
  found by the info hash stored with the download — the reviewed metadata, the magnet or the
  stored `.torrent` — and, before the engine has started, struck from its persisted session.
- **A torrent whose download is gone is not restored at start (RD-120-68).** Before the engine
  loads its persisted session, every torrent no queue entry claims is removed from it, so its
  files are never created. librqbit 9.0.1 offers no option for this and restores every entry,
  paused or not, so the list is cleaned before it is read. Files an earlier version already
  created stay where they are: removing never deletes what a download wrote.
- **A link that turns out to be a `.torrent` is named after the release (RD-120-68).** The
  check that re-routes such a link now reads the torrent and names the link, and so its
  package and folder, after the torrent's `info.name`; before, the download token the address
  ended in (`JKs2Jt3Fo=_l3XcwZVDZXkBOYSX+nhd+A==`) became the package. The file tree is stored
  for review as it is for an uploaded `.torrent`.
- **The branch check notices a plugin version that has to rise with a shared crate.**
  `scripts/check.sh` compared plugins with their signed package only when the change touched
  them under `plugins/`; a change to a crate every plugin links changes every component, and in
  1.2.2 a change to `rd-plugin-api` reached 72 of them unnoticed until the release pipeline. A
  change to `rd-core`, `rd-plugin-api` or what they pull in — read from the plugins' path
  dependencies, not listed — now asks about every plugin.
- **An application release no longer changes every plugin.** The plugin crates, and `rd-core`,
  `rd-plugin-api` and `rd-provider-registry` which they link, took the workspace version, and a
  crate's version goes into its component — into the metadata and into every mangled symbol. So
  the bump to 1.2.3 changed seventy components that had not changed a line, and signing refused
  them under their unchanged plugin versions. These crates now carry a fixed version of their own
  (`1.2.2`, the one the signed packages were built with, so they rebuild byte for byte);
  `crates/rdownloader/tests/plugin_crate_versions.rs` refuses one that inherits it again.

## [1.2.2] - 2026-09-25

### Fixed

- **The workspace lints clean on macOS and Windows (RD-120-67).** The first CI run on GitHub
  failed Clippy on both: the autostart display name was unused on macOS, where a launchd agent
  has no such field, and on Windows two test imports of the post-processing script runner and
  the log-capture helpers of a Linux-only RAR test were unused. Each now carries the platform
  gate of the code that uses it. `cargo xwin clippy` with `--all-targets --all-features`
  reproduces the Windows findings locally, and AGENTS.md now runs it that way.
- **The Put.io rate-limit tests no longer race the wall clock (RD-120-67).** They expected the
  wait from Put.io's `X-RateLimit-Reset` to be within five seconds of five minutes, measured
  against the system clock, and a slow runner that took six seconds to load the plugin got 294.
  The plugin host's clock can now be set by the host the plugin runs under (the system clock
  unless a test pins it), and both Put.io tests expect exactly 300 seconds.

### Changed

- **70 bundled plugins carry a new patch version, with no change of their own.** RD-120-67 gave
  `rd-plugin-api` a way for the host to pass its clock to a plugin; every plugin that links that
  crate builds a different component, and an installation only takes a newer version, so each
  raised its patch number (RD-120-47). Nothing a plugin does is different.

- **CI fits the test build on a hosted runner (RD-120-67).** The Linux test step ran every test
  binary of the workspace in one go — 55 `rd-api` integration binaries at about 550 MB each —
  and filled the runner's disk until the runner died writing its own log. The Rust and
  component jobs now free the runner's unused SDKs on Linux and macOS, the Linux leg links with
  mold, and the tests run in groups at two build jobs, deleting each group's executables once it
  passed (`rd-api`'s integration binaries eight at a time); a failed run logs disk and memory.
- **CI actions and runner images (RD-120-67).** `actions/checkout`, `setup-node`,
  `upload-artifact`, `download-artifact` and the Docker actions are on their current major
  versions, all on Node 24, in the CI, release and SDK plugin workflows. Runners are named
  (`ubuntu-24.04`, `windows-2025`, `macos-15`) instead of `-latest`, so an image change — which
  also moves the glibc floor of the Linux release binary — is a commit of its own.

## [1.2.1] - 2026-09-24

### Fixed

- **A crafted link, magnet or file could carry an account's credential to a stranger
  (RD-120-66, security).** The host filled a credential placeholder such as
  `{{secret:linksnappy_password}}` wherever a plugin's request carried one, including inside a
  value the plugin had copied from somewhere else. A link like
  `https://evil.example/f?x={{secret:linksnappy_password}}` handed to a LinkSnappy account went
  to LinkSnappy with the account password in it, and LinkSnappy then fetched that address from
  whoever made it. The same held for a TorBox remote job's web address or magnet (the TorBox
  API key, to that site or tracker), a `.torrent` that happened to be valid UTF-8 (the key in
  its tracker address), and the WebDAV destination, where a downloaded file named or filled with
  `{{secret}}` was stored with the WebDAV credential in its name or its content. Now an address
  with a placeholder at any layer of percent-encoding never reaches a plugin: resolving,
  checking, crawling, stream transforms, remote jobs and transfers refuse it with its own
  message (a link check reports it as unknown); only a body declared as a form or JSON is
  filled, never an upload or a `PUT`; and a placeholder in a plugin's address counts only where
  the plugin wrote it itself.
- **A notification could send the destination's token in its text (RD-120-65, security).**
  Since notification plugins were introduced, a package name, file name or feed title containing
  `{{secret}}` was sent with the destination's stored token in its place: the Telegram bot token
  in the message text in the chat, the ntfy access token in the title and body of the topic
  (which on `ntfy.sh` anyone who knows the topic name can read), the Discord webhook token in
  the embed posted to the channel. The host expanded vault markers in the text the plugin had
  copied from the notification as if the plugin had written them. Every `{` in a notification's
  text, and every `%7B`, is now shown as `❴` before a plugin sees it, so no text somebody else
  wrote can name a secret; a name with named or Basic markers no longer makes the delivery fail
  either. The destination address as entered is unchanged.
- **Telegram and ntfy notifications, Box shared links and KrakenFiles free downloads never
  worked (RD-120-60).** All four failed from the day they were introduced with "Resolver HTTP
  header is not allowed": each plugin set a request header the host does not send, and no test
  put these plugins through the host's header check. Telegram set its own `Content-Length`; it
  now sends no header and the host states the empty body's length itself (plugin 0.9.2). ntfy
  sent title, priority and tags as headers; they now travel as ntfy's `title`, `priority` and
  `tags` query parameters, which ntfy reads the same way, and a title keeps its non-ASCII
  characters (plugin 0.9.2). Box opens a shared link only through its `boxapi` header, and
  KrakenFiles answers its free-download form only with the page's `hash` header; the host now
  allows both, and it refuses to expand any vault credential into either of them, as it already
  did for `Referer`. A new test reads every header name every bundled plugin can set and fails
  on one the host would refuse.
- **A used-up traffic budget ends with its profile (RD-120-64).** When the bandwidth profile
  whose daily or monthly budget was used up stopped being active (its time window ended, or it
  was removed from the schedule), new transfers kept waiting until another profile took over,
  although without a profile no budget applies. Switching to another profile also kept the old
  profile's "used up" verdict until the next budget sample. The verdict now follows the active
  profile at once, and `budget_exhausted` is still sent only once per profile and period, even
  when the profile ends and returns the same day.
- **What only the browser can fetch stays in the browser (RD-120-63).** With **Take over
  browser downloads** on, the extension handed over only the address and erased the browser's
  copy; rDownloader then fetched it again without the browser's session. A Newznab indexer's cart
  (NNTmux `/getnzb/…`) answers that with an error, and a one-time link is already spent. NZB,
  torrent and ZIP downloads are no longer intercepted: recognised by `application/x-nzb`,
  `application/x-bittorrent`, `application/zip` or `application/x-zip-compressed`, or by a
  `.nzb`, `.torrent` or `.zip` ending on the file name, the address or an observed
  `Content-Disposition`, they are left to the browser without a word. A hotfolder on the
  browser's download folder takes `.nzb` and `.torrent` from there into rDownloader; a ZIP has to
  be unpacked into it first, since a watched folder does not take `.zip`. A `POST` the extension
  observed is now refused before the download is paused instead of after, so the browser's
  download is never touched. **Not fixed:** in a default installation the extension holds no host
  access for the hoster and cannot tell a `POST` from a `GET`, so such a download is still handed
  over as a `GET`; RD-130-16 plans the real fix, handing over the bytes the browser has already
  received. Everything else is handed over as before. The one rule is `staysInBrowser` in
  `extension/src/intercept.js`.

- **The sidebar's footer grows with the sidebar (RD-120-61).** Since the open sidebar has its
  240 px floor (RD-120-53), the separator above the footer and the language and theme selects
  stayed 165 px wide, ending short of the edge — 105 px short with the sidebar dragged to its
  maximum at 1440 px. On the 64 px rail the separator ran 9 px past the rail over the page. The
  footer block was a flex item sized to its content; it now takes the sidebar's full width and
  runs where the navigation's separator runs, open at every width the sidebar can take and on
  the rail, where the connection dot and the sign-out button now stack. Measured in Chromium at
  1280 and 1440 px, open (default, 14 %, 21 %) and collapsed, in all four languages.

- **Notifications keep what they say (RD-120-62).** A failure a notification plugin reports as
  permanent — a rejected Telegram token, a deleted Discord webhook — now ends the delivery as
  "Failed" with its reason on the first attempt instead of being retried six times over about
  30 minutes; temporary failures (429, 5xx, network) are still retried. Apprise is found in the
  tool folder set under Settings → Tools, like every other helper. And the `budget_exhausted`
  event is actually raised: the scheduler announces a traffic budget running out once, on the
  transition, and a rule that selects the event gets its delivery.

## [1.2.0] - 2026-09-24

### Added

- **Several `.torrent` or `.nzb` files go to a remote job at once (RD-120-51).** The form on
  the Remote jobs page takes any number of files, chosen together or dropped anywhere on the
  page, and makes each one its own job: they are handed over one after another through the same
  `POST /api/v1/accounts/{id}/remote-jobs` a single file used, so the 16 MiB ceiling, the type
  check and the duplicate lock — which answers "already running" before anything leaves the
  machine — apply to every file on its own. Each file has a row with its state in words; one the
  provider refuses keeps its reason on that row and the others are sent regardless. A file of
  the wrong type or over the ceiling is listed as not sent, with the reason, and never read.
  While the form is shown a drop on that page goes to it rather than to the LinkGrabber. No
  new route and no DTO change.

- **A hoster session in your browser can be handed over to the account (RD-120-45).** The
  service cannot read a browser's cookies, so **Take over from browser** at an account — shown
  where the provider's plugin declares a `cookie_scope`, DDownload's for one — opens a request
  that the browser extension answers. The popup names the site and the account; **Hand over this
  session** is the consent and the gesture for the browser's own prompt for `cookies` and that
  one origin, requested then and given back after the read (`cookies` stays an optional
  permission, so installing warns about no cookie access). The site is always the plugin's
  scope, carried by the request and re-read from the service before a cookie is touched; a page
  or the popup cannot name another. Only cookies of the scope's host or a domain above it leave
  the browser, only to the paired rDownloader over its capture token, and the service refuses
  the whole set if one row lies outside the scope. They are stored exactly as cookies typed into
  the account are — one vault entry behind `accounts.cookie_ref` — and the account is checked at
  once. A capture token can list, answer once or decline a request, never start one or pick the
  account; a request waits five minutes and lives in memory. New routes
  `POST|GET|DELETE /api/v1/accounts/{id}/browser-session` (secrets scope) and
  `GET /api/v1/capture/browser-sessions`, `POST /api/v1/capture/browser-sessions/{id}` and
  `…/{id}/decline` (capture scope); none is an MCP tool. `/api/v1/providers` carries
  `cookie_scope_host`.

- **The last thirteen capabilities are tools too, where the owner's line allows (RD-120-55).**
  RD-120-32 left thirteen capabilities unclassified; the owner answered on 2026-09-24 that they
  come in unless one meets a mark of his line. Each was checked against the four marks first —
  the verdicts are in the job file — and **the toolbox goes from 132 tools to 162**: the
  providers that take a remote job, the power state and stopping a countdown, a plugin's run
  history and its message catalogue, the automation runs, versions, vocabulary and dry run, the
  notification deliveries and destination catalogue, a subscription's review list, its polls,
  switching it and polling it now, recording schedules, their occurrences and recording now,
  the diagnostic bundle's preview, the metrics exposition, the reconnect status, an account's
  hosters, and the routing regex tester. Every tool calls its route's handler at its route's
  price — `get_metrics` costs `api:metrics`, `list_account_hosters` `api:secrets` — and one test
  refuses each of the thirty a step short of it.

  **Three parts stay out on the line itself.** Probing an indexer's capabilities is its key
  test (or takes a new key in); approving a diagnostic bundle is the person's approval that
  they saw what goes in, so the preview is a tool and writing and fetching the bundle are not;
  reconnecting changes the public address, outside this machine and for good. The health probe
  is public, so no tool can be priced by it, and what it says is already in the MCP handshake.
  The coverage table now reads 69 capabilities, 51 covered and 18 out, 12 of them on the
  owner's line. **Found on the way:** an indexer hit's download address carries the indexer's
  API key; `list_subscription_items` masks every address in its answer, and the canary test
  seeds one to prove it.

- **Everything the interface can do, an agent can do (RD-120-32).** RD-120-29 measured the
  toolbox against the interface and left forty capabilities out on a premise the owner never
  held — that a toolbox should do "the right thing" rather than everything. Re-sorted under the
  right one, **the toolbox goes from 78 tools to 132**: the LinkGrabber link by link
  (`list_candidates` first, then rename, move, reorder, enqueue one link, media variants,
  torrent and directory-listing plans), mirror groups and the standing mirror preference,
  LinkGrabber package editing and ordering, the NZB review (`list_nzb_imports`,
  `get_nzb_import`, `update_nzb_import`, `enqueue_nzb_import`, `delete_nzb_import` — an NZB
  handed in with `import_nzb` no longer waits for a person), queue order, renaming a download,
  a package or its folder, clearing finished packages, unpacking on demand, a torrent's files,
  trackers and seeding, the post-processing inventory and queue, the managed external tools,
  storage capacity, and writing a site rule. Every tool calls its route's handler; where one
  question has several routes — the six read-outs of a torrent's panel — the tool takes a
  `view`, and a test holds every further route it reaches to exactly the tool's price.

  **What stays out, stays out for a stated reason.** Nine capabilities are the owner's decision
  of 2026-09-23, with one line for all of them instead of nine arguments: a tool that hands out
  a secret, takes one in, gives a consent, or changes something outside this machine
  irreversibly is not offered — signing in and API tokens, provider OAuth, testing stored
  credentials, remote logins, captchas, consent to replay a paid link, whole-area import and
  export, plugin installation and trust, deleting a remote job at the provider. Four more are
  redundant. The coverage table now reads 65 capabilities, 38 covered and 27 out. Each new tool
  is refused without its route's permission and accepted with exactly it, in one test over all
  54, and the secret canary now searches every answer they give.

- **A container comes in as JSON too (RD-120-31).** The four import routes —
  `/api/v1/containers/import`, `/api/v1/dlc/import`, `/api/v1/torrents/import` and
  `POST /api/v1/nzb/imports` — take `application/json` beside `multipart/form-data`: the file as
  base64 in `content`, next to the upload's own fields. **It is the same route, not a second
  one**: one extractor reads either body into the same shape and the handler that was already
  there does the rest, so a JSON import answers, names its package and refuses exactly as the
  upload does — tests feed the same recorded `.dlc`, `.torrent` and `.nzb` through both bodies
  into fresh installations and require the same result — and it costs `api:intake` because it is
  the intake, through the same `scope_policy` entry.

  **The size limit is decided, not inherited.** The body limit is 65 MiB and base64 inflates by
  a third, so a JSON body carries a file of at most **48 MiB** — whose base64 is exactly 64 MiB,
  leaving one for the rest of the body. A larger file is refused before it is decoded under
  `container.too_large`, and a JSON body over the service-wide limit, which the limit layer used
  to answer with a bare `413` and English text, now carries `request.body_too_large`. Nothing is
  truncated. A larger NZB still arrives as an upload. Invalid base64 is
  `container.base64_invalid`; `container.file_invalid` and `container.format_unknown`, which the
  container route already emitted untranslated, are now translated in all four languages.

  **A container reaches a remote job.** `POST /api/v1/accounts/{id}/remote-jobs` takes a
  `container` field (base64, at most 16 MiB) beside `magnet` and `address`, and the form under
  **Remote jobs** a `.torrent` or `.nzb` file. Until now nothing constructed a container source
  at all: Premiumize, TorBox, Put.io, Seedr and Real-Debrid accepted one and never received it.
  Over MCP, `submit_remote_job` takes the same field, and `import_container`, `import_torrent`
  and `import_nzb` hand a file to the LinkGrabber at the price of their routes; the coverage
  table moves "Handing in a container file" from out to covered. An NZB handed in that way waits
  for review like an uploaded one, and queueing it has no tool yet.

- **A file in your own MEGA account downloads, and the rule that prevented it is replaced
  rather than removed (RD-120-30).** `https://mega.nz/fm/<handle>` lists a folder of the
  signed-in account in the LinkGrabber, `https://mega.nz/fm/file/<handle>` resolves one file,
  and both run the existing decrypt-while-writing path. A node key in an account is wrapped under
  the master key; the plugin hands the wrapped key to the host's key derivation and gets back
  that one node's key, never the master key.

  **The rule.** RD-120-20 made every derivation chain open with PBKDF2, which is right for a
  password and meaningless for a random key. The first step now follows the credential's
  origin, and the host decides the origin from where it reads the value: a typed credential
  (`accounts.secret_ref`) still opens with `pbkdf2-hmac-sha512`; the key material a sign-in
  stored (`auth_flows.key_ref`, migration `0091`, written only by the host's `store-token` path)
  opens with `aes-ecb-decrypt`, keyed by all sixteen bytes; a `take` first is refused for both,
  because it would read a stored key a byte at a time. A plugin cannot choose the origin: the
  handle is a name and the WIT is unchanged. What a hostile guest gains is decryptions under a
  key it may name, never the key — `docs/adr/0020-*`, addendum.

  **A sign-in may now keep a session beside the password.** It stores
  `{"token": …, "key": <16 bytes, base64url>}`; the host sends the token as
  `{{secret:<flow slot>}}` and keeps the key where only a derivation reads it. Any other JSON
  object is refused (`plugin.store_token_session_invalid`) rather than sent whole. MEGA's
  provider row carries `mega_password` and `mega_session`, and a `filled_by = "flow"` slot is
  allowed on `username_password` providers.

  Tested against an invented account built from the public example files
  (`crates/rd-plugin-ext/tests/mega_account_contract.rs`), **not against a real MEGA account**.
  New codes: `plugin.key_derivation_needs_key_step`, `plugin.store_token_session_invalid`,
  `mega.account_node_missing`, `mega.account_key_foreign`, `mega_crawler.not_in_account`,
  `mega_crawler.session_expired`.

- **Pixeldrain, and a provider whose download address does not expire (RD-120-07).** Two plugin
  packages: `plugins/pixeldrain/` resolves `/u/` and the API's own file addresses through the
  documented `GET /api/file/{id}/info`, and `plugins/pixeldrain-crawler/` unpacks a `/l/`
  list through `GET /api/list/{id}` into one candidate per file under the list's own title. Two
  rather than one because a manifest carries exactly one `plugin_type` and `resolve` answers with
  exactly one download. **The WIT contract was not touched.**

  **The address is the identifier.** What goes into the LinkGrabber is
  `pixeldrain.com/api/file/<id>?download` — no signature, no deadline, no session — so a job may
  wait an hour for its turn and still have a working address when it comes. Nothing re-resolves
  per attempt, which is the opposite of Put.io's problem and the shape worth copying when a
  provider allows it. The metadata is still re-read on every call: that is what turns a file
  deleted in the meantime into an honest refusal rather than a 404 mid-transfer.

  **There is deliberately no credential slot, and the reason is this project's contract rather
  than the provider's.** Pixeldrain authenticates its API key as an HTTP *Basic* password, which
  means base64-encoding the value together with an empty user name. A plugin never holds its own
  credential — it writes `{{secret:…}}` and the host substitutes it verbatim — and
  `rdownloader:plugin@0.7.0` offers no host primitive that computes over a credential it holds
  (`key-derivation` does PBKDF2, AES-ECB and `take`, no base64, and the `resolver-plugin` world
  does not import it at all). An `api_key` slot would be a field a person fills in that nothing
  can ever send, so `manifest.toml` carries `credentials = "none"` and records why. **The premium
  path Pixeldrain requires for hotlinking is therefore not reachable**, and
  `docs/roadmap/jobs/120-07-pixeldrain.md` records that as open rather than closing it quietly.
  *Closed before release by RD-120-38 (under Fixed): Pixeldrain now takes an optional API key.*

  **The limits are reported, never worked around.** The per-IP allowance, the transfer volume,
  the concurrency ceiling and the captcha state each carry their own stable code and a
  translation in German, English, Spanish and French; none is retried past and none is dodged by
  asking for another address. `GET /api/misc/rate_limits` is read before a download is handed to
  the queue, because the transfer engine fetches the bytes and a spent allowance would otherwise
  arrive as a 429 in the middle of a transfer. An answer that cannot be read is deliberately not
  a refusal: blocking a good file would be worse than the 429 it guards against.

  **`availability` is collapsed deliberately (RD-120-36).** `enum link-status` holds
  `online | offline | unknown` and Pixeldrain distinguishes more, so the collapse lives in one
  place: only a moderation block counts as offline, because a file behind a captcha or a spent
  allowance exists and is a wait rather than a dead link.

  **No run against the live service is claimed.** Every response shape comes from the feasibility
  measurement of 2026-09-22 and the provider's published API; the fixtures are sanitised and
  invented, and no test opens a socket.

- **Seedr, and the credential shape the plugin contract could not send (RD-120-04).** Two
  sibling plugins and a shared crate: `plugins/seedr/` resolves the per-file addresses and
  carries the `[provider]` row, `plugins/seedr-jobs/` runs magnets and `.torrent` files as
  transfers on the account, and `plugins/seedr-common/` holds the one address both of them have
  to agree on. Only `www.seedr.cc` is reached, and only its documented REST v1.

  **The host learned `{{basic:<reference>}}`.** Seedr's API is HTTP Basic and nothing else -- its
  own page says "Rest API v1 is only available to use with HTTP basic auth" -- and no marker
  could express that: `Basic {{username}}:{{secret:...}}` sends the pair unencoded, and a guest
  cannot encode them itself because `{{secret:...}}` substitutes on the way out and the guest
  never holds either half. The new marker expands to base64 of `username:secret` and nothing
  more -- no header name, no scheme word -- through the same finder the secret marker uses, so
  the credential-mode gate and the domain gate run for it unchanged, and the username gate runs
  as well. **The WIT contract was not touched**: `value_template` already carried it, so no
  component went stale.

  Three things are worth saying out loud. **A finished transfer stops being a transfer**: Seedr
  moves it out of the transfer list and turns it into a folder, so `GET /rest/transfer/{id}` can
  only answer "not here" -- which is also what a deleted transfer answers. The poll is the
  account's root listing instead, one request that shows both the running transfer and the
  folder it became. **The API is a premium feature** by Seedr's own documentation, so a plan
  that does not include it is refused under its own code rather than as an unremarkable 4xx.
  And **Seedr asks nobody which files they want**, so the job never answers `awaiting-choice`;
  every file of a finished transfer arrives in the LinkGrabber as one package, which is where
  the picking happens, and `choose` refuses under a stable code -- the same answer TorBox,
  Put.io, Offcloud and Premiumize reached independently (RD-120-35).

  What is deliberately **not** built: a Seedr download address carries no credential, because
  the transfer engine attaches an account's own only for OAuth providers. The bytes are fetched
  with an HTTP Basic authentication profile a person configures once for `www.seedr.cc`, which
  rDownloader already matches by address. `docs/roadmap/jobs/120-04-seedr-feasibility.md` records
  that, and the run against a real premium account, as open. *Closed before release by
  RD-120-38 (under Fixed): the engine attaches the account's own Basic pair, no profile needed.
  The run against a real account is still open.*

  One boundary of the marker is worth knowing for Pixeldrain (RD-120-07), which sends its API
  key as a Basic *password* under an empty user name: `{{basic:<reference>}}` as built refuses an
  empty user name, because for Seedr a request built from half a credential is a 401 that reads
  as an expired sign-in. Pixeldrain's shape would need that rule relaxed for providers with
  `username_required = false` -- a small change, deliberately not made here. *Made by RD-120-38.*

- **Each subscription chooses its view in the LinkGrabber: list or cards (RD-120-37).** The
  owner's request, with a reference picture (`docs/design/120-37-karten-vorlage.png`). An indexer
  subscription's pending hits appear either as the list that exists today — the default, so an
  existing subscription looks exactly as before (migration `0090`, `view` defaults to `'list'`) —
  or as a slider of equally tall cards: cover or a coloured initials tile, size with a lock for a
  protected archive, release group, name, episode and year, release name, chips (IMDb, language,
  resolution, codec, genre, grabs), and *Queue*, *Details*, *Dismiss*. *Details* opens one panel
  under the slider with the same details the list expands. Every value comes from one shared
  reading of the stored attributes (`web/src/utils/subscriptionHit.ts`), which the list row and
  its details now use too, and what an indexer did not send is absent rather than filled in.
  Queue and dismiss are one component in both views, so neither view can have an action the other
  lacks. The slider pages by arrows, arrow keys, swipe and operable page dots, keeps cards at
  least 15 rem wide (fewer per page in a narrow window), keeps one card height with or without
  cover, size or group, and honours "external images off". The initials tile's colour is derived
  from the name, so a series keeps its colour.

  **Autoplay is a second per-subscription option, off by default** (same migration). One fixed
  interval of six seconds, wrapping from the last page to the first, with a visible pause
  control; it holds while the pointer is over the slider, while anything in it has keyboard
  focus, while the details panel is open and while the tab is hidden, and it does not run at all
  under `prefers-reduced-motion: reduce`. Both settings travel over REST (`view`, `autoplay` on
  the subscription body), MCP (`create_subscription` / `update_subscription` definitions), the
  settings backup and the subscriptions export.

- **Each card slider has its own aspect ratio (RD-120-42).** The owner's request: a music
  subscription with square covers should show them square, a series subscription with banners
  wide. The subscription form offers **1:1, 2:3, 3:2, 16:9, 4:3 and 2:1** beside *View* and
  *Autoplay*, and only when the view is *Cards*. **2:1 is the default** — the closest of the five
  to the fixed picture height cards had until now — and every existing subscription gets it
  without anybody touching it, through a **new** migration `0092` (`card_ratio`, default
  `'2:1'`); `0090` is untouched, because it is applied on installations. The card's picture
  area takes the ratio as CSS `aspect-ratio`, the cover fills and crops it with `object-cover`,
  and the initials tile fills the same area; the text part keeps a fixed height, so all cards of
  one slider stay equally tall, and a narrow window keeps the ratio and shows fewer cards. The
  setting travels over REST (`card_ratio` on the subscription body), MCP, the settings backup and
  the subscriptions export like `view` and `autoplay`. **An unknown ratio is refused**, over REST
  and MCP alike, with `subscription.card_ratio_unknown` (HTTP 422, the value as a parameter)
  rather than drawn as the default.

- **TorBox, and the first remote job that carries all three kinds (RD-120-01).** Three sibling
  plugins, the shape RD-106-04 decided: `plugins/torbox/` resolves and carries the `[provider]`
  row, `plugins/torbox-jobs/` runs the jobs, `plugins/torbox-auth/` checks the pasted API key.
  Only `api.torbox.app` is used, and only its documented v1 API.

  **One state machine, three kinds of job.** TorBox runs torrents, Usenet downloads and web
  downloads behind three sets of endpoints that answer in one envelope and move through one set
  of states, and the contract already had room for all three: `job-source` carries `magnet`,
  `container` and `address`, and a container is a `.torrent` or an `.nzb` depending on its bytes.
  So the kind chooses the path and nothing else about the sequence changes — and **the WIT
  contract was not touched**, which is the difference between one branch's work and every
  component in the tree going stale.

  Three things are worth saying out loud. **The content key carries the kind**:
  `torrent:<sha1>`, `usenet:<md5>`, `web:<md5>`, all three derived locally and all three TorBox's
  own digests — the ones its `checkcached` endpoints take and its lists carry — which is the only
  reason a job created seconds before a crash can be recognised afterwards instead of created a
  second time. **The finished address carries no credential**: TorBox's `requestdl` wants the
  account's key as a query parameter and a plugin never holds one, so what a finished job hands
  back is the stable address that names the file, the resolver sibling adds the key, and the
  short-lived ticket behind it is minted again on every attempt — a download paused for an hour
  resumes rather than failing on a dead link. **TorBox asks nobody which files they want**, so
  the job never answers `awaiting-choice`; it fetches everything and the whole file list arrives
  in the LinkGrabber as one package, which is where the picking happens. `choose` refuses under
  its own code rather than confirming a selection nothing would act on.

  What is deliberately **not** built is the cached-item display. `enum link-status` can say
  online, offline or unknown and nothing else, and a magnet never reaches a resolver at all
  because dispatch runs on domains — so a cache badge needs contract work rather than TorBox
  work, and `docs/roadmap/jobs/120-01-torbox.md` names exactly what. Rather than show a cache
  status that could be stale, nothing shows one. A run against a real TorBox account is open too.

- **Put.io, the third provider whose job runs at the provider (RD-120-03).** Three sibling
  plugins and a shared crate: `plugins/putio/` resolves one file and carries the `[provider]`
  row, `plugins/putio-oauth/` signs the account in,
  `plugins/putio-transfers/` hands a magnet or a `.torrent` to the account and watches it run,
  and `plugins/putio-common/` holds what all three have to answer identically. Only
  `api.put.io/v2` is used — `transfers/*` for the job, `files/*` for what it produced,
  `account/info` for the account, `oauth2/*` for the sign-in.

  Three answers differ from the reference implementation, and each is a property of Put.io
  rather than of this code. **There is no selection at the provider.** Put.io fetches a torrent
  whole and its files exist only once it has finished, so `poll` never answers `awaiting-choice`:
  it answers `ready` with the complete tree — every file with its name, its size and the folder
  it sat in — and the choice is made in the LinkGrabber, which is still before anything is
  fetched to this machine. Expressing a selection by deleting the unwanted files at Put.io was
  considered and rejected; that is exactly the implicit remote deletion ADR 0003 forbids.
  **A `.torrent` is handed over as the magnet it is equivalent to**, because `transfers/add`
  takes one address and Put.io's only way to accept container bytes is a resumable upload
  session on a second host — several requests and a session a restart could lose, inside a world
  whose calls are all meant to be short. The container is read locally and re-expressed with the
  same info hash, the torrent's name and its trackers, so a magnet and the matching file are
  still one job. **What goes into the queue is the stable per-file address**,
  `api.put.io/v2/files/<id>/download`, and never the signed storage address Put.io would also
  hand out: that one expires, and a job that waited an hour for its turn would fail with a
  refusal nobody can act on. The account's token is attached by the host, because the `putio`
  provider row declares `api.put.io` among its secret domains — so the address in the queue
  carries no credential and needs no renewal.

  The sign-in is a redirect flow with no PKCE, which Put.io does not publish support for; what
  protects the callback is `state`, drawn from the host's random source and compared by the
  host. Put.io issues no refresh material and states no expiry, so nothing is renewed in the
  background — `refresh` is implemented as the ordinary exchange all the same, so the day Put.io
  changes its mind no edit is needed. No client id and no secret ship in this repository: Put.io
  counts its rate limits per application, so each installation registers its own, and the
  account holds both its client secret and the token, neither written over the other.
  Discarding a remote job cancels the transfer and deletes no files.
- **Offcloud, the third remote-job provider and the one that leaves the most out (RD-120-02).**
  Two sibling packages rather than three, because Offcloud has no sign-in flow to run: `plugins/offcloud/`
  resolves hoster links through `POST /api/instant` and carries the `[provider]` row with the API
  key the person copies from their account page, and `plugins/offcloud-cloud/` runs the cloud half
  as a persistent remote job — `POST /api/cloud` to start it, `POST /api/cloud/status` to follow
  it, `GET /api/cloud/explore/{requestId}` for the file tree it finished with. Only
  `offcloud.com` is reachable from either, neither is granted cookies or a captcha, and the key
  travels as `{{secret:offcloud_api_key}}` in an `Authorization` header rather than as the
  `?key=` query parameter the published API also offers, because a query parameter ends up in
  every redirect chain and every log line that quotes an address.

  What it leaves out is what made it useful beyond itself, because each omission showed a part
  of the contract that had been Real-Debrid's habit rather than the world's. **Offcloud has no
  file-selection step** — it fetches the whole of what it was given — so `poll` never answers
  `awaiting-choice` and `choose` refuses under a stable code instead of asking a question the
  provider will ignore. **Its sources have two content-key spaces**, `btih:` and `url:`, because
  its cloud takes an ordinary web address as readily as a magnet and an address carries no info
  hash; the prefixes exist because `remote_jobs(account_id, content_key)` is one unique index and
  two bare digests in it could collide. **Its folder structure costs a second call**, because the
  status call knows at most one address and no paths at all. Adoption after a crash compares
  content keys rather than addresses, so a magnet recorded by the provider in base32 is still
  recognised when it was pasted in hex.

  One defect found on the way is worth naming, because it is about serde rather than about
  Offcloud: a struct deserialises from a JSON *sequence* as readily as from a map, taking the
  elements in field order — so a bare array of addresses read into an error envelope becomes
  `{error: <first>, not_available: <second>}`, a refusal invented out of a perfectly good answer.
  A finished cloud download arrived as an empty package because of it. A body is now checked to
  be an object before a refusal is read out of it, and the file tree is branched on by shape
  rather than by whichever untagged variant parses first.

  **No run against a real Offcloud account is claimed.** Every shape comes from the provider's
  published API documentation and from two independent, maintained clients of it;
  `docs/roadmap/jobs/120-02-offcloud.md` records that gap and the three endpoints the published
  README does not list.
- **Logs, audit records and statistics can be emptied from the settings (RD-120-34).** Testing
  against the running service meant reading the interesting part out of what earlier runs had
  left behind: the retention limits (`log_retention_records`/`_days`,
  `audit_retention_records`/`_days`, `stats_retention_days`) govern the ageing, not a fresh
  start, and the only way to start from nothing was to swap the database out, which throws away
  far more than was meant. *Settings → System* now empties each of the three stores from the
  section that already sets how long it is kept.

  **Three actions and deliberately no fourth that does all of them.** Keeping the statistics
  while clearing the logs is the usual wish, and a single "reset everything" cannot say it.

  **The count comes before the question.** The dialog says how many records will go, and also
  what stays — the other two stores, the queue, the packages, the settings — because that is the
  fear a delete button actually raises. The answer reports the number that went and the interface
  shows it.

  **Emptying the audit log writes itself into the emptied log as its first entry**, in the same
  transaction as the delete, with the moment, the account that triggered it and the number of
  records removed. This is the condition on which the action is offered at all rather than a
  nicety: an audit log that is empty with nothing in it saying why has lost exactly the trace
  that explains where it starts. The append-only guarantees are otherwise unchanged — no route
  writes or edits a record, migration `0079`'s trigger still aborts any `UPDATE`, and no route
  can pick which records go.

  **The confirmation is a value, not only a dialog.** Every one of the three requests carries
  `confirmed: true` and is refused with `data_reset.not_confirmed` without it, so a client that
  never drew a dialog clears nothing — and so the same three can be offered over MCP, where an
  argument the caller had to set is the statement the dialog makes. `get_data_reset_preview`,
  `clear_log_records`, `clear_audit_records` and `clear_transfer_stats` join the toolbox;
  `POST /api/v1/diagnostics/logs/clear`, `/api/v1/audit/records/clear`,
  `/api/v1/stats/transfers/clear` and `GET /api/v1/system/data-reset` are the routes, all four
  at `api:admin` — reading the figures is a status page's business, throwing them away is not.
  Three new audit words: `logs_cleared`, `audit_cleared`, `stats_cleared`.
- **Premiumize takes jobs now, not only questions (RD-120-23).** `plugins/premiumize/` resolves a
  foreign link through the account and stores nothing in the cloud, which the provider's own
  documentation says out loud. The other half was missing: hand a source over and let it run.
  `plugins/premiumize-transfers/` is that half — a sibling package, because a manifest carries
  exactly one `plugin_type` — and it is the fifth `remote-job` provider, after Real-Debrid,
  TorBox, Put.io and Offcloud. `submit` is `POST /api/transfer/create`, the poll is `GET /api/transfer/list`,
  and ending one is `POST /api/transfer/delete`; what a finished transfer produced is read
  through `item/details` or `folder/list`. `transfer/clearfinished` is deliberately never
  called: it takes no argument and would delete transfers rDownloader never created.

  Like `plugins/torbox-jobs/` it claims **all three** shapes of `job-source`, and it is the
  first that needs the multipart half of a provider's submit to do it. A magnet and a plain
  `http(s)` link go out as a form body; a container goes out as the multipart upload `src`
  also accepts, under a file name sniffed from the bytes — `.torrent`, `.nzb`, `.rsdf` or
  `.dlc`. A `.ccf` has no marker of its own and is refused rather than uploaded under a guessed
  extension, which is named as an open remainder in the job file rather than papered over.

  Three decisions are worth reading. **The body is the answer, not the status line**: Premiumize
  reports every refusal with HTTP `200` and `{"status":"error","code":"…"}`, so the envelope
  decides and the status only speaks when there is no envelope to read. The documented `code`
  travels as `api_code`; the sentence beside it is classified and then dropped, so no provider
  text reaches a log or the interface — twenty stable codes in four languages instead.
  **`seeding` is ready**, neither a wait nor a failure: the files are there and will not change,
  and waiting for an end nobody controls would postpone a download that is finished. The
  transfer goes on seeding at the provider afterwards, undisturbed. **`adopt` answers nothing,
  on purpose**: `transfer/list` carries nothing derived from what was handed over, so a transfer
  this installation created cannot be told from a stranger's, and matching on a name would
  eventually poll somebody else's. The duplicate guard is then entirely migration `0065`'s row
  on `(account_id, content_key)`, which is what that row is for.

- **The remote-jobs form stops offering accounts that cannot take a job (RD-120-23).** Reported
  from use: a `ddownload` account could be picked, and the refusal — `remote_job.no_plugin` —
  arrived only after the button. The form asked `/api/v1/accounts` and handed every account
  through; nobody asked whether the service had a `remote-job` plugin at all. It does now, over
  the new `GET /api/v1/remote-jobs/providers`, whose answer is the key set of the runner table
  that fills itself from the `claims` of installed manifests — there is no list of providers in
  the code, and there cannot be one, because a provider exists exactly as long as its plugin
  does. With no fitting account the card says so under a stable code in four languages instead
  of showing an empty picker, and if the list cannot be read at all it says that rather than
  filtering on a guess. `remote_job.no_plugin` stays: a plugin can be removed between the form
  being drawn and the button being pressed, so the refusal is still right — it is now rare
  instead of routine. This landed here rather than in a job of its own because the filter is
  only demonstrable with a second provider: with one, a correct row and a wrong one look the
  same, and the test now covers two services with a plugin and one without.

- **Box, as the fourth cloud drive and the first that could not be a public OAuth client
  (RD-120-05).** Three sibling plugins and a shared address crate, the shape RD-106-04 decided:
  `plugins/box/` resolves one file and carries the `[provider]` row, `plugins/box-crawler/` lists
  a folder or a shared link, `plugins/box-oauth/` signs the account in, and `plugins/box-common/`
  holds what all three have to answer identically. Only `api.box.com/2.0` is used — file and
  folder metadata, `shared_items` for a link, `/items` for a listing — and the bytes come from
  `/files/<id>/content`, which Box answers with a redirect to its own storage host.

  Three things are worth saying out loud. **The download address names a version.** A resolver
  that answered `/content` alone would be answering "whatever is in that file now", and a
  transfer that outlived an edit would splice the head of one file onto the tail of another and
  still look finished; `?version=<file_version.id>` makes the answer one set of bytes, and the
  SHA-1 handed back beside it belongs to that same version. **The shared-link password lives in
  one header.** It reaches Box only as `shared_link_password` inside `boxapi`, never in an
  address the bytes come from, and the one address it does travel in — the hand-over from the
  crawler to the resolver, because nothing else passes between two sibling packages — carries it
  as a parameter the core now strikes out of every log line. **The account holds two credentials
  at once.** Box's token endpoint requires a `client_secret` on every grant and accepts no PKCE,
  so the person's own registration and the token the sign-in obtains get a slot each, the
  arrangement RD-106-03 built for Real-Debrid; neither is ever written over the other, and both
  travel as markers the host expands. No client id and no secret ship in this repository: Box
  counts its rate limits per application, so each installation registers its own.

  Large folders are paginated by offset, ten pages deep, and a folder that pages further says so
  in the log rather than ending quietly. A refusal behind a shared link is reported as a shared-
  link refusal — Box deliberately does not distinguish a wrong password from a missing
  permission, and the plugin does not invent the difference.
- **The administrator password can be changed (RD-120-22).** Until now it could not -- not on any
  screen, not from the CLI, not over REST. `setup()` wrote `auth.admin_password_hash` exactly once
  and refused every later call, so a password that had ended up in a note, a chat or a screenshot
  could only be removed by editing the setting out of the SQLite file by hand. Whoever could not
  do that, or did not dare, kept running the service with a password they believed to be
  compromised. The wiki work found it, while trying to write down how it was done.

  There is now a card on **Settings > Security** and a `POST /api/v1/auth/password` behind it, and
  three decisions carry the design. **A wrong current password is refused exactly as a wrong
  sign-in is** -- same status, same code, same body, and counted in `rd-authn`'s login limiter
  rather than in a second, weaker copy of it -- because an attacker who can tell "the old password
  was wrong" from anything else has learned something. **The policy check on the replacement runs
  before the current password is consulted**, so `auth.password_too_short` can never come back
  meaning "and your old one was right"; the other order is the natural one to write and is a
  password oracle. And **every session ends**, the caller's own included, with a fresh one handed
  back in the same response: a change that leaves the sessions opened with the old password alive
  protects against nothing, while one that also signs you out of the screen you made it on looks
  like a failure and invites changing it back.

  What it deliberately is not is a second way in. `setup()` still refuses its second call, and
  there is no recovery *without* the current password -- that has its own traps, and will get its
  own cut if it is wanted. A configured second factor is not demanded again here: it was already
  weighed when the session that reached the route was opened, and demanding a code as well would
  make a lost phone a lockout on the one screen that exists to undo a leak. The change touches
  neither the factor nor the recovery codes, and a test holds both. API tokens keep working,
  because they do not hang off the password; the card says so and points at the token list, since
  anyone who knew the old password could have minted some. The operation is in the audit log --
  `password_changed`, success and failure, naming who acted, from where, and how many sessions it
  cost -- and neither password is anywhere in it, which a canary asserts.

- **pCloud, and the two data centres nobody is asked about (RD-120-06).** Three plugins on the
  shape RD-106-04 decided, plus a shared crate: `pcloud` resolves one file and owns the provider
  row, `pcloud-crawler` lists a folder of the account's own drive or a public link, `pcloud-oauth`
  signs the account in, and `pcloud-common` carries what the three must answer identically. Only
  the official HTTP JSON API — `stat`, `checksumfile`, `listfolder`, `showpublink`, `userinfo`,
  `getfilelink`, `getpublinkdownload` — and no page scraped anywhere.

  **The part worth reading is the region.** pCloud runs two separate installations, and an
  account, a `fileid` and a public link code exist in exactly one of them; the other answers
  `result: 2094` to a token and the 7xxx family to a link code, both under HTTP 200. Taken at
  face value that reads as a bad password or a dead link, which is how this goes wrong in a way
  nobody can diagnose. So nothing is guessed twice: a pCloud address names its installation in
  its host, **only those two refusals** are retried and only **once**, at the other one — a
  missing file, a denied operation and a rate limit are settled where they were asked — and the
  installation that answered is then pinned for the rest of the invocation, written into every
  address the crawler hands the resolver, and shown on the account row. There is no region
  setting, because there is nothing for a person to know.

  **The expiring download link is answered by never keeping one.** `getfilelink` hands back
  content servers, a path and an `expires`, and pCloud offers no stable byte endpoint at all. The
  durable address of a download is therefore the canonical `fileid` or link code the queue
  already holds; every attempt re-resolves from it and mints a fresh ticket, and the size and
  checksum read with it say it is the same bytes. The account's token never reaches the content
  host — the ticket is already authorised — so pCloud's content servers are named by
  `*.pcloud.com` in `download_domains` and are deliberately absent from `secret_domains`.

  Public links in every spelling pCloud uses, folders and files of the account's own drive, and
  files inside either; a public link holding one file arrives as that one file. Checksums follow
  the installation, as pCloud documents: SHA-256 in Europe, MD5 in the United States, SHA-1 in
  both. Refusals carry pCloud's decimal `result` and never its `error`, which is an English
  sentence. The sign-in is a confidential client with the person's own application key and
  secret — pCloud offers no PKCE — and issues no refresh material, so `refresh` is an honest
  refusal that the host's renewal sweep never reaches. A password-protected public link is
  refused with a code rather than opened: pCloud's API documents no parameter for supplying one,
  and none was invented.

- **What the interface can do, the toolbox can do — or it is written down why not (RD-120-29).**
  The question was whether newer functions had been left out of MCP, and the honest answer needed
  measuring rather than guessing. `grep -rn remote_job crates/rd-api/src/mcp/` found nothing:
  remote jobs were a whole view with its own state vocabulary, a selection step and two ways to
  end a job, and over MCP they did not exist — which is also why RD-120-23 could not hand a
  container in, the path was not there at all. The surfaces that arrived with 1.1 were checked
  rather than assumed, and four of the five were uncovered too. **The toolbox goes from 61 tools
  to 71**: `list_remote_jobs`, `submit_remote_job`, `choose_remote_job_entries` and
  `forget_remote_job`; `get_transfer_stats`; `list_log_records` and `list_audit_records` with the
  filters the views offer; `list_site_rules`, `set_site_rule_enabled` and
  `set_site_rule_group_enabled`.

  The second half is the part that keeps the answer true. Mirroring every REST route into a tool
  is deliberately not the goal — a toolbox that can do everything is harder for a model to use
  well than one that can do the right thing — so the measurement is a table of **capabilities**
  with a **decision** each: 62 capabilities, 22 covered, 40 out on purpose with the reason
  written beside them. Deleting a remote job *at the provider* stays out, because the route
  refuses anything without an explicit confirmation and an assistant supplying that confirmation
  is not one; `forget_remote_job`, which only touches this installation's own list, is in.
  Handing in a `.torrent`, `.nzb` or `.dlc` stays out for a measured reason rather than a
  judgement: all four routes take `multipart/form-data` and a tool call carries JSON and no file.
  The table lives in `crates/rd-api/src/mcp/coverage.rs` and the build enforces it — an operation
  in the OpenAPI document that belongs to no capability fails the test suite, so a REST route
  cannot arrive undecided and the coverage answer cannot quietly go stale. The comparison in the
  job file is generated from it by `scripts/mcp-coverage.sh` and compared by a test, the same
  arrangement `rd_core::failpoint` has with `docs/recovery-matrix.md`. Every new tool costs the
  scope its REST route costs, read from `scope_policy` rather than written down again, and no
  tool returns a secret: a canary walks every read tool's answer looking for a stored credential.


### Fixed

- **An indexer's API key could reach a model through `list_collector` and `list_candidates`
  (RD-120-57, P1).** An indexer hit's download address carries the indexer's `apikey`; RD-120-55
  masked it in `list_subscription_items`, but a hit taken into the LinkGrabber became a candidate
  with the address in clear, and both tools handed it to whatever model called them. Every MCP
  tool answer — result, tool error and refusal — now passes one mask on its way out
  (`crates/rd-api/src/mcp/mask.rs`): every string that is or contains an address loses its
  credential values through `rd_core::redact_url`, names kept, so no present or future tool can
  forget it. Ids and anything with nothing to hide come out byte for byte. The shared list of
  secret query parameters also learned `passkey`, `authkey`, `torrent_pass`, `rsstoken`,
  `auth_key`, `access_key`, `secret_key`, `client_secret`, `private_token`, `api-key` and
  `x-api-key`, which masks them in logs and the diagnostic bundle too. REST answers to the web
  interface are unchanged. Still open: the classic Newznab link form
  `…/getnzb/<guid>.nzb&i=…&r=<key>` carries the key as `r` in the path and is not masked yet.

- **RAR archives unpack under Windows into a folder with a space again (RD-120-56, P0).** A
  regression since RD-108-30 (2026-09-18): every RAR extraction with `unrar` under Windows into a
  path containing a space or tab failed with `extract.cannot_write`, single- and multi-volume,
  while the integrity test before it passed. The destination went to `unrar` as a positional
  argument with a trailing separator; Rust's quoting doubled the backslash before the closing
  quote, `unrar` reads its command line with its own parser that keeps every backslash, and the
  `\\?\` path then named `…\.rd-xabc\\file`, which Windows refuses. The destination now travels as
  `-op<staging>` in front of `--`, so no argument ends in a separator, and under Windows every
  `unrar` argument is written in the form `unrar`'s parser reads back unchanged — which also lets
  a password containing `"` arrive as typed; it used to gain a backslash. `unrar` older than 6.10
  does not know `-op` and is refused with the new code `extract.tool_too_old` (the bundled one is
  7.23 on both platforms). Each run of `unrar`/`7z` is now logged with the tool, its arguments —
  the password shown as `-p***` — and the exit code. Linux and the 7z path were not affected.

- **A remote job no longer returns to "waiting for a choice" after it was answered
  (RD-120-35).** If a job asks for a selection again after `choose`, the host now ends it under
  `remote_job.choice_not_kept` (with a text in all four languages) instead of storing the
  question again. That old behaviour moved the row back and forth between `awaiting_choice`
  and `working`. **The plugin contract is unchanged, and that was decided by checking the
  providers.** Of the six bundled remote-job plugins, only Real-Debrid can select files on
  the provider's side (`torrents/selectFiles`). It also keeps the answer: its status leaves
  `waiting_files_selection` once the selection is made, so the poll after `choose` reads the
  answer from the provider, and the guest has nothing to remember. TorBox, Premiumize, Put.io,
  Seedr and Offcloud cannot select files at all and never ask. `docs/plugins.md` and
  `sdk/README.md` now state the rule: only a provider that keeps the answer may use
  `awaiting-choice`.

- **Settings pages, the sidebar and the queue no longer cut or hide what they show
  (RD-120-53).** Found in the second screenshot run, measured in Chromium at 1280 and 1440 px
  in all four languages:
  - On a settings page the sidebar's settings group is now open and highlighted — it was
    closed and plain on all 24 pages, because the menu read its open state once, before the
    first route had resolved, and the pages are not child routes of `/settings`. The navbar
    names the page (*Bandwidth*, *Tools*, …) instead of *Settings* on every one.
  - The open sidebar keeps at least 240 px: at 15 % of a 1280 px window it was 192 px and cut
    the application's name, "Téléchargements" and "Entfernte Aufträge". The tagline under the
    name, which needed up to 223 px of 117, is gone (owner's decision); language and theme sit
    one per row instead of cut to "En…" and "Da…"; long settings labels wrap.
  - The size column of a queue package is 144 px, so `585 MiB / 10.5 GiB` is no longer
    `10.5 G…`. The transfer rail shows the application's name only where there is room, so
    the volume line stays whole at 1280 px.
  - A bandwidth profile's line names its monthly and daily budget, parallel-file cap and
    upload limit. A profile with only a 1 TiB monthly budget read "Unlimited · 0 scope limits".

- **An imported cookie no longer reaches hosts outside its scope (RD-120-49). Before this fix
  a cookie could be sent to hosts its profile or account never named — in the worst case to
  every site under a whole top-level domain.** Three holes, one rule now closes all of them:
  - A Netscape row for a domain above the scope was accepted right up to the top: a row for
    `com` in a DDownload account, or `co.uk` in a profile for `example.co.uk`, went into the jar
    and from there to every `*.com` or `*.co.uk` host a download touched. Such a row is now
    refused against Mozilla's Public Suffix List (compiled in through the `psl` crate, no
    runtime fetch; an unknown single label such as `lan` counts as a suffix too), with the new
    codes `authprofile.cookie_public_suffix` and `browser_session.cookie_public_suffix`, and
    nothing is stored.
  - A cookie pasted as a `Cookie:` header always went to every subdomain, even when the profile
    said `include_subdomains = false`. It now reaches the scope's host only, unless the profile
    includes subdomains.
  - A row for a parent domain widened the profile: `.example.com` in a profile for
    `www.example.com` went to every `*.example.com`. It is still accepted — the browser hands a
    page its parent domain's cookies — but stored for the scope's host.
  Profile import, account cookies, the browser handover and the resolver's jar all ask the same
  `rd_http::CookieScope` now; the browser handover's second copy of the rule is gone. The
  yt-dlp cookie file for media downloads followed separately (RD-120-52, below).

- **The cookie file handed to yt-dlp keeps to the profile's scope as well (RD-120-52). Before
  this fix it still had all three holes RD-120-49 closed, for every host one yt-dlp run
  touched.** The file for `yt-dlp --cookies` is now written through the same
  `rd_http::CookieScope` rule, not a copy of it: a row for a public suffix such as `.com` or
  `.co.uk` is left out of the file; a row for a parent domain is written for the profile's host
  rather than its own domain, so `.example.com` in a profile for `www.example.com` no longer
  reaches `dl.example.com`; and every row, a pasted `Cookie:` header included, reaches
  subdomains only when the profile includes them. Rows the rule refuses are dropped from the
  file, as foreign rows of a browser export always were, rather than failing the download; a
  profile left with nothing for the page reports `media.cookie_scope_empty` as before. A row
  for a subdomain *below* the profile's host is now dropped too, which the HTTP engine already
  refuses.

- **An auth profile checks its cookies against its scope when it is saved (RD-120-54).**
  Creating or editing a cookie profile only checked the size of the pasted cookies, so a row for
  a foreign domain or a public suffix was stored and refused only at the first download (the
  yt-dlp path silently left it out). `POST` and `PUT /api/v1/auth-profiles` now run the same
  `rd_http::CookieScope` import the download runs, before anything is stored, and refuse with
  `authprofile.cookie_public_suffix` or the new `authprofile.cookie_outside_scope` (with the
  scope's `host`); a row for a parent domain stays allowed. An edit that moves the profile to
  another host or changes its subdomain setting re-checks the cookies it keeps.
  `POST /api/v1/capture/cookies` reports a foreign row with the new code too, instead of
  `authprofile.credentials_invalid`. The settings card now shows a refused save beneath the
  form, with the input still there, rather than at the foot of the page.

- **Media pages whose formats yt-dlp reports without codecs download again (RD-120-50). A
  regression since 0.8.0** (`a2beadf6`, the switch to format criteria): dumpert.nl, which
  reports every format with both codecs `null`, downloaded with 0.7.0 and was queued without a
  selection from 0.8.0 on, failing with "Media download has no variant selection"; arte.tv lost
  its audio tracks the same way. yt-dlp says `"none"` for a missing track and leaves the field
  `null` when nobody knows, and the format list read both as missing. Now only `"none"` means
  missing, a format with both codecs unknown counts as one that carries both, and pictures and
  subtitle tracks listed among the formats are dropped. A page on which no quality resolves
  offers **Best** again, handing the choice to yt-dlp as before 0.8.0 — so playlist entries,
  which carry no format list, have a selection again too; without ffmpeg a page known to serve
  only separate video and audio streams still offers nothing. A media link that still has no
  selection is refused when it is queued (`collector.media_selection_missing`) instead of
  becoming a download that can only fail, and the format selector names the page's actual
  problem — no formats, or no audio track (`media.audio_missing`, previously reported as a
  missing ffmpeg) — instead of a filter combination nobody set. `GET
  /api/v1/collector/candidates/{id}/media` carries the reason as `unresolved_code`.

- **The Remote jobs page no longer opens on an empty account picker (RD-120-51).**
  `GET /api/v1/remote-jobs/providers` built the remote-job runners to answer, and building them
  re-verified *and compiled* every installed plugin component — on the first open after a start
  the page showed an empty selection for as long as that took. It now answers from the
  installed manifests, signature-checked, revocation-checked and with switched-off plugins left
  out exactly as a load does, without compiling anything; the answer is the same set (a test
  compares both over the six bundled remote-job plugins), and a package whose component would
  not compile is now offered and refused on submit as `remote_job.no_plugin`. Until the list of
  services and the accounts have both arrived, the form says what it is waiting for instead of
  drawing a picker.

- **An OAuth renewal with a registered application no longer sends one credential in place of
  the other (RD-120-39). Renewing Real-Debrid, Put.io and Box with an application the person
  registered was broken before this fix.** Such a renewal names two vault references in one
  request — the application's client secret and the refresh material of the sign-in — and the
  host loaded only the first reference it found and wrote that value into every marker.
  Real-Debrid and Put.io therefore received the client secret as the refresh code, Box the
  refresh token as the client secret, and each provider refused the renewal. Every distinct
  `{{secret:…}}`/`{{basic:…}}` reference is now loaded on its own, through the same gate a
  single one passes (active slot, renewal reference, address), and each marker gets its own
  reference's value; one reference failing its gate refuses the whole request before anything
  is sent. A request may name at most four distinct references
  (`plugin.secret_references_exceeded`). No plugin and no WIT contract changed.

- **What the coordinator's screenshot run of 2026-09-23 found (RD-120-48).**
  - **Saving the settings no longer breaks the next LinkGrabber intake.** The settings document
    stores an unset `excluded_domains_file` as `null`, and the blocklist reader took that for a
    malformed string and logged an ERROR on every intake after an ordinary save. A `null` field
    now reads as "not configured" for every typed field reader; a value of the wrong type is
    still reported. A test saves through `PUT /api/v1/settings` and then runs an intake.
  - **No icon is fetched from the internet any more.** Only Nuxt UI's own 43 icons were
    bundled; the 165 the interface names itself were loaded from `api.iconify.design` at run
    time and were missing offline. Every icon is now bundled at build time, and the icon
    renderer is the offline build of `@iconify/vue`, which has no API client to fall back to.
    `iconBundle.test.ts` fails when a named icon is not in an installed collection or escapes
    the build's scan.
  - **The transfer rail no longer overlaps itself** at 1440 px with the sidebar open: its
    breakpoints were the window's, not the panel's. The controls keep their width, the volume
    line truncates rather than running under the speed-limit field, and the "Made with ♡"
    signature appears only once the rail is wide enough. Measured at 1280, 1440 and 1920 px in
    all four languages.
  - **The LinkGrabber toolbar** keeps "Select all" on one line; its filters flow onto a second
    row where the panel is too narrow, instead of scrolling the count out of view.
  - **Statistics** places its bars on the range's time axis: one busy hour was drawn across the
    whole chart with the same moment at both ends. A turnaround under a second reads "under
    1 s" instead of an empty tile.
  - **The setup wizard** shows no "Back" on its first step, and an **empty list** — the
    wizard's storage roots and nine others — gets the padding its caller gave it: `DataState`
    rendered its empty state as a fragment, which inherits no classes.

- **A sign-in no longer waits in silence when the browser is signed in already (RD-120-45).**
  DDownload's sign-in meets a Turnstile on the login page it fetches as a guest; the extension
  opened that page in a browser holding a session, the site redirected straight past the form,
  and nothing could ever be answered — the account test sat until the captcha timed out. The
  reader now reports a page on which no widget, widget container or widget frame appears within
  15 seconds (`POST /api/v1/capture/captchas/{id}/no-widget`), and the wait ends at once with
  `captcha.page_without_widget`: the hoster named, and the two ways out — take the session over,
  or sign out in the browser and test again — in all four languages. The account test passes that
  code through instead of quoting it inside `account.check_failed`. The hoster tab stays open and
  is forgotten, so closing it declines nothing.

- **An applied migration can no longer be changed unnoticed (RD-120-41).** After a one-line
  comment edit in `0065` stopped the owner's installation at startup, the rule is a test instead
  of prose: `crates/rd-db/migrations.sha384` pins the checksum of all 89 migrations — the SHA-384
  sqlx stores in `_sqlx_migrations`, compared against a real table in the test and against the
  89 rows of the owner's live database — and `rd-db`'s `migration_checksums` test fails when a
  file changes, when a pinned file disappears, or when a new migration has no pin, naming the
  command that adds it: `scripts/migration-pin.sh <file>`. `0059` and `0069`, edited once long
  ago, are pinned as installations applied them, `0065` at its restored original bytes.

- **An authentication profile's header no longer follows a redirect to a foreign host
  (RD-120-43). This was a credential leak.** A `Basic` or `Bearer` profile scoped to `a.example`
  sent its `Authorization` header to `b.example` whenever `a.example` redirected there: the probe
  followed the redirect and reqwest stripped the header at the change of host, but the chunks
  were then fetched from where the redirect ended, directly, with the header put back. The
  profile's scope had only ever been checked against the link as it was added. It is now decided
  again for every address a request goes to — the probe's (a resolver may answer with another
  host) and the transfer's — by the profile's host scope; a redirect within the scope keeps the
  header, and a header chosen for an HTTPS address is never sent over plain HTTP. The same hole
  RD-120-38 closed for the account's own credential. A test runs a real TLS redirect from the
  covered host to a foreign one and fails without the fix. Cookie profiles were not affected:
  their cookies sit in the client's jar bound to their domain, which decides per request; a test
  now pins that too.

- **A DDownload account whose API key works passes its account test again (RD-120-44).** With a
  key and an imported cookie session, the test failed with "DDownload did not confirm the cookie
  session either way", although downloads worked. The session check introduced by RD-120-13
  asked the homepage, which in DDownload's current design shows neither the sign-out link nor
  the guest header. It then failed the whole test, even though the key had already proved the
  account. It now asks the account page, `/?op=my_account`, and reads it the way sign-in
  accounts already did. An expired session is still reported (`download_session_expired`),
  recognised by the redirect to `/login.html` measured on 2026-09-20. A page that says neither
  no longer fails the test: the account is valid with premium status and traffic from the API,
  and the label says "cookie session not confirmed". What the account page looks like while
  signed in has still not been measured, and nothing has run against the live site yet.

- **The same homepage check in three more places, and an unrecognised page now leaves a trace
  (RD-120-46).** DDownload's cookie-only accounts — no key, no sign-in — still asked the
  homepage and so lost their test every time; they now ask the account page as well. A guest
  page there is still an invalid session; a page that shows neither marker is reported as "did
  not confirm the cookie session", retryable, because in this branch nothing else proves the
  account. KatFile with an API key no longer fails its test over a page that settles nothing:
  the account is valid with the API's figures and the label says "cookie session not
  confirmed". KatFile and FileJoker keep asking the homepage — for neither has any page been
  measured, so an account page would only swap one unmeasured page for another. In all three,
  an unrecognised page is now logged as a warning with its title, length and which of a fixed
  list of markers it carries, **never its content**, which can hold the address and the API
  key; a canary test holds that line to it. Plugin versions: DDownload 0.10.8, KatFile 0.9.3,
  FileJoker 0.7.4.

- **An installation that had applied migration `0065` refused to start.** RD-120-20 edited one
  comment line in that file in place, and sqlx checksums every byte of a migration, comments
  included — so any database that had run `0065` stopped at startup with "migration 65 was
  previously applied but has been modified". The file is back to the bytes installations
  applied; nothing is lost, since the column it describes carries no constraint the comment
  mattered to. Checked against a live database: all 85 applied migrations match their files.

- **A Basic account downloads without a second profile, and no account credential follows a
  redirect to a foreign host any more (RD-120-38).** Seedr's jobs finished, their files reached
  the LinkGrabber, and the download itself then failed with 401: the engine attached an
  account's own credential to a transfer only for OAuth providers, and Seedr's file addresses
  answer HTTP Basic and nothing else. The only way round was an HTTP Basic authentication
  profile for `www.seedr.cc` holding the password a second time. A provider row may now declare
  `transfer_auth = "basic"`, and the engine sends `Authorization: Basic` built from the
  account's user name and secret — by the same host code as `{{basic:…}}` — over TLS and only
  to that credential's `secret_domains`. Seedr declares it; the profile is no longer needed.

  **Pixeldrain takes an optional API key.** Its key is the Basic *password* under an empty user
  name, which `{{basic:…}}` refused. An empty name is now allowed for exactly the rows with
  `username_required = false`, and for no other: Seedr, which requires one, still refuses an
  account without its e-mail address rather than sending half a credential. Pixeldrain is an
  `api_key` provider again, the key rides on its API calls and on the transfer, `/api/user`
  checks the account, and a key it refuses (`authentication_failed`, measured) has its own
  code. Without an account Pixeldrain runs exactly as before.

  **The redirect leak this uncovered was older than Basic.** The engine probes the source, and
  reqwest drops `Authorization` at the change of host on a redirect — and then the chunks were
  fetched from where the probe ended *with the probe's own headers*, so the OAuth `Bearer`
  token of RD-106-04 went straight to the foreign host the redirect had just stripped it for.
  The credential is now decided again for the address the bytes actually come from.
  `crates/rd-scheduler/tests/provider_transfer_credential.rs` pins it against a real TLS
  listener, and a canary recorded down to `TRACE` finds the password and the encoded pair in no
  log line and no failure. Neither provider has run against a live account.

- **A second MEGA sign-in no longer computes over the first one's session (RD-120-30).**
  `store-token` wrote MEGA's session over the account's password, so the password was gone after
  the first sign-in and the next one derived its key from a session identifier. A provider with
  a flow-filled slot now keeps the session beside the person's credential.
- **A wildcard reach was judged by its bare suffix (RD-120-30).** The key-derivation reach check
  stripped `*.` and asked whether the credential could be sent to the bare suffix — one host of
  the many the plugin could reach. It now compares patterns: a `*.suffix` reach needs the same
  wildcard in the slot.
- **`build-plugins.sh --list-stale` and `--list-missing` see the shared target from a worktree
  (RD-120-40).** Without `CARGO_TARGET_DIR` set they looked into the worktree's own, empty
  `target/` and named every component as missing — all 72 of them. A linked worktree is now
  recognised and pointed at the main checkout's target, the default `worktree.sh` already had.

- **The speed-limit field in the status bar cut its own placeholder short.** At `w-32` the box
  left 57 px for text once the `MiB/s` suffix and the number stepper had their share, and the
  longest placeholders — `unbegrenzt` and `sin límite` — measure 72 px in JetBrains Mono at 12 px,
  so every language showed a clipped word (`unlimite` in English). It is `w-40` now, which leaves
  89 px: room for the longest of the four with 17 px to spare, measured in Chromium rather than
  estimated. `w-36` would have left one pixel.

- **`scripts/i18n-key.sh` can write an error code (RD-120-24).** It is the one sanctioned way to
  add a translation key, and the group where most new keys appear was out of its reach: the codes
  in `server.json` are literal keys with dots inside them, flat under `codes`, and the script
  split every dot into a group level. A dotted argument built a nested group that resolved
  nowhere — in all four languages at once, which is exactly what the language comparison cannot
  see. A backslash now escapes a dot, and the script refuses to open a group inside one whose keys
  already carry dots, printing the escaped form instead. The first version of that refusal keyed
  on "holds only strings" and would have blocked a new subgroup in 407 ordinary groups; the dot in
  an existing key is the actual signature, and only two groups in the tree have it.

- **The queue called a postponed PAR2 volume a mirror (RD-120-16).** `skipped` is written by two
  different places for two different reasons, and only one of them was translated. The scheduler
  writes it for a mirror sibling -- another link to the same bytes, standing by in case the one
  that runs gives up -- and the NZB intake writes it for a recovery volume that is deliberately
  held back until a repair turns out to need it (RD-107-04). The badge said "Mirror" for both, so
  a Usenet package's `vol` rows made a claim about redundancy where a statement about timing
  belongs: there is no second provider they would be fetched from instead.

  The two are told apart by the group key. A mirror is only ever written as skipped together with
  the `mirror_group` it stands down for -- the same test `stand_down_siblings_of` makes before it
  touches anything -- and a postponed volume never carries one. The field was already on the
  `DownloadFile` response, so nothing new is stored, no migration was needed and the API contract
  is unchanged. Postponed rows now read "Postponed" / "Zurückgestellt" / "Aplazado" / "Différé";
  mirrors read as before. Nothing about the behaviour of either changed -- both still count as
  settled for auto-removal, both are still left out of the SABnzbd queue, and a volume is still
  re-queued only by the block count a repair asks for.

- **An obfuscated Usenet set wrote one warning per article instead of one per file
  (RD-120-33).** Posters vary the yEnc name from part to part, and a fully obfuscated set gives
  every article its own random name, so `yEnc segments disagree on the output filename` appeared
  in the log as many times as the file had segments -- hundreds of lines whose only new content
  was a name nothing went on to use. The choice of name was never in doubt: on a disagreement the
  NZB subject decides, the way SABnzbd does it, and the worker was already tracking the same
  condition separately in order to make that decision. The deviations are now counted and
  reported once per file, naming the number of segments that disagreed and the name the file
  actually receives -- which, after a disagreement, is none of the names the articles carried.


- **A transfer could carry the client secret where the access token belonged (RD-120-05).** An
  OAuth provider whose person registers their own application holds two credentials at once
  (RD-106-03): the client secret in the account's own slot, the access token beside the sign-in
  flow. The plugin side has always told them apart; the transfer side read
  `accounts.secret_ref` and would have put the client secret into the `Authorization` header of
  every download. Real-Debrid is the other provider of that shape and never reached it, because
  its download addresses are generated and carry no bearer at all — Box is the first whose bytes
  come from the API host itself, which is where this became reachable.
  `crates/rd-scheduler/src/worker.rs` now asks which of the two the account is holding, and an
  account whose sign-in has produced no token yet sends nothing rather than falling back to the
  wrong credential.

- **The merge gate said "never verified" for a branch that was demonstrably green (RD-120-25).**
  `rd_target_dir` falls back to `<path>/target` when `CARGO_TARGET_DIR` is unset, and worktrees
  share the main checkout's target directory — so the green marker a worktree's own `check.sh`
  had recorded sat in one place while `scripts/worktree.sh finish` looked in another, found
  nothing, and refused the merge. It cost two refused merges before the cause was found.
  `worktree.sh` now exports the shared directory itself, which is the documented setup anyway.
  A gate that says "unverified" when it merely cannot see the record is a gate people route
  around, which is worse than no gate.

### Changed

- **A package says which version it is without being started.** The Windows and Linux
  packages carry a `VERSION.txt` at their root: version, commit (marked `-dirty` for an uncommitted
  tree), build time and platform — the owner's request of 2026-09-24.

- **Verification without idle runs: two levels, and component staleness by content (RD-120-58,
  tooling).** `scripts/check.sh` without `--full` now runs at branch level: clippy and every test
  of the touched crates, the library and binary tests of one level of reverse dependencies,
  `rd-api --lib`, and only the `rd-api` integration binaries `scripts/lib/rd-api-tests.map`
  selects — changed test files, the binaries whose routes a changed source area serves, and all
  of them for any path in `rd-api`, `rd-core` or a migration the table does not map. Touching
  `rd-core`, `rd-db`, `rd-scheduler`, `rd-files` or more than six crates no longer runs the
  whole workspace at branch level. `--full` runs everything, as before, once per wave on
  `development` and in the release chain, which now calls it; it records a green per half and by
  tree, and `tag-release.sh` and `package-windows.sh` refuse a tree without one (documentation
  changes excepted; `RD_UNVERIFIED_PACKAGE=1` builds a package marked `UNVERIFIED.txt`). Every run
  prints the time per stage and, at branch level, that `--full` is still due. Plugin components
  are compared with their sources by content: `build-plugins.sh` stamps each component it builds
  with the hash of its sources and of itself, `--list-stale` and `rd_plugin_host::artifact`
  compare the stamp, and a checkout or rebase no longer names a component stale — five of eight
  branch checks on 2026-09-24 stopped on exactly that. `build-plugins.sh --components-only`
  builds and stamps without signing; CI's components job uses it. No test was dropped, and no
  product behaviour changed. After the merge every component lacks a stamp and counts as stale
  once, until the next `build-plugins.sh` run.

- **The LinkGrabber can say "cached", and when it was checked; the plugin contract is now
  `rdownloader:plugin@0.8.0` (RD-120-36).** `enum link-status` has a fourth case, `cached`:
  the provider holds the file in its own cache right now, which is more than `online` and
  expires without notice. The host records the time of the check (`cached_at` on the link,
  migration `0093`), and the candidate row shows it as a neutral chip, `cached: <time>`, with a
  tooltip explaining that this is when it was measured, not a promise. The link's state stays
  `online`. The MCP candidate rows carry `cached_at` as well. Premiumize's `cache/check` no
  longer reports a cached file as `online`: `true` is `cached`, and a file Premiumize knows but
  has not fetched stays `online`. The new case changes what a check means, so the package
  version was raised. All eleven `sdk/templates/*/wit/` copies are byte-identical, and all 72
  bundled plugins raised their `api_version` and their own `version`. **Not built:** checking a magnet
  (a source without a host reaches no plugin) and TorBox's three `checkcached` endpoints, which
  today's routing cannot reach. Both are recorded as a 1.3 gap in the job file.

  **Upgrade note: components built against `0.7.0` stop loading.** `SUPPORTED_API_VERSIONS`
  is only `0.8.0`, and the linker binds only the `@0.8.0` interfaces. An installed `0.7.0`
  package is therefore refused at its manifest check under `plugin.capability_unknown` and
  stays listed in the plugin manager. It is not linked. The bundled `0.8.0` packages carry
  higher versions, so the upgrade installs them over the old ones. For downloads pinned to an
  old version, this is what the code does (checked in `rd-plugin-host`, `rd-db` and
  `rd-plugin-transfer`, not assumed):
  - **Resolver pins recover on their own.** Every component load, at startup and at every
    reload, calls `clear_unsatisfiable_resolver_pins`
    (`ResolverService::load_components_from_registry`). It deletes each
    `download_resolver_pins` row whose plugin id and version are no longer loaded. The
    download is then resolved again through the current version of the **same** plugin id. A
    queued or paused download does not fail with `plugin.pinned_version_missing` and does not
    need to be added again.
  - **Remote jobs** find their plugin by plugin id, not by version, so they continue on the
    new build. None of the six remote-job plugins changed its `job-state` format.
  - **The one exception is a transfer-backend checkpoint** (`plugin_transfers`). That pin is
    deliberately not released, because only that build can read its half-written file. A
    download in progress on a `0.7.0` transfer plugin fails with
    `plugin.pinned_version_missing` and has to be started again. The only bundled transfer
    plugin is `example-transfer`.

- **Vulnerabilities are reported privately on GitHub.** `SECURITY.md`, the README and the
  feature list point at the repository's private vulnerability reporting
  (`https://github.com/degoya/rDownloader/security/advisories/new`) instead of a confidential
  GitLab issue — the owner's decision of 2026-09-24, in line with every other source link now
  pointing at GitHub.

- **A plugin that changed cannot be signed under its old version any more (RD-120-47).** An
  installation only takes a bundled plugin package whose version is newer than the one it has.
  On 2026-09-23 six plugins — `ddownload`, `mega`, `mega-auth`, `mega-crawler`, `pixeldrain`,
  `seedr` — had changed and kept their version, so the owner's instance kept running their old
  code, and a fixed bug came back word for word; every test runs freshly built code, so nothing
  else could see it. `scripts/build-plugins.sh` now refuses to package a plugin when a signed
  `<name>-<version>.rdplug` of the same version already holds a different manifest, component or
  locale file, names the plugin and the version to raise, packages the others and fails at the
  end. The comparison is byte for byte, member by member, against the package it would replace
  and against the main checkout's `dist/plugins/`, and never re-signs; the same content at the
  same version is a plain rebuild and passes, and an absent `dist/plugins/` has nothing to
  compare against. `scripts/check.sh` asks the same question earlier and without signing
  (`build-plugins.sh --list-unbumped`), right after the staleness check, for the plugins the
  change touches, and `AGENTS.md` states the rule: a plugin change raises its version in the same
  commit.

- **A subscription hit shows its genre in the row, before anything is expanded.** Every Newznab
  and Torznab attribute an indexer sends was already kept and stored, `genre` included, but it
  sat in the expanded details panel with the technical facts. It now stands in the row beside the
  IMDb score and the language, for every category that states one — music above all, where the
  genre is often the one thing the title does not already say, and films and series alike. As
  with the other two, it is shown once: the details panel no longer repeats it, and a hit whose
  only extra attribute is a genre offers no chevron to expand into nothing new.

- **The navigation shortcuts are the sidebar, read top to bottom: `1` through `0`.** Three views
  that arrived with 1.1 had no shortcut at all — Statistics, Logs and the audit log — because the
  catalogue still knew the seven entries it was written for while the sidebar had grown to ten.
  So `7`, `8` and `9` now reach those three and `0` reaches Settings, which was `7`. The sidebar
  toggle moves from `0` to `b`: it held `0` only because the digits stopped at seven, and `0`
  belongs to the last entry in the list. Learning the set now costs one glance at the sidebar
  instead of a trip to the `?` help — which only holds while the two orders agree, and the
  comment above the catalogue says so.

### Added

- **The host computes over a credential the guest never sees, and the plugin contract is now
  `rdownloader:plugin@0.7.0` (RD-120-20).** `interface auth` promises that a plugin never sees a
  credential, and the host keeps it by substituting `{{username}}` and `{{secret:…}}` on the way
  *out* of the sandbox. A guest can therefore send a password but not compute with one — and
  MEGA's sign-in wants exactly the second thing: `us` takes `uh`, the second half of
  PBKDF2-HMAC-SHA512(password, salt, 100 000), while the first half unwraps the account's master
  key. RD-120-11 measured what that would have cost in the sandbox and **overturned the
  assumption the job had been postponed for**: RSA in the guest is cheap (189 444 831 fuel, 9.5 %
  of a default budget); the expensive part is the password derivation MEGA prescribes
  (3 338 300 549, 167 %), which a manifest may simply declare. Fuel was never the obstacle. The
  contract was.

  So the contract grew one function. `derive` takes a **handle** — one of the references the
  plugin's own manifest already lists under `capabilities.secrets` — and a chain of steps
  (`pbkdf2-hmac-sha512`, `aes-ecb-decrypt`, `take`); the host runs the chain over the credential
  and answers with the **last step's output only**. Every intermediate stays on the host, which
  is what lets `plugins/mega-auth` obtain `uh`, the master key and the RSA private key block in
  three separate calls without the *password key* — the one value that is a direct function of
  the password — ever entering the sandbox. It is gated by a new `capabilities.key_derivation`
  bit, accepted only on the three plugin types whose worlds import the interface, and refused for
  a plugin whose network grant reaches beyond the domains the credential itself may be sent to.

  **What it costs is the measured guest price**, not a token fee and not what it costs the host:
  33 383 fuel per PBKDF2 round per hash block and 8 166 per AES block, both from RD-120-11's
  measurement of the same computation inside the sandbox. Computing on the host is therefore
  never a discount — it only moves the credential — and the cap stays a cap. The charge is made
  before the work, inside the call, against the store the guest is running on.
  `docs/adr/0020-the-host-computes-over-the-secret.md` records why the returned bytes do not
  reconstruct the credential, with the attacker's parameters assumed hostile throughout.

- **`plugins/mega-auth`: a MEGA account signs in (RD-120-20, closing RD-120-11's last box).**
  Two requests and three derivations, against recorded answers and without a credential in the
  repository. What is stored is the session identifier and the master key, because an account
  file's node key is wrapped under the master key and nothing else opens it. MEGA account
  **version 1** is refused by name — its legacy derivation is 65 536 AES rounds over the
  password, a stage nobody has an occasion for yet.

- **`plugins/mega` now declares `[provider]`, so a MEGA account can exist at all.** Until now the
  only two MEGA manifests were both `[extension]` sections, and the provider registry is filled
  solely from `[provider]` declarations — so there was no MEGA account row anywhere, and the new
  sign-in would have claimed a provider that did not exist. A stream-transform plugin is the one
  extension type now allowed to own its provider, because it is a resolver in everything but the
  world it exports (ADR 0011). The sign-in was tried as the owner first and is the wrong one: a
  manifest's message namespace is its provider slug, so the row there would have moved
  `mega_auth.*` into `mega.*`, which the file plugin already owns.

- **`job-source` carries an address (RD-120-20).** The remote-job contract knew a magnet and a
  container, and its own comment already said "a `.torrent` file, **today**". A debrid provider's
  `src` takes an ordinary `http(s)` link as readily as a magnet and answers about it the same
  way, so the third case is `address(string)`. It rides the same row (`source_kind = "address"`,
  no migration), the content key stays the guest's to derive, and the REST endpoint takes either
  `magnet` or `address` — exactly one of them. The two older cases are unchanged, with a test
  that says so. RD-120-01 (TorBox) and RD-120-23 (Premiumize) were waiting on it.

  It travels in the same contract bump on purpose: raising the version drags all eleven
  `sdk/templates/*/wit/rdownloader.wit` copies and every bundled manifest with it, and CI checks
  the parity byte for byte. Doing that twice in one release is pure churn.

### Fixed

- **One setting per row on the interface and unattended pages, and the system page in reading
  order (RD-120-27).** Both pages laid their settings out in `sm:grid-cols-2` grids: language
  beside theme and byte scale beside byte unit on `settings/interface`; two quiet-hours switches,
  the completion action beside its countdown and four power-context switches on
  `settings/unattended`. Label and hint are not the same length, so the pairs sat at different
  heights and the eye was asked to jump between entries with nothing to do with each other — the
  finding of RD-120-21, which fixed it for forms beside a list and named these tab bodies as a
  cut of their own. The grids are gone, and with them the `sm:col-span-2` on the completion
  approval switch, which only ever existed to escape one of them. On `settings/system` the row of
  figures — version, web UI address, CNL2, hotfolder interval, NZB limit — moved from below the
  audit retention to directly after the setup check, before the log store: what the service is
  now comes before how long it keeps things. The row itself is unchanged. `design.md` carries the
  rule in its wider form and, so it does not read as "never two columns", names the three cases
  that justify a two-column grid on a settings page.

- **No plugin shows a diagnostics accordion with nothing behind it (RD-120-28).** Every card in
  the plugin manager carried the accordion whether or not that plugin had ever been invoked, and
  opening an empty one answered "nothing recorded yet" — a click spent to end up knowing less,
  because that sentence cannot tell *never ran* from *ran and was unremarkable*. The control is
  now absent, not disabled, when there is nothing to unfold. The difficulty was never the
  condition but where it gets its answer: the entries are fetched only when somebody opens the
  panel, so before the first open the page knew nothing about them. It still does not — the
  inventory now carries `execution_count` per plugin, a number and never an entry, from one
  grouped read over the index on a table that is bounded by construction at 50 rows per plugin.
  Opening the panel is the only thing that fetches an entry, and a test asserts that mounting
  the tab sends no request to the executions endpoint at all. While that request is in flight
  the open panel says it is loading instead of rendering an empty state that is not yet true;
  `plugins.diagnostics.empty` became unreachable and is gone from all four catalogues.
  `design.md` gained the rule this stands on — a control promises what is behind it — together
  with the three cases in which an empty view is the right answer rather than a broken promise.

- **A test no longer waits on a network it was never meant to ask (RD-120-26).** Intake starts an
  online check for every fresh link, and `wait_for_candidates_ready` gives it five seconds — a
  budget sized, as the comment above it said, for a check that "simply fails to reach the
  network". That assumption did not hold for one file: `storage_capacity.rs` handed in
  `https://example.com/…`, and the bare `example.com` is the one documentation name IANA actually
  answers for. So the check did real name resolution and a real connection attempt, and on a
  loaded machine five seconds was not enough — `a_root_is_released_again_once_its_threshold_fits`
  passed in 3.9 s alone and timed out at 9.29 s inside a full run. All eight hostnames in the five
  files that use the helper were measured: exactly one resolved. It is now
  `files.example.com`, which the neighbouring tests already use, and the same test passes in
  3.888 s with a three-minute `cargo build` running beside it. The comment now says what actually
  holds, and that the timeout is not the thing to raise when this panics.

### Fixed

- **The diagnostic bundle's approval page is no longer half English.** The page that asks what a
  support recipient may see had a translated frame around English sentences: every file
  description, every redaction note and the three never-included lines were prose in
  `crates/rd-diagnostics/src/bundle.rs`, including the lines saying what had been redacted. They
  are now stable codes (`diagnostics.bundle.entry.*`, `diagnostics.bundle.redaction.*`,
  `diagnostics.bundle.exclusion.*`) translated in `web/src/locales/{de,en,es,fr}/logs.json`, and
  the `replaced: …` line is a code with a parameter, so the field names it names stay data in
  every language. `manifest.json` inside the archive stays English and gained a description per
  entry, for the support engineer who opens the zip without the application; that English is read
  at compile time out of `web/src/locales/en/logs.json`, so the two cannot drift. The inventory
  digest still covers only what an entry is, so the same state gives the same bundle whatever
  language anybody reads in (RD-120-15).

### Added

- **The status word of every job file is now checked by a test.**
  `docs/roadmap/jobs/README.md` has fixed five words since RD-109-33 — `Open`, `In progress`,
  `Partial`, `Implemented`, `Blocked/No-Go` — and expressly not `Done`; nothing enforced it, and
  `Done` came back twice within two days. `crates/rdownloader/tests/job_status_words.rs` reads the
  first `- **Status:**` line of every `docs/roadmap/jobs/*.md`, accepts a qualifier after the word
  and names file, found word and the five allowed ones when one does not fit. A job file with no
  status line fails too, unless it is one of the release working files that carry none on purpose.
  Run it with `cargo nextest run -j 4 -p rdownloader --test job_status_words` (RD-120-14).

### Changed

- **A run now checks what the change touches, and the serialisation is code rather than a
  sentence every agent has to remember (`120-25`).** `scripts/check.sh` used to do the same thing
  every time — `touch` on every `crates/**/*.rs`, the whole workspace, 51 `rd-api` test binaries in
  13 batches, the failpoint matrix, `vue-tsc` twice, 104 Vitest files, a full `vite build` — for a
  one-line change in `rd-files` as much as for a release. It now derives the change set once, from
  the older of the branch point and the last green run, and runs what that demands: the touched
  crates plus **one level** of reverse dependencies, the `rd-api` integration batches only for
  `rd-api`, `rd-core` or a migration, the crash matrix only for its four owning crates, sqlx only
  for `rd-db` or a `.sql` file, the web and extension halves only for `web/` and `extension/`.
  Every run prints what it skipped and why, because otherwise a scoped green looks like a full
  one. `--full` is the old behaviour and is what the merge and the release chain use — one level
  of reverse dependencies is not a transitive hull, and that limit is stated rather than hidden.
  `--defer` commits a triviality (text, translations, a colour) against the checks that catch its
  mistakes, without recording a green; `scripts/worktree.sh finish` and the release preflight
  refuse a branch whose HEAD no green run has seen, so the postponement expires at the merge.
  `scripts/lib/lock.sh` re-runs `check.sh`, `build-plugins.sh`, both packaging scripts,
  `release.sh`, `release-pipeline.sh` and `api-contract.sh` under `flock` on `/tmp/rd-build.lock`,
  so two checkouts can no longer build into one `target/` at once; the pure `--list-*` queries,
  `set-version.sh`, `worktree.sh check` and `--defer` stay lock-free. That lock is what makes the
  new `target/` stamp correct: the sources are touched when the checkout changes rather than on
  every run. `web/package.json` no longer type-checks inside `build` — `typecheck` is incremental,
  `typecheck:full` is what CI, the packaging scripts and `--full` run — and the new
  `scripts/web-dist-stale.sh` answers whether `web/dist` is current instead of merely present.
  `build-plugins.sh` and both packaging scripts finally respect `CARGO_TARGET_DIR`, so
  `--list-missing` in a worktree no longer reports every plugin as missing.
- **`AGENTS.md` is 200 lines instead of 537.** The crate-by-crate account with its job history and
  the reasoning behind every rule moved, complete and unshortened, to the new
  `docs/architecture.md`, one section per crate so a rule can point at it
  (`docs/architecture.md#rd-siterules`). What stayed is the working card: the rules, the commands
  and the numbers, plus a crate map of one line per crate and a new *Several agents at once*
  section. That every command, number and rule of the old file is still findable in one of the two
  was checked mechanically, not by care; the method and its result are in the job file (`120-25`).

- **The rule form holds the convention it was breaking, and its group is chosen rather than
  typed (`120-21`).** `SiteRuleEditor.vue` stands in the left column of a `FormListLayout`, which
  `design.md` gives one field per row, and then undid it with three `sm:grid-cols-2` grids inside
  that column: identifier beside name, group beside revision, path patterns beside former hosts,
  probe address beside a date. Labels and hints of different lengths put the paired fields at
  different heights, and the two columns had nothing to do with each other. All three grids are
  gone. The group is now a `UInputMenu` that offers the groups this installation already has and
  still accepts a new name — a typo could previously open a second group with one member without
  saying so, and nothing in the form showed which groups existed. The groups come from the rule
  list the page already loads (`GET /api/v1/site-rules` returns them), so there is no new
  endpoint. `design.md` now says that one field per row governs grids *inside* the form column
  too, which is the reading that had been missing.

- **The site-rule format keeps its two measured limits, and neither costs code (`120-12`).**
  A rule still cannot say "this host, no" and still cannot walk a list;
  `docs/adr/0017-the-rule-format-keeps-two-limits.md` records why each stays and what reopens it.
  The filter limit is narrower than it was written down: the page that produced it did not
  survive RD-110-11's re-measurement, the *positive* form is already in the format — a `regex`
  with `all` over an alternation of the accepted hosts, which the `satdl` rule already uses — and
  a rule-level `drop` would remove the stray address together with the
  `collector.crawl_not_a_file` count that announces a page has changed. The loop limit fails on
  its own page: `serienjunkies.org` wants a browser fingerprint in the request body as well, so a
  loop would not reach it, and its 39 releases do not fit `max_pages` = 24 either. `FORMAT_VERSION`
  stays 1, the seven step kinds stay seven, and `crates/rd-siterules/resources/site-rules.json` is
  untouched and keeps its signature. `docs/site-rules.md` now states both as decided rather than
  open, and documents the positive filter as the shape to reach for.

- **MEGA downloads public files and folders, and the sign-in that was holding it up has been
  priced instead of guessed (RD-120-11).** The number this job was moved out of 1.1 for is
  measured: an RSA-2048 private operation costs a WebAssembly guest **189 444 831 fuel**, under a
  tenth of a plugin's default budget and under half a percent of the largest a manifest may
  declare. The expensive stage is the one nobody was looking at — PBKDF2-HMAC-SHA512 at MEGA's
  mandated 100 000 rounds, **3 338 300 549 fuel**, 1.67 times the default budget on its own. A
  whole sign-in is 8.8 % of the ceiling, so it would fit. What does not fit is the contract: a
  plugin never sees a credential, the host substitutes secrets on the way *out* of the guest, and
  MEGA's `us` call wants a value derived from the password rather than the password. The sign-in
  is therefore a decision with three named options rather than a task, it is written up in
  `docs/roadmap/jobs/120-11-mega.md` and `docs/adr/0011-*`, and **no `mega-auth` plugin was
  shipped** — a signed package that cannot do its own job would be worse than none. Measure it
  again with `scripts/measure-mega-login-fuel.sh`.

- **A MEGA folder now comes back whole or not at all.** `mega-crawler`'s own limit was five
  thousand files while the host trims any crawler's answer to a thousand without saying so, which
  meant a folder of between one and five thousand files arrived as a package quietly missing its
  tail. The plugin's limit is now the host's, so the answer is either the complete listing or
  `mega_crawler.too_many_files` with the limit in it.

- **A MEGA refusal carries the wait MEGA asked for.** A `509` puts its countdown in
  `X-MEGA-Time-Left`, and that was being thrown away: every 5xx became a generic transient
  failure. Both MEGA's own header and `Retry-After` are now read, a `509` becomes a rate limit
  with that number under the new code `mega_crawler.quota_exceeded` (translated into all four
  languages), and a used-up quota is no longer reported as throttling. The date form of
  `Retry-After` is deliberately not guessed at — a guest has no clock, and the scheduler would
  wait out a wrong number literally.

- **Version set to 1.2.0, and the milestone's working file exists.**
  `docs/roadmap/jobs/120-00-release-koordination.md` records the base, the waves, the pre-assigned
  migration numbers and the per-branch verification chain for all fourteen jobs of milestone 1.2 —
  the ten hoster and remote-download jobs plus the four carried over from 1.1 (`120-11` MEGA,
  `120-12` rule format, `120-13` account test, `120-14` status-word lint). None of the fourteen has
  an unmet dependency.

- **The 1.2 feasibility gate was measured, and it closed three of five services.** Seedr
  (`120-04`) and Pixeldrain (`120-07`) are a Go and stay in the milestone; UploadGig (`120-08`),
  SwissTransfer (`120-09`) and Smash (`120-10`) take status `Blocked/No-Go` with a record each —
  `docs/adr/0014-uploadgig-no-go.md`, `0015-swisstransfer-no-go.md`, `0016-smash-no-go.md`. No
  code, no plugin directory, no provider entry; the bundled component count does not change.
  Two failed the familiar way, with the operator's own `robots.txt` withholding the download
  path. Smash failed differently and is the one worth remembering: it has the best-documented API
  of the five, official and MIT-licensed, but every package in it is account-scoped and sold to
  the sender, and there is no documented route to a transfer somebody else created.

- **The hosters the shipped rules hand over to were measured, and none of them passes the gate
  (`120-19`).** Four addresses that a shipped site rule produces and no resolver claims:
  `icerbox.com` via `avxhm.se`, `nfile.cc` and `dwp.la` via `downmagaz.net`, and ViperGirls'
  `oxy.cloud`, recorded here for the first time. All four are a No-Go, and only one of them for
  the reason the job expected.
  `docs/adr/0018-icerbox-no-go.md` is the substantial record: a permissive `robots.txt` and a
  clean private JSON API, and a No-Go all the same, because the terms forbid "robots or similar
  data gathering or extraction methods" and any "software designed to automate any functionality
  on the Service", the service states it sells premium subscriptions only, and four of four
  files reached through the shipped rule answer "The owner of this file disabled free downloads"
  — `avxhm.se` is an IcerBox affiliate, so its own uploads have the free route switched off.
  `docs/adr/0019-the-address-a-board-shows-is-not-the-hoster.md` holds the other three, and its
  finding is the reusable one: `nfile.cc` and `dwp.la` are not hosters at all but affiliate
  link-cloakers, redirecting to `novafile.org` and `downup.me` — and `xfs-generic` has claimed
  `downup.me` all along, so that chain never needed a resolver, only the redirect followed.
  `oxy.cloud` no longer exists as a domain (NXDOMAIN with the registrar's SOA). ViperGirls needs
  no hoster work either: its four file hosts are already covered by `rapidgator`, `katfile`,
  `keep2share` and `xfs-generic`. No code, no plugin directory, no provider entry, no manifest
  change; the bundled component count does not change.

### Fixed

- **A package name a site rule read off the page is no longer thrown away (RD-120-17).**
  A rule hands the release title to every link it found as its `package_hint`, intake builds the
  package from it, and then the online check that follows intake always ended in a regroup of the
  batch — and that regroup re-derived every *auto-named* package from the file names then known,
  with no hint to go on. So the title lasted exactly as long as the check: at `downmagaz.net`
  it was visibly replaced, at `getcomics.org` it never appeared to arrive at all. The link the
  report suspected only supplied the replacement — with no file name of its own it fell through
  to the host fallback, so the package took its host name. It was not an unsupported hoster
  doing the renaming, and on that page it was not a hoster at all: RD-120-19 measured both of
  `downmagaz.net`'s addresses and found two affiliate link-cloakers.
  `rd_collector::Group` now says whether a name was *stated* by the source or inferred here, and
  a stated name is written as `auto_named = 0`: the regroup never reaches it. An explicit package
  name behaves exactly as before. No migration — `collector_packages.auto_named` has been there
  since `0015`.
- **`libgen` and `satdl` are out of the shipped rule pack (RD-120-17).** The owner opened every
  shipped rule on its live service on 2026-09-22; libgen.bz no longer resolves the address its
  rule reads out of the mirror row, and satdl.com was withdrawn. A rule that claims an address
  and then delivers nothing is worse than no rule, because the generic crawlers never get to see
  it. Pack sequence 5, eight rules, each carrying 2026-09-22 as its measurement date.
- **The rule list stops saying nobody has checked the shipped rules (RD-120-17).** A shipped rule
  with no local self-test result read "Not checked", which was untrue the moment the pack was
  signed. It now reads "Checked", with the day out of the signed pack behind it; a local
  self-test still wins over both, and a rule somebody wrote here keeps "Not checked" — there its
  `checked` date is the author's own claim. Translated in all four languages.
- **A missing resolver no longer reports itself as a missing file (RD-120-18).** Four addresses
  from the owner's live check of 1.1 — `controlc.com`, `nfile.cc`, `dwp.la` and `icerbox.com` —
  all ended on `collector.check_not_a_file`, "this address answers with a page, not with a file".
  The sentence is literally true of a hoster's download page and names the wrong cause: the site
  rule had resolved correctly and handed over an address this installation has no resolver for,
  and the reader went looking at the rule, the site or their browser. The link check now asks
  `rd_provider_registry::provider_for_url` — the same authority `rd_plugin_host`'s
  `account_required` consults, and for the same documented reason it is not the resolver list —
  and reports the new code `collector.check_no_resolver` with the host as a `{host}` parameter in
  all four languages; `collector.check_not_a_file` stays for the case it was meant for, a
  supported host that really serves a page. The sentence deliberately stops at what can be
  established and does not claim a plugin is missing: ADR 0019 measured `nfile.cc` and `dwp.la` to
  be affiliate cloakers rather than hosters, with `downup.me` behind `dwp.la` already claimed by
  `plugins/xfs-generic`. Following such a redirect at intake remains open.
- **A green account test now means the cookie session works (RD-120-13).** In `api_key` mode
  `ddownload` asked the API about the account — "Premium active, 195 GiB" — and then *counted*
  the cookies in the jar: "8 cookie(s) loaded for downloads" said only that eight cookies were
  lying around, never that they were still a session. The reported evening went into finding
  that out, because the download then met a captcha and nothing explained it. All three
  XFS-family plugins now verify the session with a request instead: `ddownload`, `katfile` (in
  both of its branches — the cookie-only one accepted any 2xx answer, which is what an expired
  jar receives) and `filejoker`, whose check already verified and now says so in its label
  (`plugin.account.session_active`). An expired session has its own stable code, apart from the
  one for an invalid account, in all four languages:
  `ddownload.download_session_expired` and `katfile.download_session_expired` name the API key
  as fine and the browser session as the thing to replace. Zero cookies stays green on purpose:
  link checks run on the key alone, and the label states the absence in words.
- **A silent fallback leaves a trace (RD-120-13).** `direct_link` swallowed four error paths
  with `.ok()?` and wrote nothing, which is why the running installation's error log held no
  line about ddownload at all. It still falls back to the cookie premium flow, but the reason
  goes out exactly once per attempt as one of five fixed phrases from
  `xfs_common::api::DirectLinkSkip` — a constant, so no file code, address or key can travel
  in it.

- **Every download with a content transform failed before it fetched a byte.** RD-110-33 built
  both ends of the key's journey and left the middle out: the plugin host answers
  `key_reference: None` and leaves filling it in to its caller, no caller did, and
  `StreamTransform::new` refuses a description without a vault reference on purpose — so MEGA
  ended as `transform.key_missing` every time. Nothing caught it because no test ran the real
  component's answer into the real engine. Migration `0089` adds `downloads.transform_key_ref`,
  `Database::adopt_transform_key` puts the key away and hands back the reference, and the
  scheduler calls it before it builds the transform. It is idempotent by design: the same key
  gets the same reference, so a continuation still recognises the chunk MACs it wrote itself,
  while a key that changed gets a new reference and the file starts over rather than being
  decrypted with one key over bytes written under another. Deleting the download forgets the key,
  as deleting it already forgot the link fragment.

- **Three documents said something that had stopped being true.** `AGENTS.md` claimed
  `remote-job` has no adapter in `rd-plugin-ext`; it has had one since RD-108-03
  (`rd-plugin-ext/src/remote_job.rs`), and the durable half — migration `0065_remote_jobs.sql`
  with its pre-network duplicate guard, `rd-db/src/remote_job_store.rs` and
  `rd-api/src/remote_job_service.rs` — is the baseline a new remote-download provider builds on
  rather than something each one brings. `120-05-box.md` and `120-06-pcloud.md` still opened with
  "no provider-specific cloud-drive connector exists"; four do, and the shared cloud-source
  interface they name as a dependency was decided in RD-106-04. Both now point at
  `plugins/google-drive*` as the template and name the plugin packages instead of
  `rd-collector`/`rd-scheduler` as their touch points.
- **The jobs index's aggregate was wrong too, and on no reproducible basis.** It said `88 of 290
  jobs are still open or partial`; measured on 2026-09-22 it is `57 of 322`. The line now states
  how to recompute it — every job file except `README.md`, minus the six without a status line
  (the five release-coordination working files and `108-00-abnahme-checkliste.md`).
- **The jobs index counted wrong in five places.** `docs/roadmap/jobs/README.md`'s inventory said
  20, 31, 46, 43 and 10 for milestones 1.0.7, 1.0.8, 1.0.9, 1.1 and 1.2 where its own catalogue
  lists 21, 33, 47, 45 and 15; the total moves from 317 to 328. The catalogue also gained the row
  for `120-00`, as `110-00` has.


## [1.1.0] - 2026-09-22

### Changed

- **Three jobs moved to milestone 1.2, and 1.1 closes on what it promised.** Beside MEGA below,
  `110-35` became `120-12` and `110-36` became `120-13`. Both were cut on 2026-09-22 out of what
  this milestone *found* rather than out of what it undertook: the site-rule format's two measured
  limits need a decision (a filter step, and a loop step — `110-10`, `110-12` and `110-13` each
  measured a real page that needs one), and the provider account test reports green without ever
  asking whether the stored cookie session still holds, in ddownload, filejoker and katfile alike.
  References to `RD-110-35` and `RD-110-36` stay valid and are not rewritten.
- **MEGA's remainder is milestone 1.2's `120-11`, not 1.1's `103-02`.** Everything MEGA needed
  from 1.1 was delivered in 1.1: the twelfth contract world `stream-transform` with the cipher on
  the write path, the chunk MACs in the checkpoint and the integrity value checked in constant time
  (RD-110-33), the way a key that lives in a link fragment reaches the vault and comes back to the
  resolver (RD-110-38), and the three plugin directories with contract tests over the real
  components. What is left is the account login — its own `auth` package and an **unmeasured** fuel
  price for RSA in the guest — and the measurements behind three acceptance criteria. A milestone
  should not stay open on one unknown estimate, so 1.1 closes with the hard half of MEGA done
  rather than with MEGA undone. As the last time this identifier moved, references quoting
  `RD-103-*` in commits, sources and `docs/adr/0011-*` are **not** rewritten and still point at
  the same undertaking.

- **A link whose key is its fragment now keeps that key — in the vault** (RD-110-38). Two
  accepted decisions collided head on and MEGA was unresolvable because of it. Intake strips
  the fragment off every address before a candidate row exists (RD-109-32), because nothing
  distinguishes a share password from an anchor name; MEGA encrypts on the client and its file
  key rides in exactly that fragment (ADR 0011). The address that reached the queue carried the
  handle and not the key, so neither a file link nor the folder crawler's child links could be
  resolved. A provider now **declares** the hosts whose fragment is key material in its manifest
  (`secret_fragment_domains`, see `docs/plugins.md`); for those the intake puts the fragment in
  the credential vault before it shortens the address and writes only a `vault://` reference
  beside the row (migration `0087`), and the scheduler restores it onto the address one call
  before it asks the plugin — the same shape as the `{{secret:…}}` expansion the host already
  does on an outgoing request. Ownership moves with the link: the queue row inherits the
  reference when a candidate is enqueued, and deleting whichever row owns it removes the secret
  from the vault. Without the declaration nothing changes and the fragment is dropped as before;
  there is no host list in the code and no special case for any service. A canary holds the line
  — no row of any table, no event, no log record, no diagnostic bundle, no OpenAPI answer and no
  interface carries the fragment — and the MEGA contract test proves the key comes back through
  the real components. `docs/adr/0011-*` carries an addendum naming the point that was missed
  when it was accepted.

- **Two files are back under the house limit** (RD-110-37). `crates/rd-core/src/subscription.rs`
  (602 lines) and `crates/rd-api/src/subscription_service.rs` (562) were split along seams the
  code already had, into submodules that the parent re-exports: the filter set and its refusal
  reasons became `subscription/filters.rs`, the item archive and the poll runs
  `subscription/archive.rs`, the three `rd-subscription` port implementations
  `subscription_service/adapters.rs`, and the one path from a subscription item into the
  LinkGrabber `subscription_service/intake.rs`. A move, not a rewrite: no type, function or
  public path changed, and the test counts of the touched crates are identical before and after
  (353 for `rd-core`/`rd-subscription`, 255 for `rd-api`'s library and subscription suite).

- **The settings have a structure, an entry page and one header** (RD-110-29). The twenty-three
  settings pages now sit in six rubrics, the same in the sidebar and on the entry page:
  *General* (General, Interface, Desktop client), *Downloads* (Storage & rules, Hotfolders,
  Bandwidth, Unattended operation, Post-processing), *Sources & protocols* (Accounts, Captcha &
  solver, Usenet, BitTorrent, Media, FTP/SFTP/WebDAV), *Integrations* (Services, Plugins, Tools,
  Notifications, API & MCP), *Network & security* (Network, Security) and *Administration*
  (Backup & restore, System). Six pages are new homes for cards that moved without changing:
  the tool status, the managed tools and the vendor folder left *Interface* for *Tools*; quiet
  hours, the completion action and the power context left *Bandwidth* for *Unattended
  operation* (which, unlike *Bandwidth*, shows the save bar those fields always needed); the
  torrent card left *Media* for *BitTorrent*; the captcha card and the FTP/SFTP/WebDAV
  credentials left *Network* for *Captcha & solver* and *FTP, SFTP & WebDAV*; the hotfolders,
  with their poll interval, left the *Storage & rules* tabs for a page of their own. `/settings`
  no longer redirects to the system page but shows one card per page under its rubric, each
  with the page's title and description and each a link; every earlier page address still
  works, `?tab=` links still redirect, and an address naming no page lands on the overview.
  Desktop client, API & MCP, Security and Accounts now open with the same eyebrow, title and
  description as every other page, and a test mounts each page and counts exactly one such
  header. The captcha prompt's link to the solver settings leads to the new page.
- **Remote jobs are a page of their own** (RD-110-29). The card that sat at the foot of the
  accounts page is unchanged but lives at `/remote-jobs`, with an entry beside *Subscriptions*
  in the sidebar and the shortcut `5`; a job that runs at a provider is something to watch and
  answer, not a setting. The navigation keys keep their sidebar order, so *Automation* is now
  `6` and *Settings* `7`.

### Added

- **MEGA: the plugins that describe a stream the host decrypts** (RD-103-02, phase 2 of ADR
  0011). Two bundled plugins and a shared library. `mega` is the first plugin of the twelfth
  world: it reads a file address, asks MEGA's documented command endpoint for the storage
  address, opens the encrypted attribute block to learn the name and the modification time,
  and answers with **a description rather than a byte** — AES-128-CTR under this key and this
  counter prefix, chunk MACs at MEGA's own boundaries, and the condensed value the provider
  published. The keystream, the MAC chain and the check before promotion are the host's, on
  the write path RD-110-33 built, so a MEGA download costs no second pass and no second copy
  on disk — which is what both reference implementations pay. `mega-crawler` lists a public
  folder: one call, every node, names, sizes and structure, with the folder's own name as the
  package suggestion. The cryptography is not quoted but recomputed: the key schedule, the
  attribute block, the modification time inside the fingerprint and the folder's node keys are
  asserted against a public example measured on 2026-09-22, and a canary proves no request
  this plugin makes to MEGA ever carries the key. The scheduler now asks the twelfth world
  where the resolver chain said nothing, and migration `0086` keeps the finished chunk MACs of
  a transformed stream across a restart together with the description that wrote them.
  **Not usable yet:** an address's fragment is dropped before it reaches a row (RD-109-32),
  and MEGA's key is its fragment. The collision is recorded in the job file and in
  `rd_core::collector`'s tests; deciding it needs the project owner.

- **A proposed mirror group can be taken apart** (RD-110-34). A group built on a shared file
  name alone is a proposal, and until now it was as irreversible as a declaration: whoever could
  see that two links are a different cut of the same release could only pin one of them. The row
  of such a group now offers *Ungroup these links*, asks once, and afterwards its links stand on
  their own. The decision is stored as the pairs of links a person stated are not the same file
  (migration `0085`), which is what makes it survive all three places the groups are recomputed
  — intake, the regroup after the online check and a move between packages — including the case
  that would otherwise undo it, the online check handing the same two links a matching size. A
  link that arrives later with the same name joins nothing, because the set it would join is the
  one that was refused. Only a proposal: a group a site rule declared, or one two sizes
  corroborated, is refused with `collector.mirror_group_not_proposed` rather than asked about,
  because a contradiction against those is a finding about the source and clicking it away would
  fix one package and leave the rule to repeat itself on the next page.
- **A release page can be watched as a subscription** (RD-110-21). A series, category or tag
  page is given to *Subscriptions* as the new type *Release page*: every half hour at most, the
  listing is fetched conditionally, the links on it that a site rule reads become items, and
  each one enters the LinkGrabber through the ordinary intake — where the rule produces the
  hoster links and RD-110-18's grouping makes the mirrors one row rather than forty. What makes
  it usable is the recognition: an episode is identified by its *release name* — series, season
  and episode — and not by its address, so the same episode posted again by another group, in
  another quality or three weeks later counts as already had instead of being queued a second
  time. Quality and language are read with the closed token lists RD-110-18 established, so
  `Show.2160.mkv` is still not 2160p, and they feed the minimum-resolution, language and
  exclusion filters that were already there. *Keep every release* is the explicit
  counter-choice for somebody who collects versions: identity is then the address again.
  Migration `0084`.
- **A release is one row in the LinkGrabber, with the mirror a choice** (RD-110-19). A release
  page that offers the same episode in three qualities at five hosters used to arrive as forty
  candidates; it now arrives as one row per mirror group — the chosen mirror's own row on the
  shared queue grid, with the other mirrors behind the chevron pair. The badge in the name cell
  says how the group was formed, and a proposal changes its **noun** rather than only its
  colour: `5 mirrors` where a page declared them or a size confirmed them, `5 possible mirrors`
  in `warning` with a dashed edge where only a file name is shared, each with the evidence
  spelled out in its title. A group whose mirrors are all gone says so and stays queueable
  (RD-101-06). Quality, language and hoster stand in the toolbar as the **standing
  preference**: inside a group they pick the member the queue will fetch, outside one they hide
  what cannot match, and they are stored on the server under `collector.mirror_preference`, so
  they still decide the package that arrives tomorrow. Any group can be overruled with *Use
  this mirror*, which pins the member: migration `0083` keeps that decision apart from the
  derived selection, so neither a later preference nor a regroup takes it back. New routes:
  `GET`/`PUT /api/v1/collector/mirror-preference` and
  `POST`/`DELETE /api/v1/collector/candidates/{id}/mirror`. The pattern is in `design.md`.
- **The rule pack recognises seven more services** (RD-110-11). Sequence 4 of the shipped pack
  carries ten rules instead of three: beside `scnlog.me`, `downmagaz.net` and the ControlC
  pastebin there are now Scene-RLS (`scene-rls.com`, `scene-rls.net`), GetComics, AvaxHome,
  CGPersia, Library Genesis, SatDL and the ViperGirls board, the last in a group of its own,
  `adult`. Every one of them was fetched from the live service on 2026-09-21 with a current
  Chrome user agent, no account and nothing solved, every one carries that date as its
  `checked`, and `rdownloader doctor site-rules` answers `ok` for all ten. Three of the new
  rules walk a gateway on the service's own host before the address they hand over — AvaxHome's
  `/go/` token, Library Genesis' `ads.php` page, SatDL's download page — and end at a redirect
  the rule keeps without following it, which is how the answer may sit on a hoster the rule does
  not claim. Eight more domains these services have left behind are carried as `dead` -- nineteen across
  the pack -- so an address on `getcomics.info`, `avaxhome.ws`, `avxhm.is`, `libgen.lc` or `libgen.org` is
  rewritten to the canonical host rather than refused. Four candidates from the same list carry
  no rule and the reason is a measurement, not an omission: `hdencode.org` keeps no address in
  its HTML and hands the links out behind a reveal form with a Cloudflare Turnstile widget and
  an image-captcha fallback, whose submission was not measured and which therefore gets no rule
  written on an assumption, `metalarea.org` and `hi10anime.com` replace
  the link block with a registration notice for a guest, and `13dl.to` — the service's only
  domain — now answers with a parking page. RD-110-10's finding that `scene-rls.net` mixes its
  own addresses into the links block was re-measured over eight release pages and does not
  hold; what does hold is that two containers of the ten carry one address that is not a file,
  and RD-110-07's crawl verdict refuses and counts those rather than the rule excepting them.
- **The site rules have a settings page** (RD-110-08). *Settings → Site rules* lists every rule
  this installation knows, grouped by its group, and each row says who wrote it — shipped or your
  own — which hosts it claims and what the last self-test (RD-110-09) found, with the refusal's
  own reason in the badge's title. Two switches: one per rule and one per group, and a rule the
  project ships is switched off rather than edited or deleted, because it lives in the signed
  pack. Both decisions survive a restart (migration `0082_site_rule_switches`), and both take
  effect on the next paste rather than after one. A rule of your own is written in an editor whose
  fields are fields: the seven step kinds of RD-110-05 each draw the fields that kind needs, the
  steps can be reordered because their order is what they mean, and there is no JSON box anywhere.
  Before saving, the rule can be run against a real address you name: the trial shows the links it
  found, the package name it read, how many requests it took, and — judged by exactly the rule
  RD-110-07 applies to a real paste — which of the links answer with a page rather than a file and
  would be dropped. Rules of your own can be exported to a file and imported from one. **An
  imported rule is unsigned, so it is never automatically active**: it is validated the way a rule
  from the pack is validated, refused individually if it collides with an existing identifier, and
  stored switched off. The import endpoint carries no field that could ask for anything else, so
  switching one on is a separate, deliberate request per rule — enforced by the service, not by
  the interface. Every string ships in German, English, Spanish and French.
- **A dead mirror no longer stops the download** (RD-110-20). When the link that holds a mirror
  group's turn fails for a reason that lies at the hoster — offline, gone, a limit, a login
  refused, a web page instead of the file — the next mirror of the group takes over by itself,
  and the partial data of the one that was given up is discarded so the file can never contain
  bytes from two sources. A cause that lies on this machine never moves the job on: a full disk,
  a destination that cannot be written to and a file that could not be put in its place now
  carry stable codes of their own (`download.local_io`, `download.local_promote_failed`), so
  five mirrors are not burnt discovering what the first one already said. A plain server hiccup
  still waits for its retry on the same mirror rather than spending one. Each mirror is tried at
  most once per attempt, so a fallback cannot become a loop, and which ones have been through
  survives a restart. The abandoned mirror says what happened and who took over
  (`download.mirror_handover`), and a group that has run out of mirrors ends with the reason the
  *chosen* mirror gave rather than whichever was tried last (`download.mirrors_exhausted`) — all
  of it in German, English, Spanish and French. The new crash point
  `scheduler.before_mirror_promoted` covers the window between recording the failure and
  promoting the successor; a start that finds a group with nobody running gives it its next
  mirror.

- **Rules say themselves when they have aged** (RD-110-09). `rdownloader doctor site-rules` asks
  every site rule about the `probe` address it names itself and sorts the answer into four
  states: `ok` (reachable, structure fits, at least one link), `structural` (reachable, nothing
  came back — theme or layout changed), `blocked` (something answered and refused) and `dead`
  (nothing answered, or the page is gone). The command prints a table and exits non-zero as soon
  as one rule is not `ok`, so a release preparation can stop on it; `--rule <id>` narrows it to
  one rule. The result is stored per rule in the new `site_rule_checks` table with the refusal's
  own code beside the state, which is where the rule list of RD-110-08 will read it, and a rule
  found `dead` is skipped by the crawler selection from the next start on — skipped, not
  deleted: it keeps its place, stays switched on, and the next run that finds the service alive
  brings it back. Every shipped rule is now held to naming a probe its own `match` claims and a
  `checked` date that is not in the future. The run is triggered and never scheduled: a download
  manager does not go knocking on board pages by itself. `docs/site-rules.md` carries the
  admission rule as a binding sentence — no rule and no protector plugin is implemented without
  the service having been measured first, with the measurement date on the job.

- **A paste that holds a link list becomes the links in it** (RD-110-12). The shipped rule pack
  carries a third rule, `paste-generic`, for `controlc.com` and the four addresses that today do
  nothing but forward to it — `pasted.co`, `tinypaste.com`, `tny.cz` and `binbox.io`, each with
  its `www.` form, plus `www.controlc.com`, which the origin server answers with a 522. All nine
  are rewritten to `controlc.com` with the paste's own identifier, so an old bookmark works and
  the redirect costs no request. What comes out is the paste's own text block: the recorded
  answer carries fifteen absolute addresses and eleven of the service's own pages, and the seven
  in the paste are what the rule returns, named after the paste. A deleted paste answers a real
  404 and refuses with `site_rules.page_dead`; an address on the same host that is not a paste
  is not claimed at all, so the selection moves on to the generic crawlers. Which of the
  addresses in a paste is a file stays RD-110-07's question, not the rule's: a paste is running
  text, and the probe drops what answers with a page.

  The second branch the job planned, base64-encoded lists, has **no live recipient left** and
  ships as a measured No-Go. The two services of this class that carried it — `prialepaste.com`
  and `anonymizer.link` — no longer do; the first answers its `f.php` with a redirect to its own
  front page, the second redirects everything to Google. Of the other services JDownloader
  carries here, `spaste.com` puts a captcha in front of every paste, `compupaste.com`,
  `lupaste.com`, `gpaste.us`, `pasfox.co`, `pasfox.com`, `hopepaste.download` and
  `fullpaste.todofullxd.com` are gone, and the three that still answer deliver nothing but
  advertising shorteners, which this job excludes.
  `docs/roadmap/jobs/110-12-paste-und-linklisten.md` records every measurement and its date.
- **A release and its mirrors are one thing, not five** (RD-110-18). Links in one LinkGrabber
  package that point at the same file now carry a mirror group, so the five hosters a release
  page lists arrive as one file with five mirrors instead of five candidates of which four get
  deleted by hand. A mirror is deliberately not a duplicate: a duplicate is the same address a
  second time and is set aside, a mirror is a different address for the same bytes and is kept,
  because it is what remains when the chosen one goes offline. Nothing in the grouping reads or
  writes the duplicate state; the one place the two meet is the rule that two links with the
  same address are never mirrors of each other. Three sources, in the order they are trusted: a
  site rule whose new `"mirrors": true` says its page is one release, a file name and a size
  that agree once the online check has run, and a shared file name alone -- the last recorded
  as `name` rather than `name_and_size`, because it is a proposal and has to read as one. One
  member of each group is marked as the chosen mirror: the first in the package's order, since
  nothing has been measured at that point and a choice that changes between two views of the
  same list is worse than one that is merely arbitrary. Quality and language sit on each mirror
  where the source names them or the release name spells them out (`1080p`, `German`, `DL`, and
  a closed list of the other tokens -- nothing is guessed, so `Show.2160.mkv` is not 2160p);
  the hoster is the link's own provider. The group is recomputed at intake, after the online
  check and when links are moved between packages, and it survives a restart: migration `0080`.
  The interface for it is RD-110-19, and the queue still decides its own mirror turns until
  RD-110-20.
- **serienjunkies.org and dokujunkies.org carry no rule, and the reason is written down**
  (RD-110-13, `docs/adr/0013-serienjunkies-a-list-a-rule-cannot-walk.md`). Both were measured on
  2026-09-21 and neither is defended: the series page is an empty Vue shell, but
  `GET /api/media/<id>/releases` hands the whole episode list to anyone who asks — no captcha, no
  cookie, no account — and the reCAPTCHA sits only on the link handout,
  `POST /api/releases/<id>/downloads/<hoster>`, which also wants a fingerprintjs2 browser
  fingerprint this project will not produce. What stops a rule is the rule format itself: a
  template expands to a list's *first* value, no step repeats itself once per element, and a
  search pattern is a literal rather than a template — so a season address cannot become the 39
  requests its episodes need, and a single-release address cannot look up its own release by the
  name it just read. The shipped pack is unchanged and still carries `scnlog` and `downmagaz` at
  sequence 2; `dokujunkies.org` has left RD-110-11's list, because it was never a second pattern.
  `docs/site-rules.md` now names both measured format limits — this one and "this host, no" from
  RD-110-10 — beside each other, so the next rule meets them before it is written.
- **The first release pages are recognised by rule alone** (RD-110-10). The shipped rule pack
  carries two rules: `scnlog.me` and `downmagaz.net`. A pasted release address of either becomes
  one package named after the release, with the hoster links in it and none of the page's own
  links — the scnlog release page carries sixteen of those and no rule reaches them, because
  what a rule reads is the block that holds only the hoster links. Both sites were measured
  alive on 2026-09-21 and each rule carries that date in `checked`, which RD-110-09 will read.
  Addresses on `scnlog.eu` and `scnlog.life`, neither of which resolves any more, are rewritten
  to `scnlog.me` instead of being refused. A release that was taken down refuses with a code —
  `site_rules.page_dead` for a 404, `site_rules.structure` for a page whose links are gone —
  rather than producing an empty package. Every rule is proven against a recorded, sanitised
  answer, so the test run touches no network.

  The third site the job named, `warez-world.org`, **no longer exists**: it and every address
  under it now redirect to a parked "for sale" page, and its two sister domains are a JS bot
  wall and a domain that changed owner. The captcha branch of that site went with it, so no
  shipped rule has one yet; `docs/roadmap/jobs/110-10-die-ersten-drei-seiten.md` records the
  measurement and names the candidate for RD-110-11.
- **Rules take their place in the crawler selection** (RD-110-06). A pasted address is now
  offered to three kinds of source in one fixed order: the crawler plugins that name a service,
  then the site rules, then the generic crawlers. A plugin was built for exactly that service;
  a rule describes a service more broadly; a generic crawler recognises a shape rather than a
  service and stays the last resort. Among the rules the person's own go first, so a shipped
  rule that has gone wrong can be bridged without waiting for a release. Only "this was never
  my page" keeps the search going — a rule that says the page is dead, guarded or changed ends
  it with its own stable code, now translated in all four languages, instead of letting the
  next source produce a second answer about the same page; an address nobody claims stays
  exactly what it was. What a rule found goes through the same acceptance a plugin's answer
  does, and the package name it read travels as the `package_hint` every crawler already
  fills, so the links of one release page form one package named after the page. The
  executor's four ports are implemented in `rd-plugin-host`, where the proxies, the TLS roots
  and the captcha broker already are: the fetcher reports redirects instead of following them
  and connects to the addresses the executor checked rather than resolving the host again,
  which is what keeps the ban on private addresses from being theatre.

- **PEEPLink and AlfaLink entries are resolved** (RD-110-17). `plugins/peeplink-crawler/` turns
  a `peeplink.in` or `alfalink.to` entry address into the hoster links behind it. It is the one
  service of the eight link protectors measured on 2026-09-21 that survived that measurement:
  the other seven are dead, parked, a maintenance page or behind a Cloudflare managed challenge,
  and each refusal stands with its reason in the job file. An entry page is a single `GET` with
  the links in clear text inside its `<article>`; nothing outside that element is read and no
  link on the page is followed. The plugin declares neither `captcha` nor `cookies` and has no
  captcha branch, because the reCAPTCHA, hCaptcha and QapTcha markers those pages carry belong
  to the login and register popups — `name="pwd"` and `type="password"` are on every page of the
  service, the `404` pages included, and reading either as "protected" would make every entry
  unreadable. The two domains write the same thing two ways, as `<a href>` on `peeplink.in` and
  as bare text inside `<article class="articless">` on `alfalink.to`, and both are read, with
  duplicates and the service's own addresses dropped. Four refusals are told apart:
  `peeplink_crawler.entry_not_found` for an identifier the service never knew (`404`) *and* for
  one it has deleted (which answers `200` at the front page, so the address the answer came from
  is checked as well as its status), `peeplink_crawler.entry_empty`, `password_required`,
  `password_wrong` and `site_unreachable`, in all four languages. Seven pages recorded from the
  live service on 2026-09-21 back the tests, with the 24, 1, 9 and 14 links the measurement
  counted. **The access-password branch is built and untested**: no protected entry was
  findable, and an invented page would only show the code agreeing with itself.
- **A rule is executed, not just described** (RD-110-05). `crates/rd-siterules` gained the
  executor: the seven step kinds (`fetch`, `fetch-json`, `regex`, `decode`, `form`, `redirect`,
  `captcha`) run in order over named variables and end in a list of links, a package name and
  the number of requests it took. It runs natively rather than as a Wasm guest, because a rule
  is a record and not somebody else's code — which puts the three bolts in one place instead of
  in every plugin and lets a rule edited in the interface take effect without a signed rebuild.
  The bolts: a rule reaches only the hosts its `match` names plus the address it was given;
  redirects are followed by the executor itself so every hop is checked again, and a hop onto a
  foreign host is refused before that host is asked; and no target may resolve into a private,
  loopback or link-local address, checked against what DNS answered rather than against the
  name, with one non-public address among several enough to refuse. Depth, request count, link
  count, total run time and response size each have a cap and their own stable code; a run that
  returns to an address it already fetched ends as `site_rules.cycle` instead of spending its
  budget; an empty result is `site_rules.no_links` rather than an empty package. Exactly one
  refusal, `site_rules.not_claimed`, means "this was never my page" and lets the crawler
  selection keep looking (RD-110-06); every other code is a statement about the page and is
  reported. **There is no JavaScript interpreter and there will not be one** — base64, hex,
  rot13, percent-encoding and concatenated JavaScript string literals are decoded, and what is
  genuinely a program ends as an honest `site_rules.decode_failed`. `docs/site-rules.md` now
  documents the executor, its steps, its bolts, its limits and every code.
- **The executor keeps `rd-siterules` a leaf** (RD-110-05). The job file had planned `reqwest`
  and `rd-captcha` as dependencies of the crate; that was overturned during implementation, with
  the reason recorded in the job. The executor is defined against four small traits its caller
  supplies — a fetcher that reports redirects instead of following them, a host resolver, an
  optional captcha solver and a clock — so the three bolts and all five limits are proven by
  tests that neither touch a network nor wait, and the HTTP client stays in the crate that
  already has one. The fetcher is handed the addresses the executor already resolved and
  checked, and connecting to those rather than resolving the name again is part of its
  contract: otherwise a record with a time-to-live of zero decides which address is really
  dialled and the whole bolt is theatre. An IPv4 address hidden inside an IPv6 one is decoded
  and judged as the IPv4 address it is — mapped, compatible, 6to4, NAT64 with the well-known
  prefix and ISATAP — while Teredo and NAT64 with a network-specific prefix are refused as
  whole blocks, because decoding them would judge the wrong machine or have to guess. The
  `reqwest` and `rd-captcha` adapters arrive with RD-110-06.

- **An append-only audit log, and an optional trace export** (RD-110-03). Security-relevant
  actions now leave a record that the action itself waits for: a sign-in accepted or refused,
  a session signed out, an API token used, created, re-scoped or revoked, the settings document
  written or reset, a plugin installed or removed, a signing key or a package digest withdrawn
  or reinstated, and the destructive ones — a download, a package, a category or a storage root
  deleted, a configuration backup restored. Each record names who acted (a session, a token with
  its label, anonymous, or the service), the address it came from, what was acted on, how it
  ended, and the trace it belonged to. **No record ever carries a secret value**: a refused
  sign-in names the stage that refused it, never the attempt; a token is its id and its label,
  never its bearer or its digest; every string passes the same redaction the log store's records
  pass, and a canary test holds that line. The log lives in a table of its own with its own
  retention (100000 records or 365 days by default), and it is append-only in three places at
  once — no writer command updates a row, no route writes, edits or deletes one, and the
  database aborts any `UPDATE` on the table outright. Deleting a download, a package, a category
  or a storage root leaves its record behind, and restoring a configuration backup does not
  touch the log. Read it at **Audit log** in the sidebar, filter it by action, outcome, actor,
  target or trace, and export exactly what is shown as newline-delimited JSON. Retention is set
  under *Settings → System*. Both routes cost `api:admin`.
- **A trace context runs through the API, the scheduler, the resolver and post-processing**
  (RD-110-03). Every request takes the caller's `traceparent` when it sends a readable one and
  starts a trace of its own otherwise, and answers with it; every log record written inside that
  request carries the trace id, and so does the audit record of what it did. Queued work derives
  its trace from the job instead of inheriting one, because it outlives the request that queued
  it — so the scheduler's attempt, the resolver call inside it and the package's post-processing
  all share one id for download `42`, across restarts, without anything being stored or passed.
  Paste a trace id into the log viewer or the audit filter to see one piece of work end to end.
- **Traces can be exported over OpenTelemetry** (RD-110-03), off by default. Switch it on under
  *Settings → System*, give it an OTLP/HTTP endpoint such as `http://127.0.0.1:4318/v1/traces`,
  and finished spans are posted as OTLP/JSON in batches. It is written so that it cannot cost
  anything: spans go into a bounded channel that drops rather than waits, a failed batch is
  dropped and never retried, and a collector that is down, slow or gone costs a counter and one
  log line. Every span attribute is redacted before it is encoded, and the request span carries
  the matched route pattern rather than the URL. No OpenTelemetry SDK was added — the exporter
  writes the documented OTLP/JSON wire format directly, for the same reason the metrics route
  writes the Prometheus text format directly.

- **The contract carries a transform description, and the core decrypts the stream**
  (RD-110-33, ADR 0011). A provider that encrypts every file on the client and keeps the key out
  of its own reach could not be carried by any existing world: `resolved-download` is an address
  and headers, with no field key material could travel in, and a header is logged and replayed
  on every chunk request. `rdownloader:plugin` therefore gains a twelfth interface and world,
  `stream-transform`. A plugin answers with the address **and a declarative description of the
  transform** — a named primitive with its parameters — and the host owns the bytes: the cipher
  runs on `rd-http`'s own write path, where the buffer is already allocated at an offset that is
  already known, so there is no second pass over the file and no second copy on disk. Two
  primitives are implemented, the ones one provider needs: `aes-128-ctr`, with the counter block
  built from an 8-byte nonce and a big-endian u64 block index, and `cbc-mac-chain`, a CBC-MAC
  per chunk condensed by a second CBC-MAC and folded to eight bytes. A primitive this build does
  not know is refused when the description is taken in, under `transform.cipher_unknown` or
  `transform.integrity_unknown`, and never falls back. Key material goes into the vault the
  moment it arrives and appears in no log line, no event, no error message and no diagnostic —
  a canary test after the pattern of RD-110-02 holds it to that, and both the key and the copy
  the transform works from are overwritten when they are dropped. The provider's integrity value
  is verified before the part file is promoted, in constant time, since one side of that
  comparison is a function of the key: a mismatch is `transform.integrity_mismatch`, a
  failed attempt that keeps the partial file, never a wrong file presented as complete. The
  checkpoint carries the finished chunk MACs and a fingerprint of the description that wrote
  them, so a continuation with the same description resumes and one with a different description
  starts over; the new crash point `http.after_chunk_mac` covers the window between finishing a
  chunk MAC and recording it. Parallel connections survive where their boundaries line up with
  the provider's chunk boundaries and fall back to a single connection where they do not, rather
  than condensing a value they cannot compute. `api_version` stays at `0.6.0` — the change is
  additive, so every plugin built against the contract without the new world still satisfies the
  world it declares. There is no SDK scaffold for the world yet; `docs/plugins.md` says how to
  write one from any other template, and `plugins/example-stream-transform/` is a working
  reference. MEGA itself is RD-103-02, phase 2: this job delivers the capability, not a provider.

- **The hotfolder poll interval is a setting** (RD-110-31). How often every watched folder is
  reconciled used to be a constant of thirty seconds; it is now `hotfolder_poll_seconds` in the
  settings document, 5 to 3600 seconds, default 30, one value for all folders. It is set on the
  hotfolder tab of *Storage & rules*, above the list, and saved on its own like everything else
  on that tab; each folder row states the interval in force. A value outside the bounds is
  refused as `settings.hotfolder_poll_invalid`, with `min`, `max` and the offending `seconds`
  as parameters. A saved value reaches every running watcher at once — the watcher re-arms its
  ticker in place rather than restarting, so what it has observed about half-written files and
  the digests it has already imported survive the change — and the service starts its watchers
  at the saved value after a restart. A re-armed or freshly started ticker fires one interval
  after the scan that has just run instead of immediately, which also drops the second scan
  every watcher used to make right after its first.

- **KrakenFiles downloads without an account** (RD-103-08). A new bundled resolver plugin,
  `krakenfiles` 0.1.0, with the same protocol logic compiled in as the native fallback. It
  recognises `krakenfiles.com/view/<id>/file.html` and the site's own `embed-video/<id>`
  links, with or without `www.` and in any spelling of the id, checks links through the
  metadata endpoint behind the embed player (`/json/<id>`: the file's name and size, `[]` for a
  deleted file), and downloads through the file page's own form: the page's token, the empty
  `userdata` and `fingerprint` fields exactly as JDownloader and pyLoad post them, and a
  Cloudflare Turnstile answer obtained through the host — a solver service or the person in
  their own browser. Nothing is bypassed and nothing is faked. Every way the site says no is a
  coded, translated failure: `krakenfiles.file_unavailable` for a deleted file,
  `krakenfiles.captcha_rejected` after a second "captcha not valid", `krakenfiles.download_refused`
  carrying the site's own words, `krakenfiles.direct_link_missing` when the site accepts the
  request but hands out no link — never the page itself, so no HTML is ever saved under a file's
  name — `krakenfiles.direct_link_foreign` for a link outside the manifest's download domains,
  `krakenfiles.rate_limited` with a one-hour retry when the direct link answers 403, 404 or 405,
  and `krakenfiles.page_layout_changed` naming what the page lacks. The successful answer of the
  download form could not be measured without a solved Turnstile; its shape follows the two
  reference clients and the fixture is labelled synthetic, so the first real download is the
  measurement that confirms it. Premium accounts, password-protected files and folder links are
  not part of this cut. Thirteen bundled resolvers now, twelve of them with a native fallback.

- **Turbobit and HitFile resolvers** (RD-103-10, RD-103-11). Two signed plugins over one shared
  crate, `plugins/turbobit-common`, because the two are one operator's brands with the same Vue
  shell and the same JSON API under `app.<host>/api` (measured 2026-09-21): a link is checked
  through the operator's documented `links/check`, and downloaded without an account the way
  the site's own page does it — `download/info`, `free/init`, the Cloudflare Turnstile from
  `captcha` answered through the captcha broker (a solver service or the person's own browser
  through the extension), the countdown the site states, `free/prepare`, `free/start`. The link
  that comes out is good for exactly one `GET` from this IP and expires, so it is resolved right
  before the transfer, never probed and never remembered, and a stopped, failed or restarted
  transfer resolves again. With an account (`username_password`; the password only ever reaches
  `app.<host>`, the sign-in answers the login's Turnstile the same way) the premium mirror is
  taken without captcha or countdown; the account check reads `status`, `expiredAt` and the
  day's traffic and claims premium only where it read one. Every refusal the API is known to
  give — a deleted file (`404`), a premium-only file (`premiumOnlyDownload`, refused before any
  captcha is spent), the guest window (`directHit`, handed to the scheduler as an IP block),
  a rejected captcha, a rate limit — ends in one of nineteen `turbobit.*`/`hitfile.*` codes
  in four languages, and no answer of the API — the 2.6 KiB SPA shell included — can ever be
  stored as a file. Every live domain of both brands (`turb.pw`, `trbt.cc`, `hil.to`,
  `hitfile.ru`, …) is claimed and rewritten to the main site, never fetched. The native
  fallback and the component pass one contract test suite
  (`crates/rd-plugin-host/tests/turbobit_hitfile_contract.rs`). Not measured and said so in
  the code: the length of the guest window (assumed 60 minutes), the countdown's value, and
  everything behind the login, which follows JDownloader's reading of the same API.

- **MediaFire** (RD-103-06). Two new bundled plugins. `mediafire` resolves public file links
  — `/file/<key>`, `/download/<key>`, `/view/<key>`, `/?<key>`, `download.php?<key>`,
  `mfi.re` and `app.mediafire.com` — through the documented API (`file/get_info`: name,
  size, SHA-256, privacy and password state, link checks in batches of a hundred) and takes
  the direct link from the file page's download button; no account, no cookie, no countdown.
  A page without a button ends in `mediafire.no_direct_link`, never in the page address, so
  nothing downstream saves the HTML page as the file. Everything the site can answer instead
  has its own code: a deleted file, a private or password-protected one, the malware
  advisory, the per-IP threshold (reported to the scheduler as a wait, never worked around),
  every `error.php?errno=` page JDownloader knows, and a captcha, which is handed to the host
  as reCAPTCHA v2 or answered as MediaFire's own checkbox — once, and refused as
  `captcha_rejected` when it comes back. `mediafire-crawler` lists a public folder
  (`/folder/<key>`) and a key list (`/?key,key`) through `folder/get_info` and
  `folder/get_content`, chunk by chunk, under the same walk limits as the other crawlers; a
  bare `/?<key>` is asked about as a folder first and handed on to the resolver when it is a
  file. The resolver has a native fallback that passes the same contract as the component
  (`crates/rd-plugin-host/tests/mediafire_parity_contract.rs`). Premium is deliberately not
  included: the official route needs an application registered in the person's own MediaFire
  account, which an open-source plugin cannot ship a key for; the job file records the
  decision and the terms-of-service clause it was taken against.

- **The event streams resume after a dropped connection** (RD-110-23). The service keeps the
  last 4096 events of its bus, up to 8 MiB of payload, in memory beside the live channel, and a
  client that reconnects with `Last-Event-ID` is handed exactly the events after that id — no
  event twice, none lost between the buffer and the channel, because both are written under
  one lock. The replay goes through the same filter as the live stream: the capture agent's
  stream still carries only the intake and the redacted captcha count, and a read-only token is
  not handed an account change it could not have seen live. An id the buffer no longer holds —
  it fell out, the service restarted, or it never was an id — is answered with the marker
  `stream.expired` naming that id, never with silence, and the stream runs on live from there.
  Every stream now opens with `retry: 5000`, so the service decides how soon its clients knock
  again: the capture agent keeps the value undoubled instead of climbing from 2 s to 60 s, and
  the browser takes it into its own reconnect. **A resume does not survive a service restart**
  — the buffer lives in the process, and after a restart every id is expired; the web interface
  then re-reads its state exactly as it did before on every reconnect, and the agent writes a
  warning that links which arrived during the outage were not announced. A persistent event
  store is RD-110-03, not this. `useEventStream.ts` no longer closes the `EventSource` on every
  error: while the browser is still reconnecting by itself it is the one sending `Last-Event-ID`,
  and only a stream the browser gave up on — a 401, a wrong content type — is reopened with the
  interface's own backoff.

- **Prometheus metrics and a statistics page that survives a restart** (RD-110-01).
  `GET /api/v1/metrics` exposes the service in the Prometheus text format — queue depth by kind
  and state, a histogram of how long waiting downloads have waited, the current rate, active
  runners, completed and failed transfers, bytes, retries and turnaround by kind and provider,
  accounts by provider, blocked hosts, and free, total and blocked space per storage root. The
  route costs a scope of its own, `api:metrics`: a token holding it reaches this one route and
  nothing else — not the queue, not the statistics, not MCP — and `api:read` does not reach
  it. Label values come from closed sets only (enum names, provider ids, storage-root ids);
  never a URL, a file name, an account label or a host, and a test grows the queue by three
  hundred rows to prove the series set does not move. No metrics crate: the format is a
  hundred lines with a golden test.

  Behind it, the statistics are persistent (migration `0077`): every completion, retry and
  final failure is added to an hourly bucket and to an all-time total in the same transaction
  that changes the download's state, so the figures never disagree with the queue. Hourly
  buckets fold into daily ones after *Keep hourly figures* days and daily ones are deleted
  after *Retention* days (Settings → System; 30 and 365 by default). The sweep works through
  the serialized writer in batches of five hundred rows with a pause in between, so a queue
  write issued during a sweep over thousands of rows is answered before the sweep is over — a
  test in `rd-db` measures exactly that. The all-time totals are never thinned, which is what
  keeps the Prometheus counters monotonic. `GET /api/v1/stats/transfers?range=day|week|month|year`
  serves the folded buckets at `api:read`, and a new **Statistics** page draws them: six
  figures, bytes per hour or day as bars, and the range by kind and by provider. The metrics
  vocabulary and a scrape configuration are in `docs/observability.md`.

- **A structured log store, a log viewer and a diagnostic bundle** (RD-110-02). Every `tracing`
  event the service lets through its filter is now also kept in the database, and it is redacted
  *before* it is stored: the capture layer runs every message and field through the central
  redaction, replaces credential-named fields as a whole, and hands the record to a bounded
  channel without ever blocking the thread that logged. The new *Logs* page filters by level,
  component, stable code, correlation id and text, opens a record's fields behind the chevron,
  says when a page is full and offers the page behind it. Retention is two settings under
  *Settings → System* — 20 000 records or 14 days by default — and the sweep deletes in steps of
  at most 2 000 rows with a yield between them, so a queue mutation never waits behind it. The
  same page builds a diagnostic bundle for a support request: versions, the configuration with
  its credentials scrubbed, the system checks `rdownloader doctor` prints (now shared with the
  command through `rd_api::diagnostics_checks`), and the newest errors — written under
  `<data>/diagnostics/` on this machine only, and only after the inventory was previewed and
  approved with its digest, so an inventory that changed under the approval is refused with
  `diagnostics.preview_stale`. The manifest is deterministic: the same state and clock give the
  same archive. New crate `rd-diagnostics`, migration `0078`, four `api:admin` routes under
  `/api/v1/diagnostics/`, and `docs/diagnostics.md`.

- **The provider account's label is translated** (RD-110-28). What stands next to an account
  after a check — *Signed in as alice*, *Premium until 2027-01-01*, *3 cookies loaded*, *the
  subscription was not checked* — now appears in the language of the interface. The contract
  changes for it: `account-status.label` is no longer free English text but a list of
  `label-part { code, params, message }` records, the shape a `failure` already travels in, and
  the interface translates every part in the active language, then in English, prints the
  `message` text only for a code no catalogue knows, and shows the code itself before it shows
  nothing. A part without a code is refused by the host as `plugin.account_label_invalid`.
  Eight `plugin.account.*` codes cover what every plugin says the same way and are built once in
  `plugin-common`; `premiumize.account.fair_use` and `1fichier.account.access` are the two
  provider-specific ones. Every bundled resolver sends the new form, the provider's name and
  the word "premium" left the label because the row already shows both, and `api_version`
  stays at `0.6.0` under the no-installed-base rule, like the captcha kinds before it.
  `translateServerMessage` gained the middle rung of the documented chain on the way: a plugin
  that ships only `en.json` now shows its English catalogue in a German interface instead of
  the backend's raw text. `POST /api/v1/accounts/{id}/test` returns `label` as that list.

- **Two captcha kinds the plugin contract did not know: click-point and CutCaptcha** (RD-110-15).
  `captcha-challenge` gains `click-point(image-challenge)` — a picture answered by clicking one
  spot in it — and `cutcaptcha(cutcaptcha-challenge)`, which carries both keys the solver task
  needs, the widget's own and the page's misery key; a single site key could not have expressed
  it. The answer to a click is a coordinate, and `captcha-solution { token }` cannot carry one,
  so the interface gains `solve-challenge`, which returns `captcha-answer`: a token or a
  `click-point { x, y }` in pixels of the image as the hoster served it. `solve-captcha` and its
  answer stay exactly as they were, and refuse a click-point challenge with `captcha.answer_shape`
  before anyone is asked. Both kinds were cut for filecrypt.cc, the most used link protector in
  the German-speaking web; the measurement of 2026-09-21 then found that site unreachable for a
  different reason entirely (RD-110-16, ADR 0010), so they stand here as what the remaining
  protectors of RD-110-17 are measured against rather than as the key to that one site.

  In the web interface a click-point captcha looks like an image captcha and is answered by
  clicking the picture: the first click marks, another moves the mark, and the button sends it —
  a wrong spot fails the download, which is why nothing is sent on the click itself. The
  interface translates the click into the image's own pixels whatever size it rendered the
  picture at, and the coordinate reaches the plugin through a new route,
  `POST /api/v1/captchas/{id}/click`. Text for a click, or a click for text or a widget, is
  refused with `captcha.answer_shape` while the challenge keeps waiting. A solver service
  answers a click as a `CoordinatesTask` and a CutCaptcha as a `CutCaptchaTaskProxyless`.

  A CutCaptcha is answered by a solver service alone: no browser rDownloader drives can read
  its token, so it is neither queued for a person nor offered to the extension, and without a
  configured service the download fails at once with `captcha.cutcaptcha_needs_solver` instead
  of waiting out the manual timeout — a coded refusal, never an empty token.

  The four existing cases are untouched and every bundled plugin builds unchanged. What the
  change is *not* is binary-additive: Wasmtime checks a variant for its exact number of cases,
  so a component built before it no longer instantiates. `api_version` stays at `0.6.0`, for the
  reason written into the job file and now into `docs/plugins.md` — there is no installed base,
  and the bundled components are rebuilt and re-signed for every release; the rule that will
  move the version once there is one is written down beside it. The contract is mirrored
  byte-for-byte into all eleven `sdk/templates/*/wit/` copies, and `plugin-common` gains the
  two cases, `CaptchaAnswer` and `PluginHost::solve_challenge`.

- **A release page is recognised by a rule, not by a plugin — the format and its carrier
  (RD-110-04).** JDownloader needs 148 lines of Java for `scnlog.me`, and nearly all of it is an
  address, a regular expression and an error text. `crates/rd-siterules` now holds that as data:
  a rule names the hosts and path patterns it claims, the steps that turn a page into links, where
  the package name comes from, the domains the service once had — rewritten to the living one
  rather than refused — a real address for the self-test and the date the service was last
  measured alive. The project's rules ship as one signed pack under a trust root of their own
  (`Role::SiteRules`, domain `rdownloader.site-rules.v1`), so a signature over a tool manifest
  does not verify as a rule pack and the reverse; the pack is verified at start, and one with an
  unknown format version, a withdrawn digest or a single invalid rule is refused as a whole with
  a stable code rather than read up to the fault. A person's own rules are stored beside it
  (`site_rules`, migration `0076`), survive a restart, announce `site_rule.changed`, and may never
  take a shipped rule's id — the catalogue refuses the collision instead of covering the shipped
  rule in silence. `rdownloader site-rules sign` and `plugin keygen --role site-rules` are the
  publishing side. The pack shipped today is empty and signed; the executor is RD-110-05, the
  rules are RD-110-10 onwards, and `docs/site-rules.md` is the format with a commented example.

### Changed

- **The collapsed sidebar keeps the logo, and its gear says what it holds** (RD-110-32). On
  the 64 px rail the header showed only the collapse switch — the wordmark had yielded to it,
  because the header keeps 32 px between its paddings and holds exactly one control. Now the
  logo is that control: the 32 px mark is the face of Nuxt UI's own
  `UDashboardSidebarCollapse`, with the tooltip and the accessible name *Expand sidebar* in
  all four languages, and it is the same element in both states, so a keyboard user who
  presses it keeps their focus on it. The footer's language-and-theme menu wore the theme
  icon and was named *Theme*, promising half of what it held; it now wears a gear
  (`i-lucide-settings-2`, distinct from the navigation's settings icon) named *Language and
  theme* (`common.preferences.language_and_theme`, de/en/es/fr). Expanded, nothing changes:
  wordmark, plain switch, two selects. Covered in `ControlRoomLayout.test.ts` and the new
  `PreferencesFooter.test.ts`, each with an axe run over both states.
- **GoFile gets a documented No-Go instead of a resolver** (RD-103-07). The feasibility
  measurement of 2026-09-21 found that gofile.io's only account-free listing route is the
  site's own bot gate — an `X-Website-Token` from an obfuscated, server-rotated script whose
  misuse the operator answers with an IP ban — and that the documented `GET /contents` API is
  Premium-only, declared BETA, and not measurable without a paid account. Recomputing the
  token (JDownloader's way) or hard-coding it (pyLoad's, dead since 2024) is the anti-bot
  bypass the job excludes. `docs/adr/0005-gofile-no-go.md` records the decision and the two
  conditions that reopen it; the measured facts are in the job file.

- **filecrypt.cc gets no crawler** (RD-110-16, ADR 0010). The feasibility measurement of
  2026-09-21 looked at a container page rather than the domain root, and found a service the
  job had not been cut against: all nine probes — three public container identifiers on
  `filecrypt.cc`, `.co` and `.to` — answer `200` with the same "Security Check" interstitial,
  a proof-of-work widget that posts back `pow_id`, `pow_nonce`, `pow_elapsed`, `pow_pauses`,
  `pow_data` and `pow_x`. The SHA-1 work at difficulty 20 is mere cost; `pow_elapsed` and
  `pow_pauses` are evidence about how a human used the widget, and `pow_x`/`pow_data` come out
  of two obfuscated browser-signature modules. The DLC route the job calls the stablest
  answers `200` with zero bytes, the per-line route needs identifiers only the passed gate
  yields, the password field is commented out of the delivered page, the gate does not lift
  across a session, a hidden `/Link/1` trap waits for whatever follows it, and `robots.txt`
  disallows every path. Passing it means faking a browser fingerprint and inventing
  interaction evidence, which RD-110-16 and RD-110-14 both exclude, and which the captcha
  contract cannot express in any case — it is no image, no sitekey, nothing a solver service
  or the extension could answer. The job is `Blocked/No-Go`;
  `docs/adr/0010-filecrypt-no-go.md` records the four routes considered and the three
  conditions that reopen it. Nothing changes for the route that works: open the folder in a
  browser and press Click'n'Load, which the capture agent already receives, or import a DLC
  file downloaded by hand. The click-point and CutCaptcha kinds added in RD-110-15 stay in the
  contract as what RD-110-17's remaining protectors are measured against.

- **The remaining link protectors, measured: one crawler, seven refusals** (RD-110-17). The
  survey of 2026-09-21 looked at real container pages rather than domain roots, and found the
  class mostly gone rather than mostly defended. **All nine domain roots answer `200`, and five
  of the eight services are dead anyway** — the lesson filecrypt taught, confirmed at a second
  sample. `peeplink.in` and its sibling `alfalink.to` are the one Go: four real entries
  (`6.651`–`11.147` bytes, 1 to 24 hoster links) hand over the links in plain HTML on a single
  `GET`, with no proof of work, no captcha and no session — the reCAPTCHA, hCaptcha and QapTcha
  markers on those pages belong to the login and registration popups — and an unknown
  identifier answers `404`. They get a crawler plugin, planned in the job file down to manifest,
  message codes and the seven recorded fixtures. The seven refusals share no cause:
  `linkcrypter.xyz` and `linkguard.org` are GoDaddy parking landers that answer every path with
  the same 114-byte redirect, `multi.hotshare.biz` answers every path — `robots.txt` included —
  with one 617-byte maintenance page and its sibling `uploadmagnet.com` no longer resolves,
  `linkcrypt.ws` redirects its entire domain to the ad page `ww17.linkcrypt.ws` including three
  real container identifiers, `get-to.link` answers `cf-mitigated: challenge` with `403` on
  every path but the root, `dlcrypt.net` has an open route — its own page carries the `/views/`
  link that skips the captcha — but all twelve publicly known containers from 2024-04 to
  2026-03 end at "Link Removed or Expaired" with zero links, and `lnk.snahp.eu` lives behind a
  gate the contract could answer (a password, or a 100×40 GD image captcha) but has no container
  identifier in any public source, its forum being behind Cloudflare. No shared ADR: the class
  did not end as a No-Go, and a single reason would have been invented. Every measurement,
  command and byte count is in `docs/roadmap/jobs/110-17-die-uebrigen-protektoren.md`.

- **Five sites behind a browser gate get no rules** (RD-110-14, ADR 0012). The feasibility
  measurement of 2026-09-21 looked at real content pages rather than domain roots, and found
  one gate five times: `serienfans.org`, `filmfans.org`, `newalbumreleases.net`,
  `multipaste.org` and `zpaste.net` all answer `403` with `cf-mitigated: challenge` on every
  path of their own application — the root, a real release or paste page, the site's own
  `/api/v1/` endpoint, the feed, the sitemap, and `robots.txt` too, so the operator cannot show
  a non-browser even its own crawling policy. The body is always Cloudflare's managed-challenge
  interstitial (`cType: 'managed'`, `Enable JavaScript and cookies to continue`, a script from
  `/cdn-cgi/challenge-platform/`), which is a program to run rather than a document to read. It
  sets **no** cookie, so a session handed over from the person's own browser has nothing to
  carry — and `FetchRequest` has no field for a cookie or a header in any case. No mirror, no
  ungated variant, no documented route: `www.`, IPv4 only, `http://` and `HEAD` all end at the
  same refusal. Passing it means running the challenge script or imitating a browser's
  fingerprint, the line ADR 0010 drew for filecrypt. Nothing behind the gate was ever seen,
  which is a second and independent reason: a rule describes a response, and there is no
  response to describe. All five jobs' verdicts, every command and every byte count are in
  `docs/roadmap/jobs/110-14-seiten-mit-bot-schutz.md`; `docs/site-rules.md` gains "No browser,
  either" beside "No JavaScript interpreter". The error code for the case was already there and
  is confirmed rather than changed: `site_rules.blocked`, in all four catalogues, which is what
  RD-110-09's `blockiert` state is fed from. The extension question — may it read a gated page
  in the person's browser and hand the links over — stays open, exactly as ADR 0010 left it.

- **WeTransfer gets no resolver** (RD-103-12, ADR 0009). The feasibility measurement of
  2026-09-21 found the public API retired by the operator on 2022-05-31 with
  `developers.wetransfer.com` answering `503`, the terms of service (last changed
  2026-08-03) forbidding "automated or manual data extraction, gathering or scraping methods
  in connection with the Service" and incorporating the service into another product, the
  only remaining route being the Next.js frontend's own `api/v4` XHR traffic, the recipient
  login a browser-session token the SDK has no credential mode for, and no live transfer to
  verify against because every transfer expires within a week. The job is `Blocked/No-Go`;
  `docs/adr/0009-wetransfer-no-go.md` records the routes considered and what would reopen
  the case.

- **FileFactory gets no resolver** (RD-103-09, ADR 0007). The feasibility measurement of
  2026-09-21 found the official `api.filefactory.com` accepting no TCP connection and no
  longer used by any reference client, the website rebuilt in April 2026 around an internal
  Next.js web API that changed three times in a year, an account path that runs over a
  WebSocket the plugin sandbox does not have behind a login captcha the reference client
  bypasses with a forged cookie, and no live public file to verify the guest route against.
  The job is `Blocked/No-Go`; `docs/adr/0007-filefactory-no-go.md` records the routes
  considered and what would reopen the case.

- **The desktop tray's state machine is tested on Linux.** When the icon swaps to the busy
  mark, when "Open rDownloader" is enabled, what the status item and the tooltip say: these
  were decided inside `tray.rs`, on `tray_icon::MenuItem` handles, in a file no Linux check
  compiles and CI only builds. They now live in `crates/rd-capture/src/tray_state.rs`, which
  names no type from `image`, `tao` or `tray-icon`: it describes the surface a tray is built
  with and hands back, per event, exactly which parts to redraw, so the rule that the icon is
  only touched on a real change — redrawing it every poll flickers on Windows — is now a test
  rather than a comment. `tray.rs` keeps the handles and applies the result. Sixteen transition
  tests, `rd-capture` from 138 to 154; nothing the tray shows has changed, including the
  tooltip's habit of not following the server state, which is now pinned by a test so that
  changing it is a decision (RD-110-24).

- **The LinkGrabber's link rows and the subscription rows follow the queue's width rule.** The
  rule RD-109-30 wrote down — the name is the row's first obligation — had been applied to the
  download rows and nowhere else. A link row was its own flex line with two loose icon buttons,
  a spelled-out hoster and a dash where no size was known, and the name was the first thing
  squeezed; a subscription row carried five icon buttons, two worded badges and a `Disabled`
  badge beside the switch that already said so; an indexer hit's title was the one item allowed
  to shrink to nothing. The link row now sits on the shared `.queue-row` grid, so it wraps to two
  lines below 560 px exactly as the queue does, with enqueue and the dots beside it and rename,
  the MP3 switch and delete under them, the hoster or the variant picker in the metadata cell and
  the size only where it is known. The subscription rows keep a wrapping line — their column is
  half the panel and the grid's tiers are measured for the whole of it — but the name now claims
  the 200 px the accounting reserves before anything else takes the line: the chevron moves in
  front of the name, kind and mode become glyphs that carry their word as accessible name, the
  switch is the row's state, check now and the dots are the two controls beside it, and why a hit
  was skipped stands in a line under the row instead of in it. The link row also gained a single
  root, so the frame the view had been handing it since RD-106-12 actually arrives — a template
  with several roots cannot take a class from its parent, and Vue had been dropping it silently.
  `design.md` records where each list stands on the grid and why (RD-110-27).

- **Release 1.1.0 is planned, and two of its jobs close without code.** RD-110-25 (the dead
  `captcha-webview` profile) and RD-110-26 (share passwords already stored in URL fragments) had
  carried a dated "no action needed" appendix since 2026-09-20 while their status line still read
  Open; both now say `Blocked/No-Go` for the reason the appendix gives — there is no installed
  base to migrate. RD-110-22 is settled through RD-110-23, the real event resume, rather than
  through a candidate counter on the capture surface. The working file is
  `docs/roadmap/jobs/110-00-release-koordination.md` (RD-110-00).

### Fixed

- **What no crawler resolves never becomes an HTML file in the download folder** (RD-110-07). A
  crawler — a plugin or a site rule — may answer with any address, and nothing checked what came
  back: a rule whose container pattern reached one element too far put the board page itself into
  the review list, and the queue stored that page under the name of an episode. Every address a
  crawler returns now passes a verdict before a row exists for it. One that a resolver, a transfer
  backend or a recognised provider claims goes through untouched and is deliberately **not**
  probed — the resolver produces the file, and a HEAD against a hoster's landing page would prove
  nothing. One nobody claims is probed, and is kept only when
  `rd_http::ProbeResult::looks_downloadable` says the response is file content; that is the same
  single judgement the transfer engine applies mid-download, asked rather than rebuilt, so
  `Content-Disposition: attachment` with `text/html` still counts as the file it is. An address
  whose probe *answers with a page* is refused with the stable code `collector.crawl_not_a_file`
  and counted; one whose probe gets no answer at all — a timeout, a refused connection — is kept
  unconfirmed, because RD-101-06 already decided that a failed check must not cost a link more
  than a proven-dead one. The intake answers with how many addresses the crawlers found and how many were
  dropped, and the LinkGrabber says "12 of 40 found links were not files and were dropped" instead
  of looking like an empty page. The online check learned the same judgement for a direct link and
  gained the candidate state `unresolvable` for it — shown in all four languages, told apart from
  *offline* and from *not checkable*, carrying its reason in the badge's title, and the one
  candidate state that cannot be queued. A link that is merely *not checkable* stays queueable, as
  RD-101-06 decided; an indexer link re-routed to its NZB or torrent import keeps its place, since
  a small XML document is not meant to be downloaded at all.

- **An intake that arrived while the capture agent was disconnected is announced after the
  reconnect** (RD-110-22, through RD-110-23). The agent already kept the id of the last frame
  it saw and did nothing with it; now it reconnects with that id as `Last-Event-ID`, and the
  service replays the `collector.intake` it missed, so the toast still comes. A reconnect with
  nothing missed announces nothing — a notification on every reconnect would have been worse
  than the loss it replaces.

- **The indexer drawer shows the hits that arrive after every one was rejected** (RD-110-30).
  The badge counted them and the group stayed empty until the page was reloaded by hand. Two
  causes, both in `web/src/stores/subscriptions.ts`. A list was re-read when the summary's
  pending count disagreed with the page's — a symptom, blind to a dismissal and a new hit in the
  same window — and it kept the page it had last been read at: every hit rejected from page two
  left the page at two, and the hit that arrived afterwards fit on page one, so the list read an
  empty page under a badge that counted it. A list is now invalidated by the cause of its
  change — a finished poll that created rows for its subscription, a review count that moved
  between two reads of the summary, or a stream marker saying events were lost. An open group
  re-reads at once, a closed one on its next expand, and a page that no longer exists is read at
  the last one that does. A finished poll that created nothing, and an event about another
  subscription, read no list.

## [1.0.9] - 2026-09-20

Forty-five jobs, and the milestone's own premise did not survive them. It was cut as "The
Extension and the Way In" from an audit of `extension/` and `crates/rd-capture/`, and what the
audit had derived turned out, again and again, to be wrong in the detail that mattered. The
browser extension's manifest had said `0.1.0` since it existed, so no store could have accepted
an update and no two builds could be told apart. A captcha window the desktop agent still carried
was measured, in a person's hands, not to work at all — and because it opened for every waiting
widget without asking whether the extension had the case, it reached the person first and the
working path never did. It is gone, along with `wry` and 471 lines of lockfile.

The release's sharpest finding was not in the audit. A 1fichier download reported `Completed`
with a checksum, on 1.1 KiB. The file was the hoster's favicon: the page said "all free guest
slots are currently in use" in wording the parser did not know, the fallback then took the first
link on the page that pointed at a content host, and nothing compared the 1150 bytes it fetched
against the 405.44 MB the same page had printed. A size the server contradicts is now a failure
with a code, not a green tick.

Three more messages were found saying things nobody had established: an account check that
reported "Premium active" without reading any subscription — in three plugins, and in the SDK
template every third-party author copies — a captcha flow that reported success it could not
verify, and one sentence, "Check result missing", standing for three different situations at
once. Each of them now says what it knows.

The platform gate that keeps a GUI stack out of the headless Linux build was rewritten twice:
first from string matching to parsing the manifest, which immediately exposed two ways past it
that had always been open, then extended with a check of the resolved dependency tree, because a
window toolkit arriving behind an innocent name is invisible to any manifest.

### Added

- **The browser extension's popup names its version.** Three buttons and nothing else
  meant the only way to tell a temporarily loaded build from an installed one was
  `about:addons` — and on 2026-09-20 that cost a whole measurement run, which was made
  against a stale 0.1.0 copy and drew a conclusion that had to be withdrawn. The line reads
  the manifest rather than restating the version, so there is no second place to keep in
  step, and a build without one shows nothing instead of half a line (RD-109-46).

- **The sidebar can be collapsed, by button or with `0`.** It was already collapsible in every
  respect but one: `ControlRoomLayout.vue` threaded `collapsed` through the header, the
  navigation and the footer, and nothing on screen or on the keyboard could change it — the only
  way in was to drag the resize handle all the way to the edge. The switch is Nuxt UI's own
  `UDashboardSidebarCollapse`, placed in the sidebar header rather than built a second time, and
  `0` joins the shortcut catalogue, which means it also appears in the `?` help and is suppressed
  while a field or a dialog holds the focus like every other single-key shortcut. On the 64 px
  rail the wordmark yields its place to the switch, because a control that vanishes when used
  leaves no stated way back. The choice is the session's: it survives every page change, and the
  names — *Collapse sidebar*, *Expand sidebar*, and the help line — ship in all four languages
  (RD-109-31).
- **A granted replay approval is visible in the LinkGrabber and can be taken back.** A link whose
  captured request was approved carries a shield badge in its row, and its details panel names the
  moment of the approval next to a confirmed *Withdraw approval*. This closes a real gap rather
  than decorating one: the enqueue asks exactly once — a candidate that already holds an approval
  is queued without the dialog — so an approval left behind by a cancelled duplicate prompt, a
  dismissed bulk dialog or a failed enqueue would have sent the captured credentials on the next
  attempt with nothing on screen saying so, and deleting the link was the only way out.
  `DELETE /api/v1/collector/candidates/{id}/replay-consent` had been wired since the flow was
  built and had never had a caller (RD-108-19).
- **A job running at a provider is visible, answerable and can be ended — the confirmed remote
  deletion included.** RD-107-06 built the eleventh world and RD-108-03 the flow that drives it,
  and between them they touched no REST surface at all: `remote_job.changed` was announced into a
  bus nobody listened on, a job parked in `awaiting_choice` waited for an answer nothing could
  deliver, and `discard` was a contract call nothing reached. Five endpoints close that — list,
  hand a magnet to an account, answer the selection, delete at the provider, remove from the list
  — with a card beside the accounts they run on showing the stage, the progress the provider
  measured and the entries a job is offering, updated off the event rather than a poll. Only
  entries the job itself offered can be picked; anything else is dropped rather than forwarded.
  **Deleting at the provider is kept apart from removing the row, which is the whole point.** It
  is the only path in the application that reaches a plugin's `discard`, and it needs the
  confirmation as a value in the request as well as a dialog that names the account it reaches,
  so a client that skipped the dialog deletes nothing; the server refuses the rest under
  `remote_job.not_confirmed`. What it did is recorded rather than erased — the row stays in
  `discarded`, still naming the job the provider knew, so an account that lost a torrent can be
  shown which request removed it and when. `DELETE /api/v1/remote-jobs/{id}` is the other act and
  calls nothing at all. Texts in all four languages; `design.md` now carries the rule that a
  destructive action reaching past this machine says where it reaches and is never the same
  button as the local one (RD-108-04).
- **A password-protected Nextcloud or ownCloud share is now loaded, not only listed.** RD-107-05
  could open such a share and read its listing, but the addresses it handed back carried no
  credential, so every download from one failed — the defect it reported against itself. Two
  decisions closed it, both recorded in `docs/plugins.md`. The password still reaches the crawler
  as the fragment of the pasted address (`…/s/<token>#password`), because `crawl` runs in the
  sandbox under a fuel and time budget and has nobody to ask, and because a fragment is the one
  part of a URL that is never sent to a server. And a protected share is deliberately **not** an
  account: an account is a per-provider login with a life of its own, while a share password
  authenticates one share and means nothing anywhere else — so it becomes an **authentication
  profile**, the shape that already exists for a credential scoped to a host and a path prefix,
  with its secret held encrypted in `rd-secrets` as a `vault://` reference. The crawler puts only
  the *user name* the public endpoint fixes (`anonymous`, or the share token on the legacy
  endpoint) into the addresses it returns; the host lifts it out, **drops any address whose
  userinfo carries a password**, and mints one profile scoped to the longest path every found
  file shares — never to a whole host. The queue then picks that profile up by matching the
  address it is about to fetch, so the online check and the transfer send the same
  `Authorization: Basic` the listing used. The password appears in no candidate address, no REST
  answer, no database column that is not the vault and no log line: the one line that prints a
  crawled address now prints it without its fragment. A wrong password still ends with
  `nextcloud_crawler.password_wrong`, in all four languages, rather than as an empty folder
  (RD-108-07).
- **The eleventh world has a scaffold, so somebody else can write for it.** `remote-job` shipped
  in 1.0.7 with a contract, a host wrapper and one plugin, and in 1.0.8 with the sweep that drives
  it - but `sdk/templates/` held ten worlds and `plugin new --type remote-job` did not know the
  type. The only way to start was to copy `plugins/realdebrid-torrents/` out of this repository by
  hand, which made ADR 0003's central promise - that Premiumize, AllDebrid and Debrid-Link are one
  plugin each and no contract change - something nobody could act on. `sdk/templates/remote-job/`
  is that template now, and `rdownloader plugin new --type remote-job --out DIR` writes it. Like
  the other ten it is a working plugin of its type rather than a sketch: the seven short calls,
  none of which waits; the content key derived locally in `src/source.rs` from a magnet or a
  bencoded container, which is the whole reason the host's duplicate guard can exist; SHA-1
  written out in `src/digest.rs` for the reason the oauth scaffold writes SHA-256 out, so the only
  dependency stays `wit-bindgen`; and the provider's state vocabulary mapped in `src/reply.rs`,
  where the arm that matters is the last one - an unknown state waits rather than failing a job
  that is going perfectly well. It carries the contract byte for byte, builds outside this
  repository, packages, passes conformance and runs twelve unit tests before a line of it is
  changed. The CI step that builds a scaffold outside the workspace now names `remote-job` beside
  the other ten, so a contract change that breaks it is caught here and not by a third party
  (RD-108-05).

### Changed

- **The browser extension declares the versions it is actually supported on.** The Firefox
  floor was `115.0`, an ESR guess nothing had ever been measured against, and Chrome had no floor
  at all. They are now `156.0` and `153` — the versions in use — so a browser below them refuses
  the install instead of running untested. Both are named constants the build and its test share,
  and a test holds that neither target's key reaches the other manifest (RD-109-47).

- **The rules the capture agent's tray follows are measured on Linux, where the tray does not
  exist.** `tray.rs` sits behind `cfg(any(windows, target_os = "macos"))`, so a Linux `cargo
  check`, `cargo clippy` or `cargo nextest` said nothing whatsoever about it — and two things in
  there were not tray code at all. The activity badge drawn over the icon is arithmetic on an RGBA
  buffer, and it now lives in `crates/rd-capture/src/icon.rs`, which carries the tray's gate plus
  `test` the way `status.rs` already did: where the badge sits, how it scales, which pixels it
  covers, and its refusal to half-paint a buffer whose length is not the image the caller claims.
  Decoding the shipped PNG and `Icon::from_rgba` stay in `tray.rs`, because `image` and
  `tray-icon` are two of the four crates the Linux agent must never link. The health poll's
  classification moves the same way: `status::health_probe` reduces one request to healthy,
  unhealthy or silent, and `status::HealthWatch` carries the only thing the poll remembers between
  requests — whether the service has ever answered, the one-way latch that turns the start-up
  grace off for good. The sequence it governs, silent then healthy then silent again, could not be
  played through anywhere before; it can now, and so can the poll interval's relation to the grace
  it is measured against, which is why `HEALTH_INTERVAL` moved next to `HEALTH_GRACE`. Nothing
  visible changed: the same status texts, the same five-second poll, the same ninety-second grace.
  `rd-capture` goes from 100 tests to 112 (RD-109-13).
- **The guard over the platform gate judges the manifest instead of its wording, and the line it
  holds is written down (RD-109-14).** The check that keeps a window toolkit out of the headless
  Linux capture agent matched literal strings — `"\n<crate>.workspace = true"`, against a manifest
  cut in two at one known table header. Writing the same dependency as `open = { workspace = true }`
  or `image = "0.25"` in the ungated `[dependencies]`, while leaving the gated entry in place, passed
  both of its assertions in silence; so did a `[build-dependencies]` entry, a second `[target.…]`
  table, and a dependency renamed with `package = "tao"`. It now parses `Cargo.toml` as TOML, decides
  every target table by evaluating its `cfg` against a Linux configuration rather than comparing it
  to a string, and turns the rule around: every dependency a Linux build reads has to be named with
  the reason it carries no window stack, so a crate added tomorrow is refused until somebody writes
  that reason instead of being waved through until somebody extends a list of forbidden names. A
  `cfg` the check cannot decide — a feature predicate, an unknown key, a malformed expression —
  counts as reaching Linux. The same treatment for the source side: module declarations are found at
  any indentation and in any file under `src/`, and the `cfg` in front of them is evaluated, not
  matched. And the line is now stated where it actually runs: `arboard` is ungated and pulls `x11rb`
  and `wl-clipboard-rs` behind it, so the Linux agent does link X11 and Wayland *client* libraries —
  a clipboard client is not a window toolkit, it needs no GTK, and with no display server reachable
  it returns an error instead of taking the agent down. Its feature set is checked too, because
  `image-data` would pull the `image` crate in behind a name the list has a reason for.

- **The headless line of the capture agent is held against the tree the build actually resolves,
  in CI (RD-109-37).** RD-109-14 named four ways around its own guard and closed none of them; the
  serious one was that no parse of `crates/rd-capture/Cargo.toml` can see a window toolkit that
  arrives *transitively* — behind a new dependency of `rd-core`, behind a feature flipped on
  somewhere in the workspace, behind a `[patch]` that bends a harmless name onto a fork with GTK
  underneath. `scripts/check-capture-linux-tree.sh` asks the resolver instead: `cargo tree` under
  the same features, the same `[patch]` tables and the same `.cargo/config.toml` the build uses,
  and every one of the 344 packages in the Linux tree held against a named list of window, widget
  and rendering stacks. The offenders are reported shallowest first with the chain that pulled the
  shallowest one in, because "gtk-sys reaches the Linux build" is only actionable once you know
  who to talk to about it. It runs on the Linux leg of the `rust` job in CI and in
  `scripts/check.sh` right after `cargo fmt` — resolve only, no compile — and a case in
  `platform_gate` keeps the two hand-maintained lists from drifting apart. That also settles the
  second and fourth of the four ways, which differ from the first only in how the crate gets in.
  The third stays deliberately open: window code written straight into an ungated file is caught
  by the Linux compiler, categorically, and CI compiles this crate on Linux — a text scan for it
  would be the weaker guard and the string matching RD-109-14 removed. `rd-capture` goes from 137
  tests to 138.

- **One section issues the 1.0.8 job numbers, and a job status is one of five words.** `docs/roadmap.md`
  carried two sections headed `## Milestone 1.0.8`, and both announced `108-01` and `108-02`. The
  second was the older one — it was written when the milestone held exactly those two jobs, and the
  larger section was later laid above it without folding it in — and both meant the same two jobs,
  so nothing was renumbered: the duplicate heading and its `Jobs:` lines are gone, its text stands in
  the one section under the merged title, and the issuing line now also names `108-31`, which no
  section had assigned. The doubled number space was what produced the miscounted inventory row.
  Separately, 32 job files carried `Status: Done`, a word no document defines; it marked nothing
  `Implemented` does not — its files carry open acceptance boxes exactly like the others — so they
  are normalised, along with the 64 index cells and four statuses that stood outside any vocabulary.
  `docs/roadmap/jobs/README.md` now names the five words in use and the two forms around them, its
  catalog lists all 294 files exactly once (five were missing from a listing, the way `RD-108-31`
  was), and its recounted totals still read 290 (RD-109-33).
- **The capture agent's `main.rs` is an entry point again, not the whole agent.** It had grown to
  1967 lines — four unrelated subjects and one test module spanning all of them — and was the
  worst offender against the 500-line rule in the repository. The four subjects the audit named
  now sit in their own modules beside their own tests: `cli.rs` (the clap types and the defaults
  a bare invocation runs with), `commands.rs` (the one-shot subcommands), `clipboard.rs` (the
  clipboard loop, its decided-versus-retried refusal rule and the backoff) and `status.rs` (the
  tray's server-state rules, next to `activity.rs`, with the module declaration carrying the
  `cfg(any(windows, target_os = "macos", test))` gate that keeps them tested on Linux). The
  background-task notices, the supervision wrapper and the Click'n'Load binding — none of which
  existed when the job was written — went to `supervision.rs` for the same reason. `main.rs` is
  down to 484 lines and holds the entry point, `dispatch`, the two exit-code error markers, the
  agent loop and the platform-gate assertion. **Nothing changed behaviour:** the suite ran 125
  tests before the move and 125 after, every one of them the same test (RD-109-12).
- **The pre-resume refresh of an expiring download link stays deliberately transient.** RD-108-12
  had left it open whether a renewed transfer address should be written back into the stored
  request template, and the half-built chain for doing so was kept rather than removed. It is
  answered now, against persistence: neither refresh source states how long its address is good
  for — a resolver's `resolved-download` has no expiry field, and the plain re-request hands back
  a bare redirect target — so a stored address would be a bet on an unknown lifetime. A dead
  single-use link would cost a real failed attempt, burn the one reactive refresh reserved for a
  genuine 401, and need renewing afterwards anyway, where discarding it costs one probe. The
  durable address is the link the person added, which every attempt re-resolves from, and a
  signed URL's deadline is read back out of the URL. The last remnants of the abandoned chain —
  the unread `refreshed_at` and `refresh_count` on the stored template — are gone, the reasoning
  is in the code beside the refresh result, and a test pins the second resume against a
  write-back being reintroduced. Existing rows keep working; the fields are simply ignored on
  read (RD-108-17).
- **The capture agent percent-encodes with `url` instead of its own encoder (RD-109-16).** The
  hand-written one was kept by a comment saying a dependency for one function was not worth the
  supply chain — but `url` is already a direct dependency of the crate and is used throughout, so
  nothing was being avoided except a second, less-tested implementation of the same algorithm.
  The scheme handler's form body now goes through `form_urlencoded::Serializer`, which is the
  encoding that belongs to the declared content type; a space travels as `+` rather than `%20`
  and the receiver resolves it back, which is what the new test checks.
- **`rdownloader://open` no longer claims to take a `.torrent` (RD-109-03).** The extension was
  in the allowlist and in both doc comments, but the import path uploads the content as
  `application/x-nzb` to the NZB endpoint, so such an address parsed cleanly and then broke. It is
  refused up front until there is a torrent import path with its own acceptance.
- **The capture token can be handed over without putting it in `argv` (RD-109-04).**
  `rdownloader-capture configure --token-stdin` reads it from standard input; `--token` still
  works but is no longer the documented way, because an argument is readable through `ps` and
  kept in the shell history. `README.md` shows the new form.
- **Pairing with a non-loopback address now requires `https`, or an explicit
  `--allow-insecure-service` (RD-109-04).** The capture token is a long-lived bearer credential
  that rides on every poll and for the whole life of the event stream, and nothing used to stop
  it going out in the clear. Existing pairings are untouched — the rule is enforced at pairing
  time — but the agent now names the address in a warning on every start until it is paired
  again over `https`.
- **`autostart install` no longer opens the keyring just to check that pairing happened
  (RD-109-04).** It asks `config::is_paired`, which exists for that question, instead of
  `config::load` — one fewer macOS Keychain prompt for an answer it never used.
### Removed

- **The desktop agent no longer opens a window for widget captchas.** RD-107-03 built it — the
  hoster's own page in a system WebView (WebView2 / WKWebView) on the tray's event loop — and a
  run against DDownload on the 1.0.9 Windows build ended it. The agent's window opened instead of
  the browser extension's tab, the challenge could not be solved in it across several attempts,
  and full sign-in credentials typed into the page it showed went nowhere, because the plugin
  signs in over its own connection. None of that was repairable: Cloudflare reads
  `navigator.userAgentData.brands`, WebView2 names itself there, the client hints come from the
  runtime rather than from a user-agent override, and hiding the name would be defeating a bot
  check rather than answering one. Keeping the broken path beside the working one also made the
  working one unreachable — the captcha watcher started on every desktop run, with no switch and
  no check whether the extension was already handling the challenge, so the window always got
  there first. **Widget captchas — reCAPTCHA v2, hCaptcha and Turnstile alike — are now answered
  in your own browser through the rDownloader extension, or by a solver service, and the
  interface says so in all four languages**, including to someone with neither. Gone with the
  window: `crates/rd-capture/src/captcha.rs` and `captcha_window.rs`, the window lifecycle in
  `tray.rs`, the agent's second SSE connection to the capture event stream, the three captcha
  calls on its HTTP client, the `RDOWNLOADER_CAPTCHA_USER_AGENT` and
  `RDOWNLOADER_CAPTCHA_WIDGET_ERROR_PROBE` diagnostics, and the `wry` dependency. Four audit
  findings went with them rather than becoming their own work: an IPC handler that accepted
  messages from any frame including the vendor's iframe, a navigation fence that WebView2 never
  applied to subresources, a `receiver.await` with no deadline that also left the event-stream
  socket unread for the lifetime of a window, and an injected `setInterval` that was never
  cleared. The window's browsing profile is no longer created or read; an installation that has
  one can delete `%LOCALAPPDATA%\rDownloader\Capture\data\captcha-webview` (RD-108-09,
  RD-109-11).

- **Two REST endpoints that no interface could reach are gone.** Both promised something the
  application already did elsewhere, and neither had a client.
  `POST /api/v1/collector/candidates/{id}/torrent/plan/preview` offered a trial run of exclusion
  patterns "before the change is stored" — but it existed only for a candidate, where nothing is
  transferring, and the `PUT` that saves the plan answers with the same resolved tree, naming the
  pattern that dropped each file; clearing the field is the undo. Its doc comment made a promise
  the interface never kept, which is why it goes rather than gains a second, inconsistent
  two-step editor. `POST /api/v1/downloads/{id}/replay-restart` repeated a blocked `POST`
  download from zero — which `POST /api/v1/downloads/{id}/reset` has done for every download kind
  since RD-062-01, is offered on the queue row in exactly the blocked and failed states, and does
  strictly more. It was also broken where it was meant to be used: it put the row back to
  `queued` without clearing the scheduler's stop reason, so after the pause it demands the
  download would never have been dispatched again until a restart. Route, scope entry, handler,
  OpenAPI operation and the `download.replay_restarted` text are removed in all four catalogues
  (RD-108-19).

- **The browser extension no longer reads request bodies, and authenticated `POST` downloads
  stop being an advertised feature.** The path that captured the body of a form-triggered
  download so rDownloader could repeat it had never run in a normal installation: it waited for
  broad host access (`https://*/*`) that no part of the extension ever requested, and without
  host access the browser delivers no `webRequest` events for a hoster's addresses at all. In
  two and a half weeks of shipped builds nobody missed it, and "read every `POST` body on every
  site" is the widest permission this extension could hold. It is gone — the body listener, its
  permission gate, the encoder, the second intake payload shape and seven message texts — rather
  than made reachable. A download the page started with a `POST` is now always kept by the
  browser, which finishes it, and the notification says why; it is never replayed as a `GET`.
  The server side of the capture contract is unchanged and still accepts a body (RD-109-20).

### Fixed

- **A package no longer keeps the hoster's name once the real one is known.** A single
  `https://1fichier.com/?8x6wertoi51r8vptrojn` arrived as a package called `1fichier.com` holding
  a file called `download.bin`, although the link page states the release and its size. RD-109-36
  fixed the file name; the package name stayed, because it is decided at intake and intake knows
  only the address — this one carries no path segment, so `group_links` has nothing but the host
  to fall back on, and `common_stem` refuses a single name by construction. The name is therefore
  taken where it first exists: the moment the resolver reports a file name, a package that is
  still called exactly after its hoster, and holds this one file, takes the release behind that
  name. **The folder is named, not renamed.** This runs while the download is still resolving,
  before the destination directory is created, so in the reported case nothing on disk moves at
  all — the folder is simply created under the right name. Only an earlier attempt that already
  created the folder leaves anything to carry over, and that goes through the two-phase move
  RD-106-13 built, so an interruption between the row and the disk is finished by the next
  completed attempt instead of losing the data. A package anybody named keeps its name, a package
  with a second file in it is left to grouping, a resume that already has committed bytes is not
  moved out from under itself, and a folder of the new name that is already there stops the
  rename rather than merging into somebody else's data (RD-109-45).

- **Premiumize was read more strictly than it answers, and a permanent read failure was retried
  forever.** A 1fichier link resolved through an active premiumize.me account failed with
  *Invalid Premiumize response* and went to `Retrying`; it would have gone on failing to
  `max_retries`, because `parse_json` classified every read failure as `Transient` and a body
  whose shape does not fit never parses on a second attempt. It is `Permanent` now, and it says
  which field it stumbled over — `serde_path_to_error` keeps the path, so the new
  `premiumize.invalid_response_field` carries `content[0].size` rather than discarding the
  reason. Only the path is reported, never the value, which may be a signed delivery link. The
  reader itself is no longer stricter than the API: `content[].path` and `content[].size` are
  optional, a size quoted as a string or sent as a float is read through the tolerant type that
  already sat in the same file unused by this endpoint, `content: null` counts as no content, and
  the deprecated top-level `location`/`filename`/`filesize` serve as the fallback for the legacy
  single-file answer premiumize still sends. Which of those shapes actually broke the reported
  link is not known and cannot be known without a live account — the tolerance is the fix, the
  named field is how the next occurrence will say so itself. Alongside it, `ensure_success`
  matched six error codes that appear nowhere in premiumize's published error table, so every
  real code fell through to `Permanent`: `service_down` and `rate_limit_reached` were never
  retried, and the rapidgator link's `service_unsupported` was reported as a generic permanent
  failure rather than an unsupported one. The documented vocabulary is matched now, the old
  strings kept as aliases in case an older deployment still sends them, and an answer carrying no
  code at all is judged by its message instead of being called permanent unseen. The plugin runs
  18 tests, 9 of which failed before the change (RD-109-44).
- **„Check result missing" said nothing about what was missing, and usually was not even true.**
  Two links of two different hosters stood in the LinkGrabber with that sentence, `0 B` and the
  mark `duplicate`, while the download itself worked without any account. The sentence was passed
  unconditionally for every candidate of a provider batch, and the store kept it for every result
  of status `Unknown` — so one line stood for three unrelated situations: the plugin was never
  asked about this URL, the plugin answered that it cannot tell, and the hoster's check needs an
  account the installation does not have. In the reported case it was the second, which makes the
  sentence simply false: a row can only carry `duplicate` through the branch that *has* a result.
  Each situation now carries its own stable code beside its English text, the way REST errors
  already do, and the interface translates it — eleven codes in all four languages. The one that
  matters most says it outright: the hoster could not answer because the *check* needs an account
  for this hoster, while the *download* may work without one. The direct probe stopped lying too:
  a timeout or a refused connection used to be stored as "No HTTP client available", the one
  thing it was not. **And the `duplicate` mark now names its reference** — it reads *already in
  the list* and its title says the address is already in the LinkGrabber or the queue and may be
  added again deliberately. The mark itself was never wrong: the dedup key is the full address
  compared over the whole table, so two different URLs cannot collide (RD-109-43).
- **The Windows half of the workspace can be linted again, and two findings came out of it.**
  `cargo xwin clippy --target x86_64-pc-windows-msvc -- -D warnings` had been failing on
  `rd-files` since before this release: `MountTable::read` gated its only call to `parse` behind
  `#[cfg(target_os = "linux")]`, so on the Windows target `parse` and `unescape` had no caller
  and counted as dead code — a warning under `check`, an error under `-D warnings`. Three jobs
  (RD-109-11, RD-109-13, RD-109-14) each hit it and each recorded it as pre-existing, which is
  precisely how it survived: a lint that cannot start reports nothing, and the code behind
  `cfg(windows)` is the code no Linux build compiles. The target check is now `cfg!` rather than
  `#[cfg]`, which keeps one body instead of two and keeps the parser compiled — and therefore
  linted — on every target. With the run unblocked it reached the rest of the delivery and found
  a second defect it had been hiding: `rd-capture`'s tray, gated to Windows and macOS, borrowed a
  temporary into a generic parameter that takes it by value. Neither changes behaviour anywhere,
  on Linux least of all — `rd-files` runs the same 62 tests before and after (RD-109-40).
- **A 1.1 KiB favicon was downloaded, checksummed and booked as a finished 405 MB release.** The
  free 1fichier path read the download link out of the page with a fallback that scanned *every*
  `href` on it and accepted any host under `.1fichier.com`. The head of every 1fichier page
  carries `<link rel="icon" href="https://img.1fichier.com/favicon.ico">`, so on a page without a
  download button the favicon was the first match: 1150 bytes, no `Content-Disposition`, saved as
  `download.bin`, verified against its own length and shown with a green tick. The fallback now
  reads anchors only, as JDownloader does. It was reached at all because 1fichier's out-of-slots
  wording had moved to "High demand: all free guest slots are currently in use", which no marker
  matched; that wording is now recognised and holds the hoster back instead. The free path also
  reads the name and the size the link page prints, so a transfer no longer starts with neither.
- **A transfer whose length contradicts what the hoster announced fails instead of completing.**
  The existing exact-length guard compares the received bytes against `total_bytes`, which by
  then is the probe's own number — 1150 bytes agreed with themselves. The comparison now happens
  at the probe, where the announced size (from the resolver, else from the online check) and the
  offered length are still independent, and fails with `download.size_mismatch` in all four
  languages. The rule fires only where both numbers exist and disagree: one percent of slack
  absorbs a rounded announcement, and a 4 KiB floor keeps legitimately small files out of its
  reach entirely (RD-109-36).

- **The capture agent reads its event stream the way the specification defines it, and announces
  what it finds on a window.** Three readings in `sse.rs` were the shape a stream takes today
  rather than the shape one may take: a frame boundary was searched for as `\n\n` or `\r\n\r\n`
  only, so a peer ending lines with a lone `\r` — a proxy normalising line endings, a later
  service — would never have yielded one, the buffer would have grown to the oversized-frame
  guard, and the agent would have reconnected into the same wall forever without a single event
  arriving. Several `data:` lines of one frame were concatenated with nothing between them,
  which turns the first payload carrying a line break into invalid JSON that is dropped without
  a word. `trim_start()` removed every leading space where the specification removes exactly
  one. `id:` and `retry:` fell through entirely, so a service that knows it is overloaded had no
  way to tell its agents how long to wait; both are read now, `retry:` sets the reconnect
  interval, and the last `id:` names in the log how far the agent got. The reconnect delay is
  spread by ten percent around its base — the same jitter `rd-subscription`'s schedule uses — so
  agents that lost a restarting service no longer all come back in the same moment. The toast
  moved out of the stream reader: it used to be shown inline in the drain loop, so nobody read
  the stream while the desktop drew it and a page handing links over in a loop produced a toast
  per submission, each one slowing the reader further. The reader now hands each intake to an
  announcer over a bounded channel it never blocks on, and one toast per three-second window
  names everything that arrived in it. The second SSE connection the audit found went with the
  captcha window in RD-109-11; one reader was already left (RD-109-09).
- **The agent's two status polls read one interval constant instead of two that had to match.**
  The transfer poll in `main.rs` and the tray's health probe in `tray.rs` both ticked every five
  seconds, each stating the interval itself, and a comment recorded that they *must* agree or
  the icon and the status line under it contradict each other. Both read
  `config::STATUS_POLL_INTERVAL` now. They stay two loops deliberately: the health check answers
  before the agent is paired, the summary needs the capture token, and the tray must never read
  the keyring itself (RD-109-09).

- **The account form asks how the account signs in before it asks anything that depends on it.**
  Reported from use with a screenshot, minutes after somebody created a DDownload account. The
  form ran provider, display name, username, connection route, *sign-in method*, secret, cookie
  session — but the sign-in method is what decides whether a username is required, whether a
  cookie session is asked for at all, and whether the field below is a password or an API key.
  Filling it from the top meant answering three questions before the one that says whether those
  questions exist. It moves directly under the provider. **And neither of its two radio buttons
  was selected**, which is why the secret field read *DDownload password or API key* — one box
  for two answers. The watcher meant to preselect the provider's first method was bound to
  `accountForm.provider`, which never changes when the tab is first opened, and even
  `{ immediate: true }` would only have run against a provider catalogue that had not arrived
  yet; it now watches the offered modes themselves, so the default is set both when the
  catalogue lands and when another provider is picked. No visible text changed, so no catalogue
  key was added or became redundant (RD-109-35).
- **The DDownload account check no longer fails on the session it is checking, and no longer
  reports a subscription nobody read.** Reported from use: seconds after *"DDownload signed in
  with the stored account credentials · Premium active"*, pressing *Test* on the same account
  answered *"DDownload's login page did not contain the expected sign-in form"* — while a
  download was running on that very session. `check_account` signed in unconditionally, on the
  reasoning that "testing the account *is* signing in", and signing in begins by demanding a
  sign-in form on `login.html`; once the session exists in the host's cookie jar, that fetch goes
  out as a signed-in user. Measured against the live site on 2026-09-20, from the address the
  failed run used: `login.html` served a visitor without a session a 200 of 135 460 bytes
  carrying the form and a Turnstile widget, with no Cloudflare interstitial, and `/?op=my_account`
  answered such a visitor with 302 to `login.html` — so the site does distinguish by session,
  and the only difference between the fetch that succeeded here and the one that failed in the
  application is the `xfss` cookie. The check now asks the account page first and signs in only
  when there is no session, which is JDownloader's order too; the batched link check does the
  same; a sign-in that does land on a signed-in page returns instead of inventing a login
  problem; and a login page that really carries no form now names what arrived instead, in all
  four languages. **And premium is reported only where it was measured.** Both branches without
  an API key — a credential sign-in whose account page carries no key, and an imported cookie
  session — answered `premium: true` outright, so a free account was shown *Premium active* by
  the identical code path; they answer `false` now and say in the label that the subscription was
  not checked, and only the metadata API's expiry makes it true. The interface stops stating the
  negative as a finding as well: `premium: false` is what a plugin answers when the expiry is
  absent or unparseable as much as when it has lapsed, so *"no active premium status"* became
  *"premium status not confirmed"*. What the browser does is no part of this — the application
  holds its own session, the browser stays signed out — and the account form's sign-in hint now
  says so in all four languages (RD-109-34).
- **FileJoker and KatFile stop reporting a subscription nobody read, either.** The same
  hard-coded `premium: true` RD-109-34 removed from DDownload stood in both plugins' cookie-only
  branch, with the same reasoning beside it — the label even conceded that "premium status is
  checked on file access" while the flag next to it asserted the opposite, and the interface
  prints the flag. A free account with a working cookie session was shown *Premium active*
  exactly like a paid one. Neither branch has a subscription to read: KatFile's `premium_expire`
  lives behind `api/account/info`, which knows only API keys, and FileJoker has no metadata API
  at all. Both now answer `premium: false` and say in the label that the subscription was not
  checked; KatFile's API-key branch still reports premium from the expiry it actually reads. A
  search of every plugin for the same claim — hard-coded, or a variable that can only be `true` —
  found no third case (RD-109-38).
- **The download row gives the name back its width, and stops drawing text over text.** Reported
  from use with two screenshots. In a 1280 px window the package name was left 68 px and rendered
  as `Lieblin…`, while a 30-character password, two badges spelling out `Complete` and
  `Extracted`, a file counter, a bar at 100% with `100%` printed beside it, `1.7 / 1.7 GiB`, a
  category select, a spelled-out priority select and seven icon buttons shared the rest — the one
  thing the row exists to say was the only thing that could not be read. Below 1189 px it was not
  merely cramped: the grid overran its container by up to 142 px and laid text over text, because
  the columns were switched on by *window* width while the row only ever gets the window minus the
  sidebar and the panel padding. Six things changed. The archive password leaves the row once the
  unpack has succeeded and stays while it is outstanding or failed; `Complete` and `Extracted`
  became glyphs keeping their word as their accessible name; a full bar no longer prints a
  percentage beside itself; the priority became an arrow whose name reads `Priority: <level>`,
  with every level in a menu beside it; the seven icon buttons became the package's start/stop
  control and a dots menu, the actions keeping their labels; and the Usenet segment panel dropped
  its size column, which was the *posted* yEnc size and so stood as `722 MiB` directly above the
  download row's `699 MiB` for the same file. The breakpoints were then re-measured rather than
  guessed: 560 px for one line, 768 px for progress, 1280 px for size and metadata, with the row
  breaking into two lines below 560 px. Measured in Chromium at every width from 320 to 2560 px,
  the row now overflows at none of them, and the name holds 296 px at 1280 px where it held 68 px.
  The reasoning is written up in `design.md` under "What a queue row may spend its width on"
  (RD-109-30).
- **The browser extension's captcha flow now reports what the server was told, not what it
  attempted.** Closing the hoster's tab discarded the result of the decline call and announced
  *Captcha declined* unconditionally: with an expired capture token the POST answered 401, the
  person heard that it was declined, and the waiting download ran into a timeout instead of
  failing with `captcha.skipped` — nobody could see the difference. The mirror image cost
  RD-108-02's acceptance a run on 2026-09-20: a captcha ticked in a real browser closed its tab
  and read as success, with nothing saying whether the answer had reached rDownloader. A tab
  closes on a rejected token exactly as it does on an accepted one, and again when a poll finds
  the captcha no longer waiting, so the closing tab is no longer the report. Every ending now
  names the hoster and what the server answered: accepted, refused with the reason, declined,
  *not told that it was declined* with the reason, or no longer waiting. Four more things that
  were not what they looked like went with it. `openTab` read the tab map, awaited `tabs.create`
  and wrote the whole map back, overwriting any removal that happened meanwhile — the resurrected
  entry carried the hoster origin, so the permission was never given back; every change to the
  stored maps is now serialized between its read and its write. The reader injected into the
  hoster's page cleared its 500 ms poll only on success, and the deliberate re-injection after a
  navigation started a second poll in the same document that could send the same token twice; it
  now stops itself at a deadline and a second injection into one document does nothing. Captcha
  state fell back to `storage.local` in a browser without `storage.session`, where tab ids
  survived a restart and a poll then closed tabs belonging to somebody else's windows — it stays
  in memory instead, and what an older version left on disk is dropped at startup. And
  `rdownloader:captcha-decline` was the one runtime message answered without checking that it
  came from one of the extension's own pages; it goes through the same handler and the same check
  as the rest, as does the popup's listing, which is now the background's own poll instead of a
  second listing that announced nothing and left the badge standing (RD-109-23).

- **The LinkGrabber no longer offers a way into an empty drawer.** The button that opens the
  indexer review drawer was shown whenever the section was, so with nothing left to decide it
  promised a room that was empty — and pressing it cost one page load per subscription to be
  told so. It now appears on the same condition as the count badge and the hint beside it, and
  comes back as soon as a hit arrives, without a reload. "Check all" stays where it was
  (RD-109-29).

- **An enricher's answer no longer has to win a race to reach the queue.** Storing the fields a
  metadata enricher found and promoting an auto-queue subscription's links are two commands on
  the serialized writer with no order between them: the watcher promotes on a task of its own,
  and the enqueue works from the snapshot it took when it claimed the links. An answer that
  arrived after that claim was written to a candidate row the enqueue detaches moments later, so
  the rating was on nothing a person ever sees — the same loss RD-107-02 set out to end, only
  rarer and dependent on how long an enricher's service took to reply. The fields are now carried
  onto the rows the candidate became, whichever of the two writes lands first: a release keeps its
  enrichment on the package and on every queue row regardless of the order. Three stored orders
  are pinned by tests — before the claim, during the enqueue and after it — and the two failing
  ones failed against the old code exactly as the field report described (RD-108-15).
- **A plugin version bump no longer leaves its old package behind.** `scripts/build-plugins.sh`
  writes `dist/plugins/<name>-<version>.rdplug`, and nothing removed the package the new one
  replaces — invisible while a version stays put, because the file is then overwritten. After
  `realdebrid-torrents` moved from `0.1.0` to `0.1.1`, both packages lay side by side and
  `scripts/package-windows.sh` stopped with `44 packaged, but 43 plugins have a manifest`: its
  count check, which is there so no plugin is missing, had found one too many. The build now
  drops the superseded packages of the same plugin once the new one is written, so there is
  exactly one package per plugin in `dist/plugins`. The count check itself is unchanged, and the
  version stays in the file name — it is what the release publishes (RD-108-21).
- **The browser extension's test suite now covers the parts that actually break.** It was green
  and said less than it appeared to: `background.js`, the file that wires everything together,
  was imported by no test at all, and neither was `browser.js`, through which every setting and
  every visible string passes. `build()` was never called, so nothing checked that the files the
  manifest points at end up in the output. The manifest test validated a hand-kept list and left
  `version`, `manifest_version`, `default_locale`, the popup and the options page without any
  assertion. The catalogue test compared the four languages with each other, never against the
  code, and fell over on any stray file in the directory. Two tests were tautologies — one
  asserted the capture contract version against its own literal, the other replaced
  `clearInterval` with something that emptied the array it then polled, so a harvester sending
  the token twice would have passed. Two more left a promise pending and a handler hanging after
  the test had ended, and the popup-to-background boundary was never crossed. The suite runs 111
  tests now against 63 before; it imports the background and drives its listeners, calls the real
  build into a temporary directory, checks the contract version against
  `crates/rd-core/src/capture.rs`, carries one fake browser instead of two near-copies, and
  `extension/package.json` makes the run work on Node 20 without a reparse warning (RD-109-26).
- **Dead and duplicated code in the browser extension.** The header denylist was a backstop hung
  *behind* the allowlist, where no name that reaches it can match — it runs before it now, so it
  catches the day somebody adds a credential-bearing name to the allowlist without thinking.
  `isLoopback` compared a host against an unbracketed `::1`, which a URL never produces, and is
  now covered for both spellings. The waiting-captcha notification passed a second argument its
  receiver discarded, which read as if captcha notifications were distinguishable from any other.
  The two message keys left over from a removed permission are gone in all four languages, and a
  new test fails on any catalogue key that nothing in the sources, the pages or the manifest
  reads — the catalogue tests had only ever compared the four languages with each other. The
  stale `extension/dist/` output folder is no longer hidden by `.gitignore`; the extension is
  built into `artifacts/browser-extensions` (RD-109-25).
- **Sharing a browser session now includes the cookie that does the authenticating.** The
  extension read cookies by domain only, and a domain query matches that domain and everything
  *below* it, never above — so a share of `https://www.hoster.com/files` missed the login cookie
  set on `.hoster.com` and produced either nothing at all or a profile the server accepted and
  that authenticated nobody. It asks the browser instead: `cookies.getAll({ url })` returns
  exactly the cookies the browser would send to that page, parent domains included, and needs no
  public suffix list to do it. Cookies named `__Host-` or `__Secure-` are no longer skipped —
  both prefixes are attribute rules on ordinary transferable cookies, so a hoster whose session
  cookie is called `__Secure-session` had been exporting a profile that could not log in. And
  `includeSubdomains` bounds the read it names: it used to be passed to the server while the read
  ignored it, and the permission the browser was asked for now says the same thing (RD-109-22).
- **Nothing the browser extension does fails silently any more, and no notice contradicts the
  state it describes.** Six findings of one family. `downloads.cancel` was the only unguarded
  browser call in the handoff, so a download that finished while the capture POST was in flight
  left its notification standing for good and never reported the success. Every handler in the
  background was dispatched as a bare `void p`, so a throw became an unhandled rejection in the
  service-worker log with nothing naming where it came from — and in the notification chain it
  also swallowed the fallback, which made a click on "keep in browser" do nothing at all. A
  failure notice was shown even for a download the person had just taken back, which is the one
  download that is doing exactly what they asked. The action badge had two owners writing it
  without agreement, so a successful link send cleared the number of waiting captchas and left
  it cleared until that number happened to change; it now has one owner, which composes the two
  inputs. An unparseable server address such as an unbracketed `::1` rejected the options page's
  save listener with no status text, nothing saved and no explanation; it is named as invalid
  instead. And the options page no longer skips the host permission for a loopback address on a
  port other than 8710: the manifest declares only `127.0.0.1:8710` and `localhost:8710`, so such
  a service held no permission at all and worked only because rDownloader answers
  `Access-Control-Allow-Origin: *` (RD-109-24).
- **The browser extension keeps its memory across a service-worker teardown, and a stopped
  service is no longer reported as an old one.** Chrome puts an MV3 service worker to sleep after
  about thirty idle seconds, and three pieces of state did not survive it. The capture version
  negotiated with the service was renegotiated before *every* download, so each handoff carried
  an extra `/capture/ping`. The "configure server and token first" hint and the "this rDownloader
  is too old" notice were meant to be said once and were said again on every download — the
  second one precisely while the person had not entered a token yet. All three now live in
  `storage.session` with an expiry, so a stale entry is never worse than none and nothing reaches
  the disk. Separately, `version = 0` had meant both "predates the capture contract" and "nothing
  answered on that address", so a service that was simply not running announced itself as too
  old and sent the person after a version problem that did not exist; an unreachable service is
  now named as unreachable and the browser keeps the download (RD-109-19).
- **A browser download the extension could not correlate is no longer handed over as a `GET`.**
  `correlateRequest` returning nothing and observing a real `GET` had the same value in the
  code, so a download started by a form whose buffered entry had been evicted skipped the whole
  refusal machinery: the browser's own copy was cancelled and erased, and rDownloader fetched
  whatever a `GET` to that address answers — an HTML page, an error, a different file — and
  stored it under the file name the browser had reported. The two cases are now separate. Where
  the extension holds host access for the address and still finds no matching request, the
  download stays with the browser and says why; where it could not watch at all — which is
  every hoster in a default install — a plain download is handed over on what the download API
  reports, as before. Two things that made the eviction likely are fixed with it: the fifty-entry
  correlation buffer now drops a `GET` before any `POST`, so a page polling in the background no
  longer pushes out the one entry that decides the handoff, and an entry is aged from its newest
  event instead of its first, which had silently shortened the ten-second window by the time a
  slow hoster took to answer. The test clock is controllable, so the window, the eviction and the
  five-minute capability cache really expire in the suite instead of standing still (RD-109-18).
- **An NZB that cannot be read leaves a trace instead of disappearing.** `NzbImportState::Failed`
  and `nzb_imports.last_error` had no writer at all, so a file dropped into a watched folder that
  would not parse produced no row: it was moved to `failed/` under its own name, a warning went
  into the service log, and nothing else. Whoever had set the folder up and walked away had no way
  to learn that anything had arrived, let alone why it did not work. A refused drop is now handed
  back to the service that owns the intake and recorded as an import in state `failed` carrying
  the reason, which the LinkGrabber already knew how to show — the row names the file and the
  reason sits beneath it, and deleting the row is what makes room for the file to be imported
  again. The same bytes arriving a second time update the reason rather than adding a row, an
  import that already became a package is never retracted, and a failed import can no longer be
  queued (it holds no files): the button is disabled and the endpoint answers
  `nzb.import_failed`. Torrents and link containers keep the log line and the `failed/` directory
  they had; only NZBs have a table to be recorded in (RD-108-20).
- **A plugin contract test without its component fails now instead of passing in silence.** The
  loader returned `None` for a component that had never been built, every contract test began
  with `let Some(bytes) = component(...) else { return }`, and nextest hides the stderr of a
  passing test — so a fresh checkout reported `370 tests run: 370 passed` in three seconds
  having entered no component at all, which is the same number the real run produces in two
  minutes. A missing artefact now fails with the command that builds it, exactly as a stale one
  already did; `scripts/build-plugins.sh --list-missing` names them and `scripts/check.sh` stops
  on them before it spends anything. A checkout that cannot build components leaves those tests
  out on purpose with `cargo nextest run -P no-components`, which counts them as *skipped*
  rather than passed — and CI, which did exactly that by accident, now says so: its `rust` job
  uses the profile and its `components` job gained `rd-plugin-host`, whose two
  component-dependent tests had until now run nowhere at all (RD-108-16).
- **An upload destination plugin's progress reaches the display.** A storage plugin has always
  called `source.progress(done, total)` while it uploaded, and nothing was ever on the other
  end of it: `SourceInvocation::with_progress` had no caller, so the host dropped every report
  without a word and the upload step's bar stood still for the whole transfer while the rclone
  path's moved. The report now travels to the caller that started the upload and into the same
  row the rclone path writes to. The numbers count the package rather than the single file, so
  the bar does not fall back to zero at every file boundary, and they are throttled at the
  receiver — a plugin may report per chunk, which is not worth a database write per chunk. The
  one path that still has no listener, the post-processing steps, logs instead of dropping
  (RD-108-18).
- **The browser extension carries the version it is shipped with.** `extension/manifest.base.json`
  had said `0.1.0` since the extension existed, while the workspace moved on to 1.0.8 — so every
  release so far published an archive with the same version number, which no store can accept as
  an update and which no two builds can be told apart by. The manifest now starts at the workspace
  version, `scripts/set-version.sh` writes and verifies it alongside `Cargo.toml` and
  `web/package.json` (a pre-release suffix is dropped, because a browser manifest version is
  dot-separated integers only), and a unit test fails the moment the two drift apart
  (RD-109-17).
- **A browser-extension build that could not write its archives no longer reports success.** The
  `zip` call sat in a `try` with an empty `catch`, and the closing line tested the Chrome archive
  and spoke for both — so a run that wrote one archive, or none, read as if it had written two.
  A failed or missing `zip` now fails the build and names what it could not write, the closing
  line states which archives are actually on disk, and `scripts/build-extension.sh` verifies
  afterwards that both archives exist and each contains a `manifest.json` carrying the workspace
  version (RD-109-17).
- **A missing segment is judged when the package is done, not when its file happens to finish
  first.** RD-108-23 moved the PAR2 question to the moment the real file name is known and wrote
  down what it could not reach: a fully obfuscated set announces no PAR2 in any subject, and a
  row is only marked as repair data once its own file has been assembled — so a payload file
  with a hole that finished before any PAR2 file was told `usenet.segments_missing_no_par2`
  about an NZB that carries PAR2. Such a file now waits in `Verifying`, carrying
  `usenet.segments_missing_awaiting_par2` and the number of segments it is missing, while any
  row of the package is still queued, resolving, downloading, verifying, repairing or waiting
  for a retry. When nothing of the set is on its way any more the verdict is taken on the
  settled package: a set that carries PAR2 sends the file to repair, a set that carries none
  fails it with exactly the code, message and count it failed with before — and a package with
  nothing else running is decided in the same breath, so that case waits no longer than it did.
  A restart in the middle keeps the assembled file instead of fetching it again: the row is not
  requeued, and `recover_interrupted` takes the verdict when the set has nothing left running
  (RD-108-24).
- **An indexer subscription no longer counts hits it will not show.** In the LinkGrabber's
  review box the number beside a subscription followed the event stream while the list of hits
  did not, so a search could announce thirty-two hits over a group that opened empty and stayed
  empty until the page was reloaded by hand. The rows now follow the same event as the number:
  where the count and the loaded list disagree, the list is read again, including when the group
  was opened while the check was still running and its first read crossed the newly written
  hits. A page read that fails is stated in the group, with a button to try again, instead of
  looking like a subscription with nothing left to decide (RD-109-28).
- **`association remove` now removes the notification sender it registered (RD-109-15).**
  `HKCU\Software\Classes\AppUserModelId\rDownloader.Capture` was written by
  `association install` and left in the registry for good, even after the agent was uninstalled.
  The cause was structural: installing was a list of entries and removing an independent sequence
  of calls, so a key added to one side and not the other went unnoticed. Both now move over the
  same list, with the deliberate restraint around the shared
  `SystemFileAssociations\.nzb` parent marked on the entry it applies to, and a test holds the
  two sides against each other.
### Security

- **A rejected address is no longer handed back in the error text (RD-109-39).** RD-109-32 keeps
  a fragment out of every stored candidate, but one path went around the store entirely: a
  structured link whose URL would not parse was refused with `Invalid link URL: <the pasted
  string>`, verbatim and password included. That is the likely input on this path, not an unlikely
  one — a mistyped address of a protected share is exactly an address no parser takes, and the
  share password rides in the fragment. Nothing was stored, but the REST answer carried it and so
  did every log that keeps an answer; and because `collector.link_url_invalid` had no translation,
  the English prose was also what the interface put on screen. The refusal now names the link's
  position in the batch and never sees the string: redacting it was the alternative and is not
  sound, because redaction works on a parsed `Url` and this is precisely the string that would not
  parse. The caller still holds the text it just sent; what it could not know is which of its
  links was refused, and that is what the position says. The same shape in rd-collector's RSDF
  decoder — the undecrypted line quoted into a context that the container import hands straight
  back over REST — loses its interpolation too. The code now has a sentence in all four
  catalogues.
- **A password pasted in a URL fragment no longer lands in a LinkGrabber row (RD-109-32).**
  RD-108-07 closed this for a link a crawler claims: the fragment is encrypted into an auth
  profile and the address is stored without it. A link *no* crawler claims took the other path
  and kept its fragment, so one wrong letter in the host name of a protected share — or a share
  whose plugin is not installed — wrote the password in clear text into `link_candidates.url`,
  showed it in the row of everyone looking at the LinkGrabber, and carried it on into
  `downloads.source_url`, `downloads.source_path` and every SSE event and REST answer naming the
  candidate. No candidate row carries a fragment any more: `rd_core::candidate_url` drops it in
  `rd_db::collector_store`, the one writer every intake path ends in — pasted text, DLC, NZB
  import, hotfolder, subscription poll, handed-over torrent. It is dropped rather than vaulted,
  because a secret in the vault needs an owner and an unclaimed link gives none: no user name and
  no scope, both of which `share_login` derives from a crawler's *answer*. Crawling itself is
  untouched — a crawler is still handed the pasted address whole, fragment included.
  `rd_core::redact_url` deliberately still leaves a fragment standing, because it also builds
  values that are shown rather than logged and a fragment is an anchor far more often than a
  secret. **Rows written before this release keep what they hold** — see the job file for why no
  migration rewrites them.
- **The Click'n'Load port now decides per route who may call it (RD-109-01).** The agent listens
  on loopback, but loopback is no boundary against a browser: every page open in the browser can
  reach `127.0.0.1:9666`, and every answer used to carry `Access-Control-Allow-Origin: *`.
  rDownloader's own route `/rdownloader/nzb` now carries no `Access-Control-Allow-*` header at all
  and refuses any request that arrives with an `Origin` or a `Referer` — its only caller is the
  agent's own `open` subcommand, which sends neither. The JDownloader-compatible `/flash*` routes
  keep the permissive headers as a named, documented exception, because a hoster page calls them
  from its own origin and an allowlist would lock out exactly the pages the mechanism exists for.
- **A refused Click'n'Load request answers with a stable code, not with prose (RD-109-01).** The
  response body used to be the error text — `invalid CNL padding` among it, which is a padding
  oracle against the AES path, and up to a kilobyte of whatever the service had just said. The
  body is now one of `cnl_invalid_payload`, `cnl_invalid_key`, `cnl_no_links`,
  `cnl_payload_too_large`, `cnl_service_unavailable` or `cnl_foreign_origin`; the detail stays in
  the log.
- **`/crossdomain.xml` is gone and `OPTIONS` no longer answers for paths that do not exist
  (RD-109-01).** The Flash cross-domain policy served `allow-access-from domain="*"` to a client
  that has not existed for years, and the catch-all fallback answered every preflight on every
  path with 200. Preflights are now registered per route.
- **Click'n'Load enforces its limits before it does the work (RD-109-02).** The 65 MiB body limit
  covered every route alike and the real 8 MiB check sat *inside* the handler, so Axum decoded a
  whole 65 MiB form before anything measured it. Each route now carries the limit that belongs to
  it — 20 MiB for `/flash/addcrypted2`, 1 MiB for `/flash/add`, 64 MiB plus framing for
  `/rdownloader/nzb` — and an oversized body is refused with 413 before the extractor runs.
- **A `jk` script can no longer stall the agent (RD-109-02).** Evaluating a non-literal
  Click'n'Load key took a thread out of tokio's blocking pool — the same pool the clipboard and
  the desktop notifications use — and `tokio::time::timeout` ended only the waiting, not the
  thread. The evaluation now runs on a thread of the agent's own, at most two at a time, and a
  caller that finds no free slot is refused at once instead of queuing.
- **An ambiguous Click'n'Load key is refused instead of guessed (RD-109-02).** The key pattern
  matched 32 hexadecimal digits *anywhere*, so an unrelated identifier in the script, or the first
  half of a 64-digit literal, silently became the key and decryption then failed with a padding
  error that pointed at nothing. The pattern is anchored on the quotes now, and a script carrying
  more than one candidate is refused with that as the reason.
- **The Click'n'Load refusal for a link-free payload names the schemes that are really accepted
  (RD-109-02).** It said "no HTTP(S) links" while the collector had long taken magnets, FTP(S),
  SFTP and WebDAV too. Both regular expressions in the request path are compiled once now rather
  than per request.
- **`rdownloader://open` refuses a network share (RD-109-03).** The handler checked only
  `Path::is_absolute()`, and a Windows UNC path is absolute — so
  `rdownloader://open?path=//evil.test/share/x.nzb`, which any web page can trigger, made Windows
  dial SMB to a host the page named: a forced network fetch and an NTLM credential leak. Both
  spellings, the verbatim `\\?\` forms and the `\\.\` device namespace are now refused, and the
  check is textual so it holds and is tested on every host.
- **The 64 MiB cap on an imported NZB is enforced while the file is read (RD-109-03).** It used
  to be measured with `metadata()` and the file read afterwards with no limit at all, so a file
  that grew in between, or a symlink pointed elsewhere, was pulled into memory whole.
- **A successful pairing leaves no older copy of the token behind (RD-109-04).** A keyring write
  used to leave an earlier `capture.token` file untouched, and the agent falls back to exactly
  that file whenever the keyring cannot be read — a locked session, a swapped backend, a
  different login path — so a revoked token kept being presented for as long as the file sat
  there. `is_paired` also asks the keyring before that file now, rather than letting a leftover
  outvote what the keyring holds.
- **The capture agent no longer falls back to loopback when `capture.json` cannot be read
  (RD-109-05).** A truncated or corrupted file was read with `.ok()`, which turned every failure
  into `http://127.0.0.1:8710` — so an agent paired against a remote host sent its bearer token
  to whatever happened to be listening locally, without a word. An unreadable configuration is
  now an error; only an absent file means "not paired".
- **The stored service address is checked for being `http` or `https` in one place (RD-109-05).**
  Loading, saving and the tray's `stored_service` all go through the same gate. Nothing checked
  it before, so whoever could write `capture.json` decided both what the tray handed to the
  operating system's "open this" and what the HTTP client sent the capture token to.
- **`capture.json` is written atomically (RD-109-05).** Temporary file, `fsync`, rename — a
  direct write truncated the destination first, which is how a half-written configuration got
  onto disk in the first place.
- **Every refusal the service sends reaches the capture agent in one shape (RD-109-10).**
  `capture_events` and `answer_captcha` flattened a refusal into a formatted string and `summary`
  and `pending_captchas` used `error_for_status`, which never reads the body — so the stable
  `code` the REST surface carries for exactly this purpose reached three of the seven calls, and
  the agent could not tell an expired widget token from a service that was merely busy. All of
  them now produce a `ServiceRefusal` with status and code. The captcha answer keeps its promise
  not to repeat the token: a body that quotes it back has its detail dropped.
- **A response body that broke off is no longer reported as "no detail" (RD-109-10).** It was
  read with `unwrap_or_default()`, so a 503 whose body was cut short by a dropped connection read
  exactly like a correct, detail-free refusal — and the hint about the connection, the one thing
  that would have helped, was gone. The three cases are distinguishable now, and the clipboard
  loop names the connection case in its log.
- **A byte count the service sends in an unreadable shape is reported, not silently zero
  (RD-109-10).** `unwrap_or(0)` turned a changed format into "no transfers" while downloads were
  running: a broken API contract that looked exactly like a quiet system. It fails the
  deserialization now, and the transfer poll logs it at `warn` and asks again on the next tick.
- **Reading the clipboard no longer blocks a runtime thread (RD-109-08).** `arboard` talks to
  the window server synchronously and waits for whichever program owns the clipboard to answer,
  and that call sat directly on a tokio worker once a second — so a frozen browser, a
  remote-desktop session with clipboard forwarding or a compositor under load could take the
  event stream, the transfer poll and Click'n'Load down with it. It runs on a blocking thread
  now, like the notification call next to it.
- **A very large clipboard text is left alone instead of hashed every second (RD-109-08).** The
  clipboard was the only input of the agent with no length bound, so a copied log or CSV of a few
  megabytes was SHA-256'd and scanned for links once a second for as long as it sat there — the
  deduplication happens after the hash, so it saved the request and not the work. Anything over
  1 MiB is now discarded, not truncated, and mentioned once rather than every tick.
- **A background task of the capture agent can no longer disappear quietly (RD-109-07).** All
  four — clipboard monitoring, intake notifications, the transfer poll and the captcha watcher —
  were started with `tokio::spawn` and their `JoinHandle` thrown away, so an `Err` was never read
  and a panic was just as silent. `watch_clipboard` returning once meant the clipboard was never
  looked at again, with nothing in the log and no change in what the tray showed. Each one is now
  held, its end is logged with its name and its reason, and the tray's status line says so
  instead of going on reporting "healthy".
- **A partial Click'n'Load bind is reported rather than counted as success (RD-109-07).** Another
  listener holding `127.0.0.1:9666` but not `[::1]:9666` left the agent bound to v6 only, one
  `warn` line nobody correlates, and browsers that resolve `localhost` to v4 still handing their
  links to the other program. The agent keeps running — half the reachability beats none, and a
  host without IPv6 would otherwise not start — but it names every affected address in one line
  and the tray says "Click'n'Load only on …". The `EXIT_PORT_BUSY` code stays reserved for the
  case where not one address can be had.
- **The error path of the agent loop cancels before it returns (RD-109-07).** It used to return
  straight to the tray, whose `finish` ends the process with `std::process::exit` — no
  destructors, no drain — so an `answer_captcha` carrying a single-use token could be cut off
  mid-flight, the token spent and the person asked to solve the same challenge again. The token
  is cancelled first and the running tasks get a bounded grace to finish.
- **Every outgoing request of the capture agent now has a deadline (RD-109-06).** The client the
  agent runs on had neither a `timeout` nor a `connect_timeout`, so a half-open connection — a
  sleeping machine, a dropped NAT entry, a WSL network after standby — left `watch_activity` and
  the clipboard loop waiting **forever**: no log line, no restart, while the tray went on
  reporting "healthy". All four hand-written builders are replaced by one `client::build`, which
  gives short calls a 30-second total deadline and every client a 5-second connect deadline; the
  event stream, which is meant to stay open, gets a 90-second read deadline instead of a total
  one. The tray no longer turns a failed build into `unwrap_or_default()` — a client without the
  very deadline it was written for — but logs it and reports the service as not reachable.


## [1.0.8] - 2026-09-19

Nine jobs, and the milestone said what it was for before it started: a correction release. 1.0.7
shipped twenty jobs and was honest about what it had not finished, and this is that list worked
off. A remote job now actually runs rather than merely having a contract; the two folder crawlers
are proven against their built components instead of only against their own unit tests; the
captcha moved from an embedded window that Cloudflare refuses to the browser the person already
trusts; and one job did nothing but remove what nothing reaches.

**What this release does not contain, on purpose.** Four jobs of the milestone depend on one of
these nine and go to the next: the surface for a remote job and its confirmed remote deletion
(`108-04`), the SDK template that lets somebody else write an eleventh-world plugin (`108-05`),
loading a password-protected share rather than only listing it (`108-07`), and the decision about
what is left of the captcha window now that the extension carries the work (`108-09`). The
acceptances 1.0.7 owed (`108-11`) were not caught up either: every single one needs a real
account, a real server, a browser or a Windows desktop, and none of that exists where this
release was built. They are listed, with what each one needs, in
`docs/roadmap/jobs/108-00-abnahme-checkliste.md` — together with the twenty-odd checks from these
nine jobs that a person still has to make.

Five further findings turned up while verifying and were written down rather than quietly fixed:
a test that races its own subject (`108-15`), contract tests that pass in silence when their
component was never built (`108-16`), and three behaviour defects the sweep found (`108-17`
through `108-20`).

**And then it did not ship on the 16th.** What was meant to be the last correction turned into
nine further audit passes over the Rust workspace and a round of polish on the surface, so the
release below is that too: roughly ninety findings fixed rather than filed, one order shared by
the two kinds of row in the LinkGrabber, an event model in which an event carries the scope of
the write that produced it, and the indexer hits moved out of the page and into a drawer. The
date moved; the version did not, because nothing in it had been published.

### Added

- **Withdrawn plugin packages have a screen.** The plugin manager lists what is withheld, marks
  the affected version on its card, and lets you withdraw one build or lift it again — stating
  both things that are easy to get wrong: it takes effect at the next start, and the author's
  signing key stays trusted, so every other plugin signed with it keeps loading.
- **A fifth audit pass, and the plan behind it
  (`docs/roadmap/jobs/108-31-was-der-audit-offen-liess.md`).** What was left needed decisions
  rather than fixes, so the decisions are written down with their reasoning — what a plugin
  withdrawal may and may not do, why the Windows key-file permission stays open, why the 103
  overlong files are not split in one go. `docs/backend-audit.md` covers all five passes and
  says, for every item still open, whether it is deferred or declined and why.
- **A withdrawn plugin version can be refused by name, and the refusal now survives a restart.**
  Blocking one exact package used to live only in the running process, so it was forgotten on
  the next start. Withdrawals are stored, listed and lifted again through the plugin area, and
  they are read back when the service starts. Withdrawing one version leaves its author's
  signing key — and every other plugin signed with it — alone, and nothing already running is
  torn down: the refusal takes effect the next time plugins are loaded, exactly as a revoked
  key does.
- **An API token now records when it was last used.** The field existed and was never written,
  so every machine token looked as though it had never been touched.

- **A fourth audit pass, and the last deferred piece of it (`docs/backend-audit.md`).** The
  report now covers all four passes, and this one also records two corrections to itself — a
  function reported as unused that is in fact called (and whose real defect is worse than the
  one first claimed), and a security note filed against the wrong component — plus one fix from
  the previous pass that was reverted because the test suite showed it traded a rare problem for
  a common one.
- **A third audit pass, and the decisions behind it (`docs/backend-audit.md`).** The report now
  records what each of the three passes fixed and what is deliberately left, with the reasoning
  for each call — why blocked downloads from an older version are not migrated automatically,
  why a corrupt subscription filter fails loudly instead of defaulting, why the event log was
  kept rather than dropped, and why the shared FTP/SFTP transfer became a crate of its own
  rather than living in either runner. The failpoint test counts in `AGENTS.md` and
  `docs/recovery-matrix.md` disagreed with each other and with the suite; those numbers are the
  only evidence that the crash tests have not compiled away to nothing, so all six were
  re-measured.
- **A second audit pass, with twenty more fixes (`docs/backend-audit.md`).** The report now
  separates what each pass fixed from what is still open, and corrects one finding the first
  pass got wrong: the thumbnail download was said to bypass a proxy the recording honoured, but
  that component has no proxy setting at all and the recording itself is fetched by an external
  tool. Everything genuinely left — a decompression limit that only applies after unpacking, a
  full disk that never restarts what it stopped, REST error codes derived from matching English
  prose — is listed with file and line and why it needs a decision rather than a patch.
- **A backend audit, and the twenty-three fixes it asked for (`docs/backend-audit.md`).** The
  counterpart to the frontend review: 245 326 lines of Rust across 36 crates and the plugin
  guests, reviewed subsystem by subsystem. The report records what was fixed, what stays open
  with file and line — including a keyring failure that can overwrite the secret vault's master
  key, a decompression-bomb limit that only applies after the archive is already unpacked, and a
  capacity release path that never restarts what a full disk stopped — and what the audit found
  intact: Clippy silent across the workspace, no `TODO` anywhere, 23 `.unwrap()` calls all on
  constants, no ad-hoc writes past the serialized database writer, and the plugin WIT contract
  byte-identical across all ten SDK templates. Every measurement keeps the command that produced
  it, so the next drift is recognisable the same way.
- **A frontend audit, and the seven fixes it asked for (`docs/frontend-audit.md`).** A review of
  `web/` at 1.0.8 with the measurement behind each finding and what was done about it: the
  section header written out 73 times across 49 files and drifted into twelve description
  variants, now one `SectionHeader` with three levels; the edit-in-place list re-implemented in
  nine components, now `useEditableList`; nineteen hand-written date formatters; six hand-rolled
  byte-to-MiB form models, now one `byteModel`; `LinkGrabberView` at 915 lines, now 523 with its
  file import, reordering and enqueueing in composables of their own; eight over-broad exports
  narrowed; and the four components over 300 lines that had no test, which now have seventeen
  cases between them. The report also records what the audit found intact — no `any`, no
  `@ts-ignore`, no leftover `TODO`, and 2 978 translation keys present in all four languages
  with no dead entry among them — and keeps every measurement as it was taken, so the next drift
  is recognisable by the same commands. `design.md` gained the two patterns that had no home.

- **A widget captcha is answered in your real browser, through the extension (RD-108-02).**
  1.0.7 built the whole path for a person to answer a Turnstile, reCAPTCHA or hCaptcha widget
  themselves and then measured that Cloudflare refuses the desktop agent's embedded WebView: the
  engine names itself `Microsoft Edge WebView2` in `navigator.userAgentData.brands`, a hint that
  cannot be set. Hiding it would be defeating a bot check rather than answering one, so the
  place changes instead. The browser extension now polls the capture surface every 30 seconds
  on an alarm (an MV3 service worker has no event source and does not outlive an open stream),
  announces a waiting widget with a notification and a badge, and lists it in its popup. One
  click there asks the browser for the hoster's origin — exactly that origin, at that moment,
  through the optional host permission the extension already declared — opens the hoster's
  own page in a tab, and injects a reader that watches the widget's answer field and touches
  nothing else. The token goes to `POST /api/v1/capture/captchas/{id}/token`, the same route the
  agent uses, straight to the waiting plugin through the `solve-captcha` contract; the tab
  closes and the permission is released. Closing the tab without answering declines the captcha
  (`captcha.skipped`), and a captcha that expires or is answered elsewhere closes its tab on the
  next poll. Nothing along the way stores, logs or echoes the token.

  The web interface can now say something true while a widget waits: the extension's poll names
  itself (`?client=browser_extension`), the service remembers when it last did, and
  `GET /api/v1/captcha-answerers` reports whether that was within the last 90 seconds. The
  captcha prompt shows either "the extension is connected and will open this page" or "no
  extension is connected" with a button to the pairing page, instead of telling everyone to
  start the desktop agent. The `captcha.widget_needs_solver` message names the extension first.
  The agent's window stays in the code for the widgets Cloudflare does not gate. Not verified
  here: a live Turnstile widget accepting the token — that needs a real browser and the
  DDownload login, and stays an open acceptance step in the job file.
- **A remote job actually runs now (RD-108-03).** 1.0.7 decided where a job that runs at a
  provider lives (ADR 0003), built the eleventh world, the host wrapper, the row and the
  Real-Debrid torrent plugin - and nothing drove any of it: `due_remote_jobs` read the rows
  and nobody called it. This release is the caller. A sweep in the service reads every due
  row on a five-second tick and does the one thing `RemoteJob::submit_step` allows for it:
  submit, adopt, poll or give up. The order of its writes is the whole idempotency argument
  and is now code rather than a paragraph: the row with its content key exists before any
  request leaves, an attempt is counted *before* the plugin's `submit` is called, and the
  identifier the provider answers with is written as the very next thing. So a second paste
  of the same magnet on the same account ends as "already yours" before anything has left
  the machine, and a restart between the request going out and the id coming back finds a
  row with one attempt and no id, asks the provider what it already holds, and adopts the
  torrent instead of creating a second one. Two attempts are the ceiling; after them the row
  fails under `remote_job.submit_unconfirmed` and tells the person to look at their account.

  The plugin suggests and the host decides: a `preparing` wait, a `Retry-After` and a
  rate limit are all clamped into the five-second-to-fifteen-minute band the core defines.
  A job in `awaiting_choice` is not polled at all, however long nobody answers; a choice is
  kept to the entries the job offered, an empty one never reaches the guest, and the answer
  puts the row back on the clock. A job the provider ended, a sign-in that expired and a
  plugin that is no longer installed each end the row under a code the interface translates;
  an outage, a spent request budget and a blocked address wait it out.

  What the provider finished goes to the LinkGrabber as one batch: http(s) addresses only,
  names and folder hints reduced to something that cannot leave the package, at most 500 of
  them, labelled with the plugin's name, checked online like a pasted list and then subject
  to the same review, blocklist and routing rules. The row is closed with the package it
  produced, and a finished job is never polled or handed over again. Real-Debrid's own paths
  are rooted at the torrent's name, and the plugin used to put that name in front of them a
  second time, so every package landed in a `Show.S01/Show.S01` folder; it names it once now,
  and the bundled plugin is `0.1.1` so an installation that already holds `0.1.0` from 1.0.7
  actually receives the fix - a bundled package is installed only when it is strictly newer.

  Two waits are the host's own. A refusal that is worth waiting out but names no wait - a
  5xx, an outage - doubles the row's previous wait each time, from the state's default up to
  fifteen minutes, and one successful answer resets it; a wait the provider named is used as
  named, clamped. And a failure to *load* the remote-job plugins - a disk error, not a missing
  plugin - skips the tick and is tried again on the next one, rather than being remembered for
  the life of the process and failing every running torrent under `remote_job.no_plugin`.

  `crates/rd-plugin-ext/src/remote_job.rs` is the adapter - a new job is routed by the
  provider slug the account carries, an existing row by the plugin id it names - and
  `crates/rd-plugin-ext/tests/remote_job_contract.rs` drives the *built* Real-Debrid
  component through it against a mock of `api.real-debrid.com`, with sanitised fixtures for
  the whole chain, a torrent the provider ended, an expired sign-in (`error_code` 8) and a
  spent budget (`error_code` 34 with `Retry-After`). The token leaves the plugin as the
  vault template on every request and never as a value. Seven server codes are new, in four
  languages. **Not in this release:** the REST surface and the page that starts, lists,
  chooses and deletes these jobs (RD-108-04), and a run against a real Real-Debrid account,
  which this checkout does not have.

- **The tray says how long the queue still needs, not just how fast it is going (RD-108-01).**
  The desktop agent's status line and tooltip read
  `3 active · 2 queued · 47% · 12.4 MB/s · 1h 12m left`. The remaining time is not a new
  calculation: `/api/v1/capture/summary` was already asking the queue for its rate, and the very
  same answer carried the estimate beside it and threw it away. `CaptureSummaryResponse` now
  carries `eta_seconds` out of that one call, so the tray and the web interface show one figure
  rather than two answers to the same question.

  It is shown exactly where the rate is shown - only while something is actually running, because
  a time beside a resting queue reads as motion - and it is absent wherever the service has
  nothing honest to say: an entry still to be fetched whose size is unknown, a rate of zero, a
  paused transfer. Nothing stands in its place, no infinity sign and no "calculating". After a
  service restart there is no rate for two samples and therefore no remaining time either; the
  sampler is deliberately not persisted.

  The counts beside it draw their line differently, and this says so rather than hiding it:
  `active` and `queued` name two states, while the estimate spans everything still to be fetched
  and so also covers an entry resolving or waiting on a retry. Widening the counts would have
  given the capture token sight of more of the queue than it is scoped for; the queue-wide
  estimate is the figure the web interface already publishes.

  An agent older than the service ignores the new field, and an agent newer than the service
  renders the line without a remaining time instead of discarding the answer. The tooltip is now
  cut to the 128 characters Windows keeps for it - hostname, counts, rate and time are each
  unbounded and the longest line they can form runs well past that - while the menu entry, which
  has no such limit, keeps the whole line. The summary still names no file, no path and no
  account.

- **The captcha window now says why Cloudflare refuses it (RD-107-18).** Two Windows
  acceptances of the widget captcha ended on the same "verification failed": RD-107-03 built the
  path, RD-107-14 blamed the in-memory profile and replaced it with a persistent one, and the
  message did not change. So the cause is not known, and this adds no third guess - it adds the
  four measurements that were missing.

  The script the window injects into the hoster's page reports, once per window,
  `navigator.userAgent` and `navigator.userAgentData` (`brands`, `platform`, `mobile`). That puts
  the user agent RD-107-14 hardcoded and the client hints WebView2 fills from its *real* runtime
  version into one log line, where a disagreement between them is visible at a glance - a
  disagreement is a bot signal in its own right, and it is the current leading suspicion about
  RD-107-14's own fix. Turnstile's `error-callback` is wired up as well, on implicitly rendered
  widgets by attribute and on explicitly rendered ones by wrapping `turnstile.render`, and the
  code it carries is logged: "verification failed" is what the widget draws, the code is the
  reason, and it is the piece that was missing both times. On the way out the window counts what
  is in its profile directory, so an empty store after an attempt would say for itself that
  RD-107-14's directory never reached the engine.

  The claim itself became a switch, `RDOWNLOADER_CAPTCHA_USER_AGENT`, so the desktop where the
  failure happens can try it both ways without a rebuild: unset keeps the compiled-in string,
  `off` (also `0`, `no`, `false`, `default` or empty) lets the installed engine send its own, and
  any other value is used verbatim.

  The token is not part of any of this and does not become part of it. All three kinds of message
  share the one channel the token already used - a second channel would be a second trust
  boundary - so a message carrying a token is read as a token and nothing else, every string a
  diagnostic accepts is trimmed of control characters and cut far shorter than any widget answer,
  the error code is refused unless it is short and alphanumeric, and the profile is counted rather
  than read. A test reads both captcha source files and fails if any `tracing` statement in either
  of them so much as names a token.


### Changed

- **The LinkGrabber's indexer hits moved into a drawer.** The list of hits waiting for a
  decision used to unfold itself in the page, above the links; every hit that arrived pushed the
  rows below it further down, so a row somebody was reading moved away under the pointer. The
  box is now one line — title, the number of open decisions, the hint — with a button that opens
  the hits in a drawer over the dimmed page. Nothing opens by itself any more; "Check all" still
  does, because that is a question whose answer is inside.
- **The LinkGrabber's "Enqueue all" is the one prominent button in its bar.** Getting reviewed
  links into the queue is what the screen is for; adding and importing them read as the ordinary
  actions they are.
- **The first entry of every category picker is called "Automatic"** instead of "Default
  category". It never was a category — it means nothing is pinned and the routing rules decide —
  and sitting one line above a category somebody had named "Default" it read as a duplicate.
- **A broken setting is now said out loud instead of quietly reverting to a default.** The
  stored service settings are one JSON document, and every layer used to read its own slice out
  of it and swallow the failure: a field with the wrong type left the service running with no
  bandwidth limits and no storage threshold, with nothing in the log. All of those readers go
  through one accessor now, which reports what it rejected, once. Where the setting decides how
  the service starts — concurrency, listen port, tool paths, transfer limits — a broken value
  stops the start instead of being replaced silently; where it is read by a supervision loop or
  a status request, the default still applies, but the log now says so.

- **The qBittorrent-compatible login hands out an opaque session value instead of the API token
  itself.** The `SID` cookie used to *be* the credential, set for the whole origin, so anything
  else served from the same address could read a full API token and a plain-http hop carried it
  in clear text. Clients are unaffected in what they send; what changes is that logging in no
  longer puts a reusable credential into a cookie, and that a session ends with a restart —
  which is what qBittorrent itself does.

- **The NNTP pool outlives the file, and articles are written where they belong rather than
  in the order they arrive (RD-108-26).** Three properties used to make connections idle for
  reasons that had nothing to do with the line. A pool was built per NZB file, so every file
  change threw away every authenticated connection and opened it again — ten TCP connections,
  ten TLS handshakes and ten logins at each of a release's fifty files. One Usenet file ran at
  a time, so the tail of a file never overlapped the head of the next. And the fetch stream
  was in order, so one article waiting for a second attempt stopped the stream from asking for
  any further articles at all, not only from writing them. The pool now belongs to the runner
  and is rebuilt only when the connection settings change — recognised by a fingerprint over
  host, port, TLS, user, proxy, priority, connection count and the password's *reference*, so
  a changed password is noticed without the secret ever leaving the store. Two files download
  at once, which raises no connection count because the shared pool still caps those. And each
  article is written at the offset its own `=ypart` range names, so the stream is unordered:
  a slow article delays nothing but itself. Measured against the fixture server, forty files of
  five articles over ten connections at 30 ms round trip: 7.72 s and 200 connections opened
  before, 2.76 s and 5 connections after. The single-file throughput is unchanged (79.8 MB/s
  before, 81.1 MB/s after). The resume follows: it checks every checkpointed range on its own
  instead of walking from the front until the first gap, keeps what its checksum proves and
  fetches only the rest, and no longer truncates the file — a `.part` file may now legitimately
  have a hole in the middle. What no article covered is zero-filled when the file is finished,
  which is what the assembly did before, and two articles claiming the same bytes are refused
  rather than silently overwriting one another.

- **A list row without a cover keeps the cover's place.** Indexer hits and subscription items
  show a thumbnail when the indexer supplies one, and the rows that had none began at their
  title instead — so the titles of a list stood in two columns and a long list read as
  restless. A placeholder of the same size now stands there, the application's own mark on the
  elevated background, hidden from screen readers because it is decoration standing in for
  decoration and not a picture of the release.

- **The Usenet path keeps the line full and the disk out of the way (RD-108-25).** Reported
  from use: SABnzbd was noticeably faster against the same provider with the same ten
  connections. A source-level comparison of both download paths found three reasons, and this
  release removes them. Every NNTP connection now carries two `BODY` requests at once, the way
  SABnzbd's `pipelining_requests` does: the next command is on the line while the previous
  body is still arriving, so a connection no longer idles for a round trip between articles.
  The `fdatasync` after every article is gone — it was a hard ceiling near 200 MB/s on an NVMe
  disk and far lower on NTFS or a spinning one, and the restart never relied on it: it
  CRC-checks every checkpointed range against the disk and re-fetches what does not match.
  The file is still synced once before it is renamed into place. And the per-file connection
  count no longer caps the servers behind your back: it defaulted to 8, so ten connections
  entered at the server meant eight used and nothing said so. `0` — now the default — means
  as many as the enabled servers allow; a value you set stays, and the General tab tells you
  whether it binds. **If you saved settings before this release, the field still reads 8:**
  set it to 0 once to use every connection your servers allow. Measured against the NNTP
  fixture on this machine (200 articles of 768 KiB over ten connections, 30 ms round trip,
  10 MiB/s per connection, on an ext4 disk): 5.1 s before, 2.0–2.5 s after; with a 1 ms round
  trip and an unpaced line, 2.8–3.0 s before, 1.6–1.7 s after. The acceptance against a real
  provider is on the release checklist.
- **The recovery matrix covers NZB assembly.** `usenet.after_article_write` is registered as a
  crash point: an article on disk without its checkpoint is truncated and fetched again on
  restart, never counted as confirmed. The Axis A run now names `rd-usenet/failpoints` as well.

- **Dead and duplicated code is gone (RD-108-12).** Twenty milestones of growth left weight that
  nothing carried any more, and a systematic look found it rather than the three items that had
  turned up by accident during 1.0.7's verification. Removed, each after proving nothing reaches
  it: seventeen Rust symbols without a single caller, plus the private store functions and
  writer commands that existed only to serve them - among them
  `Database::nzb_import_password`, the NZB import state-setter pair whose two states are written
  by the paths that cause them, the scheduler's two `enqueue_direct` wrappers over
  `enqueue_direct_to_with_network`, and `compat::is_overridden`, which duplicated a line its one
  would-be caller already wrote itself; nine workspace dependencies no crate names; 38 locale
  keys in all four catalogues, checked against every one of the 102 dynamic `t()` calls in the
  interface rather than against a scan, so that a key composed at runtime could not be mistaken
  for a dead one; four store and composable members no view calls; and a comment pointing at a
  file RD-101-13 deleted. No behaviour changed and no test was dropped.

  One removal is worth knowing about if you build the web interface: `web/vite.config.js` and
  `web/vite.config.d.ts` were compiled leftovers from the initial commit, and because Vite
  prefers `vite.config.js` over `vite.config.ts`, the stale copy - not the TypeScript source -
  had been configuring every build. Nothing was silently lost along the way: `vite.config.ts` has
  exactly one commit, the initial one, so no edit to it was ever dropped. Edits to it now take
  effect.

- **The plugin groups are readable again (RD-107-17).** The type filter in Settings > Plugins was
  a `UTabs` bar, which divides one line among its entries - so the eleventh plugin world
  (`remote-job`) pushed it to twelve entries and ten of them rendered truncated: "Benachrich... 3",
  "Ordner-Cr... 6", "Anbietera... 1". Nobody could tell what the groups were. That was growth
  rather than a defect, so the replacement is a shape that grows with the number instead of a
  wider row: a wrapping chip row, one button per group carrying the full name and that group's
  count, the selected one filled and the rest outlined. It behaves the same at 400 px as at
  1600 px - only the number of lines changes - and a twelfth type adds a chip rather than
  shortening every label, which a test with an invented type holds it to. A select and a side
  list were weighed and rejected; `design.md` carries the rule and the reasons.

- **A post-processing step says what happened in one way, not two (RD-108-08).** 1.0.7 merged two
  independent answers to the same question. RD-107-04 gave a step a `code` field with parameters
  and translated it through `server.codes.<code>`; RD-107-11 was earlier, had no such field, and
  wrote its `extract.*` codes into the message text, which the interface then split apart again.
  Both formats survived the merge because unifying them needed a design change - `checkpoint_coded`
  carried no output path, and the unpack steps need one - and that does not belong in a conflict
  resolution.

  It carries one now. Extraction records its code in the field like every other step: the four
  reporting sites in the unpack and RAR-test stages pass `ExtractionError::code()`, the tool's own
  words travel as the `detail` parameter the catalogues interpolate, and `message` is plain English
  again - the fallback for a code a build does not know, and nothing to be parsed. The interface
  lost the workaround branch and has exactly one way to translate a step result. The extraction
  texts moved to `web/src/locales/*/server.json` under `codes`, where every other step code lives,
  in all four languages, and the old `downloads.postprocess.messages.*` keys are gone. Steps
  written by earlier versions are rewritten once by migration `0069`, so a package from before the
  change reads the same as one from after it.

- **The two folder crawlers are held to their contract, not just built (RD-108-06).** RD-107-05
  shipped `nextcloud-crawler` and `directory-index-crawler` with parser tests that ran natively,
  against documents. Whether the *component* behaves in the host the way the host expects was
  untested: `crawler_contract.rs` covered `premiumize-crawler` and nothing else, so both new
  plugins reached 1.0.7 with no test that ever loaded their `.wasm`.

  Twenty-nine contract tests now run the built components inside Wasmtime under the manifests they
  ship. They cover what a person pasting a folder address is promised - a public Nextcloud 29
  share becomes its files with names, sizes and its folder structure; an ownCloud answers on the
  older endpoint after the modern one is tried first; an Apache and an nginx index become their
  files without the parent link, the sort links or anything pointing off the crawled path - and
  what has to happen when it does not: an empty, a locked and a missing address each end with
  their own stable code, a wrong guess about an address hands it to the next crawler instead of
  ending the link, and a subfolder that is refused costs that subfolder rather than the other four
  hundred files. Both walks are held to their own limits at the component, sideways as well as
  down: a directory with 150 siblings is read exactly a hundred times and a listing of 600 files
  yields exactly 500 links, because breadth is the cheaper thing for a stranger to arrange - one
  page to serve, a hundred thousand requests to obey. The documents are fixtures under
  `crates/rd-plugin-ext/tests/fixtures/<plugin>/`: real multistatus answers with sabre/dav's
  namespaces and its 401 and 403 error documents, real `mod_autoindex` and `autoindex` pages with
  their icons, columns and human-readable sizes. A test in each file keeps them free of anything
  that could be a credential or a real address - share tokens, request tokens and host names are
  exactly what copying an answer off a live instance brings along.

  One of RD-107-05's three host gaps is now covered end to end and the tests say plainly where
  the other two stop. A manifest declaring `*` is measured against the list the host builds for
  that one call, so a request arriving at all proves the narrowing admits the pasted host; that
  the allow-list *refuses* everywhere else is stood in for by a stub, and what the test asserts
  there is that the refusal is carried out rather than turned into an empty folder. The method
  and header gate is not covered by these tests at all - `method_allowed` and `allowed_header`
  live in the production host the mock replaces - so what is proven is that the guest sends a
  `PROPFIND` with `Depth: 1` and `X-Requested-With`, and `rd-plugin-host`'s own tests keep
  saying that the host lets them through. The third gap, a wrong guess handing the address to
  the next crawler, is proven where it lives. The acceptances that need a running server - an
  ownCloud instance, a Nextcloud before 29, an open Apache and an open nginx - stay open in the
  job file; this checkout has neither network nor Docker.


### Fixed

- **NZB imports can be dragged like everything else in the LinkGrabber.** They had no place of
  their own in the manual order and were slotted in by arrival time, so one row could be moved
  and the row beneath it could not. Both kinds now share one order, by drag and by arrow key, and
  the order you had is preserved when you update.
- **The paused variant is offered on every LinkGrabber row**, not only on NZB imports. Adding a
  package without starting it was available in the toolbar and on one kind of row but not the
  other.
- **A client tailored to one job now also notices a plugin coming or going.** Installing,
  removing or switching off a plugin changes several lists — which hosters exist, which
  post-processing steps and upload targets are offered, which notification destinations — and
  each of those is readable with a different kind of access. The change was being announced on
  one channel only, so a token scoped to a single job could read a list it was never told had
  changed. In a browser this was invisible, because a signed-in session can see everything.
- **What one browser tab changes now shows up in the others.** Automations, managed external
  tools, plugin signing keys, withdrawn packages, and installing, removing or switching off a
  plugin all changed things the interface was never told about — so a second tab, or the same tab
  left open, kept showing what was true when it loaded. Nine screens reconcile by themselves now,
  including the provider, routing, post-processing and notification lists that are built from
  what is installed.
- **A plugin signing key or a withdrawn package no longer announces itself to the wrong
  audience.** Those changes were reported on the administration channel while the data they name
  belongs to the credentials area — so the token that made the change never heard about it, and a
  token that may not read that data was told an entry had changed. They travel on their own
  channel now.
- **A setting the service cannot read is now reported instead of quietly ignored.** The whole
  configuration lives in one stored document, and nineteen places read a slice of it and shrugged
  off a value they could not parse — so a single bad entry could leave the service running with no
  bandwidth limits and no free-space thresholds, with nothing said anywhere. The failure is logged
  with the exact field that caused it, and the places where starting on defaults would contradict
  what you configured refuse to start instead.
- **A package no longer appears for a moment without what the enrichers found.** Rating, cover
  and the rest were written a fraction of a second after the package itself, so anything looking
  at the queue in between saw an entry with none of it. They are written together now.
- **On Windows, a finished plugin download is no longer refused at the last step.** The file was
  still open elsewhere when it was moved into place, which Windows does not allow.
- **The live view catches up by itself after the connection falls behind.** The server had begun
  reporting that events were dropped; nothing acted on the report, so the figures stayed stale
  until the page was reloaded.
- **A download package can no longer be left half written by a client that disconnects.** Adding
  a package from the LinkGrabber is a sequence of writes, and until now dropping the connection
  in the middle could stop it between two of them: a package that read as complete while some of
  its links were gone, a package without the information the enricher plugins had found, or
  links left locked so the package could never be added again. The operation now either happens
  completely or not at all.
- **A package whose very first file could not be written no longer stays behind as an empty
  entry.** It could not be removed either, because removing a package happened as a side effect
  of removing its last file — and it had none.

- **An error from the database is no longer at risk of arriving as the wrong kind of error.**
  Nineteen places worked out what to answer — "not found", "still in use", "busy" — by looking
  for English words in an internal message, so rewording one sentence anywhere in the storage
  layer could silently turn a clear "this category is still in use" into a generic server error.
  Nothing reads those sentences any more.
- **Starting the service no longer verifies every installed plugin five times over.** Each part
  of the plugin system checked, validated and compiled the whole set for itself; with a few
  dozen plugins installed that was hundreds of redundant passes on every start.
- **A plugin manifest edited on disk no longer reaches the running service.** It was checked
  before the plugin itself was loaded, but not before its provider entry and its translations
  were taken over — so an edited file still contributed the addresses that plugin may reach and
  the credentials it may ask for.
- **Importing a very large settings bundle no longer blocks everything else** while it runs, and
  the SABnzbd-compatible API key no longer appears in this service's own logs.
- **On Windows, the vault's fallback key file can no longer be overwritten.** Replacing it makes
  every stored password permanently unreadable, which is exactly what the Linux version has
  always refused.
- **A full disk now restarts exactly the downloads it stopped.** Filling a storage root paused
  its transfers, but the automatic resume only ever looked at *blocked* ones — so freeing space
  restarted everything except the transfers that had actually been stopped, and they sat there
  until somebody noticed. The same release also restarted downloads that had been blocked for
  quite different reasons, including one where the file on the server had changed mid-transfer,
  which is the case the block exists to prevent. **After the update, downloads that an older
  version blocked need one manual resume**: the reason was not recorded back then, and guessing
  it would restart exactly the ones that must not be.
- **Switching off a download type now also stops the entries that had not started yet.** It
  never did — the attempt failed silently on every one of them, and they went on being picked
  up.
- **A file that was already finished is no longer downloaded a second time.** If the service
  stopped in the moment between moving a completed file into place and recording that it was
  done, the restart fetched the whole thing again and put the second copy beside the first as
  `name (1).ext`. The restart now recognises the finished file.
- **A recovery code is no longer used up by a wrong password**, and a two-factor code can no
  longer be used twice. The code stayed valid for up to about a minute and a half, so one read
  over your shoulder or caught by a fake login page could be replayed.
- **An archive bomb is stopped while it unpacks** instead of after. The size limit was only
  checked once the unpacker had finished, so the disk was already full by the time the job
  reported that the archive was too large.
- **The certificate authority you configured now also applies to Usenet.** It reached web and
  FTP downloads but not NNTP, so a server with a private certificate failed — including the
  "test connection" button, which could report a failure for a server that downloads then
  reached without trouble.
- **A category cannot silently stop being the default any more.** Creating the first category
  without ticking the switch, or deleting the default one, left the routing with no fallback at
  all. Storage roots were fixed this way a while ago; categories now work the same, and existing
  data is repaired on update.
- **A damaged subscription filter no longer means "accept everything".** It did, so a corrupt
  entry could quietly queue an entire feed. That subscription now reports an error instead.
- **The event log is kept to thirty days.** Nothing ever deleted from it, so on a long-running
  installation it was the largest thing in the database file.
- **Setting a category on many packages at once is no longer slow enough to block everything
  else.** It issued roughly three database round trips per package; it now takes a handful in
  total.
- **`--plugin-development-mode` no longer quietly permits more than it says.** It also allowed
  every transfer plugin to reach addresses on your own network. If you relied on that, the new
  `--plugin-allow-local-targets` is what you want.
- **A withdrawn plugin version can be refused without distrusting its whole signing key.** The
  mechanism existed and was connected to nothing.
- **A keyring that was merely unreachable no longer destroys the secret vault.** Reading the
  master key treated every keyring failure as "no key stored yet" — a locked login session or a
  keyring daemon that had not come up yet looked exactly like a first start — and the service
  then generated a fresh master key and wrote it over the stored one, leaving every account
  password, Usenet credential, two-factor seed and passkey unreadable for good. Only "there is
  no entry" is treated that way now; anything else stops the start with an error. A machine that
  genuinely has no keyring, such as a headless Linux server, still falls back to the key file as
  before.
- **Events that a slow browser tab missed are no longer lost in silence.** Both live update
  streams dropped the notice that the client had fallen behind, so the interface kept showing
  stale figures with nothing anywhere saying why. The server now says it, and logs it.
- **A pause, resume or delete through the qBittorrent-compatible interface that did nothing now
  reports failure.** It answered success regardless, so Sonarr, Radarr and the like marked the
  item handled and never came back to it.
- **A video whose title contains `%(...)` no longer lands under a name nobody chose**, and the
  finished file is recognised by a marker the downloader prints rather than by guessing which
  line of its output looks like a path.
- **The clipboard watcher stops hammering the service.** An error it cannot resolve — an expired
  pairing above all — made it resend the same links every second for as long as that text stayed
  on the clipboard. It now backs off to one attempt every five minutes, while a momentary
  hiccup is still retried at once.
- **Notification retries are spread out** instead of every target for one event coming back at
  the same second, so an endpoint that has just recovered is not hit by the whole backlog at
  once.
- **Two versions of the same plugin no longer do the work twice.** Link intake, crawling and
  metadata enrichment ran every installed version, so one paste produced duplicate candidates.
- **A plugin package is checked for a valid signature before its component is parsed at all**,
  the WebDAV discovery no longer reads an unbounded response into memory when the server
  declares no length, and a thumbnail download now has timeouts, a redirect limit and a size
  limit.
- **Sessions past their grace period are finally removed** — the 30-day sweep existed but was
  never called — and the notification delivery log is trimmed per rule, while notifications
  still waiting to go out are kept.
- **Finishing a download no longer reads the entire download table** to find the other files of
  its package.
- **Four ways a completed download could be the wrong bytes.** A `206 Partial Content` answer
  was trusted on its status alone: the `Content-Range` header that says *which* part is coming
  was only ever parsed for non-`206` successes, so a server answering `Range: bytes=8388608-`
  with the head of the file had it written at offset 8 MiB, checkpointed and reported complete.
  An FTP transfer only refused a file that was too *short*, so a server that accepts `REST` and
  then streams from byte zero anyway appended a second copy behind the part already on disk and
  was promoted. Neither FTP nor SFTP synced the part file before the rename that publishes it,
  so a crash straight afterwards left a file the queue calls Completed whose tail was still page
  cache. And yEnc compared a multipart article against `crc32`, the checksum of the whole
  assembled file, whenever the poster emitted no `pcrc32` — every such article was declared
  corrupt, burned through the backup servers and was left to PAR2.
- **A paused-then-reset download no longer sits in the queue forever.** Pause and cancel record
  a stop reason and the dispatcher skips every download that has one; reset put the row back to
  `queued` without clearing it, so the job was never picked up again — with no error anywhere —
  until the service restarted.
- **The hot folder no longer stops importing after one bad file.** A file that vanished between
  the stability check and the read, a full `processed` directory or a single unreadable
  subdirectory ended the watcher task for good, and nothing restarted it.
- **Deleting a category no longer fails with a database error.** A category still referenced by
  a link in the LinkGrabber hit a foreign key and answered with a raw SQLite message instead of
  a proper one; subscriptions and stream channels kept pointing at a category that was gone.
- **A malformed magnet link or video upload date no longer drops the connection.** Both were cut
  to a fixed length at a byte offset that a non-ASCII character can sit in the middle of.
- **Two authentication weaknesses.** The `X-Forwarded-For` chain was shortened from the wrong
  end, so a client that padded it with invented entries could push the proxy's record of its
  real address off the end and name its own — which sidesteps the per-address login limit. And
  the settings blob, which costs the configuration scope, carries the switch that disables the
  administrator login together with the trusted-proxy list and the paths of the programs this
  service runs; those fields now require the administration scope.
- **A plugin can no longer widen its own sandbox.** The memory, fuel and timeout budget a
  manifest declares was only refused when it was zero, so a manifest could ask for gigabytes and
  an effectively infinite instruction count and get it. A `"*"` in the network allowlist was
  likewise accepted for every plugin type, not only for the two whose destination the host
  narrows per call.
- **A text link list is read with the size limit it always documented**, the hoster catalogue is
  matched case-insensitively, `rclone` is invoked with `--` before its paths like every other
  external tool, and a failed plugin install no longer leaves a directory behind that the plugin
  manager lists as a second copy.
- **Timestamps follow the language you chose, and two of them appear at all again.** Eight
  components carried a private date formatter under three different names, and six more
  templates formatted a date inline. Half of them passed no locale, so those dates rendered in
  the *browser's* language while the rest of the interface rendered in the application's — a
  Spanish user on a German browser saw German dates on the subscriptions page and Spanish dates
  beside it. Separately, the notification history and the bandwidth status card asked for a date
  shape that had never been registered, and vue-i18n answers an unregistered shape with an empty
  string rather than a complaint, so both rendered nothing where a timestamp belonged. All
  nineteen places now go through one pair of helpers, every shape is named in one file, and a
  test asserts that each name a template passes has one.
- **A delete that the server answers with `204 No Content` is no longer reported as a failure.**
  Eight of the nine edit-in-place lists judged a delete by whether the response carried a body,
  which is true of every delete endpoint in this application except the one that removes a
  stream channel. They were right only by accident of their own endpoints; the shared version
  keys on the error instead, so the next such endpoint cannot break them all at once.

- **The enlarged cover no longer shows the row the pointer has already left.** Crossing a list
  of indexer hits from top to bottom, the preview sometimes stood there with the previous
  row's picture. Each row carries its own overlay, and a closing one kept its content for the
  100 ms of its fade — long enough, with the two pictures landing a thumbnail's height apart,
  to read as one picture that had failed to change. The closed state no longer animates, so
  the content of the row that lost the slot goes in the same tick, and the picture carries a
  key on its address, because an `img` keeps the pixels it has until a new address decodes.

- **An extraction no longer fails on a path Windows will not take, and a destination that
  refuses the file says so (RD-108-30).** Measured on 2026-09-18: a package that unpacked by
  hand without complaint failed in the application with `unrar` exit 9, `cannot create`. The
  path in that message is 275 characters and Windows stops at 260 — not because anything was
  unusual, but because the pieces add up: a 75-character release folder, an archive whose inner
  tree repeats that name, a file named after it again. That is 247 characters before the
  extraction contributes anything; the staging directory, `.rdownloader-extract-xxxxxx`, added
  28 more. Neither the password nor the RAR version nor the choice of tool had anything to do
  with it, which is what the old message left the reader to find out. The staging directory is
  now `.rd-xxxxxxx`, and every path handed to `unrar` or 7-Zip, and every move afterwards, is
  converted to the `\?\` form that the file system accepts without the limit — drive and UNC
  paths only, untouched elsewhere, a no-op outside Windows. The containment check that guards
  against an archive writing outside its staging directory now compares both sides in the same
  form, because a deep tree would otherwise have looked like an escape. The trailing separator
  handed to `unrar` is the platform's own rather than a forward slash, which a verbatim path
  does not take. And a write failure is now its own verdict: `unrar` 5, 6 and 9 and 7-Zip's
  `can not open output file` become `extract.cannot_write` — the destination refused the file,
  no further password will change that, and the message says which of the two questions to ask.

- **A Usenet server that cannot answer no longer costs the article — and no longer quietly
  costs the file (RD-108-29).** Measured on 2026-09-18 on the live instance: one provider
  answered `400 Archive server temporarily offline.` to 2312 `BODY` commands in a single
  afternoon, against six `430`. Every one of those 2312 was read as "this server does not have
  the article", so 2313 segments across 180 files were written as holes of zeros and the files
  were reported complete — thirty to seventy of about a hundred and forty segments per file,
  which is far past what a PAR2 set can repair. The archives unpacked with broken headers and
  checksum errors, while SABnzbd downloaded the same posts through the same account without a
  fault. It survives that server because it reconnects and asks again; RFC 3977 §3.2.1 has a
  `400` mean "not available right now, the connection is closing", not "gone". A status line
  now says what it is about. `430` and `423` are the article: this server does not have it, and
  only that may leave a hole for PAR2 to repair. Everything else is the server, and the article
  is asked for again — three times, on a fresh connection each time, with 250 ms, 500 ms and at
  most two seconds between attempts, which is SABnzbd's default. A body that does not decode
  and a connection that breaks mid-article count the same way: both said nothing about the
  article, and both used to cost it. When several servers are configured, one that could not
  answer outweighs every one that said no — whoever failed to answer did not say "no". If no
  server can deliver after all of that, the file goes back into the queue as a retryable
  failure (`usenet.server_unavailable`, sixty seconds) with its `.part` file and its
  checkpoints intact, so the next attempt resumes where it stopped instead of committing a file
  full of zeros. Pipelining was ruled out as the cause before any of this was written: the log
  shows it switched off at 11:56:01, and the refusals continue unchanged at one command per
  connection until 13:14.

- **DDownload downloads again, with and without an account, on the page the site serves
  now (RD-108-28).** Measured on 2026-09-17: the file page no longer carries the first
  `download1` form at all. It carries the `download2` form directly, with a Cloudflare
  Turnstile widget inside it and the countdown beside it. The free flow looked for the first
  form, did not find it and stopped before it ever reached the captcha handling; the premium
  flow found the second form and posted it without a token, and got a page back. Both then
  blamed the account — "the page requires a login" — because that diagnosis rested on the
  header's `/login` link, which is on every page a guest is shown (four times on the one
  measured), free and premium alike. Three things change. The free flow starts at the second
  step when the page offers no first one, so an installation that still serves two forms is
  handled as before. The premium flow answers a widget on the form through the same path the
  free flow uses (the desktop agent, or a configured solver) and sends the token; a form
  without a widget is posted exactly as before, because the page was measured without a
  session and whether a signed-in premium session is shown the widget is not yet known. And a
  login wall is now the login form itself, the one whose `op` is `login`: a page that merely
  lacks the form we were looking for says so, or repeats its own message, instead of naming a
  cause it cannot know. The widget scan also starts at the form rather than at the top of the
  page: the page's stylesheet mentions hCaptcha two thousand lines before the Turnstile, and
  a page-wide scan had reported the wrong challenge with the right site key. The trimmed
  page is in the tree as a fixture, so the next change of the site fails a test instead of a
  download. DDownload plugin 0.10.4. The narrower rule cuts both ways, so the session checks
  now rest on a positive, measured marker: a page is a session when it offers a sign-out link
  or label, and nothing else is — not the homepage a refused sign-in redirects to, not a
  maintenance page, not a page that merely lacks the login form. The marker is read in any
  spelling (`logout`, `log out`, `log-out`, `log_out`, any case) and as a *word*: the captured
  guest pages carry `dialog` sixteen times, so a plain substring search would have read
  `dialog outside` as a live session and reported a dead one as healthy. It is read on the page
  with `<style>`, `<script>` and `<!-- -->` blocks removed, because that is where the word turns
  up without the thing it marks — the guest page's own stylesheet comment says
  `Dashboard/Logout`, and its markup carries 55 comments, one of which narrates the auth state
  in prose. Measured on all three captures of 2026-09-17: zero word-boundary hits of any
  spelling on a guest page. That rule now also guards DDownload's cookie-only account check, which used
  to accept any HTTP 200 without reading the page, so a lapsed cookie session showed a green
  account until a download had spent a captcha on it; it is reported as
  `ddownload.cookie_session_invalid` instead. FileJoker's session check shares the rule; its
  tests were only ever fed the header link, and they now feed the sign-out link, the guest
  page and a page with neither. And a page with neither is the third answer, not the second:
  the account check condemned the account on everything that was not the sign-out link, which
  was the same mistake in the other direction — an account is invalid when the site shows its
  own guest markup (the login form, or the guest header), and a page carrying neither marker
  is one this code does not recognize. A Cloudflare interstitial, a maintenance notice or a
  signed-in header spelled in a language the marker does not cover would have told the user
  that the premium account their bug report says works is invalid, and sent them after cookies
  that were never the problem. Those pages are reported as
  `ddownload.cookie_session_unconfirmed` / `filejoker.session_unconfirmed` and are retryable.
  The distinction lives once, in `xfs_common::page::session_verdict`, so the sign-in path and
  the two account checks cannot drift apart again. DDownload plugin 0.10.4, FileJoker plugin
  0.7.1 — FileJoker's behaviour changed here too, and a version that does not rise leaves a
  running instance on the package it already has.
- **A Usenet answer belongs to its command, not to its place in the queue (RD-108-27).** After
  RD-108-25 put two `BODY` commands on every connection, a live provider answered them out of
  step: the earlier request on a connection read the later one's body — ten articles ahead at
  ten connections — and the assembly failed the file with `non-contiguous yEnc part range`,
  threw twenty in-flight articles away, waited out a backoff and tried again from the last
  checkpoint. Files finished, after many attempts, at a rate that fell to zero between
  spurts. The client had attributed answers to commands by order alone; it could not notice.
  RFC 3977 puts the message-id on every `222` line, and that is checked now: an answer naming
  another request of the same connection breaks that connection before the request behind it
  reads a byte, both articles are fetched again on a fresh one, and that server is down to
  one command per connection for the rest of the process — the behaviour before RD-108-25,
  which the field showed it fine with — with one warning naming it. A `222` line without any
  id, or naming an id the connection never asked for (a different spelling, an article nobody
  ordered), cannot be checked: the body is kept and that server, too, drops to one command per
  connection. A `430` names no id, so a refusal read beside another request is asked once
  more on a connection of its own before it is believed — a present article booked as
  missing would be a hole for PAR2, or a package failed for want of it. And a command leaves
  in one write, one TLS record, as SABnzbd sends it. Against a server that answers in order
  nothing changes.
- **PAR2 is recognised when the name is known, not when it is guessed (RD-108-23).** An
  obfuscated post quotes the release name first and the file name second; the subject parser
  took the first quoted group, found it was no file name, gave up, and named fifty queue rows
  after their whole subject line. Every PAR2 decision hung on that name: no row was marked as
  repair data, no volume was held back, and a volume that had expired on the servers failed the
  package with "the NZB contains no PAR2 repair data" — at seven PAR2 files. Four things change.
  The parser checks every quoted group and takes the last one that is a file name (SABnzbd takes
  the first unchecked and would have named this case wrong too). The moment an assembled file is
  on disk, the Usenet runner settles its row: the real name, the marking decided again on that
  name *and* on the file's header — a file that starts with `PAR2\0PKT` is repair data whatever
  it is called — and, when the file is the main index of a set, the set's volumes still waiting
  in the queue are postponed the way they would have been at enqueue time; a volume already
  downloading or finished is never postponed after the fact. The "no PAR2" verdict is taken on
  the same footing. And the interface no longer takes a `recovery: false` from the service as
  the last word when the name says `.par2`. A user rename decides the marking again as well.
- **Firefox loads the browser extension without a manifest error, and "send this page" sends it
  (RD-108-22).** Three things had been wrong since early September and shipped in 1.0.7 as well.
  `optional_permissions` listed `requestBody`, which is not a permission in any browser but an
  `extraInfoSpec` flag on a `webRequest` listener; Firefox validates that array and refused the
  entry with a long error on every install. The code asked for the same non-permission before
  registering its POST-body listener, so that listener has never once been active in a shipped
  build — the gate now asks for the host access it really needs and says so. And neither `tabs`
  nor `activeTab` was declared, so `tabs.query` handed the popup a tab without a `url` and the
  "send this page" button reported that the page had no address. `activeTab` is now declared:
  it covers the tab the person clicked the extension on, and only from that click.
- **The enlarged cover closes again when the pointer leaves it (RD-108-13).** It stayed where
  it was, long after the pointer had moved on. The size was not the cause: the row pinned the
  cover whenever the overlay reported itself open — which it does whenever *we* open it, a
  hover included, because the overlay is controlled. A pin outlives the pointer, so the picture
  stood there. Pinning is the tap's job again and the tap's alone. The tests could not have
  caught it: their popover stub read the state it was given and never reported one back, so the
  half of the component that held the defect ran in no test at all. It reports now, and the
  enlarged picture no longer takes the pointer away from the thumbnail it belongs to.
- **The enlarged cover really uses the height it was given (RD-108-13, second attempt).** The
  first attempt swapped a fixed 70-percent ceiling for one measured from the popover's own free
  space — and changed nothing visible, because a ceiling never enlarges anything and these
  covers arrive smaller than the window. The picture now takes that measured height outright;
  its width follows from the aspect ratio and is capped only against the window, so a portrait
  cover fills the popover instead of sitting in the middle of it at thumbnail size.

- **The plugin view counts plugins, and a superseded version can be got rid of (RD-108-10).**
  A package of 43 plugins reported "local inventory 44", and the difference was not a display
  error: installing a plugin never removes the older version, because a job already under way
  keeps the version that started it. The leftover stood in the list as a card of its own,
  carrying the "not in use" badge, and the counter counted it.

  The list is keyed by plugin now. The version that is loaded is the card; every superseded
  version sits under it in a collapsed sub-entry that says what it is, and the counter -- beside
  the heading and on every type chip -- counts plugins, which is the number a person compares
  with the package they installed.

  Each leftover carries a remove action, asked for through the same confirmation every
  destructive action uses. `DELETE /api/v1/plugins/{id}/{version}` refuses with `409` and the
  stable code `plugin.version_in_use` while unfinished work is still bound to that exact
  version -- a resolver pin a job claimed, or a transfer checkpoint only that version can read --
  and the refusal names the first of those downloads beside their number, so the job in the way
  can be found rather than searched for. Only a completed download holds nothing; a cancelled one
  keeps its partial data and can be resumed, so it keeps its claim until it is deleted. The guard
  covers the loaded version as well, so an uninstall cannot pull a running job's plugin out from
  under it either.

- **The LinkGrabber's enlarged cover uses the window height that is actually there
  (RD-108-13).** The popover the picture opens in was capped at 70 percent of the viewport
  height regardless of how tall the window was, so a portrait cover ran into that ceiling long
  before it ran into its width cap, leaving a third of the window unused. The cap now reads the
  space the popover itself has already worked out is free at the row's position -
  `--reka-popover-content-available-height`, which Reka's popper publishes once it has flipped
  side and alignment as needed to stay inside the window - so a row at the very top or the very
  bottom gets exactly as much image as its own placement leaves, instead of a fixed guess. The
  popover stays anchored to its row; the width cap (about 512 px) is unchanged.
- **The film and series metadata plugin matches the release it was asked about, or says nothing
  (RD-108-14).** Three rows in a running 1.0.7 carried a stranger's data: two episodes of a 2024
  anime wore the rating and runtime of a 1994 cinema film, and an episode of one series wore
  another series' name. Two causes, both now closed.

  An episode number without a season is an episode. `Dragon.Ball.DAIMA.E15...` carries no `S..`,
  so it was read as a film and looked up in the film catalogue - which is how a Dragon Ball
  *film* ended up beside it. Anime and a good deal of German television number straight through,
  and a lone `E15` says "episode" as plainly as `S02E02` does. The season stays unknown and is
  shown as nothing at all rather than guessed at; a lone `e`-and-digits token behind the quality
  and codec markers is still read as a group name, so `...x264-E11` remains the film it is.

  And the hit is now checked against what was searched for. Cinemeta's search is fuzzy: asked
  for "Apollo Has Fallen" it answers "Paris Has Fallen", and the plugin took the first hit
  without ever comparing the name. A hit is believed only when its name reads like the one
  searched for - a Dice coefficient over normalised words, articles and punctuation dropped,
  75 of 100 - or when it clears half that bar and its year is exactly the year in the release
  name, which is what keeps a title the release name cut short (`Dune.Part.Two.2024...` searches
  for "Dune") from being thrown away. Below that there is no field at all: a thin row lets
  somebody look the release up, a wrong row lets them decide on it. A release whose German title
  the catalogue holds only in English is now silent where it used to be wrong.

- **The captcha window claims nothing about itself and touches nothing of Cloudflare's
  (RD-107-20).** RD-107-18's measurements came back from a Windows desktop and found two things
  wrong in this project's own code rather than in the hoster's.

  The user agent RD-107-14 hardcoded is gone. It claimed `Chrome/138.0.0.0 ... Edg/138.0.0.0`,
  while the client hints WebView2 fills from its own runtime reported
  `[Microsoft Edge/152, Not?A_Brand/24, Chromium/152, Microsoft Edge WebView2/152]` in the same
  log line. WebView2 derives neither `sec-ch-ua` nor `navigator.userAgentData` from a user-agent
  override and Cloudflare checks the pair against each other, so the claim did not standardise
  the browsing context - it manufactured a contradiction that reads as a bot. Nothing is stated
  now, and the engine sends the agent that matches its own hints.
  `RDOWNLOADER_CAPTCHA_USER_AGENT` survives the string: unset and `off` (also `0`, `no`, `false`,
  `default` or empty) both state nothing, any other value is forced verbatim for a measurement.

  The half of RD-107-18's diagnostic that the widget can feel is now off unless it is asked for,
  behind `RDOWNLOADER_CAPTCHA_WIDGET_ERROR_PROBE` (`on`, `1`, `yes` or `true`). Getting
  Turnstile's refusal code meant replacing `window.turnstile` with an accessor and writing
  `data-error-callback` into Cloudflare's own markup, which is precisely what `api.js` is
  hardened against; the run that shipped it reported the window open, the page there and no
  widget at all, where before the widget had rendered and merely failed verification - and no
  error code was ever logged, because nothing got far enough to produce one. By default the
  injected script now contains only what it did before that build: the read of the widget's own
  answer field, and one read of `navigator` for the environment report.

  The passive measurements are untouched and stay on, because they are what produced both
  findings: the environment report, and the count of files and bytes in the profile directory on
  the way out. That count also dismissed RD-107-14's other suspect - the profile held 237 files
  and 21 MB, so it is reaching the engine and stays exactly as it is.

- **A clipboard the collector turned down is not offered again every second (RD-107-19).** The
  capture agent submitted the same clipboard text once per tick and recorded it as handled only
  on success, so `collector.all_links_excluded` - every address matched by the domain blocklist -
  produced a warning a second for as long as the text sat on the clipboard; twelve in twelve
  seconds in the reported log, and no end. That retry is right for a transient failure and wrong
  for a decision: the collector looked at the addresses and applied its blocklist, and the answer
  will not change on the thousandth attempt.

  The agent now separates the two on the stable `code`, not on the text of the message. A refused
  response is carried out of the client as a typed `ServiceRefusal` holding the status and the
  code alongside the message the logs already showed, and three codes count as decided:
  `collector.all_links_excluded`, `collector.all_links_disabled` and `collector.no_links_found`.
  Each is recorded as handled and logged as declined, so "skipped" no longer reads like "broken".
  Everything else keeps its retry, deliberately - a network error, a 5xx, an expired token, a code
  this build has never heard of - because a real failure quietly filed as handled would never be
  reported again, and the agent regularly arrives before the service does (RD-107-15).

- **The capture agent no longer says "server not reachable" while the service is still starting
  (RD-107-15).** The tray polls `/api/v1/health` and gave a silent service twenty seconds of
  grace before calling it unreachable. That figure was set when the service was a fraction of
  what it now is: it opens the database, applies the migrations and verifies, compiles and
  installs more than forty signed plugin components *before* it binds its listener, and a cold
  machine can still be working at that after a full minute - which is exactly the moment both
  are launched together at login. The grace is now ninety seconds, justified in the code rather
  than merely raised; a service that never comes still ends up as "server not reachable", the
  message just no longer arrives while the service is coming up.

  The same poll also conflated two different answers: a refused connection and a reply carrying
  an error status were both simply "not reachable". They say different things. Nothing listening
  is what a starting service looks like from outside, so it now reads as "starting" for as long
  as the grace lasts, while something that answers without success is already there and unhappy
  and is reported as not reachable at once. The decision is a pure function next to the states it
  produces, so it is unit-tested on every host instead of living inside a tray loop that only
  compiles on Windows and macOS.

- **Only one enlarged cover is open in a hit list (RD-107-16).** Every row in the LinkGrabber's
  review list and the subscription archive decided for itself whether its cover popover was
  open, so two could stand open at once: a pinned one beside a hovered one, and a fast traversal
  where the leaving row's `mouseleave` arrived after the next row's `mouseenter`. One owner
  outside the component tree now holds which row is open, and a row releases the slot only while
  it still holds it. The three ways in are unchanged — pointer, keyboard focus, tap — and a
  pinned cover is no longer lost to a pointer merely passing another row: hovering borrows the
  single slot and hands it back, so the pinned one is covered rather than discarded
  (`design.md`).

- **The captcha window no longer browses incognito (RD-107-14).** The first Windows acceptance of
  the widget captcha failed with Cloudflare's own "verification failed" - the window opened, the
  Turnstile widget rendered, and there was no `110200 - domain not allowed`, so what failed was
  the browsing context and not the architecture. The web view was built with an in-memory private
  profile, and Turnstile relies on cookies (`cf_clearance`, `__cf_bm`) and on a context that
  survives a reload. It now uses a persistent profile directory of its own under the agent's data
  directory - `%LOCALAPPDATA%\rDownloader\Capture\data\captcha-webview` on Windows - which keeps
  the separation the private profile was reached for (no access to your browser, none to the
  credential vault, one folder that can be deleted whole) while giving Cloudflare somewhere to
  write. The window also states an explicit, ordinary user agent instead of whatever the installed
  runtime volunteers. Whether this is enough is settled on a Windows desktop and nowhere else; the
  change is guarded here by tests over the rule - the profile path and the user agent - and by a
  source check that the private profile does not come back.


## [1.0.7] - 2026-09-15

Thirteen jobs: the three the roadmap had lined up, three the previous milestones named as not
contained, and seven that a week of running the released 1.0.6 asked for. The corrections are the
half most people will notice - clearing the download list no longer reaches into a package that is
still running, a category finally reaches an NZB file, an NZB can be queued paused, and a recovery
volume nobody needed stops being reported as a failed download.

Two of those seven began as questions rather than defects, and both were answered by measuring
rather than by guessing. **Unpacking failing where the same password works by hand is a locale
problem**: `unrar` converts the `-p` password from bytes to `wchar_t` through the process locale,
so a service without `LANG` runs under `C`, the same bytes become different characters, and the
tool itself says "Incorrect password". Running the real binaries under four environments settled
it - and showed that `env_clear()` alone would have made it worse, because an empty environment
*is* the C locale. **Clearing the list does not delete a finished payload directly**; it removes
the queue rows, and post-processing then assembles its file list from those rows, so a finished
file whose row is gone is invisible to PAR2, unpack and the archive cleanup that walks the
directory instead.

**What this release does not contain**, named rather than left to be discovered:

- **Real-Debrid's torrents still do not run (RD-107-06).** What this release ships is the ADR that
  says where such a thing lives: an eleventh world, `remote-job`, with every durable piece of
  state in the host rather than the plugin. The contract, the host wrapper, the row, the migration
  and the Real-Debrid plugin are all built and the plugin compiles to a component that imports
  nothing outside `rdownloader:plugin` - but nothing drives it. There is no sweep, no adapter, no
  REST surface and no interface, so a magnet still cannot be handed to an account. The ADR was the
  point: Premiumize, AllDebrid and Debrid-Link are one plugin each after it, and no contract
  change.
- **Turnstile cannot be answered in the embedded window, and now we know why (RD-107-03).** Three
  runs on a real Windows desktop settled it, and the answer was in the agent's own log:
  `brands=[Microsoft Edge/152, Not?A_Brand/24, Chromium/152, Microsoft Edge WebView2/152]`. The
  engine announces itself as a WebView2 in the client hints, which an ordinary browser does not,
  so Cloudflare has nothing to guess. That is why the user agent did not help - it can be set,
  while the hints come from the runtime - and why the persistent profile did not either. Making
  the engine hide what it is would be defeating a bot check rather than letting a person answer
  one, so the window is not the place: 1.0.8 continues this through the browser extension, where
  the person solves it in their real browser. What this release does carry and is unaffected: the
  contract for all four captcha kinds, the broker, the capture-side REST surface, the navigation
  fence, and image captchas.
- **The earlier note about the widget captcha (RD-107-03).** The window opens and the Turnstile widget renders - so the architecture the job
  measured its way to is right, and there is no `110200 - domain not allowed` - but Cloudflare's
  own check then reports "verification failed". The likely cause is named in the job file and is
  the WebView's own comment: it is built with an in-memory incognito profile, and Turnstile needs
  cookies and a persistent browsing context. Treat the feature as not working in 1.0.7.
- **The rest of the widget captcha has still never been seen (RD-107-03).** Its acceptance is behavioural
  and belongs on a Windows or macOS desktop; this release was cut on Linux. What is proven is that
  the Linux build carries no GUI stack at all - measured, not argued - and that the WebView code
  compiles for Windows with `wry` and `webview2-com` really built. No window has ever opened, no
  Turnstile widget has rendered, and whether DDownload accepts a token produced in the agent for a
  login the plugin then sends over its own connection is a question the measurement could not
  reach. Fourteen desktop verification steps are written down in the job file.
- **The two folder crawlers have no contract test.** `directory-index-crawler` and
  `nextcloud-crawler` parse real Apache, nginx, Nextcloud and ownCloud documents in their unit
  tests, and both components import nothing outside the contract. What is missing is a run of the
  built component against the host, of the kind the new metadata enricher does have. Neither has
  been pointed at a running server.
- **The metadata enricher has never met its source.** Cinemeta was chosen for being keyless, and
  every one of its answers is parsed defensively - a body that is not JSON, a rating that is not a
  number and a missing field each yield no field rather than a wrong one. But the live response
  shape is unverified, so a thinner row is possible where a fuller one was expected.
- **No Usenet server withheld a segment.** RD-107-04's way back from post-processing into
  downloading is proven against the database and the pipeline, including a hand-built PAR2 index
  that the real parser reads. It has not been proven against a server that actually loses volumes.
- **RD-107-02 avoided a contract change and did not prove the consequence.** The indexer
  attributes ride inside the JSON the enricher already receives, so no WIT field was added and no
  shipped component needs rebuilding - the right call, because an added field in a WIT record is
  not additive for a component already signed. What was not done is running
  `sponsorblock-enricher` against the new host to watch it stay unaffected.

### Added

- **A widget captcha can be answered by the person, in the hoster's own page, without a paid
  solver service (RD-107-03).** The whole path existed except the piece that renders: the
  plugin contract has carried reCAPTCHA v2, hCaptcha and Turnstile since the beginning, the
  DDownload plugin already asks for one through `solve-captcha`, and `rd-captcha` already
  queues it. What was missing was somewhere to show it.

  The architecture follows a measurement rather than a preference. Against DDownload's real
  site key on 2026-09-08, a page served from `http://localhost:18710` earns **`110200 - domain
  not allowed`** and never renders a challenge at all, while `https://ddownload.com/login.html`
  renders it normally. A modal in the web interface is therefore dead for that key, whatever
  is done to it — the check hangs on the domain of the embedding page. A window that loads the
  *real* page is the only place the answer can be produced.

  So the desktop capture agent grows one, on the event loop it already runs for the tray icon:
  a `wry` WebView using the operating system's own engine (WebView2 on Windows, WKWebView on
  macOS — nothing is bundled). It opens the hoster's page, the person solves the challenge the
  way they would in their browser, and an injected script that reads one field and touches
  nothing else hands the token back. Closing the window declines the captcha and still reports
  `captcha.skipped`.

  **Windows and macOS share one implementation behind one `cfg`; Linux stays out on purpose.**
  The agent deliberately links no GUI stack there, and making WebKitGTK a runtime dependency
  of a binary that has none is a decision of its own rather than a side effect of this one. A
  test reads `crates/rd-capture/Cargo.toml` and fails if any of `image`, `open`, `tao`,
  `tray-icon` or `wry` leaves the target gate, so the headless Linux build cannot quietly
  acquire one. Installations with no desktop at all — a container, a NAS — are unchanged and
  keep the solver service or the browser extension.

  Four boundaries, none of them negotiable. The plugin host now holds a widget challenge's
  page to the plugin's **declared domains**, the same fence `net_http` has always had, and
  refuses with `plugin.captcha_target_not_allowed` before the broker ever sees it — on both
  the component and the built-in resolver path. The window itself may then navigate only
  within the hoster's domain and the captcha vendor's own origin, over HTTPS. Its cookie store
  is private and in-memory, it never sees the vault, and nothing is typed into it on the
  user's behalf. The token is treated as the short-lived credential it is: passed straight to
  the waiting resolver, never stored, and absent from every log, the event stream and the API.

  The lock in `manual.rs` is opened for a trusted source rather than removed: a **typed**
  answer is still refused for a widget, because nothing typed outside that page is accepted.
  New capture-scoped routes `GET /api/v1/capture/captchas`,
  `POST /api/v1/capture/captchas/{id}/token` and `POST /api/v1/capture/captchas/{id}/skip`
  serve the agent, narrowed to the page and the widget kind; the capture event stream carries
  the captcha announcement stripped to a count of waiting widgets, so a capture token learns
  that something is waiting and nothing more.

  **Not yet accepted.** The behavioural criteria are ticked on a Windows or macOS desktop, and
  this was written on Linux. `docs/roadmap/jobs/107-03-widget-captchas-selbst-loesen.md` lists
  exactly what a person has to run there.

- **A job that runs at the provider now has a place to live (RD-107-06).** RD-106-03 left
  Real-Debrid's torrent half undone and named the reason: a magnet is neither a link you
  resolve nor a folder you list. It is handed to somebody else's provider, it runs there for
  minutes or hours, it stops half-way to ask which files are wanted, and it leaves something in
  the account that only an explicit delete removes. No world in `rdownloader:plugin` carried
  any of that — and Premiumize, AllDebrid and Debrid-Link have the same mechanism waiting
  behind them, so the next provider job would have met the same wall.

  `docs/adr/0003-a-job-that-runs-at-the-provider.md` decides where it lives, with the four
  designs that were refused: native provider code in the host, a multi-call crawler, a
  `transfer` backend, and a `resolve` that answers "not yet". The last of those is the one that
  would have done the most damage — a `transient` failure means *try the same call again*, and
  trying `resolve(magnet)` again is another `addMagnet`.

  The decision is an eleventh world, `remote-job`, whose seven calls — `claims`, `identify`,
  `submit`, `adopt`, `poll`, `choose`, `discard` — are all short and none of which waits. What
  has to last belongs to the host: the row, the remote identifier, the clock, the person's
  answer and the restart. **`api_version` stays `0.6.0`**, because a new interface and a new
  world take nothing away from an existing world; the contract is mirrored byte-for-byte into
  all ten `sdk/templates/*/wit/` copies.

  The idempotency argument is written down rather than left to a sequence of statements.
  `identify` derives a content key locally and without a request; migration `0065` writes it
  into the row **before** `submit` is called, with `UNIQUE(account_id, content_key)` behind it,
  so a second submit of the same magnet cannot become a second row. The one window a row cannot
  close — a crash between the request going out and the identifier coming back — is closed by
  `adopt`, which asks the provider what it already holds for that key. Two attempts are the
  ceiling; a third would be an unbounded retry against an endpoint that is not idempotent.

  `plugins/realdebrid-torrents/` is the first implementation: magnets and `.torrent` files, the
  info hash as the content key for both shapes, Real-Debrid's ten torrent states mapped onto the
  contract's five, and twenty failure codes in German, English, Spanish and French. It is the
  forty-third bundled plugin and a sibling of `plugins/realdebrid/`, which unrestricts the links
  it hands back.

  **What this release does not carry:** the sweep that drives those rows, the REST surface, the
  interface for choosing and deleting, and any run against a real Real-Debrid account. The job
  file records each as unsatisfied rather than as covered by a mock.

- **An enricher is told what the indexer already said, and what it answers outlives the
  LinkGrabber (RD-107-02).** Two gaps with the same effect: the extra information was worth
  less than the path to it would allow.

  Newznab and Torznab feeds routinely deliver `imdb`, `imdbscore`, `imdbplot` and `coverurl`,
  and those are kept under guard on the subscription item. The `enrich-subject` a plugin is
  asked with carried only the address, the file name and the resolved media metadata, so an
  enricher had to guess the title out of a file name while an unambiguous IMDb id sat one row
  away in the database. It now rides inside the JSON `known` already carries, under an added
  `indexer` key, with the media metadata left exactly where it was at the top level. **No
  contract change:** `rdownloader:plugin` is untouched, `api_version` does not move, and none
  of the nine SDK template copies of the WIT is affected — a field added to a WIT record is
  not additive for a component already shipped, so that route would have broken every
  installed enricher, `sponsorblock-enricher` included, to give one of them a field more. The
  shape is documented in `docs/plugins.md`.

  The attributes pass the gate `crates/rd-subscription/src/attributes.rs` performs a second
  time, on the way out rather than only on the way in: names that are credentials are dropped
  whole, a passkey inside a `magneturl` or `nfo` value is redacted, cover addresses survive
  only as absolute `http`/`https`, and Newznab's `password` reaches the plugin as its flag and
  never as a password. A link with no subscription behind it is asked with nothing added and
  behaves exactly as before.

  What an enricher answered hung on `link_candidates` alone. A subscription in `auto_queue`
  mode moves a candidate on within seconds, so a rating was visible for about that long and
  absent everywhere it would later be looked for. Migration `0063` adds the enricher fields to
  the package and to each queue row, the enqueue carries them across, and the download list
  shows them as the same chips the LinkGrabber row uses — with the plugin that said so and
  when. Existing packages are not backfilled: their candidates are gone, so there is nothing
  honest to fill them from.

### Fixed

- **Clearing the download list no longer takes finished files out of a package that is still
  downloading (RD-107-07).** "Remove completed" was built entirely in the browser: the store
  picked every row whose own state was `completed`, cancelled anything active among them and
  deleted them one at a time, in up to fifteen rounds. No layer asked what else was in the
  package — not the store, not the handler, not the scheduler, not the database — so a package
  that was still downloading lost the rows of the files it had already finished.

  Tracing what that costs on disk: the removal itself only ever deletes the incomplete `.part`
  file in the staging directory, so a finished payload survives the click. What does not survive
  is the knowledge that it belongs to the set. Post-processing assembles the files it works on
  from the package's database rows, so PAR2 repair, SFV, unpack and "delete archives" no longer
  see a member whose row was cleared; when the last running file finishes, the pipeline starts
  anyway and cannot put the archive back together, and the one step that walks the directory
  instead of the rows — the extension and sample cleanup — deletes those orphaned files outright.
  Clearing with "all" additionally cancelled every running file and dropped its partial data.

  The action is now a single server endpoint, `POST /api/v1/packages/clear`, that works on whole
  packages: a package goes completely or not at all, and one member that is still running,
  waiting, paused, seeding or being post-processed keeps the whole package. The rule is the
  predicate the timed removal already used, shared rather than copied. The response says how many
  packages were removed and names each one it left alone with a stable code —
  `package.members_active`, `package.members_seeding`, `package.postprocess_running` or
  `package.members_unfinished` — translated in all four catalogues, and the notice in the list
  reports them grouped by reason instead of only counting what went.

  Deleting a package outright still cancels its running files and discards their partial data,
  but that is now refused with `409` unless the request explicitly asks to force it. The
  confirmation dialog names the cost before sending the flag; `POST /api/v1/packages/delete`
  takes `force`, `DELETE /api/v1/packages/{id}` takes `?force=true`, and the MCP tool takes a
  `force` argument. The SABnzbd and qBittorrent adapters force it, because their protocols have
  no way to carry a confirmation back.

- **A PAR2 volume that never arrived no longer makes a finished package look broken
  (RD-107-10).** Three Usenet packages carried the badges "Done" and "Unpacked", sat at
  `2.0 / 2.0 GiB` -- and still reported `47/48 - 1 error` at 99%, plus a desktop notification
  saying a download had failed. The file it named was a single `vol...par2` volume that had
  expired on the servers, and nothing had ever asked for it: the payload was complete and the
  archives unpacked without a repair.

  The server was already right about the package -- it ends `Completed` and raises no failure
  event -- but nothing on the download row said which files are repair data and which are
  payload. PAR2 was recognized only on disk, in post-processing. A queued NZB now marks its
  PAR2 files as recovery when the import is enqueued, existing queue rows are marked by the
  same rule in the migration, and a recovery volume whose segments are all gone reports the
  distinct code `usenet.recovery_unavailable` instead of the one for a lost payload file.

  With that distinction the queue stops counting what nobody needed: a failed recovery volume
  in a package whose payload is complete is dismissed the way a mirror that stood down already
  was -- out of the error count, out of the size total, and counted as settled in the file
  fraction, so the package reads `48/48` at 100%. Such a volume raises no desktop notification
  either, because whether it was needed is not known when it fails. The boundary is unchanged
  and covered by tests: a missing payload file still fails, still counts and still notifies,
  and a package that really lacks repair blocks still reports itself as failed.

- **An archive that opens by hand now opens in the service too, and a failure says what failed
  (RD-107-11).** The report was "unpacking failed, but when I copy the password and unpack the
  files myself everything is fine". Measured against unrar 7.12/6.24 and 7zz 25.01, the cause is
  the process locale: `unrar` decodes its `-p` argument through it, and a service started without
  `LANG` runs under `C`, where the same UTF-8 bytes become different characters and derive a
  different key. The same argument that exits 0 under `LC_ALL=C.UTF-8` exits 11 with "Incorrect
  password" under `LC_ALL=C` — and under an empty environment, so clearing it would not have
  helped. `LC_ALL` and `LANG` are now set explicitly before either tool starts, keeping an
  inherited UTF-8 locale and falling back to `C.UTF-8`. 7-Zip was never affected.

  The verdict itself was the second half of the problem. Any output containing the word
  `password` anywhere was called a wrong password — including a checksum error, a truncated
  volume and a rejected command line. Classification now follows the exit code, with the tool's
  wording only where the exit code genuinely cannot decide: 7-Zip answers 2 for every encryption
  failure, and a RAR4 volume without encrypted headers carries no password check value, so
  `unrar` cannot separate the two either. Those cases say "wrong password or damaged data"
  instead of picking one, damaged data stops the pointless password retry, and every message is a
  stable code translated in all four catalogues. The exit-code table is in
  `docs/postprocessing.md`.

  Three smaller defects found on the way: a package password now survives its own leading or
  trailing whitespace (the trimmed form follows as a second candidate, and `passwords.txt` stays
  the one place that trims); the entry volume of a set that contains both `Release.rar` and
  `Release.part01.rar` is chosen deterministically instead of by directory order; and
  `rar_executable` is checked against `rar_tool` before anything runs, because unrar syntax sent
  to 7-Zip means `-p-` = "the password is `-`" and produced a real wrong-password verdict for a
  correct password.

- **Categories now apply to NZB files, not only to links (RD-107-08).** A watched folder's own
  category always worked; what never ran for an NZB *file* was everything else. `add_nzb_import`
  stored the category it was handed and nothing more, so a folder without one produced an import
  with no category at all, a rule for `source = hotfolder` and `extension = nzb` could not fire,
  and the default category was never reached. The package then carried no category badge and
  category-bound post-processing fell back to the global setting — while the files still landed
  in the right folder, because the destination lookup had its own fallback. That is why the
  defect was visible only at the badge.

  An arriving NZB now runs the same category selection a pasted link runs, through the same
  `rd_collector::select_category` and the same routing configuration. The precedence is the one
  the candidate path already used, and it is written down in the code: a category somebody chose
  for this intake wins — the folder's setting, the upload form's field, a SABnzbd client's `cat`
  — then a matching rule, then the category marked as default. The web upload and the SABnzbd
  `addfile` endpoint take exactly that route, and both now resolve the destination directory from
  the category the import actually received, so folder and badge cannot disagree.

  The same file dropped a second time is routed again instead of keeping what the first drop
  decided; an import that has already become a package is left alone. The comment in the SABnzbd
  adapter that claimed the routing rules decided where those files land said the opposite of what
  the code did, and has been corrected.

- **The indexer box under the LinkGrabber list only appears where there are indexer subscriptions
  (RD-107-12).** It was mounted unconditionally, and its outer frame carried no condition of its
  own, so an installation with no indexer subscription — the majority of them — got a bordered
  box, a title, a chevron and a "check all" button under the link list whose only content was the
  sentence that nothing was set up. The section now renders only with at least one subscription of
  kind `indexer`, and stays away during the first fetch rather than appearing and vanishing again.
  With the box gone, `linkgrabber.indexers.none_configured` had no reachable caller left and was
  removed from all four catalogues; the "check all" button it used to answer for now only exists
  where there is something to check. The opening behaviour from RD-106-18 is untouched.

- **"Add paused" now holds for NZBs as well as for links** (RD-107-09). Three layers refused it,
  one of them silently. The button checked the collector packages while its neighbour checked the
  whole list, so a LinkGrabber holding nothing but hotfolder NZBs left it permanently disabled;
  the call then dropped every NZB from a paused enqueue on purpose, because the path behind it
  could not do it — `POST /api/v1/nzb/imports/{id}/enqueue` took nothing but the id and wrote
  every download row as `queued`. The endpoint now accepts an optional `{ "paused": true }` body
  (a request without one behaves exactly as before), and the state travels down to the rows the
  import creates. The silent twin is closed with it: an NZB *candidate* in a collector package
  short-circuits into the same import path, which never carried `start_paused` — so a package
  enqueued paused started its NZB immediately, with no error and no disabled control to show for
  it. The per-row NZB button and the LinkGrabber's selection bar offer "add paused" too.

### Added

- **A film or series hit in the LinkGrabber says what it is, not just what it is called
  (RD-107-01).** A second bundled enricher, `plugins/metadata-enricher/`, reads the release
  name an indexer or a torrent hands over -- dots instead of spaces, resolution, source, group
  at the end -- and adds rating, year, genre and runtime to the row. An episode is recognised
  as one and carries its series, season and number instead of a film title.

  The shape of this plugin is decided by two things it cannot do. It has no account: an
  enricher is instantiated without one, so the host expands no `{{secret:...}}` on this path
  and every service that wants an API key is out of reach until that is decided on its own
  terms. The source is therefore Cinemeta, which wants none, and `v3-cinemeta.strem.io` is
  the single entry in the manifest's domain allowlist. And `claims` cannot help it: with
  SponsorBlock the URL carries the responsibility, but an indexer hit's URL points at the
  indexer while the content sits in the file name -- so this plugin is asked about *every*
  link with an empty claim list.

  That makes the negative path the important one, and it is answered without a single outward
  request. A name is treated as a release name only when it carries a release marker -- a
  year, a season/episode marker, or one of the quality, source and codec tokens -- and an
  extension that settles the question (`.exe`, `.pdf`, `.mp3`, `.srt`, a subtitle or a sidecar)
  ends it earlier still. `setup.exe`, `holiday-photos.zip` and `holiday.mkv` each produce no
  field and reach nobody.

  Nothing it does is a failure. A source that does not answer, answers slowly or answers with
  nonsense leaves the row exactly as the online check left it, and the check stays successful;
  an episode keeps its season and number either way, because those came from the name. The
  fields are namespaced `metadata.*`, so none of them can displace something the core resolved,
  and metadata enrichment stays the global opt-in it has been since 0.9.0 -- with the switch
  off, nothing is asked at all.

- **A folder link no longer has to belong to a service somebody wrote a plugin for
  (RD-107-05).** Two new folder crawlers ship with the release, and three decisions in the host
  had to be made before either of them could exist.

  `PROPFIND` is now a reading method. It used to sit behind the write gate that only a storage
  destination passes — not because it writes, but because it is an unusual verb — and WebDAV's
  directory listing is the first thing a folder crawler needs. The gate now reads `GET`, `POST`,
  `HEAD` and `PROPFIND` for every plugin, and `PUT`, `DELETE` and `MKCOL` still only for a
  storage destination. `Depth`, the header a listing is useless without, is allowed with it;
  `plugins/webdav-storage`'s verification `PROPFIND` had been sending it into a header gate that
  refused it.

  A crawler may now declare `*` in `capabilities.net_http.domains`, the way a storage
  destination already does, and the grant that actually applies is narrower than the manifest:
  the host builds the store for one crawl and narrows it to the host of the address that was
  pasted. So `*` reads as "wherever the address points", not as "anywhere", and a redirect off
  that host is refused exactly as before. A crawler that named its domains keeps them and is not
  narrowed — `premiumize-crawler` is asked about `premiumize.me` and fetches `www.premiumize.me`.

  A crawler that claimed an address wrongly no longer ends the link. A crawler that recognises a
  share by the shape of its path cannot avoid being wrong sometimes, and the selection used to
  take the first claimer and return its answer, refusal included. A refusal of kind `unsupported`
  now means "not mine after all": the search carries on to the next crawler, and an address every
  crawler disclaims stays exactly what it was instead of becoming an error somebody has to read.
  Crawlers are also asked in an order now — a crawler that declares `extension.generic = true`
  in its manifest is asked after every crawler that names a service, and within each group by
  name so the answer does not depend on the order the installer read the directory in.

  `plugins/nextcloud-crawler/` lists a public Nextcloud or ownCloud folder share: the modern
  public endpoint `/public.php/dav/files/<token>` from Nextcloud 29 on, falling back to
  `/public.php/webdav/` for older Nextcloud and for ownCloud, with names, sizes, subfolder
  structure and the shared folder's name as the package suggestion. A password-protected share is
  opened by appending the password to the address after a `#`; without one the refusal says so
  rather than reporting an empty folder. JDownloader covers Nextcloud, ownCloud, Seafile and
  WebDAV with nothing at all across its 857 decrypters.

  `plugins/directory-index-crawler/` lists an open directory index of Apache, nginx, Caddy or
  lighttpd — JDownloader's `GenericHTTPDirectoryIndexCrawler`, and the plugin that covers the
  most ground per line of code. It claims any address whose path ends in a slash, recognises a
  listing by the way every one of those servers marks the way back up, and keeps only the links
  that resolve strictly one level below the address it was given, so a page cannot point the
  crawl at another host or further up. Both crawlers bound their own walk at four levels, 500
  files and 100 directories, exactly as `premiumize-crawler` does.

- **PAR2 recovery volumes are postponed and only fetched when a repair needs them (RD-107-04).**
  Every `vol` volume of an NZB used to be downloaded whether or not anything was damaged, which
  for an intact release is recovery data nobody ever reads. SABnzbd has held them back since
  forever (`postpone_pars`), and the reason this did not follow was not the progress display —
  it was that there was no way back: postponement is decided when the NZB is queued, but the
  re-queue only arises *during* post-processing, at a point where the scheduler already treats
  the package as downloaded and the pipeline is running. Nothing could move a package from
  post-processing back into downloading.

  That way back now exists, and it is built out of state that survives a restart rather than out
  of an in-memory wait. A `vol` file enters the queue as a `skipped` row — the main index does
  not, because it is what answers whether anything is damaged, and a set without a main index
  postpones nothing at all. When the PAR2 stage reports missing blocks, the postponed volumes are
  sorted by the block count their own names announce (`release.vol031+16.par2` carries sixteen;
  both the par2cmdline `+` and the QuickPar `-` spelling are read) and taken from the small end
  until the gap is covered and no further — SABnzbd's `get_extra_blocks`, and the reason the one
  big volume is not simply grabbed. Those rows go back to `queued`, the package goes back to
  `downloading`, which also releases the post-processing hold, and the dispatcher picks them up
  like any other queued file. When the last of them lands, the same completion listener that
  starts post-processing in the first place requests the package again and the whole pipeline
  runs from the top with the new blocks on disk. A crash in between finds queued rows and a
  downloading package and carries on; a package whose volumes are still arriving is skipped by
  the pipeline instead of being verified a second time, so the recovery pass at startup cannot
  order the same gap covered twice.

  A set that still cannot cover the gap says so with a stable code instead of failing silently:
  post-processing steps now carry `code` and `params` beside their English message, and
  `postprocess.par2_not_enough_blocks` reports how many blocks are needed and how many are
  available, in all four languages. While volumes are on their way the step reads
  `postprocess.par2_awaiting_blocks` and stays `queued`, because it is waiting, not broken.

  Recovery at startup grew one case while this was built: a package still recorded as
  `postprocessing` is now re-requested as well, not only one with a queued step. Nothing is
  running at startup, so that state can only have been left behind by an interrupted run — and a
  service killed between re-queueing a volume and recording the wait would otherwise have
  downloaded the volume with nobody left to ask for the repair.

  **Settings → Post-processing → "Download all PAR2 volumes"** (`enable_all_par`, off by default
  as in SABnzbd) restores the older behaviour of fetching every volume. The SABnzbd-compatible
  queue listing no longer counts `skipped` rows towards a package's size, which otherwise made a
  postponed volume look like bytes permanently missing; the web interface already excluded them.

- **A limit on how many connections one host sees (RD-107-13).** Nothing bounded this: a file is
  split into up to four chunks, each chunk is its own concurrent ranged `GET`, and several files
  of the same hoster run at once, so a single installation could open dozens of connections
  against one CDN -- which is what hosters answer with throttling, a landing page or a plain
  refusal. The budget is now shared by every running transfer, defaults to six (the number
  browsers settled on for HTTP/1.1, and therefore the load hosters are built for), and is
  editable under Settings > General; `0` lifts it. Bytes per second remain a separate budget.

### Fixed

- **A server that answers a range request with `200` no longer ends the download (RD-107-13).**
  Every status except `206` counted as "server ignored a required range request", permanently and
  without a retry -- including a small file arriving complete in one chunk, and the `200` RFC 9110
  *requires* a server to send once an `If-Range` validator no longer matches. `Content-Range` was
  never looked at. Now the response decides: a described range is served at the committed offset,
  a complete body on a chunk that covers the whole file is written from the start, a resume that
  gets the entity from byte zero is a changed remote, and a `200` that carries a page rather than
  payload is a retryable failure judged by the same rule the online check already uses. What is
  genuinely a refused range is retried automatically, with that host planned as a single
  connection from then on -- which is what pressing start again used to achieve by accident.
- **"server ignored a required range request" is gone from the interface (RD-107-13).** It had no
  stable `code`, so the frontend could only print the English developer sentence. That failure and
  the changed-remote one now carry `download.range_ignored` and `download.remote_changed`, and
  both are translated in all four catalogues.

## [1.0.6] - 2026-09-14

The four cloud and multihoster providers 1.0.3 was too early for, and the eighteen things running
the released build asked for. Real-Debrid, Google Drive, OneDrive/SharePoint and Dropbox all sign
in, resolve and crawl; the contract they needed — a device code that can be renewed — was built
first, and the interface the three drives share was decided once, by the first of them, rather
than three times over.

**What this release does not contain**, named rather than left to be discovered:

- **No provider was run against a real account.** Every one of the four is proven against mocks of
  its own API and its own authorization server, with sanitised fixtures. What that cannot settle is
  named per provider in the job files — whether Dropbox's content endpoints accept the argument as
  a header on a `GET`, whether Microsoft Graph hands out a child of an anonymously shared folder
  through the `items` relation, whether Drive's `files.get?alt=media` returns bytes with the
  scheduler's bearer across the redirect. Each is a first-run question, not a design one.
- **Each installation registers its own OAuth client**, and until it does, the sign-in refuses with
  a message naming the steps. No client id or secret ships in this repository: a compiled-in one
  would sit unrevocably in the git history and in every release artifact, and every installation in
  the world would share one quota. The setup path per provider is in `README.md`.
- **Real-Debrid's torrent half is split off as RD-107-06.** Magnets, file selection before the
  generated files are fetched, and remote deletion need a persistent remote job, and no plugin
  world carries one today. Deciding where it lives is its own job with its own ADR. What ships is
  the sign-in and the hoster unrestriction.
- **The reported empty-list flicker is fixed at the mechanism, not confirmed at the symptom**
  (RD-106-19). The loading surface no longer returns on every refresh — `design.md` only ever
  promised it for the first one — and four store tests fail against the old computation. But the
  reported sight could not be reproduced on a test instance: an idle empty list holds a single
  layout state and fires no queue request at all. If it persists, the job file says which two
  facts would settle it.
- **The virtualised lists have no browser verification.** RD-106-12's evidence is jsdom
  throughout, which cannot see measured row heights replacing the estimates, nor how the list's own
  scroll viewport behaves inside the dashboard panel. The measurement it does carry is real: 3,794
  ms and 64,812 DOM nodes before, 41 ms and 578 after, for 3,200 rows.
- **76 backend failure codes still have no translation in any of the four languages** — unchanged
  from 1.0.5. The list in `web/src/i18n/sourceKeys.test.ts` may only shrink, and it has not.

Two defects this release also contains are worth naming, because both had been reported as
finished and neither was: no OAuth account could ever have been renewed, since `refresh` was handed
a vault reference the host's own checks refused; and the crash-and-restart matrix had been running
none of its cases, because enabling `rd-core/failpoints` does not enable the owning crate's.

### Added

- **A complete, bounded subscription review instead of the indexer's first page
  (RD-106-18).** An indexer check follows up to five pages of 100 results, preserving a saved
  search's parameters and stopping at the first short page. The local title filter still keeps
  its exact substring or regular-expression semantics; it is applied after at most 500 fetched
  results rather than being translated into the indexer's different `q` language. A warning now
  means that all five pages were exhausted, not merely that the first page happened to be full.

  The subscription archive is filtered and paginated on the server in pages of 50, with totals
  for open, queued, dismissed and skipped hits. It opens on the complete set of open decisions,
  and queue-all or dismiss-all takes one snapshot across every page; a hit arriving while the
  operation runs remains open, and a queue failure remains open and is counted. Each subscription
  can also delete its settled hits and check records after a confirmation that says what will go
  and that a later check may discover those releases again. Open hits are never deleted.

  The LinkGrabber opens its indexer-subscription box initially only when there is something to
  decide, leaves every individual subscription closed, and preserves a manual close when later
  poll events refresh the counts. IMDb score and language now sit under the title; the remaining
  metadata stays behind the chevron. Covers open immediately in a viewport-bounded Nuxt UI
  popover at up to 512 px, by hover, focus or tap, and the visible page preloads its thumbnails.
  A real archive password is visible with the same key treatment as a download package and is
  carried per hit through the LinkGrabber into extraction. Two hits accepted together no longer
  borrow the first hit's password from the batch.

- **OneDrive and SharePoint: files, sharing links and folders through Microsoft Graph
  (RD-106-05).** The second cloud drive, and the first to follow the interface RD-106-04 wrote
  down rather than deciding it: three sibling plugins — `onedrive` resolves one item and owns the
  provider row, `onedrive-crawler` lists a folder sharing link, `onedrive-oauth` signs the
  account in — with nothing but an address passing between them. Only Microsoft Graph, and only
  the routes a sharing link is meant to be reached through: the link is handed to Graph whole,
  encoded as `/shares/{id}` the way Microsoft documents it, and never taken apart for the
  `authkey` a personal link carries. OneDrive's short links, the long personal address, and every
  SharePoint tenant's `:f:`, `:w:`, `:x:`, `:b:` sharing links are all understood.

  The canonical address the crawler hands back keeps the share it found the item through —
  `graph.microsoft.com/v1.0/shares/<share>/items/<item>` — because a link shared with the account
  grants access *through the share*, and an item reached by its own id alone can be refused for
  an account that may read it through the link. The type letter Microsoft puts in a sharing link
  decides who claims it: `f` is a folder and the crawler's, every other letter a file and the
  resolver's. The long `onedrive.live.com` address says neither, so the crawler takes it, asks
  Graph, and answers with one file when that is what it finds.

  **The download address is the stable `/content` route, never the pre-authenticated
  `@microsoft.graph.downloadUrl`.** Every item carries the latter, valid for about an hour, and it
  is deliberately not what the resolver answers with: an address whose only identity is an
  expiring signature cannot be asked for again after a restart. The `/content` route can, and the
  scheduler asks the resolver again before continuing a partial file — so a short-lived address
  is renewed without losing progress precisely because nothing short-lived is ever stored. The
  transfer carries the account's bearer token to `graph.microsoft.com` only, and drops it on the
  redirect Graph answers with, because that redirect leaves the host.

  Every way Graph says no arrives as its own translatable code, and the two that both arrive as
  HTTP 403 are told apart by Graph's own error code: a share made for another account or tenant,
  a SharePoint policy that allows viewing but not downloading, a file Microsoft's scan flagged,
  an item that is gone, a sharing link Graph cannot decode, a throttle carrying Microsoft's own
  `Retry-After`, a refused token. Nothing Microsoft wrote is repeated: `message` and `innerError`
  are never read, and an error code that is not in the shape of one is dropped whole.

  Microsoft offers both ways in, and the sign-in plugin serves both (RD-106-01): the browser
  redirect with PKCE, and the device code typed at `microsoft.com/devicelogin` on any other
  screen — the first real plugin to use the device entrance with a renewal behind it. Both end in
  the same stored token with the same refresh material. The scope is `Files.Read.All` and
  `offline_access`, and not less: a sharing link goes through `/shares`, which Graph grants to
  nothing narrower than `Files.Read.All`. The application (client) id is registered per
  installation in the Microsoft Entra admin center and travels as `{{client_id}}`, as RD-106-04
  decided for every provider; the account's hint carries the registration steps.

  The folder walk keeps the limits every crawler here keeps — four levels, 500 files, 100
  folders, ten pages per folder, a set of visited ids against cycles — and follows Graph's
  `@odata.nextLink` as given, but only while it still points at Graph; a next page anywhere else
  is refused before it is fetched. SHA-1 and SHA-256 are carried as checksums where Graph states
  them; `quickXorHash` is Microsoft's own and is not handed on as anything.

  **Not claimed:** there is no Microsoft account and no registered application in this checkout.
  All evidence is at the unit and contract level against mocks of Graph and of the identity
  platform. In particular, that `/shares/{id}/items/{item}` reaches a child of an anonymously
  shared folder, and that the transfer's bearer header gets a `/content` request through to
  bytes, is documented behaviour and not a run. `eTag` and `cTag` are read by nothing:
  `resolved-download` has no validator field to carry them, and the transfer validates against
  the HTTP `ETag` of the `/content` answer instead.

- **Google Drive: files, shared links, folders, shared drives and Workspace exports
  (RD-106-04).** Three sibling plugins rather than the two the plan assumed, and the reason is
  worth stating because the next two drive jobs inherit it: only a `resolver` manifest may carry
  a `[provider]` section, and that section is what creates the account, owns the vault reference
  and makes `{{secret:…}}` resolve at all. So `google-drive` resolves one file and owns the
  provider row, `google-drive-crawler` lists a folder or shared drive, and `google-drive-oauth`
  signs the account in. What passes between them is nothing but an address — the canonical
  `drive.google.com/file/d/<id>/view` the crawler answers with and the resolver claims — so each
  works alone and neither knows the other exists. Only the official Drive v3 API: no scraping of
  the download interstitial, no guessing at `confirm=` tokens.

  A Workspace document is not a file until somebody names the format it becomes, so that choice
  is made before anything is queued and shown as the name it will arrive under: a Doc becomes
  `.docx`, a Sheet `.xlsx`, Slides `.pptx`, a Drawing `.png`, Apps Script `.json`, overridable per
  address with Google's own `?format=`. A Form, a Jamboard and a Site export as nothing at all and
  are refused rather than queued as an empty file, and an export is reported with no size and no
  checksum because its bytes do not exist until the export runs.

  Every way Drive says no arrives as HTTP 403 and means something different each time, so each has
  its own translatable code: a download quota that resets, a file Google could not scan, an owner
  who switched downloading off, an export too large, a rate limit carrying Google's `Retry-After`,
  a refused token, a file that is not there. Nothing Google wrote is repeated — a reason that is
  not in the shape of a reason code is dropped whole rather than filtered, because filtering keeps
  the digits of a token an error document happened to quote, and `error_description` is not read
  at all.

  The folder walk keeps the limits `premiumize-crawler` set — four levels, 500 files, 100 folders,
  a set of visited ids against cycles — so what somebody gets back does not depend on which cloud
  the folder was in, plus one the Premiumize crawler did not need: at most ten pages per folder,
  because `files.list` answers a page at a time and a wide enough folder hands out page tokens for
  as long as anybody asks.

  The sign-in is authorization code with PKCE against Google's own endpoint, scoped
  `drive.readonly` because two other plugins spend the same token, with `access_type=offline` and
  `prompt=consent` — without the first Google issues no refresh material and without the second it
  issues it exactly once, so a second sign-in would leave an account with nothing to renew from.

  **Not claimed:** there is no Google account and no registered OAuth client in this checkout. The
  client id in the sign-in plugin is an explicit placeholder on a `.invalid` domain, so a sign-in
  started against it reaches Google and is refused until a real "Desktop app" client is
  registered. All evidence is at the unit and contract level against mocks of the Drive API and of
  an authorization server. Modification times are not carried either: `resolved-download` has no
  field for one.

- **Dropbox: files, shared links, folders and shared folder links (RD-106-06).** Three sibling
  plugins on the cloud-source interface RD-106-04 decided: `dropbox` resolves one file and owns
  the provider row, `dropbox-crawler` lists a folder or a shared folder link, and
  `dropbox-oauth` signs the account in. Only the official Dropbox API v2: `files/get_metadata`,
  `files/list_folder` with its cursor, `sharing/get_shared_link_metadata`, and the content
  endpoints for the bytes. No `?dl=1` redirect chasing and no `get_temporary_link`.

  The resolver answers with the **stable** content address — `files/download` or
  `sharing/get_shared_link_file` — and names the file in the `Dropbox-API-Arg` header, pinned to
  the revision it just described; the scheduler carries that header onto the transfer and puts
  the account's token beside it, because the content host is one of the provider's
  `secret_domains`. A resume therefore asks the resolver again and continues, with nothing that
  expires in the address. Dropbox's `content_hash` is verified after the download under a
  checksum algorithm of its own, `dropbox_content_hash` — SHA-256 over each 4 MiB block, then
  SHA-256 over the concatenated digests — because stated as `sha256` it would fail every file it
  was meant to verify.

  Addresses: `/s/<key>/<name>` and `/scl/fi/<id>/<name>?rlkey=` for a shared file, `/sh/` and
  `/scl/fo/` for a shared folder, `/home/<folder>` for a folder of the account's own Dropbox, and
  `?preview=<name>` — Dropbox's own spelling of a previewed file — for a file inside either. That
  parameter is what keeps the crawler's and the resolver's claims disjoint. A password-protected
  shared link is opened the one way the API offers, `link_password` in the official argument; the
  password comes from `?link_password=` (or `?password=`) on the pasted address, is redacted by the
  core, and travels on into every address the crawler hands back for the files behind the link.

  The folder walk keeps the four limits and the page cap the other crawlers have, and puts the
  cursor where a restart needs it: not in a local of a loop, but in the walk's own queue — a folder
  with more pages comes back as the next thing to read, carrying the cursor its last page ended
  on. A rate limit is reported as a hold on this provider alone: every Dropbox link waits as long
  as Dropbox's `Retry-After` asked, and nothing else does. Every refusal is its own translatable
  code — a missing link password, a file that is not there, a Paper document that has no bytes, a
  refused token — and nothing Dropbox wrote is repeated.

  The sign-in is authorization code with PKCE against Dropbox's own endpoint, redirect only,
  with `token_access_type=offline` so refresh material is issued at all, scoped
  `account_info.read files.metadata.read files.content.read sharing.read`. The app key is
  registered per installation in the Dropbox App Console and entered on the account; the
  repository ships none.

  **Not claimed:** there is no Dropbox account and no registered app in this checkout. All
  evidence is at the unit and contract level against mocks of the Dropbox API and of its token
  endpoint. In particular it is not proven that the content endpoints honour a `Range` request
  from the download engine, nor that a `GET` with the argument in the header is accepted where
  Dropbox documents `POST`. A process restart during a crawl restarts the crawl from its address:
  the cursor lives in the walk's state, and the walk is one invocation. Modification times are
  not carried: `resolved-download` has no field for one. Public shared links still need a
  signed-in Dropbox account, because the API answers nothing without one.

- **"Check all" for the indexer subscriptions, in the LinkGrabber (RD-106-17).** The box that
  lists an indexer's hits waiting for a decision now carries a button that asks every indexer
  subscription to check now — one request per subscription, because that is the endpoint there
  is, and from the place where the hits are worked through rather than a page away. The box opens
  and loads first so the hits a check turns up have somewhere to appear; the store already re-reads
  the archive of every loaded subscription when a poll finishes. Feedback follows the "check now"
  pattern from RD-106-09: a line saying the check was *started* for so many subscriptions, replaced
  by the event stream with what each one found, and a count of the checks that could not be
  started. Media, gallery and feed subscriptions are not asked — they have no review step.
- **Real-Debrid signs itself in, and unrestricts links (RD-106-03).** Two sibling plugins, because
  a manifest carries exactly one `plugin_type`. `realdebrid-auth` is the first shipped `oauth`
  plugin of the device kind: the interface shows a short code, the person confirms it at
  real-debrid.com, and the token that follows is renewed by the sweep without anybody being asked
  again — which is why it is an `oauth` plugin and not an `auth` one, since Real-Debrid's tokens
  die in an hour and the older world has no `refresh`. `realdebrid` is the resolver beside it,
  reading the same stored token: `unrestrict/link` for the address, `user` for the account,
  `unrestrict/check` for a bounded batch of link checks, `hosts/domains` for the catalogue. The
  two endpoints that need no token are called without one. Nothing the provider wrote is repeated
  — Real-Debrid answers a refusal with a sentence *and* a documented number, so the number is
  classified and travels as a parameter and the sentence is dropped whole. Two classifications
  deliberately differ from the obvious reading and say so in the code: "hoster not available for
  free users" is a capability gap rather than an account error, and "IP address not allowed"
  blocks the address rather than invalidating a credential that is perfectly good.

  **Each installation registers its own Real-Debrid application** — client id as the account's
  username, client secret as its credential — rather than sharing one shipped in the source. A
  client secret in an open-source repository is not a secret, and Real-Debrid's rate limits are per
  application, so a shipped registration would put every installation in the world into one bucket
  and let any of them exhaust it for the others. An account with none is refused before a request
  is made, with a message saying where to register and what to enter, rather than being handed the
  provider's `invalid_client` — which says the application was refused, not that there is none.
  That needed a place for the access token, since the account's credential is now taken: a provider
  can declare two credential slots and mark the one a sign-in fills `filled_by = "flow"`, and the
  host keeps it beside the flow instead of writing it over the registration.

  It claims http(s) and deliberately **not** `magnet:`: a torrent has to be uploaded, waited for
  and have its files chosen before any address exists, which is a remote job with a state machine
  and not a link that resolves. That half is **RD-107-06** in 1.0.7 rather than half delivered
  here; `docs/roadmap/jobs/107-06-real-debrid-torrents.md` carries the three criteria unchanged,
  the four reasons a torrent is not a folder crawler, and the finding underneath them — that no
  plugin world carries a persistent remote job, which is an ADR of its own.

- **A package's folder can be renamed, not only its label (RD-106-13).** `PATCH
  /api/v1/packages/{id}` changes a name; it deliberately does not move data. The new
  `POST /api/v1/packages/{id}/folder` does both, and the package editor in the download list
  offers it as a switch beside the name. Most of the crash safety was inherited rather than
  written: `previous_destination` and `Scheduler::relocate_package` already move a package folder
  in two phases for a category change, so the row is written first, the disk follows, and an
  interruption leaves a package that is findable and a move that finishes on the next pass. Two
  things were new, and both would have been quietly wrong. `postprocess_steps.source_path` is a
  step's **identity** — the table is keyed by it, `rd-extract` looks a step up through it — so a
  rename that left it naming the old folder would have made the `extract/force` action from
  RD-104-04 write a second set of rows beside the first and answer "has this package been
  repaired?" wrongly; every stored absolute path (`postprocess_steps.source_path` and
  `output_path`, `nzb_files.output_path`) now moves in the same transaction as `destination`. And
  a name that is already taken is a **refusal**, against this codebase's own habit: everywhere
  else `rd_files::collision_free_path` appends ` (1)` and carries on, which is right when a file
  lands somewhere unattended and wrong when somebody typed the name. A package with a file still
  transferring is refused as well (`package.folder_busy`), and the name goes through
  `rd_files::sanitize_file_name` — the rules the host file rename already uses — so it cannot
  walk the package out of its category directory. New crash point
  `scheduler.before_package_move`, with its row in `docs/recovery-matrix.md` and an Axis-A case
  in `crates/rd-scheduler/tests/crash_restart.rs`.

- **A device sign-in that renews itself (RD-106-01).** The plugin contract gained `device-begin`
  and `device-poll` in `interface oauth`, so a provider whose sign-in is a short code typed on
  another screen now reaches the same `refresh` a redirect sign-in does. The person types the
  code once; the token that follows is kept alive by the renewal sweep, and nobody is asked
  again. Which ways in a plugin serves is stated in its manifest — `oauth_flows = ["redirect",
  "device"]`, top-level and in the author's order of preference — so the host never calls an
  entrance nobody implemented, and a provider offering both is one plugin rather than two.
  Absent, the field means `["redirect"]`, which is what every OAuth manifest written before this
  meant. A device flow's `verification_url` is confined by the same manifest rule as an
  authorization address, `authorization_pending` and `slow_down` are waiting rather than failure,
  and being asked for an entrance a plugin does not serve is a refusal with a stable code
  (`account.auth_flow_unsupported`) instead of a sign-in that dies in front of somebody. The
  package version stays `rdownloader:plugin@0.6.0`; the three shipped authentication plugins
  export `auth` and keep their world.
  `docs/adr/0002-a-device-sign-in-that-can-be-renewed.md` records the choice and the three
  designs it beat.

- **Lists that stay usable at a few thousand rows (RD-106-12).** The LinkGrabber and the download
  queue render only the rows near the viewport instead of every row there is, through one shared
  building block rather than two solutions. The work was not the windowing but what had to happen
  first: both lists were trees — a package component that rendered its own children — and a window
  can only slide over a flat sequence. A package is now a single header row, its files and links
  are siblings, and collapsing is a filter on that stream instead of a `v-if` inside a package.
  Row keys survive a reorder, which is what lets the focus and the selection survive it too.
  Because virtualization takes rows out of the document, the row holding keyboard focus is pinned
  and stays: without that, the arrow-key reorder on a drag handle that 1.0.5 shipped as an
  accessibility fix (WCAG 2.1.1 and 2.5.7) would have ended one keypress in. A keyboard move now
  also puts the focus back on the same handle afterwards, so the second press continues it past
  the edge of what is rendered. Two things the job's acceptance criterion claimed already existed
  did not, and were built here: **shift selects a range** of rows along the visible order, and
  **"show in list" jumps back to a selected row**, opening its package if it is collapsed. A
  screen reader is told the list's real length — `role="list"` with a name, `aria-setsize` and
  `aria-posinset` per row — rather than the size of the window. Below sixty rows nothing is held
  back and the list keeps its own natural height, so an ordinary queue looks and behaves exactly
  as before. Measured on the same data, 200 packages of 15 files: the queue's first render fell
  from 3.794 ms and 64.812 DOM nodes to 41 ms and 578, and a single row's update from 642 ms to
  18 ms; the LinkGrabber from 2.851 ms and 42.214 nodes to 45 ms and 392. Those are jsdom figures
  — they say how much work the view makes, not how many frames come out.

- **The browser tab says what is running (RD-106-07).** While transfers are moving, the title
  carries the queue-wide rate and the number of active transfers — `1.5 MiB/s · 3 active ·
  rDownloader` — and falls back to the application name alone at rest, so a window in the
  background can be read without bringing it forward. The rate comes from the service, which has
  measured it since RD-104-02, and is left out entirely where there is no honest figure rather
  than printed as `— B/s`. Written in exactly one place, a composable called once from `App.vue`,
  and switchable off in Interface settings (`title_status_enabled` in the settings document), for
  whom a title that keeps changing is a distraction.
- **A fixed unit for data sizes (RD-106-14).** Interface settings now carry a second byte
  preference beside the binary/decimal ladder: the magnitude itself. `Scale automatically` is the
  default and keeps what every earlier version did — each value on the step that fits it — while
  `byte`, `kilo`, `mega`, `giga`, `tera` or `peta` prints every size in that one step, so a list
  spanning several orders of magnitude can be read down the column instead of in the head. It is
  named by magnitude rather than by unit because the unit names belong to the ladder setting: the
  same choice reads MiB on the binary ladder and MB on the decimal one. A pinned figure keeps
  roughly three significant digits, and a value below a thousandth of its unit prints `<0.001`
  rather than a `0.000` that would say nothing. Stored in the settings document as `byte_unit`,
  so it survives a restart and applies to every figure in the interface at once.
- **The areas of an API token can be changed afterwards (RD-106-11).** The token list under
  Settings → System now has a pencil beside each token: it opens the same area list the minting
  form uses, pre-ticked with what the token holds, and saving replaces the set. The token value is
  not reissued, so no MCP client, *arr instance or script has to be reconnected — and the change
  is in force at that client's very next request rather than after a restart, because the scope
  lookup reads the row on every request and has no cache to invalidate. Underneath it is
  `PATCH /api/v1/api-tokens/{id}`, which costs `api:secrets`, the same area that minting a token
  costs. An empty selection is refused rather than read as "nothing" or "everything"
  (`api.scopes_empty`); removing all access is still spelled by revoking.

### Changed

- **Settings navigation is grouped by task.** The seventeen pages now sit under Basics,
  Downloads & processing, Integrations, and Administration instead of following an arbitrary
  flat order. The groups stay stable across languages, and remain headings rather than extra
  expandable layers, so every page is still one click away after opening Settings.

- **Settings and editor cards now use the space they have consistently.** Two-factor
  authentication and passkeys share a row on wide screens, setup readiness distributes its four
  checks across two columns, and the automation form now has a heading aligned with the list
  heading beside it. Editing a subscription draws the same complete primary outline used by
  account and routing rows. The machine-token section is now named **API & MCP**, because the
  tokens it already creates serve REST, the CLI, SABnzbd/qBittorrent-compatible clients and MCP;
  the raw token and complete authorization header are presented before the MCP-specific command.
  Premiumize keeps its device sign-in, but no longer offers another “Connect” action after its API
  key slot has been filled.
- **A form that creates entries stands beside the list it feeds, not above it (RD-106-15).**
  Categories, routing rules, hotfolders, storage roots, hoster and Usenet accounts, bandwidth
  profiles and the schedule, notification targets and rules, automations and subscriptions all
  stacked the form above the list. Pressing a row's pencil filled the form and changed its
  heading — both above the fold by then — so the list looked unchanged and the edit went
  unnoticed. Every such area now uses one shared block, `FormListLayout.vue`: the form in the
  left column with one field per row, the list in the right, stacked again below 1024 px. Three
  things say which row is being edited, because a heading somewhere else does not: the row is
  outlined and carries an "Editing" badge, the form heading reads "Edit …" instead of "New …",
  and the focus moves into the first field — which on a narrow screen is also what scrolls the
  form into view. The list column carries the count that used to sit in the section header. The
  hoster account form, which had placeholders where the other forms have labels, got the labels.
  The rule is written down in `design.md` under the row and list conventions.
- **An OAuth provider's client is registered per installation, never compiled in (RD-106-04).**
  A plugin writes the marker `{{client_id}}` where a client id belongs, and the host substitutes
  what this installation registered — in the outbound request, and in the authorization URL the
  plugin returns, which is the one place the host expands anything into a string it did not send
  itself. A plugin could not do this for itself even in principle: a guest has no way to read a
  configured value, since `secret-available` answers a bool and the only thing expanded for a
  guest is an outbound request, which building an address is not. Three reasons it is worth the
  plumbing, in order of weight: providers count quota per client, so a compiled-in one would put
  every installation on one shared allowance; a client id in a public repository sits in the git
  history and in every release artifact, revocable by nobody; and the client *type* — which
  decides whether a token endpoint wants a `client_secret` — becomes each installation's own
  answer rather than this project's guess.

  The marker is deliberately *not* a secret marker. A client id identifies the application to the
  provider, not the person to the application, and the provider publishes it in the address the
  person is sent to — so it is stored in the clear as the account's username, it does not make a
  request carry credentials, and it does not narrow a redirect. Its two gates are that the
  account's provider is `credentials = "oauth"` and that the address is one the plugin's own
  manifest allows. An account with no client id refuses under `oauth.client_not_configured`
  before anybody is sent anywhere — a bad request rather than the bad gateway every other sign-in
  failure gets, because nothing is wrong at the provider and the person is one form field away
  from fixing it, so the translated text carries the steps rather than the diagnosis.

- **An OAuth-signed provider's access token now rides on the transfer itself (RD-106-04).** A
  cloud drive's bytes come from an API address that answers with the account's token and with
  nothing else, and neither existing arrangement reached it: a resolver states download headers as
  *values* and has none to state, because the point of `store-oauth-token` is that no plugin ever
  reads a credential back. The scheduler attaches the header itself now, and only where it must —
  `credentials = "oauth"`, the exact hosts the provider's own manifest listed under
  `secret_domains`, over TLS, and checked against the address the transfer actually goes to rather
  than the one it started from, so a resolver answering with somebody else's host cannot take the
  token there. Nothing in the tree declared `credentials = "oauth"` before this, so no existing
  provider changes behaviour.

- **A cover is enlarged in the row it belongs to (RD-106-08).** An indexer's cover is the fastest
  answer to "is this the film I meant", and at thumbnail size it answers nothing — so the same
  picture was drawn twice: uselessly small in the row, and usefully large inside the expanded
  detail, behind a chevron nobody opens to look at a picture. The thumbnail is now a control: a
  pointer resting on it, keyboard focus reaching it, or a tap shows the cover large, overlaid so
  the list does not move, and `Escape` or the next tap closes it again. Focus never leaves the
  button, so there is none to hand back. The copy in the expanded detail is gone; the backdrop
  banner beside it is a different picture and stays. With external image loading switched off
  there is no thumbnail and nothing to enlarge, which is the point of that switch. This reverses
  a rule `design.md` had held since 1.0.1 — "a larger version belongs in the expanded detail,
  never in the row" — and the document now says so, with the reason: the old rule was written for
  decoration, and a cover is not decoration.

- **A token is no longer promised to keep the rights it was born with (RD-106-11).** Until now the
  documentation said outright that "an existing token is never upgraded", and that was a real
  guarantee: a leaked bearer could never become more dangerous than it was on the day it leaked.
  It has been given up deliberately, and `README.md` now carries the reasoning next to the changed
  promise. What practice produced instead of narrow tokens was `api:*` granted up front, because
  widening one meant minting a second and reconnecting every client that used it — a token born
  too wide is the worse outcome for exactly the same leak. What replaces the guarantee is a
  record: issuing a token now writes a `capture_changed` event as re-scoping and revoking do, each
  naming the areas involved and, for a change, the areas it replaced. Without the issuing event —
  which never existed before — the trail of a widened token would have started at the widening.
  The two isolations are unchanged: `capture:*` is as ungrantable as it is unmintable, and only a
  token the API token list already shows can be re-scoped at all, so a browser-capture token
  cannot be turned into an API token by naming its id.

- **The SDK documentation says what the scaffold needs.** `sdk/README.md` now states that
  `plugin new` reads its templates from a checkout, which `cargo-component` version builds them,
  how `--name`, `--development` and `RDOWNLOADER_PLUGIN_SIGNING_KEY` fit in, and what the
  translation files have to contain. `docs/plugins.md` lists all ten templates rather than
  eight. `docs/README.md` is a new index of every document in the repository.
- **Every document under `docs/` was checked against the code and corrected.** Most of them
  still counted the 1.0.1 release: twenty-six bundled plugins where there are twenty-eight,
  eight plugin types where there are ten, sixteen MCP tools where there are sixty-one, 256 API
  operations where there are 262. Beyond the counts: the post-processing page had the two
  SABnzbd alias names the wrong way round and was missing the remux and upload steps; the
  external-tools matrix promised managed versions on macOS, which the manifest does not carry;
  the plugin page named a constant that no longer exists and called the `source` interface
  read-only although it renames; the reverse-proxy page had `doctor` printing an address it
  never sees and `X-Forwarded-Proto` being read when nothing reads it; the roadmap's status
  paragraphs still said no OAuth refresh existed and named a job that does not. The marketing
  package was re-baselined on 1.0.5, numbers and versions only. Three things the compatibility
  page describes differently from what the code does — the qBittorrent `category` filter, the
  `404` for unknown `/api/v2` paths and `ratioLimit=-2` — were left as written, because there
  the page may be right and the code wrong.

### Fixed

- **An empty download list and an empty LinkGrabber no longer flicker (RD-106-19).** The
  `loading` flag the four list stores hand to the fetch-state wrapper was the per-refresh flag
  ORed onto the first-fetch flag, and refreshes arrive constantly — state events, poll timers,
  the speed sampler, every write action. Each one therefore replaced the rendered empty state
  with the loading skeleton for the length of a round trip and let it come straight back. Only
  an empty queue, LinkGrabber, subscription list or automation list showed it, because the
  wrapper renders nothing at all once there is content. The loading surface is now what
  `design.md` promises: the first fetch, and only that one. A first fetch that failed still
  leaves it and shows the error, and a retry keeps that error on screen instead of flashing the
  skeleton a second time.
- **A plugin component left over from before a merge now says so, instead of failing a contract
  test on behaviour (RD-106-20).** The WebAssembly components live in
  `target/wasm32-unknown-unknown/release/`, which is per checkout and which `cargo test` never
  builds — so merging a branch that changed a plugin leaves the previous component beside the
  current expectations. Twice in this milestone that surfaced as `does not have export
  'device-begin'` and as a client identifier the job had just removed: both read as defects in
  the code, both were an artefact nobody had rebuilt, and both cost a full `scripts/check.sh`
  run. The contract tests now compare the component against the plugin's sources and the WIT
  contract before using it, and fail with the `cargo component build` command when it is
  behind. `scripts/check.sh` asks the same question right after `cargo fmt`, so the answer
  arrives in a second rather than forty minutes in; `scripts/build-plugins.sh --list-stale`
  names the components concerned.

- **An archive password embedded in an NZB is no longer discarded.** Newznab's
  `password=1` only says that a release is protected; the actual value commonly arrives later
  as `<head><meta type="password">…</meta>` in the downloaded NZB. The parser now carries that
  value onto the NZB import and Usenet package for extraction, whether the NZB came from an
  indexer subscription, an upload or a hotfolder. An explicit package password or
  `{{password}}` file-name marker still wins, and the package editor shows the stored value.
- **An indexer check notice no longer stays in the previous interface language.** The LinkGrabber
  stored “Check of 7 subscriptions started” as already translated prose, so changing languages
  left that line behind while the rest of the page changed. It now stores the notice kind and
  values and translates them while rendering, including singular/plural and later poll results.
- **The webhook target of an automation action was an empty list (RD-106-16).** Both targets
  arrived from the API; the select read each option's label from `label`, a field
  `NotificationTarget` does not carry, so every row rendered blank and the search box, filtering
  on the same field, hid them altogether — an empty dropdown with no error beside two existing
  webhooks. It reads `name` now, like the category and package selects in the same editor, and
  the search also matches the endpoint. `AutomationView.test.ts` renders the select through a
  stub that honours `label-key`, which is the shape a stub needs to catch this at all.
- **No OAuth account could be renewed, and nothing showed it (RD-106-03).** `refresh` is handed a
  `credential-ref` and the contract tells the plugin to write it as `{{secret:<reference>}}`. That
  reference is one the host minted when it stored the token, not one of the provider's declared
  secret slots — so the expansion refused every one of them with
  `plugin.secret_target_not_allowed`, and a looser check would not have helped either, because the
  branch below it expands the *account's* secret, which is the access token and not the material a
  renewal is made with. It was invisible because the OAuth contract tests answer at the host
  boundary and never run that expansion, so the reference plugin passed its renewal case
  throughout. The host now recognises exactly the reference stored on that account's own flow row
  — an equality test against what it itself wrote, so a plugin guessing at vault references finds
  nothing, another account's included — and the address is still held to the hosts the provider's
  own credential may reach.

- **The crash-and-restart matrix was running none of its cases (found while adding one for
  RD-106-13).** `AGENTS.md`, `scripts/check.sh` and the CI job all invoked the Axis-A suite as
  `cargo nextest run --features rd-core/failpoints -p rd-core -p rd-http`, which is exactly one
  step short: every crash-test file is gated on the `failpoints` feature of the crate that
  *owns* the point, and enabling a dependency's feature does not enable its dependants'. So
  `crates/rd-http/tests/crash_restart.rs` compiled to nothing and the run reported success —
  30 tests where 34 exist. All three call sites now name the owning crates
  (`--features rd-http/failpoints,rd-scheduler/failpoints -p rd-core -p rd-http -p rd-scheduler`),
  and `docs/recovery-matrix.md` states the counts to check against, because a suite that
  compiles to nothing is the one failure a coverage document cannot afford.

- **A refused API token says why, instead of nothing at all.** Every rejection at `/mcp` was a
  bare `401` with no log line anywhere, so an MCP client that would not connect gave the
  administrator nothing to work with: the token cannot be read back — only its digest is
  stored — and the service stayed silent about what it had actually received. The refusal is
  now logged with the *shape* of the credential and never the credential: no `Authorization`
  header, one that is not a bearer, an empty bearer (an environment variable that did not
  resolve), several `Authorization` headers where only the first is ever read, a digest no
  active token has, or a token whose scopes hold nothing from the API set. The digest prefix in
  the line is the same value `capture_tokens.token_sha256` stores, so a line can be matched
  against the token list without the secret leaving the client. The `401` also carries
  `WWW-Authenticate: Bearer` now, which the MCP specification has clients read to learn what
  kind of credential was wanted; no `resource_metadata` is advertised, because this service
  issues its own bearer tokens and runs no authorization server to point at.

- **The MCP panel hands over the header, not just the token.** Connector dialogs ask for a
  complete `Authorization` value, often through an environment variable — the ChatGPT desktop
  app among them — and the panel offered only the raw token, which invites leaving the
  `Bearer ` prefix off. It now shows `Bearer <token>` ready to copy, beside the existing
  `claude mcp add` command and the token itself, and says to configure exactly one source for
  the header: when two are sent only the first is read, so a correct token in second place
  never arrives.

- **The qBittorrent adapter answers three things the way its page always said it did.**
  `torrents/info?category=…` returned an empty list for every non-empty category, because a
  package's category was never reported back — and Sonarr and Radarr only ever read the list
  filtered to their own category, so a grab could sit in the queue forever without being
  imported. The category is now reported by name and matched as qBittorrent does it. A path
  the adapter does not serve answered `200` with the web interface instead of `404`, which a
  client parsing JSON reads as a server that is up and broken. And `ratioLimit=-2`, qBittorrent's
  "no ratio limit", was dropped and behaved like "use the global setting"; it now stores a
  ratio of zero, which is what switches the ratio stop off.
- **The bundled sign-in plugins stop quoting the provider back at you.** AllDebrid,
  Debrid-Link and Premiumize each built their failure message as `format!("the provider refused
  the sign-in: {reason}")` around whatever the provider had written — for AllDebrid a free-text
  `message` field. An endpoint that echoed part of a credential into its error document would
  have published it through a log line and the accounts view. All three now keep the provider's
  error *code* and drop anything that is not code-shaped whole rather than filtering it, because
  filtering keeps the digits of a leaked key. AllDebrid additionally reads the `code` field
  before the prose, which is the part worth keeping. The second half of the same defect: every
  refusal there is was reported under one translation code that said the sign-in code had
  expired, so a blocked account was told to try again — they now report `consent_denied`,
  `flow_expired`, `bad_reply` or `sign_in_refused`, each translated in all four languages. The
  1.0.5 changelog left this work to "the jobs that touch them"; this is that job (RD-106-01).
  Their packages move to `0.9.1` and the reference OAuth plugin to `0.2.0`, because a bundled
  package is installed only when it is strictly newer than what is already there — a corrected
  component under an unchanged version number would reach nobody who had the old one.
- **"Check now" says that it is checking (RD-106-09).** The button started a poll without waiting
  for the answer, showed no state at all, and left a failure in the store's error and nowhere else
  — so it got pressed again, and each press started another poll. It is now busy for the length of
  the request and refuses a second press while it is, per subscription rather than globally, so
  several subscriptions stay independently checkable. The view states that a check was started,
  deliberately not that it has finished: the server answers before the poll runs, and claiming
  otherwise is what made the unchanged list afterwards look like a check that found nothing. The
  end of the check reaches the view over the event stream — the event written when a run is
  recorded now names the subscription and carries its counts, where before every subscription
  change emitted the same anonymous envelope — and replaces the line with what the check found or
  why it failed, without a reload. The archive of an expanded subscription also stopped saying
  "nothing found yet" while it was still being read, or when reading it failed; it was the list
  RD-104-07 missed.

- **A subscription's title filter no longer looks ignored (RD-106-10).** A hit a filter rejects is
  archived rather than dropped — an unwritten one would be rediscovered on every poll forever — and
  the list showed it beside the accepted ones, told apart by a small muted label. Reported as "the
  list is not filtered by my rule any more". It is filtered, and a test now holds that answer on the
  indexer path as well: the evaluation runs in the poll service, above the adapter, so an indexer
  poll passes the same filter a feed does. The archive shows the accepted hits by default, says how
  many a filter took out, and keeps the skipped ones — with their reason — behind a selection beside
  the list. Beside it stood a second cause of the same complaint: an indexer answers at most a
  hundred hits per query and the filter runs on what came back, so a match older than that page was
  never fetched. That boundary is now named where it matters — a check that came back full says so —
  rather than papered over by sending the filter as the indexer's `q`, which would hand a substring
  or a regular expression to a server's word tokenizer and drop hits the filter accepts.

- **The `auth` scaffold builds again.** `rdownloader plugin new --type auth` wrote a plugin that
  no longer compiled against the contract it shipped with: `poll` had gained the `flow-state`
  argument and `user-prompt` the field of the same name when sign-in flows were moved into the
  database, and the template was never told. The scaffold now carries the device code through
  `flow-state` the way the bundled providers do, and explains why. CI builds, packages and runs
  conformance for every one of the ten scaffolds instead of three, sharing one target directory
  so the extra worlds cost one crate each — the unit test that guards the templates checks the
  contract copy, not that the code compiles against it, which is how this went unnoticed.
- **The SDK's CI workflow template installs a binary that exists.** `sdk/ci/plugin.yml` ran
  `cargo install --git … --tag v0.7.0`, which fails on every checkout because the binary embeds
  the built web interface, and rejected WASI imports with the byte scan the repository's own CI
  had already replaced. It now downloads the pinned release archive, reads the component's
  import section with `wasm-tools`, and names the release in one variable at the top.

- **A token renewal no installed plugin can run is now said once instead of retried forever, and
  a sign-in no longer dies of a dropped connection** (RD-106-02). The two halves of the
  authentication sweep made opposite mistakes out of one missing distinction. Every error from a
  provider plugin arrived as the same opaque message, so the renewal half read "no installed
  plugin claims this provider" as a provider that was merely unreachable and asked again every
  five minutes for the life of the process, with a warning line per attempt and no attempt that
  could ever succeed — a plugin is not installed by waiting for it. The sign-in half read every
  error as final, so a provider that was briefly unreachable ended a sign-in that would have
  gone through a minute later.

  `rd-plugin-ext` now says which of the two happened, as a type the compiler checks rather than
  as a string to compare. A missing plugin fails the flow once, with a stable code the interface
  translates into the four languages and which names the provider without quoting anything a
  provider said. A call that did not come back keeps the stored token and is tried again later.
  A sign-in the provider named no window for is given up on after fifteen minutes rather than
  polled forever, which is the same "repeat it for ever" one sweep over.

  The failure category a plugin already sends now travels with a refusal instead of being dropped
  where the host translates it, so a renewal can tell "busy, rate-limited or offline" from "this
  credential was refused" and only gives up the token for the second.

  **Not reachable in a shipped build.** No bundled authentication plugin stores refresh material,
  so no row can yet exist whose provider has no OAuth plugin. It becomes reachable with the
  device-code-with-refresh work (RD-106-01), which is why this correction goes first.

## [1.0.5] - 2026-09-08

Two milestones in one release. 1.0.4 was "Reaching What Is Already There" — seven things reported
from running the released build, each about reaching something the tree already had. 1.0.5 was the
one job 1.0.3 left behind: the OAuth world shipped without a scaffold, so nobody could write a
plugin of it. Both are here; 1.0.3 and 1.0.4 never shipped as versions of their own.

**What this release does not contain**, named rather than left to be discovered:

- **Widget captchas (RD-104-08) are measured and decided, not built.** Against DDownload's real
  site key on 2026-09-08, a page served from localhost earns `110200 — domain not allowed`, while
  the hoster's own login page renders the widget. So a modal in the web interface is dead for that
  key and a WebView in the capture agent is the only way. The WebView is Windows-only work and
  cannot be exercised where this release was cut, so none of the behavioural criteria are ticked.
- **Holding PAR2 volumes back (RD-104-04b) is split off.** Postponement is decided when an NZB is
  queued, but the re-queue arises during post-processing — and nothing can move a package from
  post-processing back into downloading. That mechanism spans five crates and is larger than the
  rest of its job. What ships is the half that unblocks people today: a failed repair no longer
  locks a package away.
- **The twelve provider jobs of 1.0.3 are still open.** RD-105-01 removes what blocked four of
  them; the providers themselves are 1.0.6.
- **76 backend failure codes have no translation in any of the four languages** — the whole
  `subscription.` and `collector.` families among them. They reach the reader as the server's
  English in every language. The new key test records them as a list that may only shrink, rather
  than pretending they are covered.

Two defects found in work this release also contains, both worth naming because both had already
been reported as finished: a free-form notification `config` accepted a credential over MCP and
handed it back, and the OAuth world merged in 1.0.3 could never store a token at all, because the
confined host did not forward `store-oauth-token`.

### Added

- **The whole configuration can be set through MCP, not only read** (RD-104-01). The MCP server
  offered sixteen tools, four of them about configuration, and every one of those could only
  read: an assistant could list the categories and then do nothing with them, because
  `create_category` did not exist. That was never a missing capability — the REST side has had
  the write endpoints all along — but a missing way in.

  Sixty-one tools now cover categories, routing rules, storage roots, watched folders, provider
  accounts, proxy profiles, NNTP servers, notification destinations and rules, subscriptions,
  livestream channels, automations and plugins. Each one calls the REST handler it corresponds
  to, so validation, ordering and the stable error codes are the ones the web interface gets and
  nothing is written twice. Supported providers stay read-only, because there is no write path
  behind them to mirror.

  Update tools merge onto the stored row instead of replacing it: a form always has every field
  in hand and an assistant does not, so "make the Movies category green" must not silently reset
  eight post-processing flags because they were not resent. A `clear` list names the fields to
  reset, since leaving one out already means keeping it.

  **No tool takes or returns a password, an API key or a cookie jar.** A row is created over MCP
  and its credential is typed into the web interface; a request body naming a credential field
  is refused with `request.credential_rejected`. Destructive tools are named individually —
  `delete_category`, `delete_account`, `uninstall_plugin_version` — and say in their description
  what they destroy.

  What a tool costs is read from the route table rather than written down beside it, so a tool
  cannot drift below the endpoint it calls. `list_configuration` was the one place a single
  price could not work — reading the categories is configuration, reading the accounts discloses
  where credentials exist, reading the plugin inventory is administration, and neither of the
  latter two scopes confers the other — so its section now carries its own price on top of the
  tool's. A refusal carries the stable `auth.scope_insufficient` code and the scope it wanted,
  instead of English prose a client cannot act on.

- **A proxy profile can be changed and removed.** It could be created and listed and then
  nothing else: there was no `PUT /api/v1/proxy-profiles/{id}` and no `DELETE`, so a typo in an
  endpoint meant living with it. An update keeps a stored password when the request carries
  none, as an account edit does. A delete is refused while an account, an NNTP server, an
  unfinished download or the global proxy setting still points at the profile — nulling those
  columns instead would silently move traffic onto the direct connection, which is the one
  change somebody routing through a proxy would least want made for them.
- **A folder link in the LinkGrabber becomes the files behind it** (RD-104-03). A
  Premiumize.me cloud folder used to end in `premiumize.multi_file_source`, whose text told the
  person to split the source in the LinkGrabber — a step nobody had implemented. The resolver
  contract could not do better: `resolve` answers with exactly one file, so an address standing
  for several had no shape to come back in.

  There is now a tenth plugin world, `crawler`: `claims-url` decides whether an address belongs
  to the plugin and reaches nothing while it does, `crawl` answers with the files behind it, and
  everything it returns goes through the same review, the same domain blocklist and the same
  routing rules a pasted link does. The decision, and the three designs that were rejected —
  extending `intake`, making `resolved-download` a variant, writing it natively into
  `rd-collector` — are recorded in
  `docs/adr/0001-resolving-an-address-that-points-at-many-files.md`, the first entry in a new
  `docs/adr/`. `api_version` stays at `0.6.0` and the ADR says why: adding a world takes nothing
  away from an existing one, and seven open jobs name `rdownloader:plugin@0.6.0` as their
  dependency word for word. A resolver signed against the contract before the world existed is
  packaged, verified and instantiated by a test rather than assumed to still work.

  `plugins/premiumize-crawler/` is the reference implementation — a sibling of the resolver, as
  `premiumize-auth` already is, because a manifest carries exactly one `plugin_type`. It walks
  `folder/list` and `item/details` breadth first under its own limits: at most four levels deep,
  five hundred files, a hundred folders, and a set of visited ids so a folder that contains
  itself is read once rather than for ever. Names, sizes and the folder each file sat in come
  back with it, and the folder becomes the package suggestion — links sharing one are grouped
  into a package of their own, so a share with three seasons in it no longer arrives as one
  heap. An empty, missing or unreachable folder reports a stable code in the person's own
  language instead of quietly producing nothing, which is the defect the job was opened for.
  `premiumize.multi_file_source` is gone from the resolver and from its four catalogues.

  `rdownloader plugin new --type crawler` scaffolds one: the tenth SDK template, with a working
  folder walk, an address matcher and a listing reader outside the component, so `cargo test`
  runs twelve unit tests in a fresh scaffold before anything is changed.

- **`plugin new --type oauth` scaffolds an OAuth plugin, and the exchange itself is tested**
  (RD-105-01). RD-103-00 shipped the `oauth-plugin` world, the host, the adapter, the renewal
  sweep and the callback endpoint — but no SDK template and no value in the command's type list,
  so the world existed and nobody could write a plugin for it. There is now a ninth template
  next to the other eight: it compiles, packages and passes conformance before a line of it is
  changed, and it is a real authorization-code flow with PKCE rather than a sketch, including
  SHA-256 and base64url in the guest, because there is no WASI in a plugin to borrow them from.
  Its response reading and its crypto sit outside the component, so `cargo test` in a fresh
  scaffold runs twelve unit tests without a WebAssembly toolchain.

  What the scaffold could not do until now was be checked. `plugins/example-oauth/` is the
  reference plugin — the sibling of `example-transfer`, pointed at a host nobody can reach — and
  the contract tests drive it as a real component against a mock authorization server that
  verifies the PKCE challenge the way a provider does: the verifier the exchange sends has to
  hash to the challenge the authorization URL published, or the exchange is refused. That is the
  end-to-end proof RD-103-00 had to leave open. Five answers are covered — granted, consent
  refused, expired code, refused refresh, and a rate limit with `Retry-After` — from sanitised
  fixtures a test keeps free of credential material.

  One defect it found, and one rule it sets. The defect: a renewal or exchange could never have
  stored anything, because the confined host every extension plugin runs behind forwarded
  `store-token` but not its OAuth sibling, so a successful exchange ended by telling the plugin
  the host had no vault. The rule: a provider's error text is a route out for anything the
  provider echoes into it, so the scaffold keeps the RFC 6749 error code and drops everything
  else whole rather than filtering it character by character — filtering keeps the digits of a
  leaked token. The three shipped device-flow plugins do not do this yet and still report the
  provider's `error` field verbatim; bringing them in line is left to the jobs that touch them.

- **An account can be signed in through a redirect, and stays signed in** (RD-103-00).
  Providers needing an OAuth authorization code rather than a device code get a plugin world of
  their own, `oauth-plugin`, and an access token about to expire is renewed a minute ahead of
  time from stored refresh material, without anybody being asked again. Google Drive, OneDrive,
  Dropbox and Real-Debrid all need this and none of it existed: the tree had device-code and
  PIN flows and no refresh path at all, so it is built once here rather than four times over.

  The redirect comes back to one fixed callback address, because a redirect URI has to be
  registered with the provider before it is first used and a per-account path could not be.
  What ties an arriving callback to an account is a value the provider echoes back: a callback
  quoting one no flow claims belongs to nobody, and the value is dropped the moment it is
  answered, so a code cannot be presented twice. Both tokens go to the vault and only the
  expiry is kept in the clear, because the renewal sweep has to be able to ask whether a flow
  is due without decrypting a secret to find out.

  A provider that refuses to renew ends the sign-in and says so. A provider that cannot be
  reached does not: a call that never completed says nothing about the credential, so the token
  is kept and tried again later rather than the person being sent back through a sign-in they
  did not need. The WIT package version deliberately stays at `0.6.0` — every interface name
  carries it, so moving it would rename the imports of all 26 signed plugin packages already in
  the field, and everything added here is additive.

- **Translation keys are checked against the code that produces them, not only against each
  other.** `locales.test.ts` compares the four catalogues, which cannot see a key that is
  missing — or misplaced — in all four at once. Two got through in a single sitting: six server
  codes written as `proxy: { deleted: … }` beside `codes` instead of flat inside it, so
  `server.codes.proxy.deleted` resolved in no language; and `plugins.type.oauth` missing
  everywhere, so an installed OAuth plugin showed the raw key. The first has a cause worth
  naming: `scripts/i18n-key.sh` splits a dotted key into a nested group, which is right for
  `plugins.json` and wrong for the flat codes in `server.json`. `scripts/README.md` now says so.

  `web/src/i18n/sourceKeys.test.ts` resolves keys taken from where they are produced: the codes
  the REST layer constructs (`ApiError::*`, `MessageResponse::new`), the plugin types
  `PluginType::as_str` accepts, the literal `t('…')` calls in stores, composables and helpers,
  and the shape `server.json` has to keep. Both cases above fail it when reintroduced. Its own
  doc comment lists what it cannot see — keys composed from a variable, codes raised outside
  `crates/rd-api/src` or passed to a helper, and whether a translation says the right thing —
  because a coverage test that suggests completeness is worse than none.

  It found 76 backend codes that no catalogue translates, among them the whole `subscription.*`
  family, most of `settings.*` and `plugin.removed`: they reach the reader as the backend's
  English prose in every language. They are listed in the test rather than fixed here — 76 codes
  are 304 sentences and belong in a translation pass — and the list may only shrink: a code that
  gains a translation has to leave it or the test fails.
### Fixed

- **A list no longer says it is empty while it is still being fetched** (RD-104-07). Opening
  the accounts tab fires three requests at once and tracked none of them: the component's
  `pending` belongs to its form, and every other flag there to an action — connect, test,
  delete. So for the length of the fetch the accounts list was empty, the empty state rendered
  off that emptiness, and the tab told somebody with accounts that they had none, right before
  showing them their accounts. On a slow link it stood long enough to be believed, and a failed
  request left the same sentence standing for good — which quietly turns a server failure into
  "nothing there".

  It was never one tab. Of nineteen settings components that fetch on mount, five tracked it;
  of six views, one. The fix is one pattern rather than twenty: an area that fetches is in
  exactly one of three states — loading, failed, empty — and `design.md` now says so beside the
  promise it previously made only for forms. `DataState.vue` draws all three and
  `useFetchState()` holds the pair behind it, with the loading flag starting true, because a
  component that fetches on mount is loading from its first render. The 28 px signal grid the
  design language already named for loading surfaces is what the loading state is drawn on; it
  existed in CSS and in seven places, but never during a fetch.

  Where a store already carried a flag — transfers, collector, subscriptions, automations — the
  view reads it instead of keeping a second copy: each of those flags covered the write actions
  or was false before the first `refresh()` was even called, which is precisely the frame the
  empty state rendered in. Routing, notifications, bandwidth, plugins, usenet, proxies, MCP
  access, capture pairing, passkeys and the four views all follow the same shape now, and the
  vendor tool card no longer reports that no directories were searched before its status has
  arrived.

  Also adds the shared component-test harness (`web/src/test/mount.ts`) the new tests use, so
  the Pinia, i18n and Nuxt UI stub boilerplate is written once instead of per file, and
  `SettingsAccountsTab.test.ts`, which did not exist at all.

### Changed

- **How long is left, per entry and for the queue — and the rate now lives on the server**
  (RD-104-02). There was no estimate anywhere: the sizes were there, the rate was not, at least
  not in one place. It was derived twice, differently. The web interface smoothed the change in
  committed bytes between two `/api/v1/downloads` reads (exponential average, weight 0.65, last
  value held for five seconds); the desktop tray took the raw difference between two
  `/api/v1/capture/summary` reads with no smoothing at all. Two answers to the same question, and
  a third — MCP, an automation, a notification — was only a matter of time.

  So the rate moved to the service. It is sampled once a second in the scheduler's existing
  supervise loop, in memory and never persisted, with exactly the smoothing the browser used, so
  the displayed figure did not change when it moved. The traffic budget already ticking in that
  loop was no substitute: it measures queue-wide totals every fifteen seconds, which says nothing
  about how fast one entry is moving. One change to the queue summary now serves REST and the MCP
  `get_status_summary` tool alike, `GET /api/v1/downloads/rates` carries the per-entry figures,
  the capture summary carries the queue's, and the tray's `RateTracker` is gone — it reads the
  rate instead of computing a second one.

  The estimate stays blank wherever a number would be invented. No known size, nothing moving, a
  rate of zero: nothing is shown — no infinity, no "calculating", no placeholder. The queue
  estimate is refused outright while any entry still to be fetched has no known size, because the
  sum would then be a lower bound presented as an answer. And it is drawn along a narrower line
  than the "remaining" byte figure beside it: waiting, resolving and downloading entries count,
  while paused, blocked, verifying, repairing, extracting and seeding ones do not, since none of
  them is being fetched. The byte figure still counts them, and its caption now says so rather
  than counting them silently.

- **Dragging and sorting work the same way in the LinkGrabber and in the download list**
  (RD-104-05). The report was that links could not be sorted in the LinkGrabber the way they can
  in the downloader. The truth was the other way round, and worse in both directions.

  In a LinkGrabber row `draggable` sat on the whole row while the grip beside it did nothing, so
  every drag across the file name started a reorder and the text could not be selected at all.
  The handle now carries the drag, as it already did in both package headers, and it names what
  it does. Files inside a download package, meanwhile, could not be dragged at all: the column
  `downloads.position` existed and was read, but nothing except the insert ever wrote it. There
  is now an endpoint that does — `POST /api/v1/downloads/reorder`, taking a package and its
  complete file list, the counterpart of the LinkGrabber's candidate reorder — plus the index on
  `downloads(package_id, position)` the candidates have had since 2024.

  Both reorder endpoints now refuse a list that is not exactly the package's rows, with the
  stable code `request.reorder_ids_mismatch`. This is a fix, not only a new rule: the candidate
  endpoint handed out the positions 1..n from whatever arrived, appended the rows the caller had
  forgotten, and fenced its `UPDATE` with `AND package_id = ?` — so an id from another package
  changed nothing at all and the answer was still "Order saved".

  A reorder that cannot be carried out now says why. Under an active filter the visible list is
  not the whole list, and writing it would misplace everything hidden; that case used to be a
  bare `return` in two places in the LinkGrabber, and the same trap was waiting in the filtered
  download list. Every drag also has a keyboard equivalent — focus the handle, `ArrowUp` or
  `ArrowDown` — because a sort reachable only with a pointer is not reachable at all for part of
  the audience. The handle and drag pattern is written down in `design.md`, the keyboard route in
  `docs/accessibility.md`.

- **A failed verification no longer locks a package away** (RD-104-04). A PAR2 set that
  arrived damaged beside archive volumes that are perfectly fine used to end the package there:
  no unpack, no cleanup, no plugin steps, and no way for a person to say "try it anyway". The
  single `par2_ok` flag that gated all of that is now one input among three, and what it does
  is a setting.

  **Post-process only verified packages** is SABnzbd's `safe_postproc`, on by default there and
  here, so nothing changes for an installation that never touches it; it is overridable per
  category. Switched off, unpacking and everything after it runs despite a failed check. Beside
  it there is a one-off: **post-process anyway**, a button on a failed package and
  `POST /api/v1/packages/{id}/extract/force`, which runs the pipeline once without changing the
  setting — because the reported case is somebody wanting this once, not a switch they then
  have to remember to put back. Either way the failure stays on the record, a user script still
  receives status `3`, and a package whose repair failed keeps its recovery data.

  Two more things follow from the same finding. **A corrupt main index is no longer the end of
  the set**: every `.vol` volume carries the same file descriptions, so verification moves on
  to a sibling before the package is given up on — SABnzbd's `promote_par2`, arrived at from
  the other side. And **SFV is a substitute check again, not a consequence**: it used to run
  only when PAR2 had already succeeded, which is precisely when it is not needed. It now runs
  on its own, and after it a **RAR integrity test** (`unrar t` / `7z t`, SABnzbd's
  `try_rar_check`) asks the archive itself, but only when neither PAR2 nor an `.sfv` index
  answered. A missing tool or an archive nobody has the password for is recorded as skipped,
  not as damage.

  What SABnzbd does that this does *not* adopt is written down with its reason in
  [docs/postprocessing.md](docs/postprocessing.md) — above all `postpone_pars`, which holds
  `vol` volumes back and fetches only the recovery blocks a repair actually needs. That one
  needs a package to be able to move back from post-processing into downloading, which nothing
  can do today; it is a job of its own and until it lands every volume of an NZB is still
  downloaded.

- **A package's archive password is readable again** (RD-104-04). It was stored and then only
  counted: the API answered `has_password: true` and the interface drew a padlock, so a person
  could not see what had been taken from the release title, from the `{{password}}` marker of a
  file name or from a feed — and could neither unpack by hand nor understand why an unpack had
  failed. The value now comes back on the package in the LinkGrabber, in the downloads and on
  an NZB import, and the edit dialog shows it prefilled instead of asking for it blind.

  This opens exactly one door. `openapi::secret_boundary_tests` walks every schema of the
  generated document and fails if any other credential — account password, API key, NNTP or
  proxy access, private key, passphrase, certificate or cookie jar — is not marked write-only.
  Writing it turned up five request bodies that had been missing the marker (`LoginRequest`,
  `SetupRequest`, `MfaDisableRequest`, `UpdateCaptchaConfigRequest`,
  `TestCaptchaSolverRequest`); they carry it now. A subscription hit still never serializes the
  password it announced: it reaches the user as a package, where it is readable, so a second
  place to hand it out would add nothing.

- **The plugin WASI guard reads the import section instead of scanning bytes.** It was
  `grep -a 'wasi:'` over the finished `.wasm`, which cannot tell an import from a string: a
  plugin whose data section merely contained the text — an error message, a locale string, a
  dependency's diagnostic — was rejected with "imports wasi:" while importing nothing at all. A
  component with zero imports and `error: wasi: interfaces are not available` in a data segment
  failed it. The new `scripts/check-plugin-imports.sh` asks `wasm-tools` for the component's
  world and rejects any import outside `rdownloader:plugin`, naming the offending interface.
  `build-plugins.sh` now runs it after every build, so a forbidden import is caught before the
  push rather than in CI, and CI runs the same script. Without `wasm-tools` installed the byte
  scan runs instead and says so, rather than checking nothing.

### Security

- **The PKCE verifier and the OAuth `state` are random again — they were computable.** The
  `oauth` SDK template and the reference plugin built both values as
  `sha256("<account-id>:<unix-second>:<salt>")`, with the salt a literal in the published source.
  Every input was public: an account id is readable with `api:read` over `/api/v1/accounts`, and
  a flow's lifetime narrows the second to a few hundred candidates. Both values could therefore
  be recomputed by anybody who cared to, which makes an intercepted authorization code
  redeemable a second time — exactly the property PKCE exists to remove — and makes the `state`
  forgeable, taking the CSRF protection with it. The module comment defended the derivation with
  "both values stay inside the application, so the exposure is bounded"; that reasoning was the
  actual defect, because a value nobody has to steal is not protected by being kept.

  The host interface has grown `random-bytes: func(count: u32) -> list<u8>`, served from the
  operating system's generator, capped at 1024 bytes and refusing anything above that — or a
  request for nothing — with an empty list. `begin` now draws 32 bytes for the verifier and 32
  more for the `state`, encodes each as base64url without padding, and fails the sign-in if the
  host answers short instead of falling back to something derived. The derived
  `pkce::verifier(account_id, now, salt)` is gone from both copies rather than deprecated, so no
  inherited scaffold can keep using it.

  `api_version` stays at `0.6.0`, for the reason the `crawler` world stayed there: adding a
  function to an interface a component already imports is additive — an older component does not
  import it and instantiates unchanged. That is verified rather than asserted, by a signed
  Premiumize package built before the change, which still verifies and instantiates against the
  new host. Anybody who scaffolded an OAuth plugin from the previous template should regenerate
  `src/pkce.rs` and `src/guest.rs`: a plugin already in the field keeps the weak values until it
  is rebuilt.

## [1.0.2] - 2026-09-08

This milestone was scoped as "Interactive Captchas and Managed Tools" and ships without the
captcha half. Answering a widget captcha inside rDownloader needs a measurement against a real
site key and a WebView in the desktop capture agent, and neither could be built or verified
where this release was cut. That work moved to 1.0.4 as RD-104-08 rather than being claimed
here. A Turnstile widget still reports `captcha.widget_needs_solver`, so the DDownload sign-in
continues to need a solver service or a pasted cookie session.

### Fixed

- **Plugin translations now arrive for the session you just signed in to** (RD-104-06). The web
  interface fetched them while starting up — before anybody was signed in — against an endpoint
  that requires a session, so every fresh start put a `401` in the browser console. Worse than
  the noise: the failed attempt was remembered as "done" and nothing tried again, so plugin
  strings — failure codes, provider names, descriptions, credential labels — stayed in the
  server's English for the rest of that session in all four languages, until the page was
  reloaded. The request now waits until a session exists and is made the moment one does, by
  either sign-in path or a reload that restores one; a refused fetch stays retryable instead of
  being remembered as complete, and signing out forgets what was merged. The endpoint's scope is
  deliberately unchanged: which plugins are installed stays behind the sign-in.

### Added

- **The managed tool manifest now names real builds, and `tar.xz` archives can be unpacked**
  (RD-102-04). Until now the manifest shipped with the release was correctly signed and
  entirely empty, so the managed-tools feature had nothing to install. It carries yt-dlp
  2026.08.19 for x86-64 and ARM64 Linux and Windows, and FFmpeg/ffprobe 9.0.1 for x86-64 Linux
  and Windows; every SHA-256 and size was produced by downloading the asset and hashing it, not
  transcribed from a release page. `ffmpeg` and `ffprobe` are two entries over one archive,
  each naming its own member. The Linux FFmpeg build is a `tar.xz`, which the unpacker did not
  understand before: extraction now handles it through a pure-Rust xz decoder, deliberately
  rather than a binding to liblzma — unpacking bytes fetched from a release host is exactly
  the place not to link a C decompressor whose 2024 supply-chain incident is still recent.
  Only regular files are taken out of a tar, so a symlink or device entry cannot make an
  extraction write anywhere; every member is flattened to its base name, so no member path can
  escape the staging directory; and what an archive expands to is counted as it is written
  rather than as its headers claim, for both `zip` and `tar.xz`.

  Two things are stated plainly rather than papered over. **FFmpeg comes from BtbN, a third
  party, not an official FFmpeg distribution** — RD-102-02 put "Tools ohne nachvollziehbare
  offizielle Distribution" out of scope, and this is a deliberate, user-approved deviation
  from that, because the FFmpeg project publishes no binaries at all. The entry is pinned to
  the immutable dated release `autobuild-2026-09-07-15-39` rather than to the `latest` tag,
  whose bytes change on every rebuild and would make a pinned hash go stale within days. And
  **gallery-dl and Streamlink are not managed**: gallery-dl distributes through PyPI and its
  last eight GitHub releases carry no assets at all, while the Streamlink Windows bundle is a
  2471-entry ZIP holding a complete Python runtime, which the flat `members` model would
  flatten into colliding names. Both remain fully supported when installed by other means.
  `docs/external-tools.md` carries the source table, the deviation and both gaps.

- **An external tool that is too old or known to be broken now says so, and blocks only what
  it breaks** (RD-102-03). Four verdicts that do not collapse into a boolean: supported, too
  old, known bad, and unknown. Unknown never blocks — FFmpeg git builds print
  `N-113522-g8b0a3d5c` with no number to compare, and those builds are newer than every
  release rather than older, so treating an unreadable version as an ancient one would turn
  every unusual build into a broken installation. A rule names the capabilities it affects and
  only those are gated: an outdated yt-dlp stops media downloads while HTTP, Usenet, torrents
  and everything else keep running. The new failure code `media.tool_incompatible` carries the
  affected capability, and the interface translates it, because a warning that says
  `media_merge` in every language names nothing. Rules travel inside the signed tool manifest
  under the same signature and replay floor as the builds they describe; any failure to read a
  delivered rule set — bad signature, stale document, unknown tool, unparseable version, a rule
  that gates nothing — drops the whole set and leaves the compiled-in base in force. An
  override is an explicit list of tools in the settings that keeps the warning and drops only
  the block, and writes a log record every time it does. `doctor`, the settings page and
  `GET /api/v1/system/media` all report version, verdict and upgrade path.
  `docs/external-tools.md` publishes the matrix, the shipped floors and the platform
  differences.

- **The tool status page stopped spawning eight processes every time it loads.** Reading a
  tool's version now happens once per binary and is cached against that file's modification
  time and size, so replacing a binary, upgrading a system package or activating another
  managed version invalidates the entry without anything having to remember to.

- **rDownloader can keep yt-dlp, gallery-dl, Streamlink and FFmpeg up to date itself**
  (RD-102-02). Off by default, because downloading executables is not something to start
  unasked. A signed manifest names every installable build with its URL, SHA-256, size and the
  application versions it is declared to work with; the manifest is verified against a
  compiled-in root and refused when its sequence is not above the one this installation has
  already accepted, so yesterday's genuine manifest cannot be replayed. Bytes stream into a
  staging directory and are hashed as they arrive — a mismatch removes the staging directory
  and nothing else. Activation is a pointer file plus one rename, not a symlink, because
  Windows makes symlinks a privilege. A running job keeps the version it started with: it holds
  a lease, and a leased version is never removed, while activating another one takes effect for
  the next job immediately. An explicitly configured path still wins over everything managed,
  so a system tool stays a system tool. New endpoints under `/api/v1/system/tools`, a new
  section in the interface settings, and a new `rd-tools` crate.

- **An indexer hit shows what the indexer knows about it.** Cover image, IMDb id, score and
  plot, season and episode, resolution, codecs, genre, grabs and size now appear on a hit, in
  both places hits are listed. None of it costs a request: `extended=1` was already being sent
  and every `newznab:attr` already parsed, but the indexer adapter built its item without the
  attribute map, so all of it was parsed and dropped one module later. The row carries the
  thumbnail and the size, the rest is behind the chevron, and attributes with no field of their
  own are listed rather than hidden.
- **An announced archive password reaches the extractor.** It is tried before the shared
  password list. Two sources: the SABnzbd `{{secret}}` marker in the title, which previously
  worked only where the title had already become an NZB file name and so never applied to
  torrent hits, and an indexer that writes a real password where Newznab specifies its `0`/`1`/
  `2` flag. The flag stays a flag: stored as a password it would make every unpack start with a
  wrong one.
- **A sign-out button**, in the sidebar footer beside the connection indicator. The endpoint
  had been there all along and is covered by tests; the interface simply never offered it, so
  ending a session meant clearing browser data or waiting for the cookie to expire. A logout
  whose request fails still signs this browser out, because the alternative is somebody seeing
  an error and walking away from an open session.
- **A switch for cover images**, on by default, under LinkGrabber settings. Nothing is fetched
  on the strength of it — the addresses arrive with the search answer — but the browser loading
  the pictures does tell the indexer which hits are on somebody's screen. Switching it off
  leaves every written detail visible.

### Fixed

- **Several subscriptions against one indexer no longer fire at the same instant.** The poll
  schedule spreads subscriptions by their own id, which says nothing about the server at the
  other end -- so four subscriptions to one indexer, one per category, became due together and
  went out together; the recorded history of such a setup shows request pairs 0.0 seconds
  apart. Requests to one host are now serialized with a quiet time between them, on the path
  both the schedule and the "check now" button take. Different hosts still never wait for each
  other.
- **An archive password is no longer printed in the package name.** A hit whose title carries the
  SABnzbd `{{secret}}` marker became a package called after the marker, so the password stood in
  plain text in the LinkGrabber and the queue. The archived item keeps the title exactly as the
  indexer wrote it, because identity is built from it; only what is handed onward, and what the
  list prints, is cleaned.
- **An archive password no longer disappears on the Usenet path.** Enqueueing an NZB link from
  the LinkGrabber read the password only from the file-name marker and never consulted the one
  the package already held, so a password announced by a subscription, a DLC container or the
  API was silently dropped for exactly the hits that most often need one.

### Changed

- **Both indexer hit lists share one row.** The LinkGrabber's review list and the archive under
  an expanded subscription had drifted into two different rows for the same thing; the archive's
  version still used plain text buttons for Queue and Dismiss, which the row conventions
  forbid. `design.md` gains the conventions for imagery in a row, which it did not cover at all.

- **The feature overview and the marketing package are back in step with the product.**
  `docs/feature-list.md` was baselined on 0.9.0 and its key-facts table still reported 0.6.1.
  Everything since is in it: the 1.0 authentication work, DDownload's own sign-in and the generic
  XFileSharing resolver, mirrors, reconnect, keeping the machine awake, unattended housekeeping,
  the four container formats, settings as pages, the provider registry built solely from plugin
  manifests, and the container image's helpers and `PUID`/`PGID`.

  Its numbering had drifted into 19 → 19c → 20 → 19b → 17a…17f → 18a → 19a → 20a, and the
  authentication material had been appended to the MCP section, which is not where anybody would
  look for it. The sections are now one sequence of 37 in reading order, the plugin types sit with
  the plugin system, and authentication is a section of its own that the security section points
  at.

  `docs/rdownloader-marketing-website-package/` was forward-looking copy for a 0.9.0 that has
  since shipped, so it is re-baselined on the released 1.0.1 with 1.0.2 and 1.0.3 as an explicit
  staging lane. The correction that mattered was not a version string: signed plugin repositories,
  staged updates, rollback and repository disablement were presented as available capability
  across five documents, and none of them exist — they are milestone 1.4 work. Every such claim is
  gone, each document carries a guardrail against restoring it from an older draft, and the stale
  counts went with it (fourteen MCP tools are sixteen, eleven resolvers are twelve, twenty-five
  bundled plugins are twenty-six). The 1.0 access-control story, which the package had no place
  for at all, is now a message-house pillar, a features-page section, a homepage trust block, a
  page brief, an SEO cluster and a set of launch posts.

  The checked-in `docs/rDownloader-marketing-website-package.zip` is removed: it was built two
  package versions ago, nothing referenced it, and a binary duplicate of the directory beside it
  can only drift again.

## [1.0.1] - 2026-09-07

Sixteen corrections and improvements reported from daily use on 0.9.8 and 1.0.0, plus two found
while setting the release up. The heaviest of them was not on anybody's list: DDownload had put a
captcha on its login form, and the sign-in reported the failure as a cookie problem the user did
not have. The most far-reaching was a duplicate bookkeeping nobody had looked at in a while —
eleven hosters were compiled into the binary *and* declared in their plugin's manifest, and only
the compiled copy had any effect.

### Changed

- **A provider exists exactly while its plugin does.** Eleven hosters were compiled into the
  binary as provider rows *and* declared in their plugin's manifest, where the registry refused
  the declaration because a built-in owned the slug. The manifest halves were dead configuration
  that had never run, and the two copies had already drifted apart.

  The visible cost was an accounts list offering DDownload, Rapidgator and nine others on an
  installation with no plugins at all — an account could be created for a provider nothing could
  resolve. The list now shows what is installed, and the registry is rebuilt when a plugin is
  installed, removed or switched off rather than only at startup. An account whose plugin was
  uninstalled stays and stays editable, so it can still be switched off; only creating a new one
  for an unknown provider is refused.

  The migration was not a deletion. Each built-in row was compared against its manifest twin
  first: ten differences across eight providers, and exactly one of them a loss — Premiumize's
  manifest had dropped the apex domain its built-in row granted and carried no cookie scope,
  which would have let the first request decide where an account's cookies live. Both went back
  into the manifest. The rest were the manifests claiming more, all of it inert or already
  reachable through host aliases.

  The rule that a plugin could never take a built-in's slug went with the table. What protects a
  well-known provider now is signature trust — an untrusted package does not load — and the rule
  that always carried the weight: no plugin may claim a credential another provider owns, which
  is what stops the hosts an existing secret may reach from being widened.

- **Plugin links point at the project site.** All twenty-six bundled manifests declared the source
  repository as their homepage, and the plugin manager renders that value as a "Website" link.
  They say `rdownloader.net` now. Without a version bump on purpose: bundled plugins are only
  replaced by newer ones, so an existing installation keeps the old link until its plugin rises
  for a reason of substance, while a freshly built package is right from the start.

### Added

- **An account is checked as soon as it is saved, and switched on or off from the list.** Adding
  an account and finding out whether it works were two separate motions, and the second one was
  easy not to make; the first now runs on its own after a successful save. Deliberately without
  making the dialog wait for it — a check reaches into the provider's resolver, and DDownload's
  sign-in can park on a captcha for as long as somebody takes to answer it. The row shows a
  spinner and the result lands when it does.

  The enabled state used to be a coloured dot in the list and a switch inside the edit form, so
  turning an account off meant opening it and saving. It is a switch in the list now, going
  through the same update endpoint as before: that one already leaves the stored password and
  cookies alone when none are sent, and a second write path would only be another place to get
  that wrong.

- **The tray icon shows when something is transferring, with the count, the progress and the
  rate.** The icon was decoded once and never replaced, and the capture agent knew nothing about
  downloads: its event stream is filtered to intake deliberately, because a capture token is a
  narrow credential and the full bus carries download paths and account names.

  That reasoning is kept rather than widened. A new capture-scoped endpoint answers with figures
  only — how many are running, queued and failed, and how many bytes of the unfinished work are
  committed — and nothing that names a file, a folder or an account. The rate is derived by the
  agent from the change between two readings, the same way the web interface derives it, because
  the service keeps no rate of its own. The busy icon is derived from the idle one at runtime
  rather than shipped as a second file, so the two cannot drift apart.

- **Subscriptions, streams and automations can be written to a file and read back on another
  installation.** One format with a section per area, following the routing bundle rather than the
  settings backup: it merges by name, leaves what is already there alone, and carries no secrets,
  which is why it needs no passphrase. Everything that crosses an instance boundary travels as a
  name — a category, a notification target, a stream channel — because an id means nothing on the
  other side.

  Stream schedules and automations were in no bundle at all until now. An automation whose action
  points at something the target does not have is skipped whole rather than imported without it;
  half an automation is not the one that was exported. A subscription that needed an API key
  arrives switched off, since the key stays in the vault and a subscription polling without its
  credential only produces failures. A file for the wrong area is refused by name instead of
  reporting "0 imported" and looking like it worked.

- **Row actions look and behave the same everywhere, and the subscriptions list was brought in
  line.** Reported as an inconsistency and a question: shouldn't this be in the design document?
  It wasn't. The document is `design.md` at the repository root, `AGENTS.md` never mentioned it,
  and so there was no findable place for such a convention to live — which is how the
  subscriptions list ended up with five text buttons, an accordion that did not look like one,
  and a delete that fired without asking, something the design document had forbidden in
  principle for a long time.

  The convention already existed in the code, unwritten, across half a dozen components. It is
  written down now — icon-only edit and delete with both a label and a title, a chevron pair with
  `aria-expanded` for expanding, a switch for anything that takes effect immediately, confirmation
  for anything destructive — and `AGENTS.md` points at it. The reported list follows it, and the
  deviations still out there are listed in the job rather than quietly left.

- **A subscription can be duplicated.** Entirely client-side on the ordinary create endpoint, the
  way category rules have done it for a while. The copy arrives switched off and without an API
  key: the key lives in the vault behind a reference, and handing the copy the same one would
  leave two subscriptions quietly sharing a credential. Filters, category routing and the
  requested categories come along; what the original has already seen does not.

- **An indexer's categories can be picked before the subscription is saved, and only the chosen
  ones are fetched.** Two halves of the same complaint. Mapping categories required a saved
  subscription, because the endpoint resolved the API key out of the vault — so an indexer had to
  be added, then reopened, before it could be configured. A second endpoint takes the address and
  the key in the request instead, uses them for that one call and stores neither.

  And the mapping only ever sorted results after they arrived: the query asked for everything, so
  a subscription interested in one category pulled the indexer's whole feed and discarded most of
  it. What to fetch is now its own field, sent as `cat`. It is deliberately not derived from the
  mapping — one can want a category fetched without redirecting it anywhere, and keep a mapping
  for a category not currently being fetched. An address that already names its categories keeps
  them: a saved search pasted out of an indexer's own RSS button means what it says.

  While fixing the round trip: neither the category mapping nor the new field was carried in the
  settings bundle, so restoring a backup quietly dropped an indexer's routing and left it pulling
  everything again. Both travel now.

- **Where a plugin is offered for selection, its version is visible.** Installing a plugin never
  removes the older version, so two can sit side by side; the highest wins at load time. That was
  always well defined and never shown — a provider dropdown read "DDownload" whether the account
  would be served by 0.10.0 or 0.10.1, and the plugin manager listed both with nothing to tell
  them apart.

  The provider response now carries the plugin and the version behind it, five separate lists
  render that through one helper instead of five spellings, and the manager marks which installed
  version is the one being loaded — saying, in the other one's tooltip, that it can be removed.

- **A QR code for the authenticator app.** Setting up two-factor sign-in meant reading a
  thirty-two character key off the screen and typing it into a phone. The `otpauth://` address
  that authenticator apps scan had been in the enrolment response since the feature shipped — the
  field's own comment calls it "URL for the QR code" — and the web UI put it behind a "copy link"
  button and showed the key as text instead.

  It is drawn in the browser now, from the address that was already there. Deliberately not
  fetched as a picture from the service: that would send the shared secret over the wire a second
  time for no gain. The key and both copy buttons stay, for an authenticator without a camera.

### Fixed

- **DDownload's sign-in answers the captcha its login form now carries.** Signing in with a
  stored username and password stopped working, and said so in the least useful way available:
  "the cookies were not sent or do not belong to a logged-in session" — for an account that uses
  no cookies at all.

  DDownload has added a Cloudflare Turnstile widget to its login form since that path shipped, so
  the credentials were never read; the answer is a page reading "Wrong captcha". Because that page
  is also a login wall, the fallback branch guessed at cookies while the real explanation sat
  unread in the same document. The page's own message is now preferred over the guess.

  Nothing was missing from the application: the plugin contract already has a Turnstile challenge,
  the captcha broker already routes one to a configured solver service or to the user, and the
  free download flow has solved widget challenges all along. Only the sign-in never asked, because
  when it was written there was nothing to ask about. It now finds the widget belonging to the
  login form — not the one in the register dialog beside it — has it solved, and sends the token
  with the submission. A refused captcha is reported as exactly that, and as temporary, because
  nothing about the account is wrong.

- **A download folder that cannot be used says which part is wrong.** Typing `/downloads` into
  the setup wizard answered "internal service error". The path is absolute, which was the only
  thing checked, so it passed — and then `create_dir_all` failed because the filesystem root
  belongs to another user, and an untyped failure reached the browser as a generic 500 with no
  code for the interface to translate.

  Six named causes replace it, each a stable error code with the path attached: the path is a
  file, no permission, a read-only filesystem, not creatable, not writable, not absolute. The
  check also writes a probe file and removes it again, because creating a directory proves
  nothing about writing into one — a read-only mount or a folder owned by somebody else passes
  `create_dir_all` and fails at the first download instead. Editing a storage root is checked the
  same way as creating one.

  `/downloads` was not a guess, either: the form started on that path, hardcoded. It is right
  inside the container image and wrong on every native install, so the application was proposing
  the failure. The service now reports the directory it actually writes to, and the wizard offers
  that instead.

- **A link whose check failed can be added to the downloader again.** A hoster account whose
  sign-in does not work makes the batched online check fail, and every link of that batch ends up
  in the `error` state. That was the one state the queue refused — while `offline`, which means
  the file is known to be gone, went through. A check that never reached a conclusion says
  something about the check, not about the file, so the link ended up worse off than one reported
  dead.

  The list of queueable states existed in five places, each copy slightly different, and the one
  that actually stopped people was in the browser: the package's "add to downloader" button is
  enabled only when at least one link is `online` or `duplicate`, so a package of failed checks
  offered a greyed-out button and no explanation. All five now derive from one list —
  `LinkCandidateState::ENQUEUEABLE` on the service side, mirrored once in the web UI.

  The same pass stopped the package summary from counting links whose check failed as *offline*.
  They are now counted and labelled separately as not checkable, and the link's own badge says
  "check failed" rather than "error" — a file was never missing here, the account was.

- **The Windows launcher starts the capture agent again, and a failed start stays readable.**
  Reported as "it no longer starts from `start-rdownloader.bat`, but it does on its own". The
  installation had the reason on disk: `Click'n'Load port 9666 is unavailable on every loopback
  address`. Nothing but a capture agent binds that port, so it was already held — a second agent
  from autostart, or JDownloader.

  Four things turned that into a mystery. The agent reported an ordinary "there is already one"
  as a crash. On Windows it runs under the tray's event loop, which ended every failure on exit
  code 1, so the distinct codes a launcher reads could not reach one from the platform that has
  the launcher. The launcher's own "is one already running" test read each candidate process's
  path and counted a path it was not allowed to read as *not running* — then started a second
  agent, which died on the port. And the window closed on the message before it could be read.

  A busy Click'n'Load port is now its own exit code, reported identically from the tray and the
  headless path, and named with the addresses actually tried. The launchers on all three platforms
  treat it as "already running". A process whose path cannot be read counts as running rather than
  absent. And a failed run holds the window open — only when it was opened by double-clicking, so
  an unattended run still cannot hang.

## [1.0.0] - 2026-09-07

1.0 was scoped as operational maturity, and that turned out to be more than one release could
carry honestly. This one carries the half that had to be right before anybody could reasonably
put the service on a network: how people and machines prove who they are, and what a credential
reaches once they have. The in-app updater, managed external tools, plugin repositories and
installer packaging were not dropped — they moved to 1.0.2, 1.0.3 and 1.0.4, and `docs/roadmap.md`
says which went where and why. 1.0.1 goes to corrections reported from daily use before any of it
starts.

Nothing here requires a migration you have to think about, and nothing changes what an existing
token or client could already do.

### Added

- **API tokens are scoped by area rather than by "read" or "everything".** Until now a machine
  token was one of two things: `api:read`, which reached eight status routes on a hardcoded
  list, or `api:*`, which reached the entire API — settings, stored passwords and all. Anything
  a monitoring dashboard could not do with the first, it had to be trusted with the second.

  There are now six areas — reading, intake, queue control, configuration, credentials and
  administration — and every one of the API's 228 operations says which it belongs to. The
  division follows what a person would actually delegate: a `*arr` instance adds links, an
  automation controls the queue, a provisioning script writes configuration. Credentials and
  administration are separate from all of it, because "may change the download folder" and
  "may read every stored password" are not the same request even though both are settings.

  Two properties are enforced rather than intended. Nothing confers the credentials or the
  administration area — not even administration confers credentials. And everything that acts
  can also look, because controlling a queue you cannot see is useless and demanding a second
  permission for it would only teach people to grant everything.

  Existing tokens keep working and keep every capability they had: `api:*` continues to mean
  every area, and `api:read` continues to mean reading. The read surface was kept as narrow as
  the list it replaces, for the reason that list gave — a token pasted into a status page must
  not become a way to enumerate the installation — so the LinkGrabber, the script and upload
  destination lists, run histories and the provider table stay out of it.

  A refusal now names the permission that was missing instead of saying only that the token may
  read status resources. Presenting a browser-capture token on an API route answers "your
  credential does not cover this" rather than "please log in", which the holder has already done.

  Minting a token now asks which areas it should hold, with the number of operations each one
  reaches shown beside it and the two that nothing else confers — stored credentials and
  administration — flagged. The form opens on reading alone, because a form that opens on
  "everything" is a form whose default everybody keeps. Those numbers are counted from the same
  table that enforces the permissions, so what you are shown while granting cannot drift away
  from what refuses you later. A permission the model does not have is refused rather than
  quietly dropped, and `capture:*` cannot be minted as an API token at all.

- **The event stream is filtered by the same six areas.** Each kind of event belongs to one, so
  a monitoring token sees queue and progress while account, proxy, credential and plugin changes
  stay out of its stream. A new kind of event is a compile error until somebody decides who may
  see it, rather than defaulting to everyone.

- **The MCP server applies the same six areas per tool.** All sixteen tools sat behind one
  check for full API access, so letting an assistant watch the queue meant letting it read
  every stored account and rewrite the settings document in the same breath. Each tool now
  costs what the REST route behind it costs — `list_downloads` is reading, `delete_packages` is
  queue control, `list_configuration` is credentials — and a token reaches the ones its
  permissions cover. The endpoint itself now admits any token carrying an API permission, so a
  read-only assistant is possible at all; a browser-capture token still reaches none of it.

- **You can see what is signed in to your service, and sign it out.** Sessions lived in a map
  inside the process: they vanished on every restart, carried no information about what had
  opened them, and could not be listed or ended. Signing out was not possible at all. There is
  now a list under Settings → Security showing each open sign-in with its device, address, when
  it was last used and when it lapses, with the browser you are sitting at marked as such. Any
  of them can be ended, and "sign out everywhere else" ends the rest while deliberately keeping
  yours — an action that also signs you out is one nobody can use to check whether it worked.

  Sessions now survive a restart, which is the part people notice first. The bearer itself is
  never stored, only its SHA-256 digest, so a copied database or a backup hands over nothing
  usable. Their lifetime stays twelve hours from sign-in rather than from last use: an absolute
  limit puts a floor under how long a stolen session is worth anything.

- **Guessing the password is now rate limited, in a way that cannot be turned against you.**
  There was no limit at all. The obvious fix — counting failures and locking the account — is
  the wrong one here, because this installation has one account: anyone who could reach the
  login form would then be able to lock you out of your own service by failing on purpose. So
  the lockout is per address, which is what actually stops brute force and leaves your laptop
  untouched while an attacker hammers from somewhere else; and a second, much gentler counter
  slows every attempt down while an attack is running, capped at two seconds and incapable of
  refusing anyone. Getting your password right clears both.

  A forwarded address is believed only from a proxy you have named. With nothing configured,
  the address rDownloader can actually see is the one it uses, and no header can change that —
  otherwise the limit would be bypassed by varying a string the client chooses.

- **Running behind a reverse proxy is a setting rather than a guess.** Three things had to be
  right for a proxied deployment and none of them could be told to rDownloader: which hops may
  speak for a client, what the outside world calls this installation, and whether the session
  cookie may travel unencrypted. They are now one section under Settings → Security, with the
  external URL carrying the scheme, host and mount point together so they cannot disagree.

  The whole application works under a sub-path — `https://home.example.com/downloads` — API,
  event stream and MCP endpoint included, with the asset and manifest references rewritten as
  they are served. `rdownloader doctor` prints the resolved contract and warns about the
  half-configured combinations: an external URL without trusted proxies, which makes everyone
  share one rate limit and one address in the session list, or a forced-`Secure` cookie on a
  plain HTTP deployment, where the browser silently drops it and signing in appears to work
  while the next request is not authenticated. Sample nginx, Caddy and Traefik configurations
  are in `docs/reverse-proxy.md`.

- **An optional second factor, with a way back in when the phone is gone.** Sign-in can now ask
  for a code from an authenticator app in addition to the password. It is off until you turn it
  on, and the whole design is shaped by one fact: this is a service you host yourself, with one
  account and nobody to prove your identity to if you are locked out.

  So enrolment happens in two steps — scanning a code does not gate sign-in until a code from
  it has been accepted once, because otherwise scanning badly locks you out by the act of
  trying to be safer. Ten recovery codes are issued at the same moment, while you are already
  looking at a screen you are meant to write things down from. Each works exactly once. And
  switching the second factor off takes the password, not a code: a lost phone must not be
  permanent.

  A wrong password never reveals that a second factor exists — the prompt for a code appears
  only once the password was right, so an unauthenticated guess learns nothing about the
  account. The seed lives in the encrypted secret store, never in the database, and never
  leaves in a settings backup; there is a test that fails if that ever changes.

- **A security policy, in `SECURITY.md`.** Where to report a vulnerability — a confidential
  issue in the project's GitLab — what belongs in a report, and how long an acknowledgement
  realistically takes for a project with one maintainer and no on-call rotation.

  It also says which version gets fixes (the most recent one; there are no backport branches,
  because a backport branch nobody tests is worse than an honest "upgrade"), and lists the things
  that look like findings and are not: the loopback-and-no-password first-run state, the
  administrator's ability to run scripts and install plugins, and a rate limiter deliberately
  incapable of locking the owner out of their own service.

- **Passkeys: sign in with your device instead of your password.** Add one under Settings →
  Security and the sign-in screen offers it above the password field. Your authenticator checks
  a PIN or a fingerprint before it will sign, so the single step already carries both factors.

  A passkey is another way in, not a replacement and not an extra hurdle. Enrolling one does
  not switch the code prompt on, and switching the code prompt off does not delete your
  passkeys — either behaviour would lock somebody out through a setting they touched for an
  unrelated reason, and both directions are held by tests. Your password keeps working
  throughout, which is what makes losing the device survivable.

  A passkey is bound to the address this installation is reached at, and that binding is the
  entire reason it cannot be phished. It therefore comes from the external URL you configured,
  never from a header the caller sends. `localhost` is the one exception, so a fresh install
  works before anything is configured. If neither applies — an IP address, or a domain with no
  external URL set — the request is refused with the reason and a pointer to the setting,
  rather than being bound to whatever was claimed.

  The ceremony itself is `webauthn-rs` rather than our own code. That is the opposite of the
  call made for the authenticator app, and deliberately: RFC 6238 publishes reference values, so
  a hand-written TOTP can be *proven* to conform. WebAuthn publishes none, so a hand-written one
  could only be checked against fixtures we generated ourselves — and "agrees with itself" is
  precisely the property a quiet authentication bypass would also have.

### Fixed

- **The qBittorrent compatibility routes are authenticated by the router rather than by each
  handler remembering to ask.** Every one of the seventeen handlers opened with the same three
  lines checking the credential. That works until somebody adds an eighteenth and does not:
  the route would then have answered normally to anyone who called it, with nothing anywhere to
  say so, because the compatibility routes carried no layer to fall back on. The check is now a
  layer around everything except login and logout, and a test walks the router's own source so
  a new route is covered whether or not anyone remembers to write a test for it. No route was
  actually unprotected — this closes the way it could happen, not a hole that was open.

- **Interrupting a download mid-write is now something the tests actually do.** A download
  manager's promise is that being interrupted costs time and not data, and until now nothing
  checked it: every test ran a transfer to the end, which is the one shape of run in which
  nothing can be lost. The instant that matters — bytes on disk that the database has not
  recorded yet — lasts microseconds and cannot be hit by timing, so it is named instead, and a
  test can stop the code exactly there and then assert what the next start makes of the result.

  Three points in the HTTP engine are covered so far: after writing bytes, after the file was
  synced but before the checkpoint was committed, and after the commit. Each case asserts the
  same four things, because they fail differently — that no byte is ever counted as confirmed
  without having been fetched, that nothing before a checkpoint is rewritten, that the resumed
  file is byte-for-byte what an uninterrupted download produces, and that nothing is left
  behind. `docs/recovery-matrix.md` records what is covered and, deliberately, what is not.

- **Upgrading an older installation is tested with a queue in it.** The migration chain was
  only ever exercised on an empty database, which is precisely the case where a migration
  cannot lose anything. A database is now built at the schema each past release shipped,
  seeded with a package, its download and a chunk checkpoint, and upgraded — and the bytes
  already on disk have to still be accounted for afterwards, because the alternative is
  silently downloading them again.

### Changed

- **The signature and trust primitives moved into a crate of their own.** Verifying an Ed25519
  signature over a length-prefixed digest, deciding whether a key is trusted, refusing a stale
  document and finding the compiled-in root to start from were implemented once, inside the
  plugin host — which links Wasmtime and every bundled resolver. Application updates, the managed
  external-tool manifest and plugin repositories all need the same four operations, and the
  updater in particular has to verify *before* the application is known to be healthy, so it
  cannot afford that dependency; the alternative, a second copy of a digest format that must stay
  byte-identical forever, is worse. The framing is unchanged and pinned by a byte-literal test,
  and every plugin package already published still verifies against the same key.

  Two things are genuinely new rather than moved. Trust roots are now a table with a role
  (application update, plugin, tool manifest, repository) and an optional expiry, so rotating a
  key is an entry with an overlap window instead of a build that stops trusting everything signed
  with the old one. And an artefact can be withdrawn by its digest without revoking the key that
  signed it — revoking the key would take down every other artefact the same author signed, which
  is a far larger blast radius than a single bad release warrants.

- **A log line about a plugin says which plugin it is.** Two places identified a plugin by its
  id and version alone — the line that reports a failed invocation, and the one that reports a
  package the host could not load. A UUID and a version number are the right thing to grep for
  and the wrong thing to read: nobody knows by heart which plugin `019d0000-…-000109` is, and
  the reader who needs the line is precisely the one who does not. Both now carry the plugin's
  name as well, in the same `plugin=` field the rest of the host already uses, with the id left
  in place beside it.

## [0.9.8] - 2026-09-06

### Added

- **A DDownload account only needs a username and a password.** Until now the only way to
  download from DDownload was to open the browser developer tools, copy the whole `Cookie:`
  request header out of them and paste it into a text field — and to do it again every time the
  session lapsed. rDownloader now signs in itself and keeps the session, the way JDownloader and
  pyLoad have always done it. An account says which of the two it holds: account credentials, or
  the API key from Account → API for anyone who would rather not store a password. Neither
  credential is ever visible to the plugin — both are substituted inside the application, and
  each is pinned to its own host, so the password only ever reaches the login form and the key
  only ever reaches the API. The session lives for as long as the process; after a restart the
  first download signs in again, which costs two requests and no attention.

- **The generic XFileSharing resolver claims 140 sites.** It has been able to drive the free
  flow for a while; what it lacked was a domain list, because a claimed domain is a promise and
  a resolver that claims one and then fails turns a plain HTTP download into a hoster error. The
  list is now derived rather than guessed: JDownloader models 315 of its hoster plugins as stock
  XFileSharing installations, naming 682 domains; 90 it has itself marked dead and nine already
  served by this application's own resolvers were removed, and each of the remaining 582 was
  asked whether it still serves the XFileSharing login form. 371 did not. What is left is 140
  sites and 211 domains, every one of them checked against the running service.

  What that does not prove is that the free flow completes on each of them, and the manifest says
  so with numbers: measured against JDownloader's own subclasses, 48 of the 140 deviate from the
  base flow in nothing, 75 in at most one thing, and the rest needed real per-site work there.
  They are claimed anyway, because a failure now carries a named cause — and a site nobody claims
  is a site nobody can report. Video and image hosts among them take nothing away from the media
  and gallery pipelines, which link intake asks first and whose host lists the user controls.

- **Every hit of one indexer subscription can be queued or dismissed at once.** A search that
  turned out to be right is ninety-eight decisions nobody wants to make one at a time. Both
  actions ask first and name the count and the subscription, because "queue everything" on a
  search that grabs too widely is precisely what the review step exists to prevent, and with
  ninety-eight hits the mistake only shows once the queue is full. Dismissing deletes nothing:
  the hit stays in the archive and is only never suggested again. There is deliberately no
  action across all subscriptions — one spanning several searches has nothing left to judge.

- **A provider can declare that it takes no account.** The provider model assumed one provider is
  one site: a manifest's `[provider]` row names exactly one cookie scope and one secret, which a
  resolver spanning several installations of the same hosting script can fill with nothing —
  an account at one clone is not an account at another. It would still have shown up in the
  accounts settings as an entry where entering anything achieves nothing. `credentials = "none"`
  registers such a provider for resolving and leaves it out of that list, and the manifest
  refuses one that describes an account anyway. Groundwork for the generic XFileSharing
  resolver; no shipped plugin uses it yet.

### Changed

- **A provider can offer more than one way to hold an account.** The provider registry gave each
  provider exactly one credential slot, which forced a choice between an API key and a password
  where a site supports both. A provider can now declare one slot per credential mode, each with
  its own hosts, and an account records which mode it uses. That separation is the point: the
  application refuses to send an API key to a login form, or a password to an API host, even
  though the same account field holds both. Written `credentials = "login_or_api_key"` with a
  `[[provider.secrets]]` table per mode; the previous singular spelling stays valid and is all a
  single-mode provider needs.

- **Indexer hits are grouped by the subscription that found them.** The LinkGrabber listed every
  hit awaiting a decision in one flat list with the subscription written small beneath the
  title. With one subscription that reads fine; with several, hits from different indexers and
  from searches with different intent interleaved by nothing but the order they were found in,
  and "98 pending" said nothing about which search was over-matching. Each subscription now has
  its own collapsible section carrying its name and its own count, sections with nothing left to
  decide are not shown, and deciding about one hit leaves the other sections exactly as they
  were.

### Fixed

- **The SponsorBlock enricher had never been loaded.** It shipped with the same plugin id as
  the file-name tidier, and since the loader keeps only the highest version per id — which was
  the other plugin — SponsorBlock was skipped on every start. No error, no message: the plugin
  list showed one id with two versions, which reads like a plugin that was updated. SponsorBlock
  moves to an id of its own, because nothing can be pinned to an identity that never loaded,
  whereas renumbering the tidier would hand a job pinned to the old id a plugin of an entirely
  different type. Installations that received both halves are cleaned up on the next start
  rather than left holding the leftover, and a test now fails if two bundled plugins ever share
  an id again.

- **A credential substituted into a form body is encoded for it.** Values put into a request on
  the plugin's behalf were escaped for JSON but not for `application/x-www-form-urlencoded`, so a
  password containing `&`, `=` or `%` did not merely arrive corrupted — it split into extra form
  fields the far end would then act on. Both body types are now encoded for what they are.

- **The local check script could not reach its own test step, and three things had broken
  behind it.** `scripts/check.sh` passed `cargo nextest` both `-j` and `--test-threads`, which
  are the same option, and nextest refuses the duplicate — so the documented core check had been
  stopping before it ran a single test. Behind that: the bundled-plugin count still said
  twenty-five after the generic XFileSharing resolver made it twenty-six, defeating the very
  assertion that exists to catch a plugin quietly dropping out of a release; and the download
  fixtures in four crates' tests had never been given the `mirror_group` field, so those test
  binaries no longer compiled. None of it was visible in a normal run, which is the point.

- **The guided tour is set in the application's own typeface again.** driver.js writes its own
  `font-family` onto the popover — `"Helvetica Neue", Inter, ui-sans-serif, …` — so the tour was
  the single surface that reached past the bundled IBM Plex Sans to whatever the machine happens
  to have installed. On most systems the substitute passes unnoticed, which is why this only ever
  showed on one: there a locally installed font in that list carried the OpenType features the
  body still asked for, and the alternates it selected rendered the tour text as a mixture of
  capital and lowercase forms. The popover now pins the application font, and the three
  Inter-specific `font-feature-settings` on `body` are gone — IBM Plex Sans defines none of
  `ss02`, `ss03` or `cv11`, so they were doing nothing here and could only ever have taken effect
  in a font the application did not choose.

## [0.9.7] - 2026-09-06

### Added

- **The container image can actually run a media library now.** It shipped no external helpers at
  all, so in Docker every media download, stream, gallery, ffmpeg remux, unpack and PAR2 repair
  failed silently — only the in-process transports worked. ffmpeg, yt-dlp, streamlink,
  gallery-dl, 7-Zip and par2 are now part of the runtime, at pinned versions visible under
  *Settings → External tools*, and a newer binary can still be dropped into `/config/vendor`.
  `unrar` stays out: it is non-free, and Debian's `unrar-free` cannot read RAR5, which 7-Zip can.
- **`PUID` and `PGID` are honoured.** The service ran as a fixed uid 10001, so a bind mount owned
  by the host user was unwritable — the single most common reason a NAS setup fails. The
  entrypoint now adopts the ids given to it and drops privileges with `gosu`, the convention
  people arriving from other download managers already expect. `/downloads` is deliberately not
  chowned recursively: on a NAS share that would run for minutes on every start.
- **A health check, and a time zone that can be set.** `docker ps` reports whether the service is
  actually answering, with a start period long enough for the plugin installation. `TZ` defaults
  to UTC and is documented, so schedules and quiet hours stop firing at the wrong time.
- **[`docker/README.md`](docker/README.md), including a Synology NAS walkthrough.** Volumes and
  why every storage root needs one, PUID/PGID, the bundled tools, building locally, and
  troubleshooting.
- **The service says when it has no bundled plugins.** `dist/plugins` is gitignored, so a local
  image build without `scripts/build-plugins.sh` produced an image with no hoster resolvers and
  nothing explaining why. It now logs that at startup. A missing directory — the ordinary case
  for a plain binary — stays quiet.

- **A storage root says when its path will not survive the container.** Configuring a root at a
  path that is not on a mounted volume created it inside the container's writable layer:
  downloads ran normally and everything was gone on the next `docker rm`. Nothing said so, and
  the wizard promised the opposite — that paths "are created if they do not exist yet", which is
  precisely what happened and precisely why the data was lost. The routing view now badges such a
  root and explains it once above the form, the readiness card flags the storage step, and the
  MCP configuration tool reports it too. The check is deliberately quiet: only a known overlay
  filesystem carrying `/` counts, so a bare-metal install with a btrfs or zfs root is never
  accused; `tmpfs` and `ramfs` count anywhere, because a download written there is gone after a
  reboot either way. The default `/downloads` path in the container image is a mounted volume, so
  a normal setup never sees the warning.

### Changed

- **Exactly one storage root is the default, always.** The flag was only ever cleared on other
  roots when an incoming one asked to be the default, so an install could end up with none at
  all — the first root created without ticking the switch, or the default being deleted. With no
  default, a download without a category went to whichever root sorted first alphabetically. Now
  the first root is the default whatever the form submitted, the last default cannot be given up
  (the switch says so instead of letting an edit quietly not take), deleting the default hands
  the flag to another root, and restoring a backup repairs a bundle that claims none or several
  instead of refusing it.

### Fixed

- **Deleting a storage root left it in the capacity supervisor.** Creating and updating a root
  reloaded the capacity configuration; deleting one did not, so the removed root stayed a limit
  target until something else triggered a reload.

- **An indexer's links are recognised even when the server does not say what they are.** The
  online check decided what a link was from the content type alone, and plenty of indexers hand
  out an NZB as `application/octet-stream` or `text/xml`. Those links stayed ordinary downloads,
  so the document itself landed in the download folder and nothing was ever fetched from Usenet.
  When the header says nothing useful, the first bytes of the document now decide — an NZB is
  XML with an `<nzb>` root, a torrent is bencode — and the decision is written to the log either
  way, so a link that is still not recognised says which content type it answered with.

- **An indexer's hit keeps the name its feed gave it.** Every hit of an indexer is fetched from
  the same `…/api` address, so the address can name none of them: two subscription hits arrived
  in the LinkGrabber as two packages both called `api.omgwtfnzbs.org`, each holding one link
  called `api`, and the Usenet job was named after the API endpoint too. The feed item's title
  now travels with the link, each container takes a package of its own instead of being grouped
  under the indexer's host, and where the source named nothing the import reads the
  `Content-Disposition` and `X-DNZB-Name` headers an indexer answers with — the ones SABnzbd
  established. An indexer refusing inside a `200 OK` — the daily API limit, an expired key —
  now says so in its own words instead of being reported as an invalid NZB. A name merely
  guessed from the address no longer counts as one the source declared, which also kept mirror
  detection from treating two unrelated files behind the same last path segment as one.

- **The login screen and the setup wizard say which version is running.** Both carry the same
  line as the transfer rail's footer, which neither of them can reach: the version, and who
  made this.

## [0.9.6] - 2026-09-06

What running 0.9.5 turned up, plus two things that had quietly fallen out of step with the
application.

### Fixed

- **An indexer subscription's archived hits stayed broken after the upgrade.** 0.9.5 taught the
  feed reader to decode `&amp;` and to keep the media type a feed declares, but items collected
  before that were already stored with the malformed address and no type, and a repeat poll left
  them exactly as they were. A poll now corrects an item nobody has decided on yet — its address
  and its declared type — so the hits already waiting for review repair themselves. Anything
  queued, skipped or dismissed is untouched: a feed re-listing an entry must never undo a
  decision.
- **Adding links produced notifications about something else entirely.** Pasting thirty links
  gave several toasts each claiming about five had been added. An NZB import emitted its own
  intake event carrying the number of files *inside* the NZB, and the online check re-routes
  every recognised NZB link to that import — so one paste fired one notification per container
  with a number that had nothing to do with the action. An NZB recognised behind a link no longer
  announces itself; a hotfolder drop or an upload still does, and says "NZB added, 5 files"
  instead of calling them links.
- **An unpaired capture agent reported itself as a failed start.** The launcher does start it,
  but it exits at once with no capture token — which is the ordinary state of a fresh install,
  since pairing happens in the web interface afterwards. All three launchers now say it is not
  paired yet instead of claiming a crash.
- **A mirror standing down no longer tells a post-processing script the download failed.**

### Changed

- **The schedule timezone defaults to Europe/Berlin.** Quiet hours, bandwidth windows and
  reconnect windows are wall-clock times, and UTC shifted every one of them by an hour for half
  the year on a machine whose owner never thought about time zones.
- **An automation's script is chosen from the scripts folder rather than typed.** A name that
  does not exist failed only when the automation ran, which is the kind of failure nobody
  notices.

### Added

- **The system settings say whether the setup is finished.** The four things the wizard makes
  mandatory — the login, a storage destination, a category, and the setup having been completed —
  each shown as settled or outstanding, with a way to the place that fixes it. Deliberately not
  the steps the wizard lets you skip: listing a choice as missing would turn a correctly set up
  system into a page of warnings.
- The app tour describes the application as it is now: five sidebar entries rather than three,
  every container format rather than two, settings as sub-pages rather than tabs, and a word
  about mirrors — the one thing in the LinkGrabber that looks wrong until you know it is not.

## [0.9.5] - 2026-09-06

### Fixed

- **Mirror recognition could group two files that were not the same.** A link whose source names
  no file gets the last segment of its address instead, and two unrelated links behind `/download`
  therefore shared a name. They were grouped, one was held back, and because a held-back mirror
  counts as settled the package announced itself complete and unpacking ran over an incomplete
  set. Links now carry whether their name came from the source at all, and only a name the source
  gave — or one the online check learned from the server — is evidence about the file.
- **Sizes are compared, not rounded.** The size was folded into a key by rounding to whole
  megabytes, so two figures a few bytes apart could land on opposite sides of a boundary while a
  known size never matched an unknown one at all — the common case, since many hosters announce
  none. Links are now compared with each other: within a percent is the same file, an unknown
  size matches, and one name holding two clearly different sizes becomes two groups.
- **A waiting mirror was stranded when the active link was cancelled or removed.** Only running
  out of retries promoted one, so cancelling left its alternatives waiting for a link that was
  never coming back and the package sat there doing nothing.
- **Post-processing could miss a package that a mirror completed.** A mirror standing down can be
  the last thing that settles a package, and that transition did not wake the check.
- **A waiting mirror can be started by hand.** It was offered nowhere in the interface, and the
  scheduler put it straight back. Choosing one now stands its siblings down; if another link is
  already downloading the attempt is refused with a reason rather than silently reverted.
- Finished packages holding a mirror are no longer kept back as though they had a failure, a
  package's progress no longer counts bytes that arrive through another link, and a user script
  is no longer told the download failed when a mirror simply stood down.
- **An indexer's links were fetched as files instead of being imported.** Two things went wrong at
  once. A feed writes `&` as `&amp;`, and attribute values were read without decoding it, so every
  Newznab download address arrived carrying a parameter called `amp;id` — the indexer did not
  recognise the request and answered with a short error body, which is where the "response ended
  at byte 109" came from. And the feed states outright what it is handing over, in the enclosure's
  type, which was thrown away and then guessed at again with a HEAD request; an indexer that
  answers one without a content type left the link to be downloaded as an ordinary document.
  Both are fixed: the type is kept and decides how the link is routed, so nothing has to be
  probed. This also explains the capability test failing with "your previous 2 attempts failed":
  those attempts were the malformed downloads, and the indexer locks out for 300 seconds after
  two of them.

### Changed

- **Subscription title filters can be regular expressions.** A pattern wrapped in slashes —
  `/^S0\d/` — is an expression; anything else is matched as plain text exactly as before, and
  the regex editor from the routing rules is available on both fields. The distinction is
  explicit on purpose: most substrings people already wrote are also valid expressions with a
  different meaning, so inferring it would have quietly rewritten existing subscriptions.

## [0.9.4] - 2026-09-05

Features borrowed from pyLoad, which covers the same ground and has had twenty years to find
out what an unattended download manager needs.

### Added

- **Finished packages can be removed by themselves.** A machine that downloads unattended fills
  its queue with packages nobody is going to look at again, and the "clear completed" button
  only ever helped somebody who was already looking. Switch it on and a package leaves the queue
  once it has been finished for a set number of hours. A package still being unpacked, a torrent
  still seeding, and — unless you say otherwise — a package holding a file that did not finish
  are all left where they are, because each of those is either still working or worth a look.
  Packages now record when they finished rather than borrowing the time of their last write, so
  renaming one or changing its priority no longer restarts the clock.
- **The machine can be kept awake while it works.** A download that finishes at three in the
  morning is no use if the machine went to sleep at one. Switch it on and standby is held off
  while a download, a repair or an unpack is actually running — deliberately not while the queue
  is merely waiting, since a queue held back by a bandwidth schedule or an IP block is not a
  reason to keep a machine running for hours. The display is a separate switch, because a
  download needs no lit screen but somebody watching progress does. Held through a helper
  process on every platform, so the machine is released even if the service is killed outright
  rather than shut down.
- **Three more container formats: RSDF, CCF and plain link lists.** RSDF is decrypted on the
  machine itself — the key is published, so nothing leaves the building — and CCF goes through
  the same service a DLC does. A `.txt` file is read as one link per line, with `[A Name]` on a
  line of its own opening a package and `;` or `#` starting a comment, which makes a hand-written
  list a container like any other. All of them, DLC included, now go through one endpoint rather
  than one per format; the old DLC address still answers so nothing that used it has to change.
  Watched folders pick up `.ccf` and `.rsdf` too, but deliberately not `.txt`: a watched folder
  is somewhere people also keep notes, and a README is not worth collecting and filing away.
- **Reconnecting to get a new address.** Hosters that limit free downloads do so per address,
  so the only way past is a different one. What that takes is specific to the router, so this
  runs a script you write, in the same sandbox post-processing scripts already run in. It only
  acts while free downloads are actually waiting on such a limit — never on a premium download,
  which is not subject to it — keeps its distance from the previous attempt, and by default
  waits until nothing is transferring rather than interrupting. Afterwards the address limits
  are forgotten and the waiting downloads are queued again. Off by default: the address checks
  it makes to confirm the change are the one part of the feature that talks to somebody outside.
- **Mirrors of the same file are downloaded once.** A release posted to three hosters used to be
  fetched three times, spending the bandwidth and the free-download slot of each to end up with
  the same bytes. Links in a package that point at the same file are now recognised as
  alternatives: one downloads and the rest wait as its fallback, shown as "Mirror" rather than
  as an error. If the one that ran gives up for good, another takes over — which is the point of
  keeping them. A link with an account wins the turn, because it is not subject to the
  free-download limits the others would queue behind. Volumes of one archive are not mirrors of
  each other and neither are the files of a torrent or an NZB, so none of those are grouped. The
  recognition can be switched off.

### Fixed

- **A DLC container could never have been decrypted.** The initialisation vector the decryption
  service's answer is unwrapped with had two characters transposed, and that answer is the
  container key, so every import would have failed once it got as far as the service. Until
  0.9.3 it never did, because the import dialogue dropped the file before any request was made.
  No test could catch it: they all built their fixtures from the same constants they verified.

## [0.9.3] - 2026-09-05

A correction release for what running 0.9.2 turned up. Most of it is one kind of defect: a
feature that looked as though it did nothing at all. A container that was never sent, a review
action that never reached the LinkGrabber, a form that could not be saved, dropdowns that
discarded what was picked, and a sign-in that named the wrong reason for failing.

### Fixed

- **A `.dlc` container now actually reaches the LinkGrabber.** The import dialogue filtered the
  files it had just been handed against a list that knew only `.nzb` and `.torrent`, and dropped
  everything else without a word. The file picker offered `.dlc`, the window drop zone accepted
  it, and then the list stayed empty and the submit button stayed dead — so no request was ever
  made and nothing appeared in the log either. The dialogue now uses the same list as the drop
  zone rather than a second copy of it, and the two failing paths on the server say so in the log
  instead of only in a response nobody was there to read.
- **Queueing a reviewed subscription item hands it to the LinkGrabber.** Review is the form's
  default, so this is the ordinary way an indexer hit becomes a download. Marking the row queued
  only moved it in the subscription's own table; the intake was never called, and the LinkGrabber
  stayed empty however often the button was pressed. Automatic polling was unaffected, which is
  why feeds in auto-queue mode appeared to work while everything in review mode did not. Both
  ways an item can be accepted now go through one function.
- **An automation with a webhook or category action can be saved.** Switching the action kind
  seeded the id it needs with an empty string, and the server — which takes a UUID and nothing
  else — refused the whole request with a parser message rather than one of the translated
  codes. Ids are now seeded with something real, and the save button reports an action that is
  still missing one instead of letting the request fail.
- **The webhook action has a field for its target.** It references a notification target, and
  the editor rendered inputs for every other action kind but not that one, so the action could be
  chosen and never completed. It now offers the configured targets, and says so plainly when none
  exists yet.
- **Dropdowns in the automation editor keep what is picked.** The category select and the dry-run
  package select passed whole objects where the component reports an id, so every choice was read
  as undefined and silently discarded. The dry-run consequently always ran without a package. The
  type casts that had been hiding the mismatch from the compiler are gone with them.
- **The regex tool button sits next to its field again.** It was wrapped in a component that no
  longer exists under that name in the interface toolkit, so the wrapper rendered as an unknown
  element and the button wrapped onto the next line.
- **A failed provider sign-in says what actually failed.** Every error from starting a flow was
  reported as "no installed plugin can sign this provider in", including the ones where a plugin
  was installed, claimed the provider and simply did not get through. The two are told apart now,
  and the plugin's own reason is carried into the message instead of being discarded.
- **Credential fields are named after what the provider wants.** The prompt said "API key" even
  while editing an account for a hoster that takes a password, because the wording was fixed
  rather than read from the provider. It now follows what the registry records, and a provider
  that ships its own wording keeps it.

### Changed

- **Device pairing and MCP access are their own settings entries.** Both were appended to the
  bottom of the system page, which is a status overview rather than a place to configure
  anything.
- **The backlog hint says what the default does on a first poll.** "Only new items from now on"
  finds nothing when every entry a feed returns is older than the subscription, which reads as a
  broken indexer rather than as the policy working.
- Entries an indexer returns without any download address, and results beyond what one poll
  takes, are now logged rather than dropped in silence.

## [0.9.2] - 2026-09-05

The interface half of what running 0.9.0 turned up: the reorganisation and the settings that
were asked for, rather than defects. Nothing here changes how a download behaves.

### Added
- **The settings sections are pages of their own.** Fourteen tabs no longer fit a tab strip —
  the titles were unreadable and choosing a section meant scanning a row of truncated labels.
  Each section now has its own address (`/settings/usenet`) and appears as an expandable group
  in the sidebar, where the rest of the navigation already is. Only the section on screen is
  mounted; previously all fourteen stayed in the DOM with their own requests and timers running,
  so the bandwidth card kept polling every ten seconds while an entirely different section was
  open. `/settings` and the old `?tab=` links redirect, so bookmarks and the captcha prompt's
  link keep working.

- **Time zones are picked from a list.** The stream schedules and the bandwidth schedule both
  took the zone as free text, so a typo — or simply not knowing the exact IANA spelling —
  produced a schedule that silently ran in the wrong zone. Both now use one searchable picker
  fed by the browser's own zone list. A zone the browser does not list is kept as an option when
  a schedule already carries it, so opening a schedule saved elsewhere neither blanks the field
  nor quietly changes its zone.

- **Sizes can be shown in binary or decimal units.** Byte counts were fixed to the IEC ladder
  (KiB/MiB/GiB, 1024); drive manufacturers and most other download tools print the SI ladder, so
  the same file appeared to be a different size depending on where it was looked at. The choice
  lives in Settings → Interface and applies to every figure at once, without a reload. Binary
  stays the default: changing it would make every size in every installation change overnight.

- **Installed plugins can be deleted and switched off.** Deleting was only offered for packages
  this build refuses, even though the endpoint always accepted any installed version; every
  plugin now offers it behind a confirmation. Switching one off is new — it stays installed and
  listed, so it can be switched back on, but it is no longer loaded, compiled or executed, and
  its provider is no longer offered when creating an account. As with installing a plugin, it
  takes full effect on the next start.

- **A post-processing step can tidy the file names.** The contract was read-only, so this could
  not be expressed as a plugin at all. `source` gains a rename that stays a handle and a name
  rather than a path — a name carrying a separator or a `..`, or one already taken, is refused
  by the host. The new bundled plugin replaces spaces with dots and collapses runs of
  separators, leaving extensions and hidden files' leading dots alone. Bracket-tag stripping and
  lowercasing are available but off, because those tags are often what identifies a release.

- **The tray says whether the server is up.** Right after login the agent and the service start
  together and the service takes longer, so "Open rDownloader" landed on a browser error page
  with nothing to say whether it was starting or simply not there. The status line now ends in
  "server starting", "server running" or "server not reachable", and the entry stays greyed out
  until the service answers. The check uses the unauthenticated health endpoint, so it works
  whether or not the agent is paired.

- **Undecided indexer hits appear under the LinkGrabber list.** A subscription in review mode
  collects matches and does nothing with them, and the only place to act on one was the
  subscriptions page — not where links are worked through. They now sit below the candidates
  with queue and dismiss, loaded when the section is opened rather than on every visit.

### Changed
- **The bulk pulldowns apply on selection.** Category, priority and level each had a confirm
  button beside them; that is three controls and one extra click for what the pulldown already
  said. An action that cannot apply to the current selection is now disabled at the pulldown
  rather than looking available until it is clicked.

- **The plugin list is grouped by type.** Twenty-five plugins of eight kinds in one flat grid
  made finding a particular one a scan. They are behind tabs built from the types actually
  installed, with a count each.

### Fixed
- **Most plugin type badges showed their translation key.** Only `resolver` and `transfer` were
  translated, so the other six — auth, enricher, intake, notifier, post-processing and storage —
  rendered raw keys on 13 of the bundled plugins.

## [0.9.1] - 2026-09-05

A correction release for what the 0.9.0 build turned up in daily use. Nothing here is new
capability; every item is something that was reported as broken, silently doing nothing, or
getting slower the longer the application stayed open.

### Fixed
- **The web interface no longer runs itself out of browser connections.** Symptoms that looked
  unrelated — the interface growing sluggish, dozens of GET requests sitting unanswered, and
  finally the settings refusing to save until Firefox was restarted — were one cause. Each of
  the three Pinia stores opened its own `EventSource` to `/api/v1/events`, and browsers allow
  roughly six connections per host over HTTP/1.1. Three permanently parked streams left three
  slots for everything else, so switching routes quickly filled them with pending requests and a
  `PUT /api/v1/settings` never got a turn. The stores now share one stream. The same change
  fixes a second defect underneath it: the transfers store had no "enabled" flag and never
  cleared a pending retry, so a reconnect timer that fired after disconnect opened a fourth,
  ownerless stream. Two further sources of avoidable traffic are gone with it — the LinkGrabber
  no longer refetches the collector when the route is entered (the application already does this
  once and the event stream keeps it current), and the transfers safety-net poll is dropped
  while the post-processing poll drops from three to fifteen seconds.

- **Pages can be scrolled to the bottom again.** Every route lost its last 44 pixels, which is
  exactly the height of the transfer rail at the bottom — hence the impression that the footer
  was at fault. It was not: the rule that cancels Nuxt UI's full-viewport minimum height on the
  dashboard panel used a child combinator, and a `<main>` element had since been introduced
  between the two, so it matched nothing. The panel then claimed a full viewport inside a
  clipping column. In the settings this swallowed the save bar entirely.

- **Dropdowns that opened empty.** Reka UI throws when a select item carries an empty string as
  its value, and a render-phase throw destroys the whole popup rather than the one row, so the
  hoster filter, the torrent proxy and bind-interface selects and the subtitle format picker
  opened blank while logging `A <SelectItem /> must have a value prop that is not an empty
  string`. They now use the sentinel this codebase already had for optional selections. Three
  further selects passed `null`, which survives the check but does not round-trip, and were
  corrected the same way.

- **The automation editor's trigger, condition and action lists.** `loadVocabulary` was the only
  action in its store without an error branch, so a failed catalogue request left every list
  empty with no message anywhere — the view's alert only renders from the error field. The
  failure is now reported, and a later attempt retries instead of being turned away by the
  "already loaded" guard. All five selects also passed the whole option object where the
  component expected the key, which left the chosen entry blank even when the list had loaded.

- **Twelve buttons showed their translation key.** The automation editor, the stream schedules
  and the Newznab subscription list asked for `common.edit`, `common.delete`, `common.save` and
  `common.cancel`; the real keys live under `common.actions.*`. The locale test compared the
  four catalogues only against each other, so a key that none of them had was invisible to it —
  it now also resolves every literal key used in a view or component.

- **Auto-queue subscriptions actually download now.** An indexer subscription in AutoQueue mode
  marked its items "queued" and stopped there. That state is subscription bookkeeping, not a
  queue state: the links are handed to the ordinary LinkGrabber intake, deliberately, so routing
  rules and the online check apply — but nothing ever promoted them afterwards, and they stayed
  in the LinkGrabber for good. Finishing the online check of such a batch now enqueues its
  packages. The subscription's current mode is read each time, so switching one back to review
  takes effect immediately, and a single unusable package no longer abandons the rest.

- **"Plugin execution failed: time limit exceeded" when several downloads start at once.** The
  plugin execution deadline is wall-clock and fixed when the sandbox store is created, and only
  a guest-requested hoster countdown or the transfer sink ever pushed it forward. A resolver's
  own HTTP call credited nothing, so waiting for the hoster was charged against the same
  fifteen-second budget as computation — and since the HTTP timeout is itself fifteen seconds, a
  single slow response was certain to end the call as a plugin timeout. Ten simultaneous
  downloads made that routine, which is why starting them again worked. The waiting time is now
  credited back, as the transfer path already did, and a timeout is treated as temporary so the
  scheduler retries it rather than the user restarting the download by hand.

- **Moving a finished package to another category takes the whole folder.** Only the file each
  download row names was moved, and only from the top level. Extraction output has no download
  row at all, and archive parts routinely sit in a subfolder that was never descended into, so
  both stayed behind while the package record already pointed at the new location — and the
  cleanup afterwards only removes an empty directory, making the remains permanent. Everything
  left is now carried over, out of the package's own folder only: the download directory and the
  category directory are shared with other packages and are left alone.

- **Setting a category for NZBs in the LinkGrabber.** The bulk bar's category and priority
  actions read only the collector half of the selection, so selecting NZB imports greyed the
  controls out and a mixed selection skipped the NZBs without saying so — while remove and
  enqueue beside them already handled both. The post-processing level is genuinely package-only
  and is now disabled with an explanation rather than quietly ignored.

- **Taking links out of the LinkGrabber is fast again, and the rows disappear at once.**
  Splitting partially selected packages and persisting the display order each awaited one
  request per package in sequence, so ten packages cost ten sequential round trips before the
  enqueue began; both now run together. The rows were also only removed after all three lists
  had been re-read, and the caller re-read them again straight afterwards. They are removed
  locally now, and the selection is cleared after the refresh rather than before it, so the
  action bar no longer vanishes while the rows it acted on are still on screen.

- **Repeated desktop notifications.** Refreshes carried no ordering guard, so an older response
  arriving last rewound the remembered states and the same transitions were announced a second
  time — including after clearing the list. Refreshes now carry a sequence number, and a
  scheduled refresh waits for one in flight instead of stacking on top of it.

- **Every settings tab is reachable by link again.** `?tab=bandwidth` and `?tab=notifications`
  were rendered but missing from the accepted list, so those links silently landed on the system
  tab. A test now holds the two lists together.

### Added
- **A desktop notification when links arrive in the LinkGrabber.** The server already announced
  every intake — from the web interface, the browser extension, the hotfolder and subscriptions
  alike — but only the separate capture agent listened, and that agent has to be installed,
  paired and running. The web interface now raises the notification itself, using the opt-in
  under Settings → Interface.

- **Keyboard shortcuts for subscriptions and automation.** The sidebar has six entries and only
  four had a shortcut, with `4` skipping past two routes to the settings. The digits now follow
  the sidebar: 4 subscriptions, 5 automation, 6 settings.

### Changed
- **Both list views group their items in one pass.** The download queue and the LinkGrabber each
  filtered the whole item list once per package, which is quadratic and re-ran on every refresh.

## [0.9.0] - 2026-09-05

Milestone 0.9 — automation, ecosystem compatibility and the remaining plugin extension types:
download-client compatibility for Sonarr and its siblings, an automation engine with a visual
editor, a command-line client, an installable web app, and all six plugin extension types
contracted, hosted and wired up with twenty-four bundled plugins.

### Added
- **A token that can only watch.** Creating an API token now offers a read-only variant carrying `api:read` instead of `api:*`. It reads the download list and summary, packages and their post-processing, the post-processing queue, storage capacity and the media tool status, and it subscribes to `/api/v1/events` reduced to queue and progress events. Everything else answers `403 auth.scope_insufficient` — intake, queue control, categories, accounts, proxies, auth profiles, the settings document and the MCP endpoint. The reachable set is an explicit allowlist rather than "every GET", because a status page is exactly the place a token gets pasted into, and enumerating the configuration of an installation is not what pasting it there was meant to allow. A full `api:*` token satisfies the read scope as well, so a client that both acts and observes still needs one token; the implication never runs the other way, and a capture token remains an intake credential that reaches neither. Scopes are fixed when a token is issued — there is no endpoint that widens an existing secret.

- **rDownloader can be a download client for Sonarr, Radarr, Lidarr and Readarr.** A SABnzbd-compatible API subset answers at `/api` and `/sabnzbd/api`, authenticated with the `api:*` token you already create for machine clients — paste it as the API key. It covers what those tools actually call: version and auth probes, the completed directory and category list, the queue with per-job and bulk delete, pause and resume, the history with the storage path an import needs, and NZB upload through `addfile`. Everything underneath is the native path — the same NZB parser, the same `release{{password}}.nzb` convention, the same cancellation and part-file cleanup on removal — so a job added from Sonarr behaves exactly like one added from the web interface, and the native REST contract is not bent to fit. Job ids are derived from package ids and therefore survive a restart, which is what lets a client keep polling the download it started yesterday. Two departures are deliberate: `addurl` is refused with a message naming `addfile`, because fetching a caller-supplied URL server-side would make an API key a way to reach whatever the service host can reach; and `del_files=1` removes the queue entry without deleting finished files. Unknown modes answer in SABnzbd's own failure shape rather than an HTTP error, which such clients would misread as the server being down.

- **And a torrent client for the same tools.** A qBittorrent Web API v2 subset answers at `/api/v2`, following qBittorrent's login shape — post to `/api/v2/auth/login`, get a `SID` cookie — with the same API token as the credential: any username, the token as the password. The cookie carries the token rather than a session id, so a restart does not log an automation client out and revoking the token takes effect at once. It covers listing and properties, the file list and its selection, adding by torrent upload or magnet, pause, resume, delete and categories. Torrents are addressed by their real info hash, not a derived id, because the client computed that hash itself from what it handed over — and while a magnet's metadata is still resolving the hash is read out of the magnet, so the torrent is findable the moment it is added rather than minutes later. Only the 40-character v1 form is accepted there; a base32 or v2 hash is left alone rather than half-converted, since a wrong hash would leave a client waiting on a torrent that can never match. As with the SABnzbd adapter, `deleteFiles=true` does not delete finished files, and only magnets are accepted in `urls`.

- **The published compatibility matrix, and the sequences behind it.** `docs/compatibility.md` lists every supported mode and endpoint, the four deliberate departures and the limits a real run would meet first — progress figures are a snapshot rather than an estimate, and a package's category is not reported back. The call sequences Sonarr, Radarr, Lidarr and Readarr issue are replayed as tests, from the connection probe through configuration, adding a release, polling, inspecting and removing it. Writing those sequences found two bugs that every hand-written request had hidden: the torrent actions read `hashes` only from the query string, while real clients send them in the form body, which made `delete` silently do nothing for them; and a freshly added magnet was invisible until its metadata resolved, which is exactly the window in which a client looks for it. Both are fixed. What the suite proves is that the contract holds end to end — not that a given version of Sonarr is satisfied, because no such instance runs in it, and the matrix says so where it would otherwise be read as a promise.

- **Automations: when this happens, do that.** rDownloader already reacted to its own events in fixed ways — routing rules on intake, post-processing after a download, retries on failure — but none of it was yours to change. There is now an engine for it. An automation listens for one of twelve moments (links arrived, a download resolved, started, finished or failed, a package finished or failed, an unpack, script or upload step finished, a storage root crossed its threshold, a subscription accepted an item), checks a condition, and runs up to ten actions: call a configured webhook, run a script from the post-processing scripts directory, move the package to a category, pause it or resume it.

  Conditions use the vocabulary the routing rules already use — case-insensitive comparison, a criterion that is not set does not narrow anything — with `all`, `any` and `not` added on top. A rule that cannot be evaluated is refused while it is being written rather than at trigger time: an invalid regular expression, a size compared with `contains`, an empty group that would silently mean "always" or "never", a script name that is a path. The alternative is a rule that never fires and gives no reason why.

  Editing an automation writes a new version rather than rewriting the old one, and a run holds the version it started under, so changing a rule never changes what an in-flight run is being judged by. The same event starts the same automation at most once, keyed on the version rather than the automation — an edited definition is entitled to act on an event the previous one already handled. A run interrupted by a restart goes back into the queue instead of disappearing. Failures back off exactly as notification deliveries do and end as a recorded failure rather than an endless retry.

  Delivery is **at-least-once, deliberately**: the idempotency is on the run, not on the individual action, so a webhook that is delivered but fails while answering will be tried again. Nothing here promises exactly-once, because an HTTP call cannot.

  The engine is a subscriber to the event bus rather than a hook inside the queue, which is what makes it safe to let people write their own: an automation that fails, loops or blocks cannot affect the download that triggered it. Actions cannot name a capability of their own either — a webhook names a configured target, never a URL; a script names a file inside the scripts directory, never a path or a command line — so there is no way to write an automation that reaches a credential or a file the application did not already hand it.

- **Intake parsers as plugins.** A signed plugin can now turn text the built-in scanner does not understand — a site's own list format, a container file, an address shape — into LinkGrabber candidates. It is asked whether it claims the input before it is shown anything, so a parser for one format is not handed every paste in the application, and what it returns are proposals: they go through the same review, the same domain blocklist and the same routing rules a pasted link does. A parser cannot queue anything, and it never supplies request metadata, which would be a credential path it has no claim to.

  Such a plugin can also normalise a URL — strip tracking parameters, canonicalise a path — but a rewrite that is not a URL, or that changes the host or the scheme, is discarded. A normalizer tidies an address; it does not redirect one.

  A parser that fails costs only its own feature: the failure is logged, the native scanner carries on, and the service still starts. `rdownloader plugin new --type intake` scaffolds one.

  Two are bundled, and both read a real format the core does not know. **Metalink** (RFC 5854 `.meta4` and the older Metalink 3.0) proposes the files a document lists, with their names and sizes — only the first mirror of each, because the LinkGrabber is a list of things to download, not of ways to download them. **JDownloader `.crawljob`** reads what browser extensions and automation tools drop into a folder, and turns `packageName` into a grouping hint. It deliberately reads only `text`, `packageName` and `filename`: a crawljob is written for another application and may also say where to download to and what to run afterwards, and a file dropped into a folder must not be able to decide where this installation writes.

- **The kernel the five remaining plugin types stand on.** Six extension worlds were declared earlier in this milestone but only intake could actually do anything: an extension invocation ran with no host and an empty domain list, so a notifier, an enricher, an auth provider or a storage destination could be installed and would then find it had no way to reach the service it exists to talk to. They now reach the network exactly as a resolver does — through the same host, narrowed to their own manifest by the same code, with `{{secret:…}}` expanded on the way out and never handed back — and an account's secret is reachable only from an invocation started for that account.

  Authentication plugins additionally get a way to write back what a flow produced: `credentials.store-token` takes an account and a value and nothing else. The plugin names no vault reference, because which one an account's provider owns is the host's to know; there is no call that reads a value back; and the interface is linked into no other world, so no other plugin type can name it. A flow can therefore only ever add a credential it already held in its own hands, for the one account it was started for.

  `rdownloader plugin new --type` now scaffolds all eight types rather than three. Each scaffold compiles, packages and passes conformance before a line of it is changed, and carries the promises the host makes to its type at the top of `src/lib.rs` — they are what shapes the code.

- **Notifications can go somewhere the application does not know about.** A notification target can now be a signed plugin. It delivers one message and reports how it went; retries, backoff and quiet hours stay with the delivery hub, exactly as they do for the built-in webhook, SMTP and Apprise targets — a destination that could set its own retry policy would be a way for one plugin to keep the queue busy for everyone else.

  The token never reaches the plugin. It writes a reference-less `{{secret}}` marker into a header, a query value, the body or even the address, and the host substitutes the one secret that delivery was granted on the way out; a marker written without a grant is refused rather than sent empty. A target naming a plugin that is not installed is refused when it is saved, not when the first event arrives.

  Three destinations are bundled, one service each so they can be updated, versioned and switched off separately — and between them they cover the three shapes a token takes. **ntfy** takes a topic and an optional bearer token. **Discord** posts to a webhook whose token is part of the address itself, which is why the marker may appear in a URL: the host expands it after the domain check and verifies afterwards that the host has not changed. **Telegram** needs two values rather than one — a bot token in the vault and a chat id in plain sight.

- **Post-processing can be extended by a plugin.** A signed plugin contributes one more step to the pipeline. It runs after cleanup — so it sees the package as it will finally be — and before a user script, which stays the last word. Steps are switched on globally or per category and the list is ordered, so "first this, then that" is expressible; a category's empty list is not "inherit" but "none here", which is how a category switches a globally enabled step off. Installing a plugin enables nothing on its own.

  A step is handed a package handle and the names of the files it may read, never a path. Stopping is an ordinary outcome: it returns a checkpoint the core stores, and a service restart resumes the step rather than running it from the beginning. So is "nothing to do" — reported as skipped, because a step that failed every package without work for it would be useless in a category it was switched on for. A step only runs for a package whose repair, verification and unpacking all succeeded; verifying checksums over a half-unpacked package would report a mismatch that says nothing.

  Two are bundled: **SHA-256** and **MD5** sidecar verification, reading the layout `sha256sum` and `md5sum` write. Two plugins rather than one, so each can be updated and switched off separately — with both enabled the package is read twice, which is the price of that.

- **Finished packages can be uploaded by a plugin, and the destination has to prove it kept them.** Set the post-processing upload target to `plugin:<plugin-id>/<destination>` and the package goes through an installed destination instead of rclone; everything after the first slash is that plugin's own vocabulary and the core does not interpret it.

  What makes this worth having as its own plugin type is one call: after uploading a file, the destination is asked *separately* whether it really holds it, and only a yes allows the local copy to be removed under `move`. A server that answers `201` and stores nothing would otherwise take the only copy with it — a gap `rclone move` still has, and one this deliberately does not close for the rclone path, because rclone cannot answer the question.

  Two grants are narrower than they look. A storage manifest declares a wildcard domain, since where somebody put their server is not knowable in advance; what actually applies is the host of the destination that upload is for. And a storage plugin is the only type allowed the HTTP methods that write — `PUT`, `MKCOL`, `PROPFIND`, `DELETE` — because uploading is what it does; everything else stays on `GET`, `POST` and `HEAD`. Credentials come from the stored remote logins the FTP, SFTP and WebDAV transports already use, matched on host and port: the plugin gets the user name, never the password.

  **WebDAV** is bundled — a folder per package, a `PROPFIND` per file, and nothing deleted locally until that answers.

- **Links can be told more about themselves.** An enricher plugin adds fields to a link that has just resolved — shown beside the file name and size, with the plugin that supplied them and when it looked them up, because a value the application did not work out itself has to be recognisable as somebody else's answer.

  It adds and never replaces, and that is enforced rather than trusted: a field whose name collides with something the core resolved — `file_name`, `size`, `provider`, `media` — is dropped and counted. A plugin cannot rewrite what the online check found by naming a field after it.

  Enrichment is off by default and the switch is checked before an enricher is even compiled. Such a plugin reaches a service outside this machine, and doing that for every media link on the strength of somebody having installed one would be a decision nobody made. What a plugin claims narrows it again: one that claims YouTube is never told about anything else, so addresses it could not answer for do not reach that service at all.

  **SponsorBlock** is bundled: how much of a video is sponsor, self-promotion or intro, before the download starts rather than after. It sends a video id and nothing else — no account, no cookies — and refuses to ask about anything that is not an eleven-character id.

- **Signing in to a provider without pasting an API key.** For a provider with an installed sign-in plugin, an account now offers "Connect": the interface shows an address and a code, you confirm it at the provider, and the credential arrives here on its own.

  What makes this safe to hand to a third party is that the plugin never sees a credential and never names one. `credentials.store-token` takes an account and a value; which vault reference that account's provider owns is the host's to look up, the new value is written before the old one is dropped, and there is no call that reads one back. The interface is linked into no other plugin world, and the account is compared with the one the invocation was started for.

  The real risk of this plugin type is the opposite direction: its whole purpose is to put an address in front of somebody and ask them to sign in there. A verification address must therefore lie on a domain the plugin's own manifest declares — the same list shown before it was installed — and it is displayed, never opened for you. A plugin that could name any address would be a phishing page with a signature on it.

  A flow lives in the database rather than in memory, and the waiting between polls belongs to the service, so closing the browser or restarting mid-sign-in loses nothing: it is picked up where it stands. Whatever the plugin needs for the next poll travels back through the contract as opaque state the host stores and never shows.

  Three are bundled, one provider each and two flow shapes between them — **Debrid-Link** and **Premiumize** as OAuth device flows, **AllDebrid** as a PIN flow. A contract that fitted only one of those shapes would have been a contract for OAuth rather than for signing in.

- **`rdownloader://` links.** The capture agent can register as the handler for the scheme, so a page or a script can hand links over with one click: `rdownloader://add?url=…` for links and magnets, `rdownloader://open?path=…` for a local `.nzb` or `.torrent`. Everything handed over lands in the same LinkGrabber review the clipboard and the file association use, so this is a second door into the existing intake rather than a new one.

  The parser is deliberately an allowlist rather than a translator, because a scheme handler is reachable from any web page: anything can put such a link on a site and have the browser hand it to this binary. Only two actions exist, handed-over links may only use `http(s)`, `ftp(s)`, `sftp` or `magnet` — no `file:`, which would turn the handler into a way for a page to make the agent read a local path — and `open` takes absolute paths to `.nzb` or `.torrent` only, with no `..`, capped at 50 links and 8192 characters.

  Windows and Linux register a handler per user. macOS does not: a URL scheme is claimed there through `CFBundleURLTypes` in an application bundle, which a bare executable cannot do, so `scheme install` refuses with that reason instead of succeeding and then never receiving a link.

- **The web interface installs as an app, and mobile browsers can share into it.** A manifest and icons make it installable with its own window and home-screen entry, and sharing a link from a mobile browser opens the LinkGrabber with that link already handed over. The share target is declared as a GET target on purpose: a POST target would require the server to accept a form post from an unauthenticated context and turn it into intake, which is a second and weaker way in. The shared link is consumed once and cleared from the address, so a reload does not hand it over again, and a share carrying no link at all is ignored instead of producing an empty batch and an error nobody can act on.

  The service worker is hand-written and small, because what matters is not offline capability — a download manager without its server is not much use — but that installing the app never makes it show something untrue. The API, the event stream and the compatibility adapters are never cached; navigation is network-first with the stored shell only as a fallback when the network actually failed.

  The SPA fallback also stopped covering for typos: `manifest.webmanifest`, `sw.js`, `icons/` and `assets/` now have to resolve to a real file. Previously a mistyped path there was answered with the HTML page and a 200, which would make a failed install look like a successful one.

- **The binary can now drive a running service.** `rdownloader queue` lists, adds to and controls the download queue; `rdownloader links` hands links to the LinkGrabber, lists what is waiting for review, queues a package or discards it. Both work against the local service by default and against any other with `--server`, authenticating with the API token you already create for machine clients — a read-only token is enough for the reading commands.

  There is deliberately only one code path: even locally the CLI speaks HTTP to the REST API rather than opening the database. A running service already holds that file, and a second implementation of every command would have its own bugs and its own answer to what the server currently thinks. Output is a table for people and the server's unchanged JSON for scripts, so `--json` carries the same contract as `web/openapi.json` rather than a second CLI-specific shape.

  Removals require `--yes` rather than asking: these commands end up in scripts and cron jobs, where a prompt either hangs forever or gets answered by accident, and a flag also leaves the intent visible in shell history. Failures exit with a code that says which kind — `2` arguments, `3` server unreachable, `4` credential rejected, `5` not found — because a script that gets `1` for everything cannot tell a wrong token from a host that is switched off.

- **And an editor for them.** A new Automation view builds its forms from the server's own vocabulary — triggers, fields, operators and action kinds all come from one endpoint — so the editor cannot offer something the API would refuse. Nested `all`/`any`/`not` conditions are shown indented, in reading order, rather than on a node canvas: "all of these, and one of those" is something you read, and a graph makes that harder as well as being unusable with a keyboard. The operator list narrows with the chosen field, so a size compared with `contains` cannot be built in the first place, and removing the last entry of a group falls back to "always" instead of leaving an empty group that silently means always or never. A dry run evaluates a trigger against a real package you pick and reports, per automation, whether the trigger matched and whether the condition held — reported separately, so "wrong moment" is distinguishable from "right moment, rule does not hold" — and by construction it changes nothing.

- Two MCP tools, `list_automations` and `toggle_automation`, so an assistant can see what is configured and switch one off without going through the web interface.

- **PAR2 recovery files can be deleted once they are no longer needed.** Off by default and switchable globally or per category. It runs after unpacking rather than straight after the repair, and only at the level that also removes archive volumes: PAR2 verification happens before extraction, so discarding the recovery data at repair time would leave a package whose unpack then failed — a wrong password, a missing tool — with nothing to repair from and no second attempt. A set is the main index plus its volume siblings, matched on the shared stem, so a second release in the same folder keeps its own.
- **Whole transfer services can be switched off.** A new Services tab turns BitTorrent, Usenet, media downloads, galleries, stream recordings and remote file transfer (FTP, FTPS, SFTP, WebDAV) on and off individually. Everything is on until you switch it off, so an existing installation behaves exactly as it did.

  Switching one off works in two places, because one alone would be a half-measure. New links of that kind are refused at intake — the LinkGrabber reports how many a paste lost, and a paste consisting only of such links is refused with a reason rather than accepted into a list where nothing would ever happen. And whatever was already queued moves to blocked with that reason, instead of sitting in "queued" waiting for a runner that will never come. Switching the service back on releases those rows into the queue again. Enforcement is in the dispatcher rather than at startup, so a change takes effect immediately and not after a restart. The `.torrent` upload endpoint is checked separately, since it does not go through intake.

- Post-processing steps for remuxing a recording showed their raw identifier instead of a name. That list is now kept alongside the kinds it renders.

- **Accessibility work toward WCAG 2.2 AA.** A skip link that is first in the tab order on every page, landmarks around the navigation and the routed view, and one polite live region that announces the queue as a summary — "3 of 9 packages downloading" — rather than one message per download, which would be unbearable during a busy queue. `prefers-reduced-motion` collapses every animation; everything that moves here is feedback that also exists as text.

  Three real defects turned up on the way, and are fixed. The accent colour failed contrast in light mode — the interface library takes shade 500 of the palette, which is 2.36:1 against this application's ground, short of the 4.5:1 text needs and short again as a button ground under white text; it is now shade 700 at 5.27:1. A progress bar in the post-processing list had no accessible name, so a screen reader announced a percentage attached to nothing. And the connection indicator was a pulsing dot with no text beside it.

  `axe-core` now runs over rendered components in the test suite, and the contrast ratios are computed from the palette and pinned — axe cannot check contrast in a test run, because its rule samples rendered pixels through a canvas that jsdom does not have. What automation cannot answer at all is written down in `docs/accessibility.md`, including the part that has not been done: nothing has yet been tried with an actual screen reader, which is why this says "toward WCAG 2.2 AA" and not "conforms to it".

### Fixed
- **Editing a category failed outright.** The statement that saves one named a column it did not supply a value for, so every save was rejected by the database — introduced with the PAR2 deletion setting in this same release, and caught only when the next column was added. Both write paths are now exercised end to end by a test that reads the result back, which is what would have caught it the first time.
- **The Streams page no longer offers a schedule form there is nothing to schedule with.** On a fresh installation it showed two empty states at once and a live "Add schedule" button whose channel picker was empty; pressing it posted an empty channel id and failed server-side. The schedule section now says to add a channel first, and appears once there is one.
- **Remaining traffic on DDownload and KatFile was shown a million times too small.** Both report the figure in megabytes, and it was passed on as if it were bytes — an account with 112 GiB left appeared as "112 KiB". The conversion now lives in the shared XFS code, so it is done once rather than in each hoster, and the unit is written down in all three places the field is declared, which is where it should have been in the first place. The tests that were supposed to cover this only asserted that a value was present; they now assert the value.

### Changed
- **A new installation no longer shares torrent data with anyone.** Uploading is now a switch of its own, off by default, and it is the outer one of the two: seeding was already off, but that only governs what happens *after* a download finishes — the engine still uploaded to other peers while it was running, and nothing turned that off. It does now, at the engine, so an installation that was never configured contributes nothing until somebody decides otherwise. Category and per-torrent seeding overrides can no longer re-enable sharing that is globally off; an override narrows, it does not widen.

  This changes existing behaviour: whoever downloads torrents today also uploads while doing so, and after upgrading will not. It is worth knowing what that means. A BitTorrent client that only ever takes is unfair to the swarm it takes from, and many private trackers ban clients that do it — the switch is in the interface, with that said next to it, precisely so it stays a decision rather than a surprise.

- An `api:*` token now reaches the whole REST surface, the same one an interactive session reaches. It was initially limited to the read-only allowlist introduced with `api:read`, which was too narrow a reading of what the scope means: that list exists to narrow `api:read`, not to cap full access. Without the correction every machine client — the new command-line client included — could only ever look. What `api:read` may see is unchanged.
- A revocable bearer token is now checked before the "setup has not been completed" gate, matching how capture tokens have always behaved. A monitoring client paired with an unattended service no longer starts failing because that service never had an administrator password set.

## [0.8.0] - 2026-09-04

Milestone 0.8 — media, feeds and subscriptions: the advanced format selector and everything
built on it, browser-cookie profiles, direct HLS/DASH intake, subscriptions with a
download archive, RSS/Atom/podcast feeds, Newznab and Torznab indexers, scheduled livestream
recordings, and recordings that survive disconnects.

### Added
- Media links are now chosen by what you actually want rather than by a resolution preset. The LinkGrabber row for a yt-dlp link expands into a format selector filtering on container, video and audio codec, HDR mode, frame rate, bitrate and audio language, with the estimated size and the resulting format shown as you go. Every preset — Best, 2160p down to 480p, and MP3 — is expressed in that same vocabulary, so a preset and a hand-built selection take the exact same path.
- A filter combination that matches nothing explains itself instead of just saying so. The selector reports how many formats each criterion would keep **on its own**, which is what distinguishes "this page has no HDR at all" from "it has AV1 and it has HDR, but never in the same format". A selection may be marked preferred, in which case criteria are dropped in a fixed order until something matches and you are told exactly what was given up, or required, in which case the download fails with `media.criteria_unsatisfiable` rather than quietly handing over something else.
- Multiple audio tracks and subtitles can be selected per media link. Extra audio languages are merged into the file where the container allows it, and subtitles are written as a separate file, embedded, or both, optionally converted to SRT, VTT or ASS. Manual and auto-generated tracks are listed and filtered apart, and auto-generated ones are **off by default and never implied** — they come from speech recognition and are regularly wrong about names, numbers and negations, and once embedded they are indistinguishable from an authored translation. A container that cannot hold what was asked for, or a language the page does not offer, is reported as a warning before queueing rather than silently dropped.
- Thumbnail, chapters, tag metadata and the description with its source URL can each be embedded into the finished file, and SponsorBlock segments can be marked as chapters or cut out. Each piece is its own switch rather than one "add metadata" toggle, because embedding is irreversible in practice — nobody re-downloads a file to strip a wrong tag. SponsorBlock is **off by default**, defaults to sponsor segments only when switched on rather than to every category, and says plainly that removing segments re-cuts the media while marking them leaves it intact.
- **A signed source URL is never written into a file.** The signature *is* the credential, and an embedded info JSON travels with the file into shares, uploads and post-processing scripts. Such a link has its description and source URL withheld, with the reason shown before anything is queued rather than discovered afterwards. What a container cannot hold — a cover image in WebM, chapters in FLAC — is likewise reported up front and skipped rather than failing the download.
- Media files can be laid out with an output template — `{uploader}/{upload_year}/{title}` and the like — set globally or per link, with a live preview of the path that will actually be written. The preview and the download share one evaluator, so what is shown is what lands on disk. The grammar is deliberately closed and has nothing to do with yt-dlp's own `-o` syntax: only an allowlisted field set is available, an unknown field is refused while you are typing it rather than ending up in a filename, and the template is expanded to a literal path *before* anything reaches the tool. Absolute paths, drive letters and `..` are refused both in the template and after expansion — a video titled `..` is not exotic — and every produced segment goes through the same name sanitiser as the rest of the application, so reserved device names and path-length limits are handled in one place. A template that turns out not to expand for one particular page falls back to the plain file name instead of failing the download.
- Private and age-restricted media can be fetched with a browser session you already approved. The LinkGrabber row for a yt-dlp link offers the cookie profiles that actually cover that address, and the automatic choice names the profile it would land on rather than just saying "automatic". Because the choice sits on the link rather than on the queue row, the *first* attempt at a private page already carries the right session instead of failing once and being corrected afterwards. **Only cookies for the page's own host are handed over.** A browser export routinely carries rows for analytics and CDN domains that happened to share the jar, and passing those to an extractor would send one site's session to another; the rows are filtered against the page host, so a profile for `example.com` never reaches `evil-example.com` or `example.com.evil.tld`. The file yt-dlp receives is created with owner-only permissions, is named after nothing in particular — a temporary directory is world-readable, and the name would otherwise say which sites you have sessions for — and is deleted when the download returns, whether it succeeded, failed or was cancelled. Only its path is ever a process argument. A profile you pinned that turns out to be disabled, expired or scoped elsewhere fails the download with `media.cookie_profile_unusable` rather than quietly falling back to the public version of the page, which is how you end up with a 30-second trailer named like the full episode.
- Direct HLS (`.m3u8`) and MPEG-DASH (`.mpd`) addresses are recognised as media rather than downloaded as text files, which is what used to happen — the playlist itself landed on disk named like a video. What a link *is* is decided from its body, not from its address: a signed CDN link ends in no useful extension and is regularly served as `text/plain`, so a cheap header check picks out the responses worth reading and the document itself has the final say. **A live stream and a recorded one are routed differently.** A playlist with no `#EXT-X-ENDLIST`, or an MPD marked `dynamic`, has no end and goes to the recorder; handing it to the file downloader produces a job that can never reach 100 %. Master variants, alternate audio languages and subtitle tracks feed the existing format selector, so everything built for media pages — criteria, tracks, metadata embedding, output templates — applies to a bare manifest URL unchanged. Relative segment paths resolve against the address the manifest actually came from rather than the one that was pasted, so a redirect no longer sends every segment to the wrong host, and a manifest's credentials stay with the manifest's own origin instead of following it to a CDN on another hostname. **DRM is detected and refused, not attempted.** A vendor `KEYFORMAT`, `SAMPLE-AES`, or a DASH `ContentProtection` element ends the check with a stated reason instead of failing obscurely inside ffmpeg some minutes into a download. Ordinary AES-128 encryption is *not* DRM and keeps working — it is common, ffmpeg handles it, and refusing it would have broken a large number of streams that were never protected in the first place.
- **Channels, playlists and gallery profiles can be subscribed to.** Each one is polled on its own schedule and what it finds enters the LinkGrabber through exactly the path a pasted link takes, so routing rules, categories, the online check and review all still apply — a subscription gets no privileges a person does not have. Review is the default and automatic queueing is a deliberate choice, because a subscription that starts downloading on its own is not something an undo button fixes. **An item is acted on exactly once.** Every item gets a canonical key — the source's own id where there is one, otherwise a normalised address, otherwise a hash of title and date — and that key is unique per subscription in the database. A feed that reorders itself, a poll interrupted halfway and repeated after a restart, and the same video listed by both a channel and a playlist all produce the right number of downloads rather than the convenient one. **Switching a subscription on does not import its history.** A channel with ten years of uploads is the most destructive thing this feature could do to a queue, so the first poll records what already exists as seen and acts only on what appears afterwards; collecting the backlog for review instead is an explicit option, and it never queues, whatever the mode says. Items can be filtered by title, duration, date, language and resolution, and **a rejected item records which rule rejected it** — "nothing appeared" is otherwise indistinguishable from a filter that is quietly throwing everything away, a broken adapter, and a channel that simply has not posted. Missing metadata never rejects, so a source that does not report durations is not silently emptied. A failing source backs off on its own schedule and blocks nothing else: each poll is isolated, its error is stored on its own row, and the backoff is bounded so a source that comes back is noticed the same day.
- **RSS, Atom and podcast feeds can be subscribed to.** Both dialects go through one parser pass rather than being sniffed apart first — they differ in element names, not in structure — and a podcast item is downloaded by its *enclosure* rather than its `<link>`, which is a show-notes page and the most obvious way to get this wrong. Season, episode, duration and publication date come along, and an item with no address at all is dropped rather than queued as nothing. Polling is conditional: the `ETag` and `Last-Modified` from the previous check are sent back, so an unchanged feed costs a `304` instead of a full download every hour. **A feed is treated as hostile input.** A DOCTYPE with an internal subset or an entity declaration is refused outright rather than trusted to the parser, which is what keeps a billion-laughs document from being a denial of service, and a truncated document is an error rather than an empty feed — half a document is otherwise indistinguishable from a channel that deleted all its entries, and the poll would have reported success. Identity, filtering, the archive and backlog protection are the same ones subscriptions already use, so a feed's first check does not import its history and an entry is acted on exactly once however often it is re-listed.
- **Newznab and Torznab indexers can be added as a source.** A saved search copied out of an indexer's own RSS button keeps its query, categories and search term; what the subscription adds is the schedule, the archive and the review list. Results are shown with their age, size and category, and are downloaded through the existing NZB and torrent import paths rather than a second one. **The API key never leaves the vault as text.** It is fetched at the moment a request is built, it is stripped from every address that could reach a log, an error message or the UI, and an address pasted with a key already in it has that key replaced by the stored one rather than sent alongside it. A wrong key is the case worth naming: an indexer answers it with HTTP 200 and an error *document*, which without an explicit check looks exactly like an indexer that has nothing new — so the refusal is detected and reported in the indexer's own wording, with the credential removed.
- **An indexer's capabilities are discoverable, and its categories can be routed.** `t=caps` returns the category tree, the search types the indexer actually answers and its result limit — and doubles as the test action, because it is the cheapest request that proves the address and the API key are both right while pulling no results. Categories can then be mapped one by one onto rDownloader categories, chosen from the indexer's own list by name instead of typed as numbers: an id like `5030` means nothing on its own and means something different on the next indexer, which is why the mapping belongs to the subscription rather than to a global table. An unmapped category falls back to the subscription's own, so adding the feature changes nothing for anyone who does not use it, and a mapping added later still applies to releases that were archived before it existed.
- **Livestream recordings can be scheduled.** A channel gets recurring weekly windows or a one-off event time, and one that has a schedule is watched *only* inside it — which is the point, since a channel that goes live for something else no longer produces a recording nobody asked for. A pre-roll and post-roll cover a broadcast that starts early or overruns, and "record now" keeps working alongside a schedule. **A local time stays a local time.** The zone is stored as an IANA name rather than an offset, because an offset does not survive daylight saving: a weekly show at 20:00 would quietly move by an hour twice a year. Both awkward edges are decided rather than left to chance — the hour that happens twice when the clocks go back produces one occurrence, deterministically the earlier one, and the hour that never happens when they go forward produces none at all rather than a recording shifted to a time nobody asked for. An occurrence is planned once and only once, guaranteed by the database rather than by the planner remembering, so a restart mid-window changes nothing. **A window that passed without the channel going live is a visible missed recording**, including one that passed while the service was down, rather than a row that stays "planned" forever or an absence nobody can notice. Recording from the beginning of a stream is offered where the provider actually exposes a replay window and what is recorded is what actually happened, so the UI never claims a capability that would have silently started the recording at "now".
- **A livestream recording survives a dropped connection.** A recording is now a sequence of segments rather than one file: a disconnect ends a segment and the recording continues in the next one, and **bytes already on disk are never given up** — a drop three hours in costs the seconds it takes to reconnect, not the three hours. The history is written after every segment, so a crash leaves a record of what is actually on disk instead of nothing. Long recordings can also be cut deliberately, by time or by size; a deliberate cut and a dropped connection are the same mechanism but not the same fact, and only the drop is recorded as a gap, because that is the difference somebody actually cares about later. Segment names are zero-padded, so they sort in the order they were recorded — without that, `part10` sorts between `part1` and `part2` and the finished film is assembled in the wrong order. A finished recording can be joined into MKV or MP4 as a **resumable post-processing step** rather than as an afterthought of the recorder: a six-hour join has to survive a restart, and the segments are only deleted once the container exists and is not empty. Metadata and a thumbnail can be saved beside the recording. **What the provider does not offer is reported rather than silently skipped** — subtitles and live chat are marked unsupported, because streamlink passes the muxed stream through untouched and live chat needs a per-provider client, and quietly producing nothing is indistinguishable from having forgotten. Falling back to the published VOD is available for a recording that fell well short of its scheduled window, and only then: without an expected length there is nothing to fall short of, so an unscheduled recording never re-downloads itself.
- What a link was chosen *by* is now stored, instead of the format id that happened to satisfy it. A `format_id` belongs to the site, not to us — YouTube rotates them, and a link that sat in the queue overnight resolves to a different one in the morning. The criteria are re-resolved at download time, and the expression handed to yt-dlp lists the pinned ids first, the same choice expressed semantically second, and a merge-free last resort third. A still-valid id therefore wins, a rotated one degrades to an equivalent format instead of failing, and the final alternative always works without ffmpeg.

### Changed
- Warnings and errors from `yt-dlp` and `streamlink` are redacted before they are logged or shown. Both tools echo the request they just made, so with a cookie file in play those lines were exactly where a session would have surfaced.
- Without ffmpeg and ffprobe, only formats that already carry both video and audio are offered at all. Previously a merge-requiring variant could be picked on such an installation and would leave two unusable stream files behind; now the rule is enforced in one place, mirrored by the UI, and a page that offers nothing progressive says so up front with `media.merge_unavailable` instead of failing at download time. MP3 is likewise no longer offered where it cannot be produced.
- Existing media downloads and API callers are unaffected. Rows written before this change carry no criteria and are understood through their preset id, the `media_variant` field of `PATCH /api/v1/collector/candidates/{id}` still accepts `best`, `<height>p` and `audio_mp3`, and livestream recordings — whose stored `format` is a streamlink quality, not an extractor expression — are passed through untouched. No stored row is rewritten by the migration.
- New endpoints under `/api/v1/collector/candidates/{id}/media`: the format inventory of one link, a `…/media/preview` that resolves a filter set without storing it, and a `…/media/selection` that applies it. The inventory is fetched per link rather than inlined into every LinkGrabber response, so a page offering two hundred formats does not weigh down the list. `media_default_criteria` in the settings document sets the default selection for new links.

## [0.7.0] - 2026-09-04

Milestone 0.7 — the plugin platform: a capability-based contract, transfer backends as a second
plugin type, diagnostics and an SDK, and one implementation per hoster instead of two.

### Added
- **Transfer protocols can now come from a plugin.** A signed package can declare itself a
  transfer backend, claim URL schemes and carry the bytes of a protocol this application knows
  nothing about; such a download appears in the queue like any other and obeys the speed limit,
  pause and resume the same way. The split is deliberate and not negotiable by the plugin: it
  moves bytes, while the application decides where they land and whether they count. It is never
  given a path — it writes into the download's own staging file and has no way to name another —
  and the application checks the length itself before moving the finished file into the package
  folder, so a backend that claims success on a short file produces a failed attempt rather than
  a truncated file presented as complete. Sockets belong to the application and reach only the
  hosts and ports the manifest names; loopback and link-local addresses are refused, because the
  service's own interface and cloud metadata live there and no transfer protocol needs either.
  A paused transfer stores whatever the backend needs to continue, along with the version that
  wrote it: the job resumes on exactly that version, so an update landing mid-download cannot
  hand a half-written file to a build that would read it differently. `plugins/example-transfer`
  is the reference implementation and `docs/plugins.md` documents the contract.

- **Plugins can be diagnosed instead of guessed at.** Every invocation is recorded with the
  plugin, its version, the entry point and how it ended — crashed, timed out, ran out of its
  budget, was refused a permission, or simply reported that the hoster said no, which are
  different problems calling for different answers. Settings → Plugins shows the newest entries
  per plugin, each with a correlation id that can be quoted in a bug report and identifies
  nothing else. The message goes through the same redaction as every stored error, so a
  credential or session token never reaches it, and the history is capped per plugin so a
  failing plugin cannot fill the disk.
- **An SDK for writing plugins.** `rdownloader plugin new --type resolver|transfer` writes a
  plugin that compiles, packages and passes conformance before a line of it is changed, with its
  own signing key and its own copy of the contract, so it builds without a checkout of this
  project. `rdownloader plugin conformance <package> --json` answers whether this application
  would run a package: it verifies the archive, manifest and signature exactly as installation
  does, checks the ABI version, instantiates the component against the world its type declares,
  and catches a resolver that claims links belonging to no hoster it declares — the mistake that
  turns every direct download into "this hoster needs an account". A reusable CI workflow ships
  in `sdk/ci/plugin.yml`.

### Changed
- **Breaking: plugins must be rebuilt.** The WebAssembly contract is now
  `rdownloader:plugin@0.6.0` with manifest version 3, and 0.7.0 loads nothing older. A plugin
  declares what it is (`plugin_type`), which ABI it speaks (`api_version`) and every permission
  it is granted in one `[capabilities]` block — web requests with their domain allowlist, account
  cookies, CAPTCHA solving, and the credentials it may expand. Each grant is one WIT interface,
  so a plugin that was not granted CAPTCHA solving cannot even link the interface, let alone call
  it; the file system and other processes have no interface at all and stay unreachable by
  construction. The plugin manager shows the type, the ABI version and the granted permissions
  for every installed package. Third-party plugins have to be rebuilt against the new contract;
  before 1.0 there is no compatibility promise across these revisions.
- A package this version cannot run is now **listed as such instead of disappearing**. Previously
  an outdated plugin was skipped in silence and the hoster it served simply stopped working with
  no explanation. Refused packages appear in Settings → Plugins with the reason — built for an
  older contract, asking for an unknown permission, or damaged — and a button to remove them.
  Nothing is deleted automatically: an installed package is the user's own, and one that vanishes
  by itself explains nothing.
- The eleven bundled hoster plugins ship as version 0.7.0 with their permissions declared
  explicitly, and Premiumize no longer declares a cookie scope it never used.

### Fixed
- **Premium accounts whose subscription had run out were still reported as premium**, and an
  imported cookie session was reported as valid without being checked. Each bundled hoster existed
  twice — once compiled into the application, once as the WebAssembly plugin that actually runs —
  and the two had drifted apart, with the plugin being the weaker of the two: it asserted things
  the built-in version verified, and it could not check an expiry date at all because it had no
  clock. Each hoster now exists once, so both halves behave identically by construction, and the
  richer behaviour is the one that survived. DDownload and KatFile also regain the API fast path
  for premium downloads that only the built-in version ever had.
- A download pinned to a resolver version that is no longer installed is no longer stuck for
  ever. The pin exists so a plugin update cannot move a job that is already running; it was never
  meant to outlive the version it names, but a job whose pinned version had gone failed on every
  retry with "the pinned plugin version is not installed". Because the built-in resolvers used to
  report the application's version rather than their own, *every* release invalidated every pin
  pointing at one. Such pins are now released at startup, before anything is scheduled, and the
  job resolves through the current version of the same plugin. The built-in resolvers now report
  the version of the plugin itself, so a pin only becomes stale when the plugin genuinely changes.
- The built-in resolvers and their packaged counterparts no longer describe themselves
  separately. Both read the plugin's own manifest, which had already drifted — DDownload's
  built-in domain list was missing the delivery host its manifest allows — and both are confined
  by the same permissions. A built-in resolver could previously reach every domain any installed
  plugin declared, and nothing stood between it and the cookie store or the CAPTCHA solver.

### Added
- A download can be reset: everything it produced is discarded and it starts over from zero. That covers the partial data, the checkpoints, the progress, the retry budget, the recorded error and the package's post-processing steps, for every transfer kind and every state a job rests in — a finished one included. It is available per row, as a bulk action over a selection, as `POST /api/v1/downloads/{id}/reset`, and through the MCP tool. Automatic retry with backoff already existed, but once its budget was spent a job stayed failed with no way back, and the only manual restart needed a stored request template and so covered authenticated replays alone. **A finished file is kept unless the confirmation dialog's separate, unticked box asks for it**, in which case the new attempt lands beside it under a free name — the file on disk is frequently the only copy there is. What was chosen rather than produced — the torrent file selection, the media criteria, the expected checksum — survives the reset, because the point is to fetch the same thing again.
- The capture agent shows a native desktop notification when links are taken into the LinkGrabber, one per import, on Windows, macOS and Linux. It reads a new capture-scoped event stream that carries the intake event **and nothing else**: a capture token is a restricted credential for handing links in, and the full event bus would tell it about download paths, accounts, proxies and credential changes. A desktop with no notification daemon is a line in the log, exactly like a tray icon that cannot be created, and never costs Click'n'Load or the clipboard. Switched off with `--no-notifications` or `RDOWNLOADER_NO_NOTIFICATIONS`.

### Fixed
- Moving a package into a category puts it in `<category>/<package>/` again, and takes its data along. The category change was the one place that wrote the destination without building the package folder, so the file landed directly in the category root; and because the change was a pure database update with no filesystem step, the folder it came from stayed behind empty. Files that are still transferring are not interrupted — they keep the `.part` they have open and are promoted into the new folder when they finish, and the old directory is swept once the last of them is done. The move goes file by file rather than by directory, because a package written before this fix points at the category directory itself, which is shared with every other package in it; and it survives a category on a different disk, where a plain rename fails. A directory is only ever removed while it is empty, so data kept there is never touched.
- The category of several downloads can be set at once again, which failed in practice on NZB packages. The bulk endpoint refused the **whole** request as soon as one selected package was finished, and a Usenet package counts as finished while it is still being repaired and unpacked. A finished package can be recategorised now that its data moves with it, so the restriction is gone. In the list a package is also judged by every file it has rather than by the ones the current filter leaves visible, so a filtered view no longer reports a half-selected package as fully selected, and ticking a package takes its hidden files along.

## [0.6.1] - 2026-09-04

### Added
- DLC containers are imported into the LinkGrabber, by upload, drag and drop, `POST /api/v1/dlc/import`, or a hotfolder. Each package the container declares becomes its own LinkGrabber package, with the file names, sizes, package name and archive password it carries, and the links are checked online right away. A `ftp://user:password@host/…` link inside a container is treated like a pasted one: the credential is stored once and the link is reduced to its bare form before it reaches a candidate row. A hotfolder DLC always stops in the LinkGrabber for review, even in queue-directly mode, because its links still have to be resolved before they can start.
- The import is **off by default** and has to be enabled under Settings → Collector. The DLC format keeps its links encrypted with a key that only JDownloader's `dlcrypt` service can unwrap — there is no offline algorithm — so an import necessarily sends the container's key to that service. Nothing is sent while the switch is off; a dropped `.dlc` is refused with `dlc.service_disabled` instead. The service address is configurable for anyone running their own, and the links themselves never leave the installation.

### Fixed
- Deleting downloads no longer recreates their package folders. Removing a queue entry cleaned up its `.part` file through a helper that created the destination directory first, so anyone who had moved the finished data away and then cleared the links found the empty package folders back on disk. The cleanup now opens the destination only if it still exists, and once a package's last file is gone its staging and package directory are removed — but only while they are empty, so data left there is never touched.
- A package without a category lands in configured storage instead of the service's own download folder. With no category marked as the default, the destination fell back to the application download directory, past every storage root; it now uses the default storage root (the first one when none is marked). The service directory remains the fallback only when no storage root is configured at all. Such packages are also counted against their root's free-space threshold now, instead of against the fallback.
- Completed packages no longer show their download priority — after the download it says nothing about the package, and only the category remains.

## [0.6.0] - 2026-09-03

First tagged release. It carries everything built so far, closing milestone 0.5 (reliable everyday
downloads) and milestone 0.6 (additional transfer protocols); the 0.5.0 version number was never
cut as a release of its own.

### Added
- FTP, FTPS, SFTP and WebDAV as transfer sources, completing milestone 0.6. FTP covers plain FTP, explicit `AUTH TLS` and implicit FTPS on port 990, with passive or active data connections and anonymous, password or stored logins. SFTP runs over SSH with password, private key (with optional passphrase) or SSH agent authentication — the agent is found the way each platform exposes it, through `SSH_AUTH_SOCK` on Linux and macOS and through the OpenSSH or Pageant named pipe on Windows. WebDAV deliberately has no engine of its own: a share resolves through `PROPFIND` and its files download over the existing HTTP engine, so they inherit parallel chunks, checkpoints, `Range`/`ETag` resume, authentication profiles, proxy, custom CA and the global speed limit instead of a second implementation of each. Links are recognised by scheme — `ftp://`, `ftps://`, `sftp://`, and `webdav://`/`dav://` for plain HTTP or `webdavs://`/`davs://` for HTTPS, never silently swapped for one another. Stable codes `ftp.*`, `sftp.*` and `webdav.*` are translated by the UI; a server's own reply never reaches the queue, because those replies routinely quote the user name and the full remote path.
- SSH host keys are confirmed once by a person and remembered per server, port and algorithm. An unknown server blocks the transfer and reports its `SHA256:` fingerprint to compare; a server whose key later changes blocks with both fingerprints side by side and is **never** accepted automatically, not even with first-use trust switched on, because a rebuilt server and an actively intercepted connection are indistinguishable from the client. First-use trust exists for unattended setups, is off by default and only ever covers a first sighting. An offered certificate is refused rather than reduced to its signing key, since pinning a CA is a different trust model that would accept every future certificate it issues.
- Resume is validated rather than assumed on all three protocols. Size and modification time are recorded on the first attempt and rechecked before continuing; if either moved, the partial file is kept and the transfer refused instead of being corrupted. An FTP server that refuses `REST` reports that resume is impossible rather than restarting on top of the bytes already on disk, a WebDAV server without `Accept-Ranges` says so before anything is queued, and a transfer that ends early fails instead of promoting a truncated file. Weak `ETag` validators (`W/"…"`) are now treated as absent everywhere, including for ordinary HTTP downloads: a weak tag only promises semantic equivalence, so resuming on it continues into different bytes.
- Remote directories are reviewed as a file tree with folder tri-state selection before anything is queued, and each selected file becomes its own queue row so it resumes and retries on its own. The selection is stored as exclusions, so a later re-listing that adds a file keeps it selected instead of silently dropping it. Listings are bounded on entry count and depth, and a listing that was cut short says why rather than presenting a short list as complete.
- Stored logins for FTP and SFTP servers, per host, port and user, with a live test action, under Settings → Network. Passwords, private keys and passphrases live in the encrypted secret store and are never returned by any API, written to a log, or placed in a backup exported without secrets. A pasted `ftp://user:password@host/…` link is accepted — that is how such a location is normally shared — and its credential is stored once while the link is rewritten to its bare form before it reaches the candidate, the queue, an event or a log line; an existing login for the same server and user is never silently overwritten. New endpoints under `/api/v1/remote-credentials`, including the SSH trust store at `.../ssh-hosts`.
- Hostile or broken servers are contained at the parser boundary. WebDAV refuses a `DOCTYPE` outright rather than expanding entities (no billion laughs, no external entity file reads), bounds nesting and body size by both the declared length and what actually arrives, and requires every `href` to resolve inside the requested collection and on the same origin, so a server cannot decide where a file is written. Paths from FTP and SFTP listings are checked against traversal, absolute paths, Windows drive letters and UNC prefixes before they can be joined onto a local destination. FTPS trusts the platform certificate store augmented by the operator's custom CA, and a CA bundle that parses to nothing is refused instead of quietly falling back to the public roots.
- SFV checksum verification in post-processing. When a package holds an `.sfv` index — the plain `filename CRC32` list releases ship next to their volumes — every file it names is hashed and compared after PAR2 and **before** anything is unpacked, for HTTP, Usenet, torrent and every other kind alike. The order is deliberate: at `+Delete` the volumes an index lists are gone once extraction succeeded, so verifying afterwards would check nothing. A differing checksum or a listed file that is absent fails the step, skips unpacking and cleanup, and fails the package rather than quietly extracting corrupt data; the step history names the affected files and a user script still runs, receiving status `3` — the code a failed PAR2 repair already used, so SABnzbd-compatible scripts need no change. Indexes are read leniently (comments, blank lines and unparsable lines are ignored, Latin-1 files still work) and an entry whose path would leave the package folder is skipped instead of followed. The check is **on by default** and existing installations pick it up without changing anything; it can be switched off globally in Settings → Post-processing and inherited or overridden per category, and it rides the routing backup. `.sfv` stays on the default cleanup list, so the index is still removed afterwards — cleanup has always run after verification.
- Bandwidth is now planned rather than capped by one number. Reusable profiles bundle a global download rate, an optional torrent upload rate, a parallelism cap and traffic budgets, and a weekly schedule with an IANA timezone decides when each applies. On top of the global rate a profile can limit a protocol, a host, a provider account or a category; a transfer passes every bucket that applies to it, so the strictest applicable limit wins by construction and `GET /api/v1/bandwidth/status` names which one it was. The speed limit set by hand in the Downloads toolbar is a bucket of its own and survives every profile switch. Schedule windows may wrap past midnight, overlaps resolve by an explicit priority and then the later start, and everything is evaluated from a UTC instant into local time — so a daylight-saving change neither skips a switch nor invents a second one, and the daily and monthly budgets, keyed by the local calendar period, can neither double-count nor lose a period across a restart. An exhausted budget holds back new starts only; running transfers finish. Every transport is covered and the ones that cannot be are named instead of silently ignored: HTTP and Usenet are paced in-band, the torrent session takes the stricter of profile and setting, yt-dlp and gallery-dl receive `--limit-rate`, and streamlink is reported as unenforceable in `GET /api/v1/bandwidth/capabilities`.
- Unattended operation, so a desktop or NAS can be left alone. Quiet hours postpone only what is configured — PAR2 repair, unpacking, uploads and notification grouping — and never touch downloads; a job already running is not interrupted, only the next one waits. Once the queue *and* post-processing have drained, a completion action runs exactly once per cycle of work: a script through the existing post-processing sandbox, standby, or shutdown. Both the open and the completed cycle are persisted, so neither a restart nor the queue briefly flickering empty can run it twice. Standby and shutdown additionally need an explicit local approval and then count down visibly over SSE, cancellable through `POST /api/v1/power/cancel`; adding new work or withdrawing the approval calls the countdown off. Battery and metered operation can hold the queue with a visible reason, and what a platform cannot report — Windows battery and metered state need WinRT — is declared unavailable in the capability matrix rather than guessed.
- Server-side notifications with a persistent delivery history, so a finished download reaches you with the browser closed. Rules route events to targets filtered by event, category and minimum severity; the idempotency key is derived from the rule and the event and is unique, so one event produces at most one delivery per rule even if the worker restarts mid-flight. Three transports: JSON webhooks signed with HMAC-SHA256 over the exact body (`X-RDownloader-Signature`, plus an idempotency header a receiver can use to drop a repeat), SMTP over STARTTLS or implicit TLS with authentication, and Apprise-compatible targets covering Telegram, Discord, Slack, Matrix, ntfy, Gotify, Pushover and Home Assistant through the external `apprise` CLI — whose target URL carries the service token and is therefore handed over on stdin, never as an argument that would stand in the process list. Target secrets live in the encrypted vault and are never returned by the API, logged, or placed in a backup exported without secrets; what a target answers is truncated and redacted before it is stored. Retries back off to an hourly cap and give up after six attempts, a non-retryable answer fails at once, each target is attempted by one task at a time so a hanging endpoint delays nothing else, quiet hours group deliveries instead of dropping them, and every target has a test action that reports what actually came back.
- One free-space policy for every transport. Before this, only HTTP and Usenet checked at all, each with its own hard-coded reserve, while torrent, media, gallery and stream jobs wrote until the filesystem said no. The threshold is configurable per storage root with a global default, and a transfer whose size no runner can state may start while a configurable multiple of that threshold is free. A root below its threshold is blocked on its own: its downloads stay queued and its intake is refused with `storage.capacity_blocked` in the LinkGrabber, the hotfolders and the torrent intake, while every other destination keeps running. The block is persisted, so switching automatic resume off survives a restart instead of silently resuming, and `POST /api/v1/storage/capacity/{target}/resume` releases one by hand and requeues exactly what it held back. Preallocation now separates a filesystem that cannot size a file up front — best effort, the file stays sparse — from actually running out of space, which surfaces as a real error instead of a confusing write failure later.
- Torrents are now steerable instead of being a single opaque queue row. A `.torrent` or a resolved magnet shows its complete file tree in the LinkGrabber with folder tri-state selection and a running total of the selected bytes; the selection is stored on the link, carried to the queue row and handed to the engine as `only_files`, so deselected files are neither requested nor preallocated, and it can still be changed on a running torrent. Files carry priority tiers (high, normal, low, skip) and comma-separated exclusion patterns (`*.nfo`, `extras/*`, `**/sample.mkv`) with a preview that shows what a pattern would drop before it is stored. Where selection, patterns and priority disagree, five documented rules decide: an explicit per-file decision beats every pattern, patterns only touch files nobody decided, `skip` deselects, a pattern-excluded file keeps its priority, and a folder priority is inherited onto its files. New endpoints under `/api/v1/collector/candidates/{id}/torrent` and `/api/v1/downloads/{id}/torrent`, with `torrent.file_index_unknown`, `torrent.plan_invalid`, `torrent.pattern_invalid`, `torrent.metadata_pending` and `torrent.metadata_mismatch` as stable codes — the last of which fails a row rather than applying a reviewed plan to different files.
- Trackers per torrent: the announce list with its BEP 12 tiers is persisted, editable and applied on every add, with a rate-limited reannounce (`torrent.reannounce_rate_limited`) and scrape counters over HTTP (BEP 48) or UDP (BEP 15) that carry their fetch time and are marked stale after fifteen minutes rather than shown as current. A private tracker's passkey never reaches the browser: URLs are redacted as userinfo, query parameter and path segment, and entries are addressed by a one-way id, so a tracker can be kept, reordered or removed without its credential ever being exposed.
- Live torrent statistics: share ratio, uploaded bytes, rates, peer count and piece progress arrive over SSE as a new transient `torrent.stats` event every two seconds, while the peer list is paginated over REST and the piece bitfield is folded into at most 512 buckets, so neither payload grows with the torrent. Nothing stale is shown as current — every sample carries its time, a `live` flag and the engine session incarnation. Peer addresses are masked to their network prefix unless the new `torrent_peer_addresses_visible` setting is turned on, and are never logged or persisted either way.
- Torrent network control: all torrent sockets can be bound to one interface (Linux and macOS), with a kill switch that pauses every torrent within ten seconds of that interface disappearing and resumes them when it returns — the combination that makes a VPN tunnel safe. Adds an IP blocklist URL, selectable peer transports, a per-torrent peer limit, a global download limit, an optional SOCKS5 proxy for outgoing peer connections reusing the existing proxy profiles, UPnP port forwarding with a separate announced port, and BEP 19 web seeds shown as redacted diagnostics. Changing the port, interface, proxy, blocklist or transport now rebuilds the engine session on save instead of waiting for a restart, and a failed rebuild keeps the previous session running.
- Seeding policy inherits global settings → category → torrent, each of enabled, ratio and seed time independently, and every torrent shows the effective value together with the level it came from. Either override can be set or cleared on its own; a lowered limit ends a running seed at once rather than at the next thirty-second tick. Category overrides ride the settings backup.
- `GET /api/v1/torrents/capabilities` reports what the embedded engine can actually do. Anything it cannot — sequential download, first/last-piece prioritisation, protocol encryption, per-traffic-class proxies, NAT-PMP, PCP and web seeds as a download source — is refused with `torrent.capability_unsupported` naming the capability, and hidden or disabled in the UI, instead of being accepted and silently ignored. File priorities are reported as emulated, because they are opened one tier at a time on top of plain include/exclude rather than being a native engine feature.
- Authenticated browser downloads can be reproduced safely. The capture contract (now version 2, announced by `GET /api/v1/capture/ping`) accepts `POST` alongside `GET`, with a bounded request body (64 KiB) that is encrypted in the secret store from the moment it arrives and never returned by any API. Before such a download is added, the LinkGrabber shows exactly what would be sent — target host, method, the addresses redirects may follow, the body's field *names* (never their values) and each category of credential paired with the host it would go to — and nothing is replayed until a person approves that specific request. Approval is bound to the request it was given for, but deliberately ignores signature parameters, so a re-signed address for the same file does not ask again. A capture that cannot be reproduced (a file upload, a multi-part form, an oversize or unreadable body) is still handed over and explained instead of vanishing, and the browser keeps such downloads itself. Reading POST bodies needs the extension's new *optional* `requestBody` permission, so a default install is unchanged.
- Expiring and signed download addresses are renewed before an interrupted download reuses its partial file, through a deliberately narrow hook: a resolver that owns the link, or one plain re-request of the original captured address. Neither runs JavaScript or drives a browser, both stay inside the approved addresses, and a `POST` is never repeated for a refresh. When nothing can renew the address the download is blocked with an explanation and the partial file is kept. A replayed download only ever talks to the approved addresses — a redirect anywhere else is refused before a single header or byte reaches the foreign host — and a `POST` the server will not resume is blocked rather than silently sent again; `POST /api/v1/downloads/{id}/replay-restart` repeats it deliberately.
- The setup assistant now includes an optional MCP step for creating a revocable API token and registering the built-in server with an AI assistant. Runtime settings can be restored to backend-defined factory defaults from Settings → System without removing routing, accounts, tokens or downloads, and category rules can be duplicated with an automatically unique name and priority.
- Reusable authentication profiles per domain, under Settings → Network: a browser session (cookies), HTTP Basic or a Bearer token, each optionally with a client certificate, scoped to one normalised host and optional path prefix. Cookies can be shared straight from the browser extension — a new page context-menu action requests the `cookies` permission for that single origin, so installing the extension still warns about nothing, and the profile it creates arrives **disabled** until you approve it in the web UI. Profiles can be activated, tested against their own domain, edited and deleted, and a single download can pin one, opt out of all of them, or leave the scope to decide. Credentials live in the encrypted secret store as opaque references and are never returned by the API, written to logs or SSE, or placed in a backup that was exported without secrets. Cookies and `Authorization` contain themselves across a redirect (the jar refuses foreign hosts, reqwest strips the header on any origin change), while a client certificate — which is offered during the handshake before anything else runs — additionally refuses to follow a redirect out of the request's origin.
- The browser extension intercepts regular downloads in Chrome/Edge and Firefox and hands them to the LinkGrabber: the download is paused first, only cancelled once the server has accepted it, and resumed in the browser if the handoff fails — so an unreachable rDownloader never loses a download. Interception can be switched off globally in the extension options, and each individual download can be kept in the browser from its notification. The capture contract (version 1, announced by `GET /api/v1/capture/ping`) gained structured links carrying the metadata needed to reproduce the request — effective URL, method, referrer, user agent, content disposition and an allowlist of headers (`accept`, `accept-language`) — which is stored with the link and shown in the LinkGrabber before queueing. Cookies, `Authorization` and other credential-bearing headers are dropped in the extension and again on the server. New ingress source `browser_download` for category rules; older extensions keep working with the previous text payload.
- Free downloads without an account for all seven hosters — Rapidgator, DDownload, KatFile, FileJoker, Keep2Share, Nitroflare and 1fichier: each plugin now runs the anonymous flow a browser would (load the file page, wait out the hoster's countdown, solve its captcha, submit the download form) instead of requiring a premium account. Multihosters (AllDebrid, Debrid-Link, LinkSnappy, Premiumize) remain account-only by nature. The resolver contract (`rdownloader:resolver@0.5.0`) gained two host capabilities for this — `wait` (the host owns the clock, so a countdown neither burns the plugin's compute budget nor trips its timeout) and `solve-captcha` — plus the failure kinds `ip-blocked` and `captcha-failed`. A hoster's free downloads are serialised to one at a time, and an IP limit holds back that hoster's other free links (with the wait it reported) while every other hoster keeps downloading.
- Captcha solving, configurable under Settings → Network: either an external solver service speaking the 2captcha `createTask`/`getTaskResult` API (2captcha, CapMonster, CapSolver, …; the API key is kept in the encrypted secret store) or manually in the web UI. Image captchas are shown for typing; reCAPTCHA, hCaptcha and Turnstile widgets are bound to the hoster's domain and can only be answered by a solver service, which the UI states instead of showing an unusable widget. Waiting captchas arrive live via the `captcha.changed` event. A widget challenge with no solver configured is now raised as a prompt of its own, naming the hoster and linking to the solver settings, and can be declined like any other captcha; a typed answer is refused (`captcha.widget_needs_solver`) rather than silently discarded. `POST /api/v1/captcha-config/test` checks endpoint and key against the service and reports the remaining balance, so a wrong key surfaces in Settings instead of on the next download — the key travels in the request body only and is never echoed back, logged, or put in a URL. The answer timeout (15–600 s) is reserved from the resolver's waiting budget, so a long timeout is no longer cut short mid-answer, and a captcha whose download is paused or cancelled stops being offered instead of lingering in the queue.
- Built-in MCP server for AI assistants (Claude Code, Claude Desktop, …) at `/mcp` (streamable HTTP, official `rmcp` SDK): 14 tools covering direct/magnet downloads (`add_downloads`), the LinkGrabber flow (`collect_links` → `check_links` → `enqueue_collector`), queue control (`list_downloads`, `get_download`, `control_downloads`, `get_status_summary`, `list_packages`, `delete_packages`, `list_collector`) and configuration (`get_settings`, `update_settings` with partial patches applied live, `list_configuration`). Tools reuse the REST handlers' logic, so validation and stable error codes are identical.
- Revocable `api:*` API tokens for machine clients: created, listed and revoked via `POST/GET/DELETE /api/v1/api-tokens` and a new "MCP server for AI assistants" card in Settings → System (with a ready-to-paste `claude mcp add` command). The capture-token store is now scope-aware — capture tokens no longer unlock anything beyond the capture endpoints and API tokens do not unlock capture intake.
- Nine new resolver plugins, bundled and built/packaged/verified in CI and release like DDownload and Premiumize: hosters Rapidgator, Nitroflare, Katfile, 1fichier, Keep2Share and FileJoker (the latter three share the `xfs-common` XFileSharing-clone helper crate), and multihosters AllDebrid, Debrid-Link and LinkSnappy. Provider metadata (domains, aliases, cookie scopes) is centralised in the new `rd-provider-registry` crate.
- Torrent metadata now follows the same LinkGrabber intake as NZBs: the multi-file import dialog and app-wide drag & drop accept `.torrent` files, imported torrents can be reviewed with package name, category and priority before enqueueing, and hotfolders support torrents in both review and direct-enqueue modes.
- The capture agent shows its version in the tray tooltip and status menu.
- Packages keep a persisted extraction outcome: after post-processing the Downloads view shows an "Extracted" badge on success and "Extraction failed" instead of the generic post-processing error (`extraction_result` on `DownloadPackage`).
- NZB history dialog in the LinkGrabber: lists every stored NZB import (including enqueued ones) with state, size and date; deleting an entry allows importing the same file again.
- Navigation badges are live app-wide: the LinkGrabber badge counts links plus pending NZB imports, the Downloads badge shows active/total files; the collector store now connects to server events globally.
- Duplicate links are probed too: a re-added YouTube link gets its real title, size and media metadata instead of staying named "watch", keeps its duplicate warning and can be enqueued deliberately.
- Full web UI internationalisation in English, German, French and Spanish: the language is auto-detected from the browser and switchable in Settings; the backend emits stable error codes with parameters that the UI translates.
- Light/dark/system theme toggle.
- SABnzbd-style post-processing: levels None / +Repair / +Unpack / +Delete configurable per package and per category, cleanup extension list plus sample-file removal, user scripts with SABnzbd-compatible positional arguments and environment variables, a post-processing queue in the Downloads view, optional pausing of downloads while post-processing runs, and package lifecycle states. See `docs/postprocessing.md`.
- Live extraction progress: percent for zip, 7z, unrar and external 7z, streamed to the UI via the `postprocess.progress` SSE event.
- Media provider via external `yt-dlp`/`ffmpeg`: media pages (YouTube, dumpert.nl, …) resolve in the LinkGrabber with variant selection including Audio (MP3), playlist expansion, media settings (tool paths) and tool status in the System view.
- Browser extension (context menu + popup) as a lightweight capture alternative for NAS/Docker setups: pairs via a capture token, CORS on the capture routes, `GET /api/v1/capture/ping`, new ingress source `browser_extension`.
- `youtu.be` short links are normalised on intake.
- LinkGrabber action "Add paused": all visible packages are moved to the downloader atomically with paused files and only started via Play.
- Live download speed globally and per file, a two-minute throughput history in the statistics, a package toggle for starting/pausing together, and restart of aborted downloads with existing partial data.
- Package folders: every package lands in `<category dir>/<package name>/`; archives are extracted directly into this folder (existing files of the same name are replaced), also for NZB packages.
- Delete whole packages (`DELETE /api/v1/packages/{id}`, `POST /api/v1/packages/delete`), also for multiple fully selected packages from the action bar; active files are cancelled first.
- Setting "Disable admin login" (`admin_login_disabled`): turns off password protection for the UI; `GET /api/v1/auth/status` reports `login_disabled`.
- Set/clear the speed limit directly in the Downloads toolbar.
- Target folder visible per package/file; a click copies the path to the clipboard (browsers cannot open a file explorer).
- Hoster catalogue as a plugin feature: resolver export `hosters` (WIT `rdownloader:resolver@0.3.0`), `GET /api/v1/accounts/{id}/hosters`, hoster list per account under "Accounts"; the automatic account fallback and the online check use every provider's catalogue (no longer Premiumize only).
- The short-link alias `ddl.to` is normalised to `ddownload.com` on intake (LinkGrabber, Click'n'Load, direct URL).
- Favicon and app logo (`favicon.svg`).
- Plugin distribution: `rdownloader plugin keygen|package` produces Ed25519 keys and signed `.rdplug` packages; `serve` automatically installs bundled packages from `plugins/` next to the EXE (newer versions), the release workflow builds, signs and ships DDownload/Premiumize as components; embedded release public key (`trusted_keys.rs`), `--no-default-plugin-key`, documentation under `docs/plugins.md`.
- One queue for everything: NZB imports become packages (kind "Usenet") on enqueue with one file per NZB file; priority, position, category, pause/cancel, rename, drag & drop and statistics thus apply to HTTP and Usenet alike. Live progress per NZB file in the download list, segment details expandable.
- File selection in the downloader with bulk actions (start, pause, cancel, extract, rename, remove) via `POST /api/v1/downloads/bulk` and `/downloads/extract`; category/priority apply to fully selected packages.
- Usenet runs as a runner inside the scheduler (`ExternalRunner`); the retry setting and recovery also apply to NZB files; PAR2 repair runs in the extraction service per package.
- JDownloader-style LinkGrabber: links are grouped into packages on intake (Click'n'Load package name, multipart archives such as `.part1.rar`/`.r00`/`.7z.001`, shared name stem), packages carry category/priority/password, links can be renamed, sorted (name/hoster/size/added), ordered via drag & drop and moved between packages; "Download all" takes over exactly the displayed order.
- Online check for links: Premiumize `cache/check` for hosters from the catalogue, DDownload `file/info`, HEAD/Range probe for direct links — provides file name, size and online/offline before the download; automatically after every intake and manually via "Check links".
- Resolver interface `check` (WIT `rdownloader:resolver@0.2.0`) for native and WebAssembly plugins.
- Click'n'Load passes package name and password to the LinkGrabber.
- LinkGrabber layout: full width, "Add links" and "Import NZB" as dialogs in the header.
- Extraction with passwords: package password (edit package), password list `passwords.txt` (one line per password, path configurable) and `{{password}}` in the NZB file name (SABnzbd convention) are tried in order; ZIP (AES/ZipCrypto), 7z and RAR/7z tool.
- Multipart archives (`.part1.rar`, `.r00`, `.7z.001`, split ZIP via 7z) are extracted as a set starting from the first volume.
- Manual extraction for HTTP packages (package header and multi-selection); post-processing steps inspectable per package.
- Setting "Delete originals after extraction".
- Packages have a category, priority (high/normal/low) and manual ordering; the scheduler processes the queue strictly top to bottom.
- The downloader groups files by package (drag & drop within a priority level, multi-selection with an action bar, compact rows with expandable details).
- LinkGrabber: category/priority per link, multi-selection, duplicates can be downloaded again after confirmation.
- Statistics below the download list (queue counters, downloaded/remaining bytes, free space per storage root) and Usenet jobs in the Downloads view.
- Premiumize: automatic use for hosters from the Premiumize catalogue, hoster list under Accounts, account badge on the download entry.
- Renaming of packages and of waiting/paused/failed files.
- Setting "Retries per file" (0–100).
- Windows start/stop scripts (`scripts/windows/`) for running in the background with log files.
- Portable Linux and macOS start/stop scripts with server/capture selection, per-user service and capture autostart through Windows Run, systemd user units and macOS LaunchAgents, plus a portable macOS `.nzb` helper app.
- Cross-platform capture clipboard monitoring on Windows, macOS, X11 and Wayland data-control sessions.
- Progress events (`download.progress`) for timely UI updates.
- Vendor folder for the external helpers: yt-dlp, ffmpeg, ffprobe, unrar and 7z are looked up in a configurable vendor directory, in `vendor/` next to the executable and in `vendor/` inside the data directory before `PATH`, so all binaries can simply be dropped into one folder. The tool status (path, version, origin) is shown in Settings and by `rdownloader doctor`.
- Full editing and deletion of storage roots, categories, category rules and hotfolders (`PUT`/`DELETE /api/v1/{storage-roots,categories,category-rules,hotfolders}/{id}`), with guards: a storage root still referenced by categories and a category still used by unfinished packages return 409; deleting a category removes its rules and detaches finished packages; an updated hotfolder restarts its watcher.
- Per-category cleanup extensions overriding the global list (`null` inherits, `[]` disables cleanup for that category).
- Domain blocklist for the LinkGrabber: a text file (one host per line, `#` comments, subdomains included) whose links are dropped at intake; the intake response reports the number of skipped links.
- Setting "UI port" applied on the next start; `--listen`/`RDOWNLOADER_LISTEN` still take precedence.
- Category, priority and package name can be chosen while importing an NZB; the priority is stored on the import and used when it is enqueued.
- Browser notifications when the queue finishes or a download fails.
- Post-processing example scripts in `resources/scripts/`.

### Changed
- Configuration backup and restore now have their own Settings tab instead of sharing the System overview.
- Dashboard content consistently uses the full available panel width; the Streams view now uses the standard dashboard panel layout so the shared footer stays at the bottom.
- The System view lists every supported external tool with the same resolved path, source and version information as Settings, including ffprobe, unrar, 7z, rclone, gallery-dl and streamlink.
- The media host list ships with roughly fifty common yt-dlp sites instead of five; existing installations can merge the defaults into their list from the media settings.
- Settings are grouped into tabs (General, Network, Media, Post-processing, Collector, Interface) with one shared save.
- The LinkGrabber shows NZB imports in the same list as collected links, with a select-all across both kinds.
- Storage roots, categories, rules and hotfolders are split into tabs instead of one long page.
- Usenet servers follow the same layout as every other configuration screen: the form on top, the server chain below it, with the fallback order adjusted where the servers are listed.
- Downloads: one combined start/pause button instead of two mutually exclusive ones, package rows carry a completion badge and the package download rate, category and priority disappear once a package is finished, file rows put their actions into one menu and show a compact size label, and the manual refresh buttons are gone because the view updates from the event stream.
- The footer shows the app's origin instead of the meaningless `SYNC_SAFE` badge.
- **Breaking (API wire format):** REST error bodies are now `{error, code, params}` and `MessageResponse` carries `code`/`params`; all server-side texts are English and the UI translates them via the stable codes.
- Plugin interface bumped to WIT `rdownloader:resolver@0.4.0`: the `failure` record now carries `code: option<string>` and `params: list<tuple<string, string>>` so plugin errors can be translated by the UI.
- Documentation translated to English; the German stage specs under `docs/specs/` were removed.
- The speed limit shows only one context-dependent action: after setting a limit, "Clear" instead of "Limit" and X at the same time; when the input changes, "Limit" appears again.
- DDownload 0.3.2 allows up to ten parallel files (still bounded by "Parallel files"); for NZB, "NNTP connections per file" is now configurable in addition to the provider-wide limit.
- Package headers show exactly one state-dependent control: pause for running/waiting files, play for fully stopped packages and no control after full completion.
- Windows EXE files contain product, version, vendor and original-filename metadata; for AV reputation, release binaries additionally have to be signed with a trusted Authenticode certificate.
- Download list with fixed column widths (name, status, progress, size, category/priority, actions) for package headers and file rows.
- DDownload: the premium form is submitted with `method_premium=Premium Download` like in JDownloader; the "no premium file" message now names the reason (login dialog, error message or page title of the response).
- Plugin fuel budget raised from 10 million to 2 billion instructions (default and manifests); plugin errors name the trap cause (fuel, time limit, panic) instead of only the wasm backtrace. Plugins DDownload/Premiumize at version 0.3.0.
- The separate Usenet worker and the "Usenet jobs" section are gone; NZB imports only know the states imported/enqueued/failed (progress and errors live on the package or the file).
- Category and priority in the LinkGrabber apply per package; the former candidate bulk endpoint is gone.
- Automatic extraction is disabled by default.
- Post-processing checkpoints are no longer tied to NZB imports (`postprocess_steps` with `owner_id`); interrupted extractions are resumed after a restart instead of downloading the files again.
- Proxy profiles are managed under "Settings"; the Accounts page contains only provider accounts.
- The Usenet and System views show real server/connection numbers instead of static placeholders.
- DDownload: clear hints that downloads need a cookie session (the API key only provides metadata); opportunistic `file/direct_link` attempt via API key.
- NZB: file names from the subject for obfuscated yEnc names; PAR2 detection by file content.

### Fixed
- Transport errors no longer leak credentials. A failing request formatted the underlying error together with its fully expanded URL, so the signature of a presigned CDN link was stored with the download and broadcast over SSE. Signed query parameters, `Authorization`, `Cookie` and secret-store references are now redacted centrally, at the point where a failure is persisted and published, in REST error bodies, and in logs — parameter *names* survive so support output stays readable.
- Captcha settings now follow the shared configuration save action instead of requiring a second, easy-to-miss save button; the solver key still stays in its dedicated encrypted settings path.
- Imported NZBs can now change category and priority in the LinkGrabber before enqueueing, matching imported torrents; the updated routing metadata is persisted and used when the download package is created.
- Adding a hoster link without a matching account silently downloaded the hoster's HTML landing page and stored it under the link's name as a finished download. Resolution now reports `resolve.account_required` for hoster links instead of falling through to plain HTTP, plugin-resolved transfers are checked for downloadable content and abort with `download.not_a_file` (quoting the page's own wording, e.g. a wait notice) instead of writing a web page to disk, and links that could not be checked at all are shown as "not checked" in the LinkGrabber rather than "online". Plain direct links keep downloading exactly as before.
- Headers a resolver attaches to a download (`Referer`, …) were dropped and its reported size was ignored; both now reach the probe and every chunk request.
- Uploading a `.torrent` while the data directory was configured as a relative path failed with "Stored torrent path is not absolute"; stored metadata paths are now canonicalized before their `file://` source is created.
- NZB uploads larger than 2 MiB failed: axum's multipart extractor applied its own default body limit despite the advertised 64 MiB limit.
- NZB files with repeated or zero-based segment numbers aborted the whole import with "Internal service error"; such segment lists are now de-duplicated and renumbered on parse.
- Package names derived from file names or URLs kept their file extension (`Video.mp4`, `file.nzb`) and carried it into the download folder name; known extensions are now stripped when a file name becomes a package name.
- Windows capture autostart no longer leaves a console window open after login; reinstalling autostart overwrites registrations left by 0.1.0 with a windowless launcher.
- Linux capture association installation now selects the NZB MIME handler and removes its MIME package cleanly; Unix capture also handles `SIGTERM` for managed shutdown.
- Re-importing an NZB that had been imported before did nothing at all — no row, no message. The server de-duplicates by checksum and returns the existing import, which the LinkGrabber hides once it has been enqueued; the outcome is now reported, distinguishing "already in the list" from "already in the download queue".
- Renaming a media link did not change the LinkGrabber row: it showed the extractor's page title instead of the file name being renamed. The row now shows the name that will be written to disk and keeps the page title as secondary information.
- yt-dlp could not write files whose name came from a long page title ("unable to open for writing"): names were capped without counting the destination folder, so the full path exceeded the Windows path limit. Names are now shortened to fit the whole path including the suffixes yt-dlp appends, package folders leave room for the files inside them, and replacement characters from broken upstream text no longer reach the file name.
- A completed download could keep showing a partially filled progress bar: external tools report progress in throttled samples and may finish without a final one. Completion now normalises the byte counters, which also removes size labels claiming more transferred bytes than the file's total.
- Without ffmpeg, a video download left two unusable stream files behind instead of one playable file, and yt-dlp's explanation was suppressed by `--no-warnings`. A single pre-muxed format is requested when merging is impossible, and yt-dlp's warnings are logged.
- The helper binaries are also found in the program folder itself, not only in a `vendor` subfolder — that is where they end up in a portable installation.
- Media downloads showed no progress: `--print` puts yt-dlp into quiet mode, which also silences the progress lines the runner parses. `--progress` is passed as well, and a merged video no longer restarts at zero when yt-dlp switches from the video to the audio stream.
- "Postprocessing: ffprobe and ffmpeg not found" although both were installed: `--ffmpeg-location` makes yt-dlp look for ffmpeg *and* ffprobe in that one directory without falling back to `PATH`. ffprobe is now resolved explicitly, the location is only passed when both binaries live in the same folder, and a missing ffprobe is reported before the download starts.
- unrar and 7z are found on `PATH` (and in the vendor folder) instead of staying unavailable until an absolute path was configured.
- NZB packages and their folders no longer carry the `.nzb` suffix.
- The bulk "Extract" action is only offered when the selection actually contains a completed archive.
- The file size label no longer reports more committed bytes than the file's total.
- The download speed chart is translated instead of being German-only.
- Empty `.rdownloader` staging folders are removed after the last package file completes; as long as partial files remain inside, they are kept for pause and resume.
- Incompatible installed WebAssembly plugins no longer prevent startup; they are logged as a warning with plugin ID and version and skipped.
- DDownload premium downloads with a cookie session: the cookies were bound to the host of the account's first request (`api-v2.ddownload.com` when an API key was stored) and never reached `ddownload.com` afterwards — every download ended with "the page requires a login". Cookies now apply to the provider domain including subdomains, a newly stored cookie replaces the cached HTTP client immediately, the redirect to the tightly allowlisted CDN (`*.zeuscdn.org`, plugin 0.3.1) is accepted and the login diagnosis no longer reacts to the `LoginModal` script present on every page.
- The DDownload plugin as a WebAssembly component crashed while resolving with "error while executing at wasm backtrace": parsing the ~200 KB file page exceeded the fuel limit.
- NNTP `BODY` sends message IDs in angle brackets (RFC 3977) — fixes `430 No such article` with Premiumize and others.
- The HEAD probe always reported 0 bytes (reqwest `content_length()` on HEAD) — downloads were immediately marked "finished" with 0 bytes.
- DDownload error responses without `result` are shown with the real message (e.g. "Invalid key").
- DDownload premium download via the `download2` form instead of a plain GET.
- Missing NZB segments no longer abort the import; gaps are filled and repaired via PAR2.
- yEnc name mismatches between segments are only warnings now.
