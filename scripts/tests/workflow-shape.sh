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

finish_tests "workflow-shape"
