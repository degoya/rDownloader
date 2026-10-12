#!/usr/bin/env bash
#
# Closes a wave's jobs after its integration branch was merged into `development` (RD-1270-14):
# the status `Implemented`, the evidence line of the wave, and every box still open marked
# "→ offen in RD-1120-24" and copied, word for word, into a section of its own there with a link
# into the archive. After 1.26 the coordinator wrote the same paragraph twelve times by hand
# (9065f0ab0, +251 lines); the numbers come from the run itself, not from memory.
#
# Read, never written: the integration's log directory (`integrate.sh`'s
# /tmp/claude-<uid>/<branch with / → ->/) — `status`, the closing line, the recorded green and the
# Summary lines of `check.log`, the run id in `public-ci.log` — and the GitHub greens in
# `.git/rd-verified-ci` (lib/verified.sh, rd_ci_covering: the commit's tree or one that differs
# from it only in documentation and version lines). It refuses (exit 1, nothing written) a run
# that is not green: `full` not 0, `windows` or `public-ci` neither 0 nor skipped, `check.log`
# without `==> all requested checks passed`, or no recorded Linux and Windows green for the tree
# — a red GitHub run holds the merge (RD-140-23). The generators' commit is named when the
# integration has one (`chore(generated): regenerate after merging …` naming one of the jobs).
#
# What only a person knows stays the coordinator's, and comes first: ticking a box the run
# proves, with its evidence, and splitting a box whose part stays open — what is still open when
# the script runs goes to RD-1120-24 as it stands (and an existing section is not rewritten). A
# `<job-id>=<note>` ends that job's evidence line with the note (which tests carry it).
#
# Idempotent: the status is set, a box already marked is not marked again, an evidence line for
# the same branch and commit and a "### Aus RD-…" section that exists are left as they are, and a
# file is only rewritten when it changes — a second run writes nothing, also after
# `archive-jobs.sh` moved the jobs (they are looked up in docs/roadmap/jobs/ and archive/).
# Next: commit, then `scripts/archive-jobs.sh` moves the Implemented jobs, commit again.
#
# Usage:
#   scripts/job-evidence.sh <integration-logdir> <job-id>[=<note>]...
#   scripts/job-evidence.sh /tmp/claude-1000/integration-1.26-w1 RD-1260-01='der Test in `about_page.rs` ist darin' 1260-02
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/verified.sh
source "$ROOT/scripts/lib/verified.sh"

JOBS="$ROOT/docs/roadmap/jobs"
LIVE_ID="RD-1120-24"
MARK="→ offen in $LIVE_ID"

die() {
    echo "job-evidence: $*" >&2
    exit 1
}

usage() {
    sed -n '/^# Usage:/,/^#$/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

[[ $# -ge 2 ]] || usage
logs="${1%/}"
shift
[[ -d "$logs" ]] || die "no log directory $logs"
for file in status check.log; do
    [[ -f "$logs/$file" ]] || die "$logs/$file is missing — is the integration run finished?"
done

# --- the run ---------------------------------------------------------------------------------

# The value of key $1 in the status file (`full=0 windows=0 prune=0 public-ci=0`).
status_value() {
    awk -v key="$1" '{ for (i = 1; i <= NF; i++) { n = index($i, "="); if (substr($i, 1, n - 1) == key) print substr($i, n + 1) } }' "$logs/status"
}
full="$(status_value full)"
windows="$(status_value windows)"
public_ci="$(status_value public-ci)"
[[ "$full" == 0 ]] || die "status: full=${full:-missing} — only a green --full closes a job"
[[ "$windows" == 0 || "$windows" == skipped ]] || die "status: windows=${windows:-missing} is red"
[[ "$public_ci" == 0 || "$public_ci" == skipped || -z "$public_ci" ]] || die "status: public-ci=$public_ci is red — it holds the merge (RD-140-23)"
grep -qx '==> all requested checks passed' "$logs/check.log" || die "check.log lacks its closing line '==> all requested checks passed'"

# check.log: the commit of the recorded green, the tree of the --full green, then the counts in
# the order the stages ran, as the hand-written lines of 1.26 named them.
summary="$(awk '
    function add(key, value) {
        if (!(key in count)) { order[++keys] = key; count[key] = value } else count[key] = count[key] " + " value
    }
    /^==> recorded green at / { commit = $5; next }
    /^==> recorded a --full green for tree / { tree = $8; next }
    /^==> / {
        header = substr($0, 5)
        if (header == "pnpm run typecheck:full") typecheck = 1
        next
    }
    /^ *Summary \[.*tests? run: [0-9]+ passed/ {
        for (i = 1; i <= NF; i++) if ($i == "run:") { passed = $(i + 1); skipped = $(i + 3) }
        if (header ~ /^tests \(everything except rd-api\)/) { add("Rust", passed); rust_skipped += skipped }
        else if (header ~ /^tests \(rd-api library\)/) add("rd-api-Bibliothek", passed)
        else if (header ~ /^tests \(rd-api integration/) add("rd-api-Integration", passed)
        else if (header ~ /^crash and restart matrix/) add("Crash-Matrix", passed)
        else { key = header; sub(/ — .*/, "", key); add(key, passed) }
        next
    }
    /^ +Tests +[0-9]+ passed/ { vitest = $2; next }
    /^ℹ pass [0-9]+$/ && header ~ /browser extension/ { extension = $3; next }
    END {
        print commit
        print tree
        line = ""
        for (k = 1; k <= keys; k++) {
            key = order[k]
            text = key " " count[key]
            if (key == "Rust") text = "Rust " count[key] " bestanden" (rust_skipped > 0 ? " + " rust_skipped " übersprungen" : "")
            line = line (line == "" ? "" : "; ") text
        }
        if (vitest != "") line = line (line == "" ? "" : "; ") "Vitest " vitest
        if (extension != "") line = line (line == "" ? "" : "; ") "Erweiterung " extension
        if (typecheck) line = line (line == "" ? "" : "; ") "`typecheck:full`"
        print line
    }' "$logs/check.log")"
{ read -r commit; read -r short_tree; read -r counts; } <<< "$summary"
[[ -n "$commit" ]] || die "check.log records no green commit ('==> recorded green at …')"
[[ -n "$short_tree" ]] || die "check.log records no --full green ('==> recorded a --full green for tree …')"
[[ -n "$counts" ]] || die "check.log carries no Summary line"
tree="$(git -C "$ROOT" rev-parse --verify --quiet "$commit^{tree}")" ||
    die "commit $commit of check.log is not in $ROOT — run this where the integration was merged"
[[ "$tree" == "$short_tree"* ]] || die "the --full green is for tree $short_tree, commit $commit has tree $tree"

# The branch: the local branch whose name integrate.sh turned into the directory's.
branch=""
while read -r name; do
    if [[ "${name//\//-}" == "$(basename "$logs")" ]]; then branch="$name"; fi
done < <(git -C "$ROOT" for-each-ref --format='%(refname:short)' refs/heads)
if [[ -z "$branch" && -f "$logs/public-ci.log" ]]; then
    branch="$(sed -n 's/^==> \([^ ]*\) ([0-9a-f]*, tree .*/\1/p' "$logs/public-ci.log" | head -1)"
fi
branch="${branch:-$(basename "$logs")}"

# German enumeration: "a", "a und b", "a, b und c".
join_de() {
    local out="" i
    for ((i = 1; i <= $#; i++)); do
        if ((i == 1)); then out="${!i}"; elif ((i == $#)); then out+=" und ${!i}"; else out+=", ${!i}"; fi
    done
    printf '%s' "$out"
}

# GitHub: every runner image and release workflow with a recorded green covering the tree.
platforms=()
workflows=()
record="$(rd_ci_record_file "$ROOT")"
if [[ -f "$record" ]]; then
    while read -r image; do
        [[ -n "$image" && -n "$(rd_ci_covering "$ROOT" "$tree" "$image")" ]] || continue
        if [[ "$image" == *.yml ]]; then workflows+=("$image"); else platforms+=("$image"); fi
    done < <(awk '{ print $1 }' "$record" | sort -u |
        awk '/^ubuntu-/ { print 1, $0; next } /^windows-/ { print 2, $0; next } { print 3, $0 }' | sort | cut -d' ' -f2)
fi
if ! printf '%s\n' "${platforms[@]}" | grep -q '^ubuntu-' || ! printf '%s\n' "${platforms[@]}" | grep -q '^windows-'; then
    die "no GitHub green on Linux and Windows recorded for tree ${tree:0:12} in $record — a wave goes through scripts/public-ci.sh before its merge (RD-140-23)"
fi
run=""
if [[ -f "$logs/public-ci.log" ]]; then
    run="$(grep -o 'actions/runs/[0-9]*' "$logs/public-ci.log" | tail -1 | cut -d/ -f3 || true)"
fi

# The generators' commit of this integration: reachable from the green, naming one of the jobs.
numbers=()
for arg in "$@"; do
    id="${arg%%=*}"
    [[ "$id" =~ ^(RD-)?([0-9]+-[0-9]+)$ ]] || die "not a job id: $id (RD-1260-01 or 1260-01)"
    numbers+=("${BASH_REMATCH[2]}")
done
generators=""
while read -r hash subject; do
    for number in "${numbers[@]}"; do
        if [[ "$subject" == *"/$number"* ]]; then generators="$hash"; break 2; fi
    done
done < <(git -C "$ROOT" log -n 30 --abbrev=9 --format='%h %s' --grep='^chore(generated): regenerate after merging' "$commit")
generated=()
if [[ -n "$generators" ]]; then
    # Each generator once, in the order integrate.sh runs them.
    for generator in api-contract mcp-coverage web-declarations licenses archive-jobs; do
        while read -r path; do
            case "$generator:$path" in
                api-contract:web/openapi.json | api-contract:web/src/api/schema.d.ts | \
                    mcp-coverage:crates/rd-api/mcp-coverage.md | \
                    web-declarations:web/components.d.ts | web-declarations:web/auto-imports.d.ts | \
                    licenses:crates/rd-api/licenses/third-party.json | archive-jobs:docs/roadmap/jobs/*)
                    if [[ "$generator" == web-declarations ]]; then
                        generated+=(Web-Deklarationen)
                    else
                        generated+=("\`$generator.sh\`")
                    fi
                    break
                    ;;
            esac
        done < <(git -C "$ROOT" show --name-only --format= "$generators")
    done
fi

evidence="— Nachweis der Welle (Koordinator $(date -r "$logs/status" +%F), \`$branch\` @ $commit): \`scripts/check.sh --full\` grün ($counts)"
[[ "$windows" == 0 ]] && evidence+=", \`scripts/check.sh --windows\` grün"
evidence+=", GitHub CI${run:+ $run} grün auf $(join_de "${platforms[@]}")"
[[ ${#workflows[@]} -eq 0 ]] || evidence+=" (dazu $(join_de "${workflows[@]}"))"
if [[ ${#generated[@]} -gt 0 ]]; then
    evidence+="; Generatoren nach dem Merge ($generators: $(printf '%s, ' "${generated[@]}" | sed 's/, $//'))"
fi

# --- the files -------------------------------------------------------------------------------

live="$(compgen -G "$JOBS/${LIVE_ID#RD-}-*.md" || true)"
[[ -n "$live" && -f "$live" ]] || die "$LIVE_ID is not an open job in $JOBS"
grep -qx '## Live-Abnahmen' "$live" || die "$live has no '## Live-Abnahmen' section"

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
sections="$scratch/sections"
: > "$sections"
declare -A target=()

# The job file of number $1, open or archived; exactly one.
job_file() {
    local found=()
    mapfile -t found < <(compgen -G "$JOBS/$1-*.md"; compgen -G "$JOBS/archive/$1-*.md")
    [[ ${#found[@]} -eq 1 ]] || die "RD-$1: ${#found[@]} job files match $JOBS/{,archive/}$1-*.md"
    printf '%s\n' "${found[0]}"
}

for arg in "$@"; do
    id="${arg%%=*}"
    note=""
    [[ "$arg" == *=* ]] && note="${arg#*=}"
    number="${id#RD-}"
    [[ "RD-$number" != "$LIVE_ID" ]] || die "$LIVE_ID collects the open boxes; it is not closed by a wave"
    file="$(job_file "$number")"
    items="$scratch/$number.items"
    # Status, marks and the open boxes: a box is its `- [ ]` line and the indented lines below it;
    # in RD-1120-24 the box's own text is indented under it (six spaces, as there), a remark from
    # its first `  — ` line on stays as it is, and the mark is left out.
    awk -v job="RD-$number" -v mark="$MARK" -v items="$items" '
        function flush(   i, remark) {
            if (!in_item) return
            for (i = 1; i <= n; i++) print buf[i]
            if (open && !marked) print "  " mark
            if (open) {
                printf "- [ ] %s: %s\n", job, substr(buf[1], 7) > items
                remark = 0
                for (i = 2; i <= n; i++) {
                    if (buf[i] == "  " mark) continue
                    if (buf[i] ~ /^  — /) remark = 1
                    printf "%s%s\n", (remark ? "" : "    "), buf[i] > items
                }
            }
            in_item = 0
        }
        BEGIN { printf "" > items }
        {
            if (in_item) {
                if ($0 ~ /^  +[^ ]/) { buf[++n] = $0; if ($0 == "  " mark) marked = 1; next }
                flush()
            }
            if (status_more) { if ($0 ~ /^  +[^ ]/) next; status_more = 0 }
            if (!status_done && $0 ~ /^- \*\*Status:\*\*/) {
                print "- **Status:** Implemented"; status_done = 1; status_more = 1; next
            }
            if ($0 ~ /^- \[[ xX]\] /) {
                in_item = 1; n = 1; buf[1] = $0; open = ($0 ~ /^- \[ \] /); marked = 0; next
            }
            print
        }
        END { flush() }' "$file" > "$scratch/$number.md"
    if ! grep -F -- "\`$branch\` @ $commit):" "$scratch/$number.md" | grep -q '^— Nachweis der Welle ('; then
        line="$evidence${note:+; $note}."
        printf '\n%s\n' "$line" >> "$scratch/$number.md"
    fi
    target["$number"]="$file"
    if [[ -s "$items" ]] && ! grep -q "^### Aus RD-$number — " "$live"; then
        title="$(sed -n '1s/^# RD-[0-9]*-[0-9]* — //p' "$file")"
        {
            printf '### Aus RD-%s — %s\n\n' "$number" "${title:-$(head -1 "$file" | sed 's/^# //')}"
            printf '[Job](./archive/%s)\n\n' "$(basename "$file")"
            cat "$items"
            printf '\n'
        } >> "$sections"
    fi
done

# The new sections go at the end of "## Live-Abnahmen", before the heading that follows it.
cp "$live" "$scratch/live.md"
if [[ -s "$sections" ]]; then
    awk -v sections="$sections" '
        function insert(   line) { while ((getline line < sections) > 0) print line; done = 1 }
        $0 == "## Live-Abnahmen" { inside = 1; print; next }
        inside && !done && /^## / { insert() }
        { print }
        END { if (inside && !done) { print ""; insert() } }' "$live" > "$scratch/live.md"
fi

# Everything was read and built; only now is anything written, and only what changed.
write() {
    if cmp -s "$1" "$2"; then
        echo "    unchanged: ${2#"$ROOT"/}"
    else
        cat "$1" > "$2"
        echo "    written:   ${2#"$ROOT"/}"
    fi
}
echo "==> $branch @ $commit (tree ${tree:0:12}): --full, GitHub $(join_de "${platforms[@]}")${run:+ (run $run)}"
for number in "${numbers[@]}"; do
    write "$scratch/$number.md" "${target[$number]}"
done
write "$scratch/live.md" "$live"
echo "==> next: commit, then scripts/archive-jobs.sh moves the Implemented jobs (and recounts the index), commit again"
