#!/usr/bin/env bash
#
# The shape of .github/workflows/ that nothing on GitHub checks before it matters (RD-1101-07):
#
# - No workflow over 500 lines (T16 of RD-191-09).
# - A called workflow (`on: workflow_call`, `uses: ./.github/workflows/…`) gets exactly the
#   secrets it reads: every one it reads is declared, every one it declares is passed by each
#   caller, and no caller passes one more. A missing mapping shows only as a channel that stays
#   "not set" at the next stable release; a beta tag never reaches the step that would tell.
# - The jobs scripts/ci-tree-greens.sh asks for by name — `rust` and the `ONCE` list of the
#   `gate` job — are jobs of ci.yml itself, not calls: a called workflow's check is named
#   "<caller> / <job>", and the gate would never find that tree green again.
# - Rust is set up only in .github/actions/setup-rust, pnpm only in setup-web (and repro.yml, for
#   the rebuilt tag's own pin) (RD-1280-15).
# - The tool versions a workflow installs (wasm-tools among them, RD-1110-08) are not checked
#   here: scripts/tool-versions.sh --check holds every copy to its one source (RD-1280-12).
# - The GitHub cache stays under its 10 GB (RD-1120-07): every Swatinem/rust-cache step says
#   whether it saves (`save-if`), only ci.yml's `rust` and `warm-cache` save `rust-tests`
#   (`save: true`), and the linker variables its key depends on are set in one place,
#   .github/actions/rust-tests-cache, not copied into the jobs that restore `rust-tests`.
# - e2e.yml's and recovery.yml's gates (RD-1270-11) ask for their own workflow's runs and match
#   jobs of their own: the matrix job `CI_TREE_GREENS_LEG`, the job names of `ONCE`.
# - The Scoop installer is fetched at a commit and checked against its SHA-256 (PIPE-06), with the
#   same pins in channels.yml and scripts/ci-platform-smoke.ps1; nothing runs get.scoop.sh.
# - channels.yml's `homebrew` job takes scripts/ci-brew-serve.sh's 3, the keychain refusal on
#   macOS, as expected only on the upgrade path (`--upgraded`, once), and no other end
#   (RD-1230-01).
#
# Pure bash and awk over the YAML as this repository writes it (two-space indents, a block `on:`).
# check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/workflow-shape.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
WORKFLOWS="$ROOT/.github/workflows"

for workflow in "$WORKFLOWS"/*.yml; do
    lines="$(wc -l < "$workflow")"
    expect "$(basename "$workflow") has at most 500 lines" "yes" "$([[ "$lines" -le 500 ]] && echo yes || echo "no, $lines")"
done

# The secrets a called workflow declares under `on: workflow_call: secrets:`, one per line.
declared_secrets() {
    awk '
        /^on:/ { on = 1; next }
        on && /^[^[:space:]#]/ { on = 0 }
        on && /^    secrets:/ { secrets = 1; next }
        secrets && /^      [A-Za-z_]+:/ { name = $1; sub(/:$/, "", name); print name; next }
        secrets && /^    [^[:space:]]/ { secrets = 0 }
    ' "$1" | sort
}

# The secrets a workflow reads, GITHUB_TOKEN aside.
used_secrets() {
    grep -oE 'secrets\.[A-Za-z_]+' "$1" | sed 's/^secrets\.//' | grep -vx GITHUB_TOKEN | sort -u || true
}

# `<job>\t<called workflow>` for every job of $1 that calls a local workflow.
calls() {
    awk '
        /^  [a-z0-9_-]+:$/ { job = $1; sub(/:$/, "", job); next }
        /^    uses: \.\/\.github\/workflows\// { path = $2; sub(/^\.\//, "", path); print job "\t" path }
    ' "$1"
}

# The secrets job $2 of workflow $1 passes, one per line.
passed_secrets() {
    awk -v job="$2" '
        $0 == "  " job ":" { inside = 1; next }
        inside && /^  [^[:space:]]/ { inside = 0 }
        inside && /^    secrets:/ { secrets = 1; next }
        inside && secrets && /^      [A-Za-z_]+:/ { name = $1; sub(/:$/, "", name); print name; next }
        inside && secrets && /^    [^[:space:]]/ { secrets = 0 }
    ' "$1" | sort
}

called=0
for workflow in "$WORKFLOWS"/*.yml; do
    while IFS=$'\t' read -r job path; do
        [[ -n "$job" ]] || continue
        called=$((called + 1))
        callee="$ROOT/$path"
        name="$(basename "$workflow") $job → $(basename "$callee")"
        expect "$name: the called workflow exists" "yes" "$([[ -f "$callee" ]] && echo yes || echo no)"
        [[ -f "$callee" ]] || continue
        expect "$name: called only, never started by an event of its own" "workflow_call" \
            "$(awk '/^on:/ { on = 1; next } on && /^[^[:space:]#]/ { on = 0 } on && /^  [a-z_]+:/ { sub(/:.*/, ""); sub(/^  /, ""); print }' "$callee" | tr '\n' ' ' | sed 's/ $//')"
        expect "$name: declares every secret it reads" "$(used_secrets "$callee" | paste -sd, -)" \
            "$(declared_secrets "$callee" | paste -sd, -)"
        expect "$name: passes every secret the called workflow declares" "$(declared_secrets "$callee" | paste -sd, -)" \
            "$(passed_secrets "$workflow" "$job" | paste -sd, -)"
    done < <(calls "$workflow")
done
expect "the repository has called workflows to check" "yes" "$([[ "$called" -gt 0 ]] && echo yes || echo no)"

# The names scripts/ci-tree-greens.sh matches (`rust (<image>)`, and each `ONCE` job or its
# matrix legs) are ci.yml's own jobs.
ci="$WORKFLOWS/ci.yml"
once="$(sed -n "s/^ *ONCE: '\(.*\)'$/\1/p" "$ci" | tr -d '[]"' | tr ',' ' ')"
expect "ci.yml's gate names the once-per-run jobs" "yes" "$([[ -n "$once" ]] && echo yes || echo no)"
for job in rust $once; do
    expect "ci.yml's \`$job\` is a job of its own, not a call" "own" \
        "$(awk -v job="$job" '
            $0 == "  " job ":" { found = 1; inside = 1; next }
            inside && /^  [^[:space:]]/ { inside = 0 }
            inside && /^    uses: / { called = 1 }
            END { print (found ? (called ? "called" : "own") : "missing") }
        ' "$ci")"
done

# The other workflows that gate a push to `main` on their own tree greens (RD-1270-11, e2e.yml and
# recovery.yml): the gate asks for its own workflow's runs, and the matrix job (`LEG`) and the
# named jobs (`ONCE`) it matches are that workflow's own jobs — a renamed job would leave the gate
# never finding a green, and every push to `main` running it all again.
gated=0
for workflow in "$WORKFLOWS"/*.yml; do
    asked="$(sed -n 's/^ *CI_TREE_GREENS_WORKFLOW: *//p' "$workflow")"
    [[ -n "$asked" ]] || continue
    gated=$((gated + 1))
    name="$(basename "$workflow")"
    expect "$name: its gate asks for its own runs" "$name" "$asked"
    leg="$(sed -n 's/^ *CI_TREE_GREENS_LEG: *//p' "$workflow")"
    if [[ -n "$leg" ]]; then
        expect "$name: the gate's matrix job \`$leg\` is a job of its own" "yes" \
            "$(grep -qx "  $leg:" "$workflow" && ! awk -v job="$leg" '
                $0 == "  " job ":" { inside = 1; next }
                inside && /^  [^[:space:]]/ { inside = 0 }
                inside && /^    uses: / { found = 1 }
                END { exit !found }
            ' "$workflow" && echo yes || echo no)"
    fi
    while IFS= read -r job; do
        [[ -n "$job" ]] || continue
        expect "$name: the gate's job \"$job\" is the name of a job of its own" "yes" \
            "$(grep -qxF "    name: $job" "$workflow" && echo yes || echo no)"
    done < <(sed -n "s/^ *ONCE: '\(.*\)'$/\1/p" "$workflow" | grep -o '"[^"]*"' | tr -d '"')
done
expect "e2e.yml and recovery.yml gate a push to main" "2" "$gated"

# The toolchain setups stand in their composite actions only (RD-191-09, RD-1280-15): Rust in
# setup-rust, pnpm in setup-web. repro.yml alone sets pnpm up itself, because the web/package.json
# that pins it is the rebuilt tag's, in a directory of its own.
expect "dtolnay/rust-toolchain only in .github/actions/setup-rust" ".github/actions/setup-rust/action.yml" \
    "$(cd "$ROOT" && grep -lE '^[[:space:]]*(- )?uses: dtolnay/rust-toolchain@' .github/workflows/*.yml .github/actions/*/action.yml | paste -sd' ' -)"
expect "pnpm/action-setup only in setup-web and repro.yml" ".github/actions/setup-web/action.yml .github/workflows/repro.yml" \
    "$(cd "$ROOT" && grep -lE '^[[:space:]]*(- )?uses: pnpm/action-setup@' .github/actions/*/action.yml .github/workflows/*.yml | paste -sd' ' -)"

# Every rust-cache step names `save-if` in its `with:` block: the action's default saves on every
# ref, and a cache nobody reads pushes main's out of the repository's 10 GB (RD-1120-07).
caches=0
# `<file>:<line>\t<save-if|no save-if>` for each rust-cache step: its own lines run up to the
# next step (`- `) or a line indented less than the step.
rust_cache_steps() {
    awk '
        function close_step() { if (inside) print where "\t" found; inside = 0 }
        FNR == 1 { close_step() }
        {
            text = $0; sub(/^ */, "", text)
            indent = length($0) - length(text)
        }
        inside && text != "" && text !~ /^#/ && (indent < step || (indent == step && text ~ /^- /)) { close_step() }
        inside && text ~ /^save-if:/ { found = "save-if" }
        text ~ /^- uses: Swatinem\/rust-cache@/ { inside = 1; step = indent; found = "no save-if"; where = FILENAME ":" FNR }
        END { close_step() }
    ' "$@"
}
while IFS=$'\t' read -r where found; do
    caches=$((caches + 1))
    expect "$where: the rust-cache step says whether it saves" "save-if" "$found"
done < <(cd "$ROOT" && rust_cache_steps .github/workflows/*.yml .github/actions/*/action.yml)
expect "there are rust-cache steps to check" "yes" "$([[ "$caches" -gt 0 ]] && echo yes || echo no)"
# `rust-tests` is saved by ci.yml's `rust` and its `warm-cache` alone; every other job reads it.
expect "only ci.yml and ci-warm-cache.yml save the rust-tests cache" \
    ".github/workflows/ci-warm-cache.yml .github/workflows/ci.yml" \
    "$(cd "$ROOT" && grep -rlE '^ +save: true' .github | sort | paste -sd' ')"
expect "the linker variables are set in .github/actions/rust-tests-cache alone" \
    ".github/actions/rust-tests-cache/action.yml" \
    "$(cd "$ROOT" && grep -rlE 'fuse-ld=mold|WINDOWS_MSVC_LINKER' .github | sort -u | paste -sd' ')"

expect "nothing runs the unpinned Scoop installer" "" \
    "$(cd "$ROOT" && grep -rlF 'https://get.scoop.sh' .github scripts --exclude=workflow-shape.sh || true)"
scoop_pin() { sed -n "s/^ *$1: *\([0-9a-f]*\)$/\1/p" "$WORKFLOWS/channels.yml"; }
scoop_pin_ps1() { sed -n "s/^\\\$$1 = '\([0-9a-f]*\)'$/\1/p" "$ROOT/scripts/ci-platform-smoke.ps1"; }
commit="$(scoop_pin SCOOP_INSTALLER_COMMIT)"
sha="$(scoop_pin SCOOP_INSTALLER_SHA256)"
expect "channels.yml pins the Scoop installer to a commit and a SHA-256" "40 64" "${#commit} ${#sha}"
expect "ci-platform-smoke.ps1 pins the same" "$commit $sha" \
    "$(scoop_pin_ps1 ScoopInstallerCommit) $(scoop_pin_ps1 ScoopInstallerSha256)"

expect "channels.yml starts the upgraded service once, with --upgraded" "1" \
    "$(grep -cF 'scripts/ci-brew-serve.sh "${log}" --upgraded' "$WORKFLOWS/channels.yml" || true)"
expect "and takes 0 and 3 from it, nothing else" "0 3" \
    "$(sed -n 's/^ *\(el\)\{0,1\}if \[\[ "${started}" == \([0-9]*\) \]\]; then$/\2/p' "$WORKFLOWS/channels.yml" | paste -sd' ')"
expect "every other start fails the step on any end but 0" "1" \
    "$(grep -cx ' *scripts/ci-brew-serve.sh "${log}"' "$WORKFLOWS/channels.yml" || true)"

finish_tests "workflow-shape"
