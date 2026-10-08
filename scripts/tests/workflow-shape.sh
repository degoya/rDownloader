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
# - Every wasm-tools a workflow installs — the repository's and the SDK's — is the version
#   scripts/build-plugins.sh requires (RD-1110-08): it encodes the components, so another one
#   changes their bytes. The component cache keys and the documents that name it agree.
#
# - The GitHub cache stays under its 10 GB (RD-1120-07): every Swatinem/rust-cache step says
#   whether it saves (`save-if`), and the linker variables its key depends on are set in one place,
#   .github/actions/rust-tests-cache, not copied into the jobs that restore `rust-tests`.
# - The Scoop installer is fetched at a commit and checked against its SHA-256 (PIPE-06), with the
#   same pins in channels.yml and scripts/ci-platform-smoke.ps1; nothing runs get.scoop.sh.
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

# One wasm-tools: WASM_TOOLS_VERSION in scripts/lib/plugin-stamp.sh.
pinned="$(sed -n 's/^WASM_TOOLS_VERSION="\(.*\)"$/\1/p' "$ROOT/scripts/lib/plugin-stamp.sh")"
expect "scripts/lib/plugin-stamp.sh pins a wasm-tools version" "yes" "$([[ -n "$pinned" ]] && echo yes || echo no)"
installs=0
while IFS= read -r line; do
    installs=$((installs + 1))
    expect "${line%%:*} installs wasm-tools $pinned" "$pinned" "$(sed -n 's/.*tool: wasm-tools@\{0,1\}//p' <<< "${line#*:}")"
done < <(grep -H 'tool: wasm-tools' "$WORKFLOWS"/*.yml "$ROOT"/sdk/ci/*.yml | sed "s|^$ROOT/||")
expect "wasm-tools is installed somewhere to check" "yes" "$([[ "$installs" -gt 0 ]] && echo yes || echo no)"
while IFS= read -r line; do
    expect "${line%%:*}: the component cache key names wasm-tools $pinned" "wt$pinned" \
        "$(grep -o 'components-[0-9.]*-wt[0-9.]*-' <<< "$line" | sed 's/^components-[0-9.]*-//; s/-$//')"
done < <(grep -H 'key: components-' "$WORKFLOWS"/*.yml | sed "s|^$ROOT/||")
# And the documents that name the version, so a bump finds every one (the public export has no
# docs/, hence only the ones present).
documents=()
for document in AGENTS.md docs/development.md docs/architecture.md sdk/README.md; do
    [[ -f "$ROOT/$document" ]] && documents+=("$document")
done
while IFS= read -r line; do
    expect "${line%%:*} names wasm-tools $pinned" "$pinned" "${line##* }"
done < <(cd "$ROOT" && grep -oHE 'wasm-tools(`| --version)? [0-9]+\.[0-9]+\.[0-9]+' "${documents[@]}")

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

finish_tests "workflow-shape"
