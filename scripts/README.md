# Build and maintenance scripts

Use these instead of retyping the commands from `AGENTS.md`. They carry the flags this machine
needs — above all a capped job count, because unbounded `cargo` parallelism has exhausted memory
and taken WSL down more than once.

| Script | Purpose |
| --- | --- |
| `dev.sh` | Run the service locally (`--fresh`, `--unsigned`, `PORT=`) |
| `check.sh` | CI-parity checks: branch level by default, everything with `--full` (`--defer`, `--rust`, `--web`, `--clippy <crates>`, `--clippy-all`); `--full` over content a `--full` green already covers up to documentation records the green and ends before the lock, `--again` runs it anyway (RD-160-06); `--windows` alone is the Windows lint, `cargo xwin clippy` over every crate with all targets and features (RD-140-23), its green kept by tree the same way (RD-160-06) |
| `integrate.sh` | Integrate a wave: integration worktree from `--base`, each branch merged (a conflict only in generated files takes our side), duplicate migration numbers and plugin ids after every merge, the generators once, unbumped components refused and stale ones built, then `check.sh --full` and `--windows` detached with logs, PID and status under `/tmp/claude-<uid>/<branch>/`, and after both are green `prune-target.sh --if-free` (RD-160-06); `--merge-only`, `--no-check`, `--no-windows`; a re-run skips what is merged (RD-140-22) |
| `public-ci.sh` | Run the public GitHub CI on a branch before its merge: export as `ci/<branch>`, `--platforms linux,windows` (all three without it) starts `ci.yml` for exactly those (the push itself carries `[skip ci]`), wait, delete the public branch on green, keep it and fail on red; greens are recorded per runner image and tree in `.git/rd-verified-ci`, and a platform green for the tree up to documentation and version lines is not dispatched again (RD-160-06); shares `lib/public-ci.sh` with the release's `public-ci` step (RD-140-22, RD-140-23) |
| `ci-log.sh` | Read a failed GitHub run: name the failed jobs, store each log without ANSI codes and timestamps under `/tmp/claude-<uid>/ci/`, print only the `FAIL`, `error[E…]`/`error:`, `panicked`, `failures` and `##[error]` lines with their line numbers; a run id, run or job URL, or `--job <id>` (RD-140-22) |
| `package-linux.sh` | Linux release build → `artifacts/linux` + tarball, with `VERSION.txt` (version, commit, build time; in `release-pipeline.sh` `Release-Build X.Y.Z (Basis <sha>)` instead of `<sha>-dirty`; `profile  release` or `profile  release-test (test package, …)`); verifies the committed site-rule file with the new binary and puts it beside the tarball as `artifacts/rdownloader-site-rules.json` (RD-130-07); `--profile release-test` (or `RD_PACKAGE_PROFILE`) builds a test package in the faster profile, default `release` (RD-150-20); the tarball is flat and holds what release.yml's does (`lib/archive-layout.sh`), `vendor/` stays in the folder (RD-180-05) |
| `package-windows.sh` | Windows cross-build from WSL → `artifacts/windows` + zip, with `VERSION.txt`; refuses a tree without a `--full` green; `--profile release-test` and the flat archive layout as for Linux |
| `package-deb-rpm.sh` | deb and rpm of one Linux architecture from its release tarball, with nfpm and `packaging/linux/nfpm.yaml.in`: `/usr/lib/rdownloader`, `/usr/bin` links, systemd user units, `install-kind` marker; `--render-only` writes the nfpm configurations (RD-180-05) |
| `package-msi.sh` | Per-user Windows MSI from the unpacked release zip, with WiX 5 and `packaging/msi/rdownloader.wxs` (Windows only; `--sources-only` writes the generated plugin fragment, licence page and `wix` arguments anywhere) (RD-180-05) |
| `update-schema-change.sh` | `<tag> [<repo>]` prints `true` or `false` for the update manifest's `schema_change`: whether `crates/rd-db/migrations/` differs from the previous plain tag (the previous beta for a beta); `true` whenever it cannot tell; `release.yml` passes it to `update manifest build --schema-change` (RD-180-02) |
| `self-update-smoke.sh` | The self-update with real binaries (RD-180-02): `<old> <old-version> <new> <new-version>`, the old one run as a portable installation and handed three updates the way the service does (backup through the local control token, journal, `apply-update` from a copy) — the new version (exit 0, with a signed-in web and a capture event stream held open across the stop, which must neither hold it up nor need the updater's stop by force), a program that ends at once and, with `RD_SMOKE_DEBUG_BUILD=1`, one declared unhealthy (both rolled back, exit 2); Linux and Git Bash on Windows; `.github/workflows/self-update.yml` runs it |
| `build-plugins.sh` | Build, sign and package the bundled plugins → `dist/plugins`, with the packager `rd-pack` in `release-test` rather than the service in `release`; refuses changed content under a signed version; `--components-only [names]` builds and stamps for the tests, unsigned; `--list-packageable` / `--list-examples` name the bundle and the examples, which are built but not bundled (RD-150-20) |
| `plugin-release-notes.sh` | The release notes of one plugin version from `CHANGELOG.md`: every entry that ends with `` `<plugin>` <version> `` (or a list of names before the version), as plain text within the index's 2000 characters; the release workflow hands them to `plugin index build --notes` (RD-160-09) |
| `check-plugin-imports.sh` | Verify a built component imports nothing outside `rdownloader:plugin` |
| `check-capture-linux-tree.sh` | Hold the resolved Linux dependency tree of `rd-capture` against the window stacks |
| `web-dist-stale.sh` | Is `web/dist` current? Exit 0 yes, 1 missing or behind a source |
| `docker.sh` | Build and run the container image (`build`, `run --port N`, `stop`) |
| `docker-smoke.sh` | Start an image on fresh volumes and check `--version`, `/api/v1/health` and apprise as the service user; CI and the release run it before any push (RD-140-25) |
| `set-version.sh` | Read, set or `--check` the release version: `Cargo.toml` is the source, `web/package.json`, `extension/manifest.base.json`, `web/openapi.json` (`info.version`) and the lock are its copies |
| `tag-release.sh` | Annotated `vX.Y.Z` tag for the current commit; refuses a tree without a `--full` green; never pushes |
| `release.sh` | The whole chain in order: version, checks, packages |
| `release-pipeline.sh` | The same chain run to the tag, with an evidence log that gates it; ends with the checkout back on `development` (RD-160-06) |
| `release-start.sh` | Start web build → `check.sh --full` → `release-pipeline.sh <version> [--push]` detached (a version is `X.Y.Z` or `X.Y.Z-beta.N`) (`setsid nohup`); log, PID and exit code under `/tmp/claude-<uid>/release-<version>/` (RD-140-06) |
| `prune-target.sh` | Keep `target/` small: per `deps` directory keep the newest hash variant per stem (`--keep N`), drop stale unhashed split debug info and every `incremental/`; never `target/wasm32-unknown-unknown/`; `--dry-run` says what would go, lock-free; `--if-free` prunes only what no build holds right now and never waits (RD-160-06); the lanes under `target/lanes/` each under their own lock, and with `--all` every worktree's own target (RD-140-06) |
| `release-smoke.sh` | Start the built binary and check the real API and UI |
| `e2e.sh` | End to end against a fresh service (RD-180-12): the Chrome extension in Playwright's Chromium, paired through its options page, sending a page from its popup (needs `127.0.0.1:8710` free), and the capture agent's `configure`, `run`, `rdownloader://` and Click'n'Load handover, each asserted in the LinkGrabber; `--browser`, `--capture`, `--bin-dir DIR` (default: the newest pair of `release-test`, `release`, `artifacts/linux`), `--build` (`release-test`, the bare cargo call under the target's lock); lock-free otherwise; the autostart step runs only in CI's `e2e.yml` (`docs/development.md#end-to-end-runs`) |
| `soak.sh` | A long run of a built binary (`--binary`, else `artifacts/linux/`, else `target/release/`; builds nothing) from a throwaway data directory under `--out` (default `artifacts/soak/<time>/`) against a local HTTP fixture with ranges that cuts every transfer off every two minutes: keeps a queue going, verifies every completed file byte for byte, samples memory, open files, threads, database size and throughput and fails naming each budget of `soak-budgets.toml` it exceeded; `--duration` (default 5m, at least 3m for the growth windows); `samples.csv`, `summary.json`, `server.log`; the nightly `.github/workflows/soak.yml` runs it for 2 h; `lib/soak.py evaluate` judges a samples file again (RD-180-12) |
| `export-public.sh` | Export a release tag to the public repository as one fresh commit, minus `public-exclude.txt` (all of `docs/` among it), refusing a link from what stays into what is left out, after a gitleaks scan; pushes only with `--push` (`--branch <name>` for an unreleased export, `--skip-push-ci` to keep its push from starting CI) |
| `export-wiki.sh` | Convert the private user wiki to GitHub-wiki form and commit it as "Handbook for <version>" into the public wiki's clone; pushes only with `--push`. Leaves out a page whose first line is `<!-- private page -->` (and its sidebar entry) and every section between `<!-- private -->` and `<!-- /private -->` lines; refuses a public link to either, a link into a path `public-exclude.txt` names, an unbalanced marker and marker text anywhere else |
| `update-website.sh` | Set the website's `app/data/release.json` (`~/projects/rdownloader-website`) to a release — version, today's date; tag, asset, image and wiki links derive from it — check its wiki links against the public wiki clone, run its tests and `pnpm run generate`, commit "Release <version>" on its `main`; pushes only with `--push`. Never deploys: the last line names the built `.output/public/`, which the owner uploads by hand |
| `api-contract.sh` | Regenerate `web/openapi.json` and the TS types (`--check` to verify) |
| `build-extension.sh` | Test, build and verify Chrome/Firefox → `artifacts/browser-extensions` (`--skip-tests`, `--test-only`) |
| `firefox-amo.sh` | `lint <dir>`: AMO's validator (`web-ext lint`, pinned) on the Firefox build, in CI (RD-160-07); `submit <dir>`: uploads it to the AMO listing for review (`--channel listed`, the review not waited for), nothing when AMO has that version already; credentials only from `AMO_JWT_ISSUER`/`AMO_JWT_SECRET`, without them or when AMO refuses a warning and exit 0 (RD-170-10) |
| `chrome-webstore.sh` | `upload <zip>`: the Chrome build as the Web Store item's draft; `publish <zip>`: uploaded and submitted for review — Web Store API v2, nothing when the store has that version already; credentials only from `CWS_CLIENT_ID`/`CWS_CLIENT_SECRET`/`CWS_REFRESH_TOKEN` and `CWS_PUBLISHER_ID`, none of them in a command line, without them or when the store refuses a warning and exit 0 (RD-170-10) |
| `package-managers.sh` | `<version> <SHA256SUMS> <outdir>`: the Homebrew formula and the Scoop manifest of a release from `packaging/homebrew/rdownloader.rb.in` and `packaging/scoop/rdownloader.json.in`, and the tap's and the bucket's README from the `README.md.in` beside each, with every archive's SHA-256 from the release's `SHA256SUMS`; `--repository OWNER/NAME` (tap and bucket are `<owner>/homebrew-rdownloader`, `<owner>/scoop-rdownloader`), `--base-url` for a local fixture; the winget manifests (`<outdir>/winget/`, RD-180-07) and the AUR repository (`<outdir>/aur/`: `PKGBUILD`, `.SRCINFO` from `SRCINFO.in`, the systemd user units; `pkgver` with `-` as `_`, RD-180-08) from `packaging/winget/` and `packaging/aur/`; the release workflow pushes or submits the result, `ci.yml` and `package-channels.yml` install it (RD-180-06) |
| `package-repo.sh` | `<incoming> <site>`: files a release's `.deb` and `.rpm` packages into the apt and dnf repository tree `<site>` (pool, per-architecture `Packages`, `Release`/`InRelease`/`Release.gpg`; rpms signed with `rpmsign`, `createrepo_c` metadata with `repomd.xml.asc`), keeps the newest `--keep` (2) versions per package and architecture, and renders `rdownloader.sources`, `rdownloader.repo`, the public key and the README from `packaging/repository/`; signs with the only secret key in `$GNUPGHOME` or `--key`; `--base-url` for the address, `--refresh` to sign again; the release workflow pushes the result to `<owner>/rdownloader-packages`, `packages-repo.yml` installs from it (RD-180-10) |
| `release-build-env.sh` | `--target TRIPLE [--release-tag vX.Y.Z] [--source DIR] [--cargo-home DIR]`: the environment a release binary is built in, as `KEY=VALUE` lines for `$GITHUB_ENV` — `SOURCE_DATE_EPOCH`, `RD_VERSION`, `RD_BUILD_TIME` and `RD_BUILD_COMMIT` from the commit (with `--release-tag` through `rd_build_stamp`: `Release-Build X.Y.Z (Basis <sha>)`, refused when the tag is not `Cargo.toml`'s version), and for a Linux target the checkout, cargo and rustup homes remapped through `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` and `CFLAGS_<triple>`, never `RUSTFLAGS`; release.yml's binary jobs and repro.yml's rebuilds both run it (RD-180-12, `docs/reproducible-builds.md`) |
| `worktree.sh` | Create, check and finish a feature worktree without the symlink traps; `new --own-target` gives it a check lane of its own (RD-140-06) |
| `i18n-key.sh` | Add one translation key to all four catalogues at once |
| `migration-pin.sh` | Pin a new migration's checksum in `crates/rd-db/migrations.sha384` (appends only) |
| `mcp-coverage.sh` | Regenerate the MCP capability comparison in `crates/rd-api/mcp-coverage.md` (`--check` to verify) |
| `licenses.sh` | Regenerate the dependency licence list of the About page, `crates/rd-api/licenses/third-party.json`, after `Cargo.lock` or `web/pnpm-lock.yaml` changed (`--check` to verify); the Rust part is `cargo tree` per shipped target, so it lists what a package contains; needs `pnpm install` in `web/` and asks the npm registry for the packages of other platforms |
| `archive-jobs.sh` | Move finished job files (`Implemented`, `Blocked/No-Go`, working files of tagged releases) into `docs/roadmap/jobs/archive/`, rewrite every link and path to them, move their index rows and recount (RD-140-19); with nothing due it only recounts a Job Inventory the catalogs contradict; `--check` names what is due, any open job lying in `archive/` and any miscounted inventory row, and exits 1 — `check.sh` runs it on every change (RD-140-24); refuses uncommitted changes under `docs/roadmap/jobs/` |
| `doc-facts.sh` | Write the release facts the documentation repeats — feature-list date and version, bundled-plugin count, MCP tool count, `rdownloader:plugin@X.Y.Z` — from their sources; `--check` writes nothing and exits 1 on a stale value, 2 on a reworded anchor; `--wiki DIR` includes the user wiki; the pipeline's `doc-facts` step and `docs-gate` run it (RD-140-24) |
| `wit-reference.sh` | Generate the plugin contract reference — every world, interface, function, record, variant and enum of `crates/rd-plugin-api/wit/rdownloader.wit` with its doc comments — into the user wiki's `plugins/plugin-reference.md` between `<!-- BEGIN wit-reference -->` and `<!-- END wit-reference -->` (`--wiki DIR`; with `--check` writes nothing and exits 1 when stale); `--print` writes it to stdout; `--check` alone reads the WIT strictly and exits 2 on a construct it does not know — CI runs that; job-id and ADR parentheses of the WIT's comments stay out of the page, a job id elsewhere in one is exit 2; the pipeline's `docs-gate` runs `--wiki --check` (RD-160-04) |
| `check-sdk-templates.sh` | Every world of the WIT has a template in `sdk/templates/<world>` building that world, with a manifest, the current contract copy, a `README.md` and a unit test, and no template lacks a world; CI's `components` job runs it (RD-160-04) |
| `check-actions-pinned.sh` | Every `uses:` in `.github/workflows/` and `.github/actions/` names a full 40-hex commit (`owner/repo@<sha> # vX.Y.Z`); local `./` actions pass, a `docker://` image only by digest; files as arguments check those instead; `check.sh` runs it every time (1.8) |
| `compat-check.sh` | The breaking-change gate of the public contracts: `web/openapi.json` and the plugin WIT against the highest `vX.Y.Z` tag not above the workspace version (`--base <ref>` for another); one line per break (`BREAK rest:path-removed:/api/v1/…`), exit 1 unless `compat-breaks.toml` acknowledges it for a later release or, for the WIT, the package version moved a major (before 1.0: a minor) step; the pipeline's `compat` step and CI's supply-chain job run it (RD-170-08) |
| `session-state.sh` | Print the repository's state for a new session: worktrees ahead/behind `development` and dirty, the last greens, running chains under `/tmp/claude-<uid>/*/pid`, tags against `origin`, open GitHub runs; `--brief` for a session's first look, `--no-network` skips the remote parts (RD-140-26) |
| `measure-mega-login-fuel.sh` | Price a MEGA account sign-in in guest fuel (RD-120-11); a measurement, not a gate |

The Cargo-heavy scripts take their parallelism from `scripts/lib/jobs.sh` (RD-130-17), the one
place the defaults live. `JOBS=<n>` sets cargo's build jobs — memory, since each job is a
`rustc` or a link; the default is 4. `TEST_THREADS=<n>` sets nextest's `--test-threads` in
`check.sh` — CPU only, since building is over by then — and defaults to twice `JOBS`, capped at
the machine's core count (RD-140-06). Both must be positive whole numbers; anything else stops the
script before cargo sees it.

**sccache, where it is installed.** Every script that takes the lock puts `sccache` in front of
rustc (`RUSTC_WRAPPER`) when `command -v sccache` finds it, with `CARGO_INCREMENTAL=0` — sccache
does not cache incremental output — and `SCCACHE_CACHE_SIZE` (default `40G`) as its ceiling. The
checkout stamp keeps its job: it makes cargo ask rustc again for this checkout's crates, and sccache
answers an unchanged file from the cache. A `RUSTC_WRAPPER` you set yourself is left alone;
`RD_NO_SCCACHE=1` switches it off. Nothing installs sccache.

**Build speed on GitHub (RD-150-10).** `ci.yml`'s `rust` job runs sccache over GitHub's cache
(`mozilla-actions/sccache-action`) beside `Swatinem/rust-cache`, and links with mold on Linux and
`rust-lld.exe` on Windows. The `components` job and the release's `plugins` job restore the
plugin components from `actions/cache` under the key `scripts/build-plugins.sh --cache-key`
prints, then build only what their stamps call stale or missing; the release still signs every
package. In the release, `binaries` compiles without waiting for `plugins`, a small `packages`
job lays the signed plugins into the archives, and the macOS Intel binary is cross-compiled on
the Apple Silicon runner. Locally, Linux links with rust-lld, Rust's default since 1.90; mold is
a per-machine choice in `~/.cargo/config.toml` (`docs/development.md`), because a repository
config cannot say "only where it is installed".

**The heavy scripts serialise themselves.** `check.sh`, `build-plugins.sh`, `package-linux.sh`,
`package-windows.sh`, `release.sh`, `release-pipeline.sh`, `api-contract.sh` and `prune-target.sh` re-run themselves
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
`--list-packageable` and `--source-hash`, `set-version.sh`, `worktree.sh check`, `release-pipeline.sh --plan`, `prune-target.sh --dry-run`, and
`check.sh --defer`, which runs no cargo.

**Lanes (RD-140-06).** A run takes two locks: first the one of the target directory it builds
in — `/tmp/rd-build.lock` for the main checkout's shared `target/`, so a checkout still on the
single-lock scripts of 1.3 waits for it too, and `/tmp/rd-build.lock.target-<checksum>` for any
other — then the first free of `RD_LANES` lanes (default 2, `/tmp/rd-build.lock.lane<i>`,
looked for every `RD_LANE_POLL` seconds). Everything that shares `target/` therefore still runs
one at a time, as the checkout stamp needs; a second lane is a target directory of its own:
a worktree made with `scripts/worktree.sh new --own-target <branch>` (`<worktree>/target`, its
`wasm32-unknown-unknown/` a link to the shared components, the flag in its git directory so it
goes with the worktree) or the release chain's Windows package (`target/lanes/windows`). A fresh
target is claimed without stamping. Two, because each lane builds with `JOBS=4` and eight
parallel rustc jobs is what 43 GB carry; `RD_MIN_FREE_MB` is checked per lane when it starts.
Opt-in, because one lane's debug working set is ~52 GiB and a wave has up to ten worktrees
whose agents do not compile (measurement: the job file of RD-140-06). A linked worktree finds its
target without `CARGO_TARGET_DIR` now (`scripts/lib/lanes.sh`); a value that is set still wins.
Every lock is held with `flock -o`, so nothing a run starts — the sccache server above all —
inherits the lock and holds it after the run. `RD_LOCK_NO_SLOT=1` takes the target's lock only,
for `prune-target.sh`, which compiles nothing.

**Do not wrap a locking script in `flock` yourself — it deadlocks.** `rd_take_lock` re-executes
the script under `flock`, so `flock /tmp/rd-build.lock bash -c '… scripts/build-plugins.sh …'`
leaves the inner call waiting for a lock its own parent holds, up to `RD_LOCK_WAIT` (7200 s).
`RD_LOCK_HELD` prevents nesting *within* the family; it cannot see a wrapper that set no such
variable. The symptom is indistinguishable from a hung job — the lock is held and nothing is
compiling — so the diagnosis is worth writing down: `fuser -v /tmp/rd-build.lock /tmp/rd-build.lock.*` names the
holders, and **two `flock` processes on the same file** is the signature. Wrap only bare `cargo`
and `pnpm` commands; the scripts need no help.

## What a run checks, and what it does not

**Two levels (RD-120-58).** Without `--full`, `check.sh` runs at **branch level**. It derives the
change set once — everything since the OLDER of the branch point and the last green run of this
checkout — and runs what that demands: clippy (all targets) and every test of the touched crates
under `crates/`; the library and binary tests of **one level** of reverse dependencies; `rd-api
--lib`; and only the `rd-api` integration suites the change needs. A suite is one module of the
six integration binaries, one per subject (`crates/rd-api/tests/<subject>/main.rs`, RD-150-10);
the run builds the binaries holding the selected suites, in batches of four, and with nextest
filters them to those suites (`-E 'test(/^(mfa|auth)::/)'`). Which suites is
`scripts/lib/rd-api-tests.map`: a changed suite file selects itself, a binary's `main.rs` its
suites, a row per source area selects the suites whose routes that area serves, and a path under
`crates/rd-api/`, one of its crates `crates/rd-api-*/` (RD-160-06), `crates/rd-core/` or a
migration that no row matches selects **all of them** — where the mapping is not clear the answer
is the wide one. The run refuses a map row naming a
missing suite, a suite no row names, a suite file its `main.rs` does not declare and a test file
directly under `crates/rd-api/tests/`, so a new suite needs its `mod` line and its row. The crash matrix, sqlx, web and extension
keep their triggers: the matrix for every crate that owns a crash point (the list in `check.sh`),
`failpoint.rs` or `crates/rd-core/recovery-matrix.md`; sqlx for `rd-db` or a `.sql` file; web and
extension for `web/` and `extension/`. A change to the toolchain, nextest or deny config runs the workspace
and every `rd-api` batch. A change to the root `Cargo.toml` or `Cargo.lock` does so only when
`lib/lock-scope.py` cannot narrow it (a profile, a member, `[patch]`, anything outside
`[workspace.dependencies]`); otherwise the members whose resolved tree changed count as touched,
and the run names them with the reason (RD-130-17). `tests/lock-scope.sh` holds those rules
against fixtures.

**The scripts check themselves** (RD-140-22). When anything under `scripts/` but its
documentation changes, and under `--full`, `check.sh` runs `bash -n` and `shellcheck
--severity=warning` over every tracked shell script (settings in `scripts/.shellcheckrc`; without
`shellcheck` installed it says so and skips it — `uv tool install shellcheck-py` provides one),
then every `scripts/tests/*.sh`. They need no build and take seconds together: the Cargo.lock
scope rules, the exports' link guard, the job archive, the scope boundary, and `worktree.sh`,
`i18n-key.sh`, `migration-pin.sh`, `set-version.sh`, the release pipeline's evidence gate and
`run_step`, `export-wiki.sh`'s private markers, `integrate.sh`'s merge half, `public-ci.sh` and
`ci-log.sh` against scratch repositories and a stub `gh`, the `rd-api` suite selection
(`rd-api-suites.sh`) on a scratch tree, the documentation's release facts
(`doc-facts.sh`) on a fixture tree, the contract reference and the template check
(`wit-reference.sh`) on a fixture contract, the breaking-change rules (`compat-check.sh`) on a
fixture contract, the plugin release notes (`plugin-release-notes.sh`) on a scratch changelog,
the package-manager files (`package-managers.sh`) on a fixture `SHA256SUMS`,
the release archive layout (`archive-layout.sh`) and the installer sources (`package-deb-rpm.sh`,
`package-msi.sh`, building the deb as well where nfpm is installed) on scratch archives,
the apt and dnf repositories (`package-repo.sh`) on fixture packages and a throwaway key,
the release build environment (`release-build-env.sh`) on a scratch repository,
the soak budgets (`soak.sh`) on recorded samples, the update manifest's schema-change flag
(`update-schema-change.sh`) on a scratch repository, the pinned actions
(`check-actions-pinned.sh`) on fixture workflows,
the session summary (`session-state.sh`) on a scratch
repository, and the scope boundary with the release's pre-bump green. A new test is a new
`scripts/tests/<name>.sh`, sharing the assertions in `scripts/tests/lib/expect.sh`; the loop
finds it by itself.
`cargo fmt`, the capture-tree check, the map check and the component checks always run: they
are seconds.

**`--full` runs everything** — the workspace, every `rd-api` suite, the crash matrix,
sqlx, web and extension. It belongs at the end of a wave on
`development`, once, after the merges, and in the release chain (`release.sh` and
`release-pipeline.sh` call it). It records its green per half and by tree in
`<target>/.rd-verified-full/`, and `tag-release.sh` and `package-windows.sh` refuse a tree
without both halves, documentation changes excepted (`RD_UNVERIFIED_PACKAGE=1` builds a package
marked `UNVERIFIED.txt`, never one for the owner). A branch green is scoped and does not count.
The record of every checkout on the target counts, since a tree is content (RD-160-06), and
`--full` applies the gate's rule to itself: for content a `--full` green already covers it records
the green for this tree and HEAD, says which green it relied on and ends before taking the lock.
`--full --again` runs it anyway.

**Every run ends with the time per stage, what it skipped and why,** and at branch level with the
reminder that `--full` is still due. Without that a branch green looks like a full one.

`--defer` is the deliberate postponement, for text, translations and appearance. It accepts only
`docs/` and `*.md`, `web/src/locales/**`, `web/src/assets/**`, and a `.vue` or `.css` change that
does not touch a `<script>` block; anything else is refused by name. It runs `git diff --check`,
the four-language locale parity test and the typecheck as the diff demands — and it
does **not** record a green. That is the whole mechanism: the change set of the next ordinary run
is measured against the last recorded green, so the postponed commits come along by themselves,
and `worktree.sh finish` and the release preflight refuse a branch whose HEAD no green run has
seen. The record is one file per checkout under `<target>/.rd-verified/`, not one file for the
directory, because `target/` is shared between checkouts.

**A follow-up round checks the delta** (RD-140-06). The boundary is normally the older of the
branch point and the last green; when the last green is a commit of this branch — past the branch
point and an ancestor of `HEAD` — it is the boundary itself, so a second round on an integration
branch checks what came after its green rather than everything since `development`. A green
`HEAD` does not contain (a rebase rewrote it) is ignored.

`RD_BASE=<branch>` compares against another base. `RD_NO_LOCK=1`, `RD_LOCK_WAIT` and
`RD_LOCK_FILE` are described above — and switching the lock off makes the `target/` stamp yours,
because `check.sh` only stamps when the checkout changes (`<target>/.rd-checkout`).

`pnpm run build` no longer type-checks. `typecheck` is incremental (`vue-tsc --build`),
`typecheck:full` is the non-incremental one that CI, both packaging scripts and every `check.sh`
run that touches `web/` use — `--defer` included (RD-150-22: the incremental one let two type
errors through to GitHub).

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
`pnpm run generate:api` is a pure transform over the JSON. Since a feature worktree does not build
the frontend, insisting on freshness turned a branch that changed a route *and* a view into a
hard stop with advice it could not follow. A stale bundle is now used as it is, and only a
missing one is fatal.

`worktree.sh` exists because a fresh worktree has no `web/node_modules` and no `web/dist`, and
`rust-embed` will not compile without the latter. `web/node_modules` is the worktree's own, a
`pnpm install --frozen-lockfile` from the shared store in a second or two (RD-150-14); until 1.5
it was a link to the main checkout's, and the unplugin generators resolved *through* it, so a
build inside a worktree rewrote the tracked `web/components.d.ts` and `web/auto-imports.d.ts` to
point at the other checkout. `web/dist` is still linked from the main checkout, the fast way to a
bundle `rust-embed` can compile; a build in the worktree needs the link removed first
(`rm web/dist`), or it writes into the main checkout's. `finish` discards a rewrite of the
declarations before merging; `check` reports it on demand.

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

- **No script builds the plugin repository index.** `release-pipeline.sh` signs the plugins
  locally, but the packages a release publishes are the ones the release workflow builds and
  signs, and the index names each by the digest of those bytes — an index built here would
  describe components that were never published. `.github/workflows/release.yml` builds, verifies
  and attaches `rdownloader-plugin-index.json` in its `plugins` job instead (RD-140-01), with
  each package's notes from `plugin-release-notes.sh` (RD-160-09); by hand
  it is `rdownloader plugin index build dist/plugins --out <file> --key <repository key>` (or the
  same with `rd-pack`), see
  `docs/plugins.md#plugin-repository-index`.
- **`check.sh` does not run `cargo clippy --workspace --all-targets --all-features` by default.**
  That single command has taken the machine into swap and required a hard restart. Branch level
  lints the touched crates by itself (`rd-api` only the binaries of its selected suites);
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
- **The examples are built, never bundled** (RD-150-20). `plugins/example-*` are left out of
  `--list-packageable` and of a signed sweep, and a signed run removes an example package left in
  `dist/plugins`; `--development` still packages them. `--list-examples` names them, and CI
  packages and conformance-checks them unsigned so they stay current.
- **The packager is `rd-pack`, built in `release-test`** (RD-150-20): the `plugin` commands of
  `rdownloader` with the same arguments, without `rd-api` and the queue, so signing no longer
  waits for a release build of the whole service. `release-test` rather than debug, because
  `plugin package` compiles each component with Wasmtime.
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
  `--cache-key` prints the component cache's key for CI and the release (RD-150-10): `deps=` over
  the registry half of `Cargo.lock`, the root `Cargo.toml` without the workspace version and
  `.cargo/config.toml`, and `sources=` over every plugin's source hash.
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
# 1. by hand, committed on development in the main checkout (preflight refuses a dirty tree):
#    CHANGELOG.md's `## [1.4.0] - <date>` section, docs/roadmap.md naming 1.4.0, README.md and
#    docs/feature-list.md where features moved, the job files' status and checkboxes
# 2. the wiki pass in ~/projects/rdownloader.wiki from that CHANGELOG section, committed there —
#    publish-public exports it, and docs-gate holds its contract, plugin count and contract
#    reference (scripts/wit-reference.sh --wiki) to the sources
scripts/release-start.sh 1.4.0              # detached: web build, check.sh --full (which ends
                                            # at once if a green covers the content), then
                                            # release-pipeline.sh 1.4.0
scripts/release-pipeline.sh 1.4.0 --resume  # after fixing the step that failed
scripts/release-start.sh 1.4.0 --push       # the same, plus the public CI run and the pushes
```

The chain writes the version (`set-version.sh`), the mechanical facts of the documentation
(`doc-facts.sh`), the job archive (`archive-jobs.sh`), the release commit, the merge into `main`
and the tag; it pushes nothing without `--push`. The steps are below.

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

`release.sh` is the older, manual chain — version, `check.sh --full`, both packages, no commit
and no tag — and still builds a package set on its own; `--plugins` signs the plugins as well,
`--no-checks` skips the test run. A release is cut with the pipeline.

The changelog, the roadmap and the job files stay by hand on purpose: they are judgement rather
than mechanics. What is mechanical — a version, a date, a count, a contract — is written by a
script, so it cannot be forgotten.

### The pipeline

`release-pipeline.sh` runs the release to the tag for when nobody is watching the output scroll
past, and adds the part that matters when nobody is: proof.

```bash
scripts/release-pipeline.sh 1.4.0            # everything up to the tag
scripts/release-pipeline.sh 1.4.0 --resume   # continue after fixing a failed step
scripts/release-pipeline.sh 1.4.0 --plan     # print the steps and stop
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

With two lanes (`RD_LANES`, default 2) `build-linux` and `build-windows` run at once, Windows in
`target/lanes/windows` under a lane of its own, both with `--skip-web` on the `web/dist` the
`web` step built. Each writes a part log (`<evidence log>.<step>.part`, named at the start for
`tail -f`); when both have ended the evidence log gets each in turn with the same header and
marker a serial step gets, so the gate cannot tell the difference. `RD_LANES=1` runs them one
after the other as before. The first parallel release cross-builds Windows from scratch in the
new directory; `target/x86_64-pc-windows-msvc/release/` in the shared target is then no longer
used by the chain.

`compat` runs `compat-check.sh` right after `preflight` (RD-170-08): a break of the REST API or
the plugin contract since the last release that `scripts/compat-breaks.toml` does not list for
this release, and a WIT break without the package version bump that versions it, stop the chain
before the hours of test and packaging. The acknowledgement is a committed decision with its
reason, so the fix is a commit and `--resume`, never a flag.

`test` does not repeat a full Rust run the tree already had (RD-140-06): when a `check.sh --full`
green of the Rust half is recorded for `HEAD`'s tree — the state before the bump, which is
committed later — and the bump changed nothing but lines carrying the old workspace version in
the version files, the step names that green in the log and carries it to the bumped tree. Any
other change, and `check.sh --rust --full` runs as before. `release-start.sh` runs that `--full`
first, detached; `check.sh` itself ends at once when a `--full` green already covers the content
up to documentation (RD-160-06).

From `merge-main` on the checkout stands on `main`, which `evidence-gate`, `public-ci` and `tag`
need; an exit trap switches it back to `development` when the chain ends, green or failed, and
says so, and `--resume` goes back onto `main` before continuing past `merge-main` (RD-160-06: on
2026-09-28 a checkout left on `main` took two commits by accident).

`public-ci` runs between `evidence-gate` and `tag`, and only with `--push`: it exports the merged
candidate to the branch `ci/<version>` of the public repository, waits for GitHub's CI on that
commit to end — Linux, Windows and macOS, which this machine cannot run — and refuses the tag
when it is red. Only the platforms no recorded green covers are dispatched (RD-160-06): the
candidate is its wave's integration tree plus a version bump and documentation, which counts as
the same content, so after the wave's Linux-and-Windows gate macOS runs alone, and with every
platform on record the step passes without a run and names the greens. There is no fixed
deadline for a run that is going (RD-150-10; every job carries
its own `timeout-minutes`): only a run that has not appeared after 15 minutes
(`RD_PUBLIC_CI_TIMEOUT`) or is still going after six hours (`RD_PUBLIC_CI_CEILING`) fails it. On green it deletes the branch; on red
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

A pre-release, `X.Y.Z-beta.N` and no other form (`lib/release-tag.sh`: `rd_release_version`,
`rd_is_prerelease`), runs the same chain without `merge-main`: the tag goes on the release commit
on `development`, `push` sends `development` and the tag only, and `publish-public` runs
`export-public.sh` alone. `export-wiki.sh` and `update-website.sh` refuse a beta outright, so the
handbook and the website wait for the stable release. `tests/release-evidence.sh` and
`tests/export-wiki.sh` hold both halves (`docs/development.md`, *Pre-releases (beta)*).

`commit-guard` stages the release and refuses any symlink, or any `node_modules/`, `dist/`,
`artifacts/`, `target/` or `.rdplug` path, before the commit exists and again on the merged tree.
It is the second line behind `.gitignore`, and it catches what a `git add -f` or an edited ignore
rule would let through.

`smoke` runs `release-smoke.sh`: the freshly built binary, a throwaway database on a free port,
the real API — including the check that a protected route still refuses an anonymous caller — and
then the real UI in Chromium, driven by the Playwright in the npx cache against the browsers in
`~/.cache/ms-playwright`.

`doc-facts` runs `doc-facts.sh` before `docs-gate` (RD-140-24): the feature list's date and
source version, the bundled-plugin count, the MCP tool count and the plugin contract
`rdownloader:plugin@X.Y.Z` in `README.md`, `docs/` and `sdk/README.md` are written from
`Cargo.toml`, `plugins/*/manifest.toml`, `TOOL_POLICY` in `crates/rd-api-mcp/src/policy.rs` and
the WIT package line. It comes after `test` because `test` carries a pre-bump green only over
a bump of version lines alone. `docs-gate` then runs `doc-facts.sh --check`, with the user wiki
(`RD_WIKI_SRC`, default `~/projects/rdownloader.wiki`) when it exists — the wiki is another
repository and is written in the wiki pass, not by the chain — and `wit-reference.sh --wiki
--check` beside it (RD-160-04): the plugin reference's generated contract part has to match the
WIT, so a contract change whose wiki pass skipped `wit-reference.sh --wiki` stops the release
here. Without a wiki it reads the WIT strictly, as CI does. Each fact sits behind an anchor, the
sentence around it; a reworded sentence is a refusal naming the file, never a silent pass.

`archive-jobs` runs `archive-jobs.sh --release <version>` right after `docs-gate` and before
`commit-guard`: the jobs this release finished move into `docs/roadmap/jobs/archive/`, with their
links and index rows, and `commit-guard`'s `git add -A` takes them into the release commit. The
working file of the release being cut counts as tagged, since the tag comes later. With nothing
due it says so and passes. `plugins/` is never rewritten — a plugin comment naming a moved job
keeps the old path rather than forcing a version bump and a rebuild.

The judgement stays where it was. `docs-gate` refuses a release whose changelog, README and
roadmap have not been brought up to date, but it will not write them. The changelog is compared
with the last shipped release, the highest `vX.Y.Z` tag under the version being cut
(`lib/release-tag.sh`, which `compat-check.sh` picks its base with) — not with `git describe`,
which on development answers an old tag because the release tags sit on main's merge commits. And the pipeline stops at
the tag: `--push` exists, and publishing remains a decision rather than a step.
