# shellcheck shell=bash
# shellcheck disable=SC2154  # the globals are release-pipeline.sh's, which sources this file
#
# The release chain's steps from preflight to smoke: the checks, the version bump, the tests,
# the signed plugins, both packages and their verification (scripts/release-pipeline.sh, whose
# step table says why they run in this order).
#
# Expects from scripts/release-pipeline.sh, which sources it: VERSION, LOG, NONCE, RESUME,
# PRERELEASE, RELEASE_BRANCH, MAIN_BRANCH, JOBS, LANES, WINDOWS_LANE, GATE_REQUIRES and
# step_command, and the working directory at the checkout root.

step_preflight() {
    if [[ "$PRERELEASE" -eq 1 ]]; then
        echo "pre-release $VERSION on $RELEASE_BRANCH; $MAIN_BRANCH stays on the last stable release"
    else
        echo "release $VERSION from $RELEASE_BRANCH into $MAIN_BRANCH"
    fi
    echo "run nonce: $NONCE"

    local branch; branch="$(git rev-parse --abbrev-ref HEAD)"
    [[ "$branch" == "$RELEASE_BRANCH" ]] || {
        echo "on '$branch', but a release is cut from '$RELEASE_BRANCH'" >&2; return 1; }

    if [[ -n "$(git status --porcelain)" ]]; then
        echo "the working tree is dirty; a release starts from a committed state" >&2
        git status --short >&2
        return 1
    fi

    # A worktree links web/dist into the main checkout, and a build here would rewrite that one.
    if [[ -L web/dist ]]; then
        echo "web/dist is a symlink — this is a feature worktree." >&2
        echo "release from the main checkout instead." >&2
        return 1
    fi

    if git rev-parse -q --verify "refs/tags/v$VERSION" > /dev/null; then
        echo "tag v$VERSION already exists" >&2
        return 1
    fi

    local tool
    for tool in cargo cargo-nextest node pnpm python3 zip; do
        command -v "$tool" > /dev/null || { echo "missing tool: $tool" >&2; return 1; }
    done
    cargo xwin --version > /dev/null 2>&1 || { echo "missing: cargo xwin" >&2; return 1; }
    echo "tools: $(cargo --version), $(node --version), $(cargo xwin --version)"

    # Concurrency is the other way this machine dies. One heavy job at a time, always.
    local others
    others="$(pgrep -c -f 'cargo (build|test|clippy|nextest|xwin)' || true)"
    if [[ "${others:-0}" -gt 0 ]]; then
        echo "$others cargo job(s) already running; refusing to pile on" >&2
        pgrep -a -f 'cargo (build|test|clippy|nextest|xwin)' >&2 || true
        return 1
    fi

    # A release never starts from a deferred state. scripts/check.sh --defer postpones a
    # triviality to the next ordinary run; the postponement expires here at the latest, and the
    # message names how many commits have never been through a green run.
    rd_verified_gate "$ROOT" "$RELEASE_BRANCH" || return 1

    # Everything that compiles nothing (RD-1120-06, audit A2): the script tests read AGENTS.md and
    # docs/development.md, and since documentation and .github/readme/ no longer cost a --full
    # (rd_inert_path), this is where a documentation change that breaks one is found. No lock of
    # its own, minutes. The job layout is the archive-jobs step's, after the docs gate.
    RD_SKIP_JOB_LAYOUT=1 scripts/check.sh --preflight \
        || { echo "the preflight is red; the release does not start (every finding above)" >&2; return 1; }

    echo "preflight ok at $(git rev-parse --short HEAD)"
}

# The breaking-change gate over web/openapi.json and the plugin WIT against the last release tag.
# A break passes when scripts/compat-breaks.toml acknowledges it for this release, or, for the
# WIT, under a large enough bump of rdownloader:plugin@X.Y.Z. Its summary line is the evidence.
step_compat() { scripts/compat-check.sh; }

step_version_bump() {
    scripts/set-version.sh "$VERSION"
    local reported; reported="$(scripts/set-version.sh)"
    [[ "$reported" == "$VERSION" ]] || { echo "version is $reported after the bump" >&2; return 1; }
    echo "workspace, web/package.json and Cargo.lock report $reported"
}

# The full Rust suite, through check.sh because it owns the one thing a naive `cargo test
# --workspace` gets fatally wrong here: rd-api's integration binaries each link the whole
# dependency graph, and building all 55 of them at once OOM-killed WSL even at JOBS=2. check.sh
# runs them four binaries at a time (six since RD-150-10). This also covers fmt, the failpoint crash matrix and sqlx offline.
# --full, because a branch-level run leaves out what a release must not (RD-120-58), and because
# tag-release.sh and package-windows.sh refuse a tree without a --full green of both halves.
#
# Not twice (RD-140-06): when a --full Rust green covers HEAD's tree — the state before the bump,
# which is committed only later — and the bump changed nothing but version lines, that green is
# this step's evidence (rd_prebump_reuse). Anything more than the version strings, and the full
# run happens as before.
step_test() {
    rd_prebump_reuse rust "full Rust run" && return 0
    JOBS="$JOBS" scripts/check.sh --rust --full
}

# Whether a green of half $1 covers the tree before the bump while the bump changed version lines
# only (rd_prebump_green). If so it names the green as the evidence of step $2, carries it to the
# bumped tree — so the gates of tag-release.sh and package-windows.sh find it — and succeeds;
# otherwise it fails and says nothing.
rd_prebump_reuse() {
    local half="$1" what="$2" green bumped
    green="$(rd_prebump_green "$ROOT" "$half")"
    [[ -n "$green" ]] || return 1
    bumped="$(rd_worktree_tree "$ROOT")"
    echo "the $what is not repeated: a green of half '$half' covers the tree before the bump"
    echo "  green:   $half half of tree $green (HEAD $(git rev-parse --short HEAD)^{tree} $(git rev-parse 'HEAD^{tree}'))"
    echo "  record:  $(rd_full_marker "$ROOT")"
    echo "  bump:    version lines only, in:"
    { git diff --name-only HEAD; git ls-files --others --exclude-standard; } | sed '/^$/d; s/^/           /'
    rd_record_full "$ROOT" "$half" "$bumped"
    echo "  carried: $half half recorded for the bumped tree $bumped"
}

# Deliberately the full workspace, deliberately alone, deliberately at 2 jobs. AGENTS.md calls
# this the run that has needed a hard restart; nothing else is running beside it here. Not again
# after a bump of version lines only when a `clippy` green — the integration gate's, or
# check.sh --clippy-all's — covers the tree before it (RD-1120-06, audit A5); its own green is
# recorded as that half.
step_clippy() {
    rd_prebump_reuse clippy "workspace clippy" && return 0
    local tree; tree="$(rd_worktree_tree "$ROOT")"
    CARGO_BUILD_JOBS=2 cargo clippy --workspace --all-targets --all-features -j 2 -- -D warnings || return
    [[ -z "$tree" ]] || rd_record_full "$ROOT" clippy "$tree"
}

# typecheck, vitest, the production web build and the browser extensions. After a bump of
# version lines only, with a web green of the tree before it (audit A5), typecheck and vitest are
# not repeated; the build and the extensions still are, because they carry the new version and
# the package steps refuse a web/dist older than web/package.json (scripts/web-dist-stale.sh).
# The green is carried only after both built; anything that fails falls back to the whole step.
step_web() {
    if [[ -n "$(rd_prebump_green "$ROOT" web)" ]] \
        && pnpm --dir web run build && scripts/build-extension.sh --skip-tests \
        && rd_prebump_reuse web "typecheck and vitest"; then
        return 0
    fi
    JOBS="$JOBS" scripts/check.sh --web --full
}

step_sign_plugins() {
    # The script refuses a plugin that changed under a signed version and exits 1 after
    # packaging the rest; without the check here, the older package of that version still sat in
    # dist/plugins/, the count below matched, and 1.2.2 shipped two stale packages as green.
    scripts/build-plugins.sh || { echo "signing refused at least one plugin" >&2; return 1; }
    local signed; signed="$(ls -1 dist/plugins/*.rdplug 2>/dev/null | wc -l)"
    local expected; expected="$(scripts/build-plugins.sh --list-packageable | wc -l)"
    echo "signed $signed of $expected packageable plugins"
    [[ "$signed" -eq "$expected" ]] || { echo "signing is short of a plugin" >&2; return 1; }
}

# The web/dist both packages embed, current by scripts/web-dist-stale.sh before either package step
# starts (RD-1130-01). The web step's build is not enough: on a --resume a green web step does not
# run again, a covered `check.sh --web --full` builds nothing, and a version bump or a checkout
# between main and development rewrites web/package.json after the build. The 1.12.0 chain
# stopped there twice. Rebuilt here, once and before the two packages start side by side; the web
# step's typecheck and vitest are not repeated, they judged the sources and not the bundle.
# Nothing to do when both package steps are already green in a resumed run.
rd_release_web_dist() {
    if [[ "$RESUME" -eq 1 ]] && step_is_green build-linux && step_is_green build-windows; then
        return 0
    fi
    scripts/web-dist-stale.sh && return 0
    echo "==> rebuilding web/dist before the packages embed it; the web step's checks stand"
    pnpm --dir web run build || return
    scripts/web-dist-stale.sh
}

# --skip-web reuses the web/dist the web step built and type-checked (check.sh --web --full), made
# current by rd_release_web_dist; web-dist-stale.sh still refuses one that is behind. Needed since
# RD-140-06 rather than merely faster: building web/dist here while the Windows package embeds it
# would race.
#
# `--profile release` is spelled out in both package steps (RD-150-20): a RD_PACKAGE_PROFILE
# left in the environment for a test package must never build the release.
step_build_linux() { JOBS="$JOBS" scripts/package-linux.sh --skip-web --profile release; }

# cargo xwin, straight from WSL. Not the Docker cross-build: it is slower, and the artifact stage
# drops the COPY'd asset directories. --skip-web reuses the web/dist rd_release_web_dist left.
#
# With a second lane it runs beside build-linux: it gives up the chain's lock (RD_LOCK_HELD) so
# package-windows.sh takes a lane of its own, and builds in WINDOWS_LANE instead of the shared
# target, where the Linux package is building. Its green records are still read from the shared
# target — RD_LANE_TARGET_DIR moves the build output only.
step_build_windows() {
    if [[ "$LANES" -gt 1 ]]; then
        env -u RD_LOCK_HELD -u RD_LOCK_LANE RD_LANE_TARGET_DIR="$WINDOWS_LANE" JOBS="$JOBS" \
            scripts/package-windows.sh --skip-web --profile release
    else
        JOBS="$JOBS" scripts/package-windows.sh --skip-web --profile release
    fi
}

step_verify_artifacts() {
    local absent=0 path
    for path in artifacts/linux/rdownloader artifacts/linux/rdownloader-capture \
                artifacts/windows/rdownloader.exe artifacts/windows/rdownloader-capture.exe \
                artifacts/rdownloader-linux-x86_64.tar.gz artifacts/rdownloader-windows-x86_64.zip \
                artifacts/rdownloader-site-rules.json; do
        if [[ -s "$path" ]]; then
            echo "ok   $path ($(stat -c %s "$path") bytes)"
        else
            echo "MISS $path" >&2; absent=1
        fi
    done
    # The packaged plugins must be the signed ones, and every package must carry the full set.
    local linux_plugins windows_plugins expected
    expected="$(scripts/build-plugins.sh --list-packageable | wc -l)"
    linux_plugins="$(ls -1 artifacts/linux/plugins/*.rdplug 2>/dev/null | wc -l)"
    windows_plugins="$(ls -1 artifacts/windows/plugins/*.rdplug 2>/dev/null | wc -l)"
    echo "plugins: linux $linux_plugins, windows $windows_plugins, expected $expected"
    [[ "$linux_plugins" -eq "$expected" && "$windows_plugins" -eq "$expected" ]] || absent=1

    # The archives unpack as the published ones do: flat, the same files (RD-180-05).
    # shellcheck source=lib/archive-layout.sh
    source "$ROOT/scripts/lib/archive-layout.sh"
    rd_check_archive_layout artifacts/rdownloader-linux-x86_64.tar.gz linux || absent=1
    rd_check_archive_layout artifacts/rdownloader-windows-x86_64.zip windows || absent=1

    # Built with the release profile, not a test package's (RD-150-20).
    for path in artifacts/linux/VERSION.txt artifacts/windows/VERSION.txt; do
        if grep -qx 'profile  release' "$path" 2>/dev/null; then
            echo "ok   $path names the release profile"
        else
            echo "!! $path does not name the release profile" >&2; absent=1
        fi
    done

    local built; built="$(artifacts/linux/rdownloader --version 2>&1 || true)"
    echo "built binary reports: $built"
    grep -q "$VERSION" <<< "$built" || {
        echo "the built binary does not report $VERSION" >&2; absent=1; }

    [[ "$absent" -eq 0 ]]
}

# Launches the binary that was just built and talks to it over real HTTP, then drives the real UI.
step_smoke() { scripts/release-smoke.sh "$VERSION"; }
