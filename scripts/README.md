# Build and maintenance scripts

Use these instead of retyping the commands from `AGENTS.md`. They carry the flags this machine
needs — above all a capped job count, because unbounded `cargo` parallelism has exhausted memory
and taken WSL down more than once.

| Script | Purpose |
| --- | --- |
| `dev.sh` | Run the service locally (`--fresh`, `--unsigned`, `PORT=`) |
| `check.sh` | CI-parity checks: branch level by default, everything with `--full` (`--defer`, `--rust`, `--web`, `--clippy <crates>`, `--clippy-all`) |
| `package-linux.sh` | Linux release build → `artifacts/linux` + tarball, with `VERSION.txt` (version, commit, build time; in `release-pipeline.sh` `Release-Build X.Y.Z (Basis <sha>)` instead of `<sha>-dirty`); verifies the committed site-rule file with the new binary and puts it beside the tarball as `artifacts/rdownloader-site-rules.json` (RD-130-07) |
| `package-windows.sh` | Windows cross-build from WSL → `artifacts/windows` + zip, with `VERSION.txt`; refuses a tree without a `--full` green |
| `build-plugins.sh` | Build, sign and package the bundled plugins → `dist/plugins`; refuses changed content under a signed version; `--components-only [names]` builds and stamps for the tests, unsigned |
| `check-plugin-imports.sh` | Verify a built component imports nothing outside `rdownloader:plugin` |
| `check-capture-linux-tree.sh` | Hold the resolved Linux dependency tree of `rd-capture` against the window stacks |
| `web-dist-stale.sh` | Is `web/dist` current? Exit 0 yes, 1 missing or behind a source |
| `docker.sh` | Build and run the container image (`build`, `run --port N`, `stop`) |
| `set-version.sh` | Read or set the release version (`Cargo.toml`, `web/package.json`, `extension/manifest.base.json`, lock) |
| `tag-release.sh` | Annotated `vX.Y.Z` tag for the current commit; refuses a tree without a `--full` green; never pushes |
| `release.sh` | The whole chain in order: version, checks, packages |
| `release-pipeline.sh` | The same chain run to the tag, with an evidence log that gates it |
| `release-smoke.sh` | Start the built binary and check the real API and UI |
| `export-public.sh` | Export a release tag to the public repository as one fresh commit, minus `public-exclude.txt` (all of `docs/` among it), refusing a link from what stays into what is left out, after a gitleaks scan; pushes only with `--push` (`--branch <name>` for an unreleased export) |
| `export-wiki.sh` | Convert the private user wiki to GitHub-wiki form and commit it as "Handbook for <version>" into the public wiki's clone; pushes only with `--push`. Leaves out a page whose first line is `<!-- private page -->` (and its sidebar entry) and every section between `<!-- private -->` and `<!-- /private -->` lines; refuses a public link to either, a link into a path `public-exclude.txt` names, an unbalanced marker and marker text anywhere else |
| `update-website.sh` | Set the website's `app/data/release.json` (`~/projects/rdownloader-website`) to a release — version, today's date; tag, asset, image and wiki links derive from it — check its wiki links against the public wiki clone, run its tests and `pnpm run generate`, commit "Release <version>" on its `main`; pushes only with `--push`. Never deploys: the last line names the built `.output/public/`, which the owner uploads by hand |
| `api-contract.sh` | Regenerate `web/openapi.json` and the TS types (`--check` to verify) |
| `build-extension.sh` | Test, build and verify Chrome/Firefox → `artifacts/browser-extensions` (`--skip-tests`, `--test-only`) |
| `worktree.sh` | Create, check and finish a feature worktree without the symlink traps |
| `i18n-key.sh` | Add one translation key to all four catalogues at once |
| `migration-pin.sh` | Pin a new migration's checksum in `crates/rd-db/migrations.sha384` (appends only) |
| `mcp-coverage.sh` | Regenerate the MCP capability comparison in `crates/rd-api/mcp-coverage.md` (`--check` to verify) |
| `licenses.sh` | Regenerate the dependency licence list of the About page, `crates/rd-api/licenses/third-party.json`, after `Cargo.lock` or `web/package-lock.json` changed (`--check` to verify) |
| `archive-jobs.sh` | Move finished job files (`Implemented`, `Blocked/No-Go`, working files of tagged releases) into `docs/roadmap/jobs/archive/`, rewrite every link and path to them, move their index rows and recount (RD-140-19); a no-op when nothing is due; `--check` names what is due, and any open job lying in `archive/`, and exits 1 — `check.sh` runs it on every change; refuses uncommitted changes under `docs/roadmap/jobs/` |
| `measure-mega-login-fuel.sh` | Price a MEGA account sign-in in guest fuel (RD-120-11); a measurement, not a gate |

The Cargo-heavy scripts take their parallelism from `scripts/lib/jobs.sh` (RD-130-17), the one
place the defaults live. `JOBS=<n>` sets cargo's build jobs — memory, since each job is a
`rustc` or a link; the default is 4. `TEST_THREADS=<n>` sets nextest's `--test-threads` in
`check.sh` — CPU only, since building is over by then — and defaults to `JOBS`. Both must be
positive whole numbers; anything else stops the script before cargo sees it.

**The heavy scripts serialise themselves.** `check.sh`, `build-plugins.sh`, `package-linux.sh`,
`package-windows.sh`, `release.sh`, `release-pipeline.sh` and `api-contract.sh` re-run themselves
under `flock` on `/tmp/rd-build.lock` through `scripts/lib/lock.sh`, so "check whether another
build is running" is no longer anybody's job. A chain such as `release.sh` holds exactly one lock;
the nested calls see `RD_LOCK_HELD=1` and pass straight through. `RD_LOCK_FILE` picks another
lock, `RD_LOCK_WAIT` changes the two-hour ceiling (giving up exits 199 and says so, rather than
failing silently). Once it holds the lock a run also waits while `MemAvailable` in
`/proc/meminfo` is below `RD_MIN_FREE_MB` (default 6144; `0` switches the gate off), reporting
every 30 s and giving up with exit 198 after `RD_LOCK_WAIT` — the lock keeps these scripts
apart, the gate holds a run back against load from anywhere else. And `RD_NO_LOCK=1` runs without a lock at all — which also makes the `target/`
stamp in `check.sh` your own responsibility, because that marker only stamps on a checkout change
and it is the lock that stops two checkouts interleaving. Deliberately lock-free: the pure
queries `build-plugins.sh --list-stale`, `--list-missing`, `--list-unbumped`,
`--list-packageable` and `--source-hash`, `set-version.sh`, `worktree.sh check`, `release-pipeline.sh --plan`, and
`check.sh --defer`, which runs no cargo.

**Do not wrap a locking script in `flock` yourself — it deadlocks.** `rd_take_lock` re-executes
the script under `flock`, so `flock /tmp/rd-build.lock bash -c '… scripts/build-plugins.sh …'`
leaves the inner call waiting for a lock its own parent holds, up to `RD_LOCK_WAIT` (7200 s).
`RD_LOCK_HELD` prevents nesting *within* the family; it cannot see a wrapper that set no such
variable. The symptom is indistinguishable from a hung job — the lock is held and nothing is
compiling — so the diagnosis is worth writing down: `fuser -v /tmp/rd-build.lock` names the
holders, and **two `flock` processes on the same file** is the signature. Wrap only bare `cargo`
and `npm` commands; the scripts need no help.

## What a run checks, and what it does not

**Two levels (RD-120-58).** Without `--full`, `check.sh` runs at **branch level**. It derives the
change set once — everything since the OLDER of the branch point and the last green run of this
checkout — and runs what that demands: clippy (all targets) and every test of the touched crates
under `crates/`; the library and binary tests of **one level** of reverse dependencies; `rd-api
--lib`; and only the `rd-api` integration binaries the change needs, in batches of four. Which
ones is `scripts/lib/rd-api-tests.map`: a changed test file selects itself, a row per source
area selects the binaries whose routes that area serves, and a path under `crates/rd-api/`,
`crates/rd-core/` or a migration that no row matches selects **all of them** — where the mapping
is not clear the answer is the wide one. The run refuses a map row naming a missing binary and a
binary no row names, so a new test file needs its row. The crash matrix, sqlx, web and extension
keep their triggers: the matrix for `rd-core`, `rd-http`, `rd-scheduler`, `rd-usenet`,
`failpoint.rs` or `crates/rd-core/recovery-matrix.md`; sqlx for `rd-db` or a `.sql` file; web and
extension for `web/` and `extension/`. A change to the toolchain, nextest or deny config runs the workspace
and every `rd-api` batch. A change to the root `Cargo.toml` or `Cargo.lock` does so only when
`lib/lock-scope.py` cannot narrow it (a profile, a member, `[patch]`, anything outside
`[workspace.dependencies]`); otherwise the members whose resolved tree changed count as touched,
and the run names them with the reason (RD-130-17). `tests/lock-scope.sh` holds those rules
against fixtures and runs when `scripts/lib/` or `scripts/tests/` change, and under `--full`;
`tests/public-links.sh`, the export's link guard, and `tests/archive-jobs.sh`, the job archive on
a fixture repository, run beside it.
`cargo fmt`, the capture-tree check, the map check and the component checks always run: they
are seconds.

**`--full` runs everything** — the workspace, all fourteen `rd-api` batches, the crash matrix,
sqlx, the non-incremental typecheck, web and extension. It belongs at the end of a wave on
`development`, once, after the merges, and in the release chain (`release.sh` and
`release-pipeline.sh` call it). It records its green per half and by tree in
`<target>/.rd-verified-full/`, and `tag-release.sh` and `package-windows.sh` refuse a tree
without both halves, documentation changes excepted (`RD_UNVERIFIED_PACKAGE=1` builds a package
marked `UNVERIFIED.txt`, never one for the owner). A branch green is scoped and does not count.

**Every run ends with the time per stage, what it skipped and why,** and at branch level with the
reminder that `--full` is still due. Without that a branch green looks like a full one.

`--defer` is the deliberate postponement, for text, translations and appearance. It accepts only
`docs/` and `*.md`, `web/src/locales/**`, `web/src/assets/**`, and a `.vue` or `.css` change that
does not touch a `<script>` block; anything else is refused by name. It runs `git diff --check`,
the four-language locale parity test and the incremental typecheck as the diff demands — and it
does **not** record a green. That is the whole mechanism: the change set of the next ordinary run
is measured against the last recorded green, so the postponed commits come along by themselves,
and `worktree.sh finish` and the release preflight refuse a branch whose HEAD no green run has
seen. The record is one file per checkout under `<target>/.rd-verified/`, not one file for the
directory, because `target/` is shared between checkouts.

`RD_BASE=<branch>` compares against another base. `RD_NO_LOCK=1`, `RD_LOCK_WAIT` and
`RD_LOCK_FILE` are described above — and switching the lock off makes the `target/` stamp yours,
because `check.sh` only stamps when the checkout changes (`<target>/.rd-checkout`).

`npm run build` no longer type-checks. `typecheck` is incremental (`vue-tsc --build`),
`typecheck:full` is the non-incremental one that CI, `--full` and both packaging scripts run.

## The three traps worth knowing

`build-plugins.sh --list-stale` and `--list-missing` find the shared target on their own in a
feature worktree (RD-120-40): with `CARGO_TARGET_DIR` unset they take the main checkout's
`target/`, recognised through `git rev-parse --git-common-dir`, and export it so a build from the
worktree writes where the queries read. Before, a worktree without the variable reported all 72
components missing. A variable that is set still wins.

`api-contract.sh` exists because both halves of the contract are easy to get wrong silently: the
generator writes no trailing newline, and forgetting to regenerate the TypeScript types leaves
the frontend describing an API that no longer exists. Since 2026-09-23 it also stamps `crates/`
before building, because a third silent failure turned up: the worktrees share one
`CARGO_TARGET_DIR`, cargo gives the same workspace crate in two checkouts the same unit hash,
and the binary it ran had been built from a *different* worktree. The contract it wrote
described an API that checkout did not have -- three audit enum values from a branch none of
whose code was present -- and nothing about the output says so.

The stamping moved into `scripts/lib/lock.sh` the same day, once the same defect had been
fixed in two scripts one at a time and turned up in a third. `rd_take_lock` now stamps `crates/`
itself, so a script cannot be written without it; the marker keeps an unchanged checkout from
paying for it. It also stopped typechecking before it generates -- that typechecked the frontend
against the schema the run was about to replace, which deadlocks whenever a branch adds a route
and a view together.

It also stopped demanding a *current* `web/dist` that day. It needs a bundle only because
`rust-embed` reads one at compile time; the `openapi` subcommand serves no assets and
`npm run generate:api` is a pure transform over the JSON. Since a feature worktree cannot build
the frontend, insisting on freshness turned a branch that changed a route *and* a view into a
hard stop with advice it could not follow. A stale bundle is now used as it is, and only a
missing one is fatal.

`worktree.sh` exists because a fresh worktree has no `web/node_modules` and no `web/dist`, and
`rust-embed` will not compile without the latter. Symlinking them from the main checkout is the
fast fix, but the unplugin generators resolve *through* the link, so `npm run build` inside a
worktree rewrites the tracked `web/components.d.ts` and `web/auto-imports.d.ts` to point at the
other checkout. `finish` discards that damage before merging; `check` reports it on demand.

`i18n-key.sh` splits a dotted key into a nested group — `plugins.type.oauth` becomes
`plugins → type → oauth`, which is what `plugins.json` wants and what `server.json` must never
get. A backend code *is* one literal key: `translateServerMessage` looks up
`server.codes.<code>`, so a code added as `codes` plus a sibling `proxy: { deleted: … }`
resolves nowhere — in all four languages at once, which is precisely what the cross-language
parity test cannot see. Six proxy codes sat like that until
`web/src/i18n/sourceKeys.test.ts` was written to check the catalogues against the code that
produces the keys.

Since RD-120-24 a backslash escapes a dot, so a server code goes through the script like any
other key — quoted, so the shell keeps the backslash:

    scripts/i18n-key.sh server 'codes.collector\.check_no_resolver' Kein Keine Ninguno Aucun

Forget the backslash and the script refuses rather than building the group: it will not open a
subgroup inside one whose keys already carry dots, and it prints the escaped form to use. That
signature is deliberately narrow — only `server.codes` and `plugins.incompatible.reason` have
it; the 407 ordinary groups that merely hold strings still take new subgroups normally.

`migration-pin.sh` exists because sqlx checksums every byte of an applied migration and an
installation refuses to start once one changes (RD-120-41). The test
`crates/rd-db/tests/migration_checksums.rs` fails on a migration without a pin and names this
script with the file; without arguments it pins every migration that has none. It is its own
script rather than a flag on `check.sh` because it writes a tracked file, and `check.sh` only
ever verifies. It refuses a file that differs from its existing pin instead of rewriting the
pin — the way out there is a new migration. The file is plain `sha384sum` output, so
`(cd crates/rd-db/migrations && sha384sum -c ../migrations.sha384)` checks it without a build.

## What the scripts deliberately do not do

- **`check.sh` does not run `cargo clippy --workspace --all-targets --all-features` by default.**
  That single command has taken the machine into swap and required a hard restart. Branch level
  lints the touched crates by itself (`rd-api` only as far as its binaries are selected);
  `--clippy <crates>` names others, and `--clippy-all` runs the full sweep at `JOBS=2` when you
  really want it and nothing else is running.
- **The packaging scripts never touch `artifacts/*/vendor`.** Those are downloaded third-party
  helper binaries (`ffmpeg`, `yt-dlp`, `unrar`, …), not build output.
- **`docker.sh` exists because three things go wrong otherwise.** `dist/plugins` is gitignored,
  so a fresh clone builds an image with no bundled plugins at all; `cargo build` inside the
  image would use every core; and Docker Desktop writes a `credsStore` pointing at a Windows
  `.exe` that cannot run with WSL interop off, which makes every pull fail with
  `exec format error`. The script warns about the first and handles the other two.
- **`build-plugins.sh` takes each plugin's version from its own `manifest.toml`,** not from the
  workspace version. Installed jobs pin the plugin version they were resolved with, so
  re-packaging must not move a plugin to a new version as a side effect.
- **A packaged plugin replaces its own older packages.** The file name carries the plugin's
  version, so a bump writes a second file instead of overwriting the first, and the count check
  in `package-linux.sh` and `package-windows.sh` then aborts on the surplus — that is what
  `44 packaged, but 43 plugins have a manifest` meant. `build-plugins.sh` removes the superseded
  packages of the same plugin after the new one is written, so `dist/plugins` holds exactly one
  `.rdplug` per plugin. It removes them afterwards on purpose: a build or a packaging step that
  fails leaves the previous package in place.
- **The sweep skips a plugin whose `min_app_version` is newer than this build,** because
  `plugin package` refuses such a package outright. That is the normal state for a plugin
  written against the release being prepared: the workspace version only moves in the
  `chore(release)` commit. `build-plugins.sh --list-packageable` answers which plugins that
  leaves, and the two packaging scripts, CI and the release workflow all ask it rather than
  keeping their own copy of the rule — so it clears itself at the version bump. Naming a plugin
  explicitly still tries, and fails with the packager's own message.
- **`build-plugins.sh --list-stale` names the built components not built from these sources,**
  and `check.sh` asks it right after `cargo fmt` and refuses to go on. `cargo test` never builds
  components and `target/` is shared, so a merge or another worktree leaves a component beside
  sources it was not built from, and the contract tests then fail on behaviour fixed long ago.
  The tests say so themselves — `rd_plugin_host::artifact` is where the rule lives — but well
  into the run. Since RD-120-58 the question is asked by **content**: every build through this
  script writes `rd_plugin_<name>.wasm.src-sha256` beside the component, the hash of its sources
  (the plugin crate, its shared plugin libraries, the WIT — sorted paths and contents, no file
  times) and of the component itself. Stale means no stamp, a stamp for other bytes (a bare
  `cargo component build`, which writes none), or other sources. File times used to name every
  component after each checkout; a checkout now names nothing. `--components-only [names]`
  builds and stamps without signing — without names, whatever is stale or missing — and touches
  the sources of each component first, so cargo compiles it from this checkout rather than
  calling another checkout's fingerprint fresh. All selected components are built in **one**
  `cargo component build` with every `-p` under the same `-j` (RD-130-17), so its memory depends
  on `JOBS`, not on how many plugins are named; if that call fails, the plugins are built one
  at a time up to the first failure, which is named, and the run fails. `--source-hash <name>` prints the hash, and a
  unit test in `artifact.rs` holds the script and the Rust side to the same value.
- **`build-plugins.sh --list-missing` names the components that were never built here,** which
  `--list-stale` does not: a file that is not there has no content to compare. `check.sh` asks
  this first and refuses to go on, because until RD-108-16 a missing component was the quiet
  case — every contract test returned early and counted as passed, so a fresh worktree reported
  a full green suite without loading a single component. They fail now instead; a checkout
  without the wasm toolchain leaves them out with `cargo nextest run -P no-components`, which
  counts them as skipped.
- **Same version, same content (RD-120-47).** An installation only takes a bundled package whose
  version is newer than the installed one, so a plugin that changed and kept its version never
  arrives — six did on 2026-09-23, and the owner's instance ran their old code. A signed
  `build-plugins.sh` therefore refuses to package a plugin whose `<name>-<version>.rdplug`
  already exists with a different `manifest.toml`, `component.wasm` or locale file, names the
  plugin and the version to raise, packages the rest and fails at the end. It compares the
  members byte for byte (`unzip -p … | cmp`), never by re-signing, against the package it would
  replace and against the main checkout's `dist/plugins/` — a feature worktree's own is empty.
  The same content at the same version is a plain rebuild and passes, so the routine re-sign of
  everything does not trip it. `--development` is exempt: unsigned packages are never shipped.
  `build-plugins.sh --list-unbumped [name…]` asks the same question without building or
  signing, printing `<name> <version> <member> <package>`; `check.sh` asks it right after the
  staleness check, for the plugins the change touches (all of them for `--full`, a shared plugin
  library, the WIT contract, or a workspace crate the plugins link — `rd_plugin_linked_crates` in
  `lib/scope.sh` reads that set from the manifests' path dependencies: today `rd-core`,
  `rd-plugin-api` and `rd-provider-registry`). An absent `dist/plugins/` has nothing to compare against and is
  not an error; `RD_PLUGIN_PACKAGES` points the query elsewhere, which
  `crates/rdownloader/tests/plugin_version_guard.rs` uses. The honest limit: `target/` is shared
  between worktrees, so a component another checkout built can differ for that reason alone —
  rebuild it from here before raising a version for a plugin you never touched.

## Order for a release

```bash
scripts/release.sh 0.9.3             # version, check.sh --full, both packages
# write CHANGELOG.md, docs/roadmap.md and the job files, then commit (documentation only,
# so the --full green still covers the tree; anything else needs another --full run)
scripts/tag-release.sh               # annotated tag on that commit
git push origin v0.9.3               # every release, not only some
# then the wiki: ~/projects/rdownloader.wiki, up to this release, from its CHANGELOG section
```

**The wiki is part of the tag, not of "later".** `~/projects/rdownloader.wiki` is the user
handbook — a separate GitLab repository, English, for people who use and operate rDownloader
rather than change it; `export-wiki.sh` publishes it as the GitHub wiki. Done at each tag, the release's own `CHANGELOG.md` section is the whole
input and it is a transcription. Skipped, it stops being one: in September 2026 the wiki had gone
five releases without an update, and catching up meant re-deriving each page from the sources and
checking every interface term against `web/src/locales/en/`, because several pages described
screens that no longer existed. It is published, so the push is the owner's decision.

**Every release gets a tag, and the tag gets pushed.** That is the rule from 1.0.5 onwards, decided
on 2026-09-08 after `v1.0.5` sat on one machine and `v0.9.2` had never left it at all — a release
nobody else can name is one nobody else can check out. `tag-release.sh` itself still never pushes,
because it cannot know whether the commit it tags is the one that ships; pushing stays the separate
step above.

The releases before that are **deliberately left untagged** and are not to be filled in:
`0.6.1, 0.7.0, 0.8.0, 0.9.0, 0.9.1, 0.9.3` … `0.9.8` have no tag, and picking a commit for each of
them now would be guesswork written down as fact. `CHANGELOG.md` remains the record for those.

`release.sh` runs the individual scripts in order; each still works on its own. It always writes
the browser-extension builds below `artifacts/browser-extensions`. Add `--plugins` when a plugin
changed — signing needs the release key, and re-packaging an unchanged plugin only produces a new
file. `--no-checks` skips the test run.

Three things stay by hand on purpose. The changelog, the roadmap and the job files are judgement
rather than mechanics. Committing is yours to review. And pushing a tag is the one action here
other people see, so it is never a side effect of building.

### The same release, unattended

`release-pipeline.sh` is `release.sh` carried through to the tag, for when nobody is watching the
output scroll past. It adds the part that matters when nobody is: proof.

```bash
scripts/release-pipeline.sh 0.9.3            # everything up to the tag
scripts/release-pipeline.sh 0.9.3 --resume   # continue after fixing a failed step
scripts/release-pipeline.sh 0.9.3 --plan     # print the steps and stop
```

Every step's raw output appends to `artifacts/release-evidence-<version>.log`, together with a
marker carrying the run's nonce, the step's exit status and how many bytes it wrote. Before the
tag, `evidence-gate` reads that log back and refuses unless every earlier step has a record from
*this* run that exited zero and actually produced output. Three failures it is built to catch:

- a step whose exit status was lost in a pipe — statuses come from `PIPESTATUS[0]`, and output is
  teed rather than filtered;
- a green read out of a log left behind by an earlier attempt — markers from another nonce do not
  count;
- a step that exited zero silently — no output is treated as missing evidence, not as a pass.

`public-ci` runs between `evidence-gate` and `tag`, and only with `--push`: it exports the merged
candidate to the branch `ci/<version>` of the public repository, waits up to 90 minutes
(`RD_PUBLIC_CI_TIMEOUT`) for GitHub's CI on that commit — Linux, Windows and macOS, which this
machine cannot run — and refuses the tag when it is red. On green it deletes the branch; on red
it keeps it for inspection, and the next run replaces or deletes it. While the branch exists
the candidate is public before it is a release; that is the price of the check. Needs `gh`,
signed in to github.com.

`publish-public` runs last and hands the tag to `export-public.sh`: the public repository at
`github.com/degoya/rDownloader` carries one commit per release and no history, built from `git
archive` of the tag minus the paths `scripts/public-exclude.txt` names — the whole developer
documentation under `docs/` and the working card for coding agents. What stays must not link into
what went: `lib/public-links.py` refuses a Markdown or HTML link into an excluded path and a GitHub
address of the repository pointing at one, and `tests/public-links.sh` holds it against its cases.
The tables tests read therefore sit beside their code (`crates/rd-core/recovery-matrix.md`,
`crates/rd-api/mcp-coverage.md`) and the README's screenshots under `.github/readme/`. gitleaks scans the exported tree before anything is
committed, with the known fixture findings allowlisted in `.gitleaks.toml`, and every
remaining reference to the internal planning is printed as a warning. The export is committed
and tagged in the local clone (`RD_PUBLIC_DIR`, `~/projects/rDownloader-public` by default) and
pushed only when the pipeline itself was given `--push`. The same step then runs
`export-wiki.sh`: the user handbook goes to the repository's GitHub wiki
(<https://github.com/degoya/rDownloader/wiki>) as one "Handbook for <version>" snapshot, converted
from the private wiki's GitLab form — `Home.md` and `_Sidebar.md`, page links by base name,
image paths relative to the wiki root — into a local clone at `~/projects/rDownloader-public.wiki`.
It refuses two pages with the same base name, any link to a page or file that does not exist, and
a link to the repository at a path `public-exclude.txt` names — `docs/` is not public, so what a
reader needs from it has a wiki page: *Building from source*, *Plugin reference*.
Private material stays in the one source, marked: a page whose first line is `<!-- private page -->`
is left out together with every sidebar or footer list item linking to it, and the lines from a
`<!-- private -->` line to the next `<!-- /private -->` line are removed. A public page linking to
a private page or to a heading of a removed section, an unclosed, nested or unopened marker, a page
marker below the first line and marker text anywhere else are refused, so no marker is published.
Last, `update-website.sh` brings rdownloader.net to the version: it sets the site's
`app/data/release.json`, from which the site derives every release link, runs the site's tests
and generator and commits "Release <version>" in the site's repository, pushed under the same
`--push` rule. It never deploys: its last line names the built `.output/public/`, which the owner
uploads to the web host by hand.

`commit-guard` stages the release and refuses any symlink, or any `node_modules/`, `dist/`,
`artifacts/`, `target/` or `.rdplug` path, before the commit exists and again on the merged tree.
It is the second line behind `.gitignore`, and it catches what a `git add -f` or an edited ignore
rule would let through.

`smoke` runs `release-smoke.sh`: the freshly built binary, a throwaway database on a free port,
the real API — including the check that a protected route still refuses an anonymous caller — and
then the real UI in Chromium, driven by the Playwright in the npx cache against the browsers in
`~/.cache/ms-playwright`.

`archive-jobs` runs `archive-jobs.sh --release <version>` right after `docs-gate` and before
`commit-guard`: the jobs this release finished move into `docs/roadmap/jobs/archive/`, with their
links and index rows, and `commit-guard`'s `git add -A` takes them into the release commit. The
working file of the release being cut counts as tagged, since the tag comes later. With nothing
due it says so and passes. `plugins/` is never rewritten — a plugin comment naming a moved job
keeps the old path rather than forcing a version bump and a rebuild.

The judgement stays where it was. `docs-gate` refuses a release whose changelog, README and
roadmap have not been brought up to date, but it will not write them. And the pipeline stops at
the tag: `--push` exists, and publishing remains a decision rather than a step.
