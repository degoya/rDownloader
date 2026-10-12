#!/usr/bin/env bash
#
# scripts/screenshots/ without a service (RD-1270-16): the view list is well formed and the same
# for capture.mjs and deliver.py, the target manifest names only views, every alt text is free of
# numbers and alts.py writes them into a scratch wiki, deliver.py copies into scratch targets —
# refusing a wrong size, a taken wiki number and a view taken by hand before it writes anything —
# shots.sh's step loop ends a run at its first failed step (stand-in steps), and the tool names
# no machine path. `magick` is a stand-in that copies.
#
# node and python3, no browser and no build: it runs in a second or two.
#
#   scripts/tests/screenshots.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TOOL="$ROOT/scripts/screenshots"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

if ! command -v node > /dev/null; then
    fail "node is not installed: the screenshot tool's checks need it"
    finish_tests screenshots
fi

for file in "$TOOL"/*.mjs; do
    run_status node --check "$file"
    expect_status "node --check $(basename "$file")" 0
done
for file in "$TOOL"/*.py; do
    run_status python3 -c 'import ast, sys; ast.parse(open(sys.argv[1]).read())' "$file"
    expect_status "python parses $(basename "$file")" 0
done

# --- the view list -----------------------------------------------------------------------------
node "$TOOL/capture.mjs" --list | awk '{ print $2 }' > "$SCRATCH/node-views"
python3 -B -c 'import sys; sys.path.insert(0, sys.argv[1]); import deliver; print("\n".join(deliver.view_names()))' \
    "$TOOL" > "$SCRATCH/py-views"
expect_true "capture.mjs lists views" '[[ $(wc -l < "$SCRATCH/node-views") -gt 40 ]]'
expect_true "deliver.py reads the same views from views.mjs" 'cmp -s "$SCRATCH/node-views" "$SCRATCH/py-views"'
expect "every name is NN-words" "" "$(grep -Ev '^[0-9]{2}-[a-z0-9]+(-[a-z0-9]+)*$' "$SCRATCH/node-views" || true)"
expect "every number is one view's" "" "$(cut -d- -f1 "$SCRATCH/node-views" | sort | uniq -d)"

run_status node "$TOOL/capture.mjs" --list --views 05,19-logs,70
expect_status "--views takes names and numbers" 0
expect "and selects exactly those, in list order" \
    "05-downloads 19-logs 70-linkgrabber-indexer-drawer-held" "$(awk '{ printf "%s%s", sep, $2; sep=" " }' <<< "$output")"
run_status node "$TOOL/capture.mjs" --list --views 05,nope
expect_status "an unknown view is a usage error, not an empty run" 2
expect_output "naming it" "no view 'nope'"
run_status node "$TOOL/capture.mjs" --list --group main --views 65
expect "a view outside the group is not selected" "" "$output"
run_status env RD_SHOTS_WORK="$SCRATCH" RD_SHOTS_DEMO="$SCRATCH" node "$TOOL/capture.mjs" --views 05
expect_status "capturing needs a group" 2
run_status node "$TOOL/capture.mjs" --list --group usenet
expect "the usenet group holds the views that need a running Usenet download (RD-1270-24)" \
    "37-settings-usenet 72-downloads-usenet-connections 73-settings-usenet-nntp-connections" \
    "$(awk '{ printf "%s%s", sep, $2; sep=" " }' <<< "$output")"
run_status node "$TOOL/capture.mjs" --list --group main --views 37,72,73,74
expect "main takes the storage rule, not the Usenet views" "74-automation-storage-threshold" \
    "$(awk '{ print $2 }' <<< "$output")"
run_status node "$TOOL/capture.mjs" --list --group nope
expect_status "an unknown group is a usage error" 2
# shots.sh runs every group but the two before seeding in `capture`, and each as a step of its own.
groups="$(node --input-type=module -e "import { GROUPS } from '$TOOL/views.mjs'; console.log(GROUPS.filter(g => !['firstrun', 'wizard'].includes(g)).join(' '))")"
expect "shots.sh capture runs every seeded group, usenet last" "$groups" \
    "$(sed -n 's/^    for group in \(.*\); do$/\1/p' "$TOOL/shots.sh")"
for group in $groups; do
    expect_true "shots.sh has the step $group" "grep -q '^step_$group()' '$TOOL/shots.sh' && grep -q '|$group|' '$TOOL/shots.sh'"
done

# --- the step loop (steps.sh), its steps stand-ins that log their call; $FAIL names the red one --
steps() {
    run_status env FAIL="${FAIL:-}" bash -c '
        source "$1"; shift
        for name in fresh start seed capture deliver stop; do
            eval "step_$name() { echo $name; [[ \"\$FAIL\" != $name ]]; }"
        done
        run_steps "$@"' steps "$TOOL/steps.sh" "$@"
}
steps fresh start seed capture stop
expect_status "every step green" 0
expect "runs them in order" "fresh start seed capture stop" "$(grep -vE '^(skipped|stop|failed):' <<< "$output" | xargs)"
FAIL=fresh steps fresh start seed capture stop
expect_status "a refused fresh fails the run (RD-1270-24)" 1
expect "and nothing after it runs on the old data, stop neither: this run started nothing" "fresh" \
    "$(grep -vE '^(skipped|failed):' <<< "$output" | xargs)"
expect_output "naming what it skipped" "skipped: capture, after fresh failed"
FAIL=seed steps fresh start seed capture deliver stop
expect_status "a failed seed fails the run" 1
expect "skips capture and deliver, but stops the service this run started" "fresh start seed stop" \
    "$(grep -vE '^(skipped|stop|failed):' <<< "$output" | xargs)"
FAIL=capture steps seed capture stop
expect "without start in the run, stop is skipped too" "seed capture" \
    "$(grep -vE '^(skipped|failed):' <<< "$output" | xargs)"
FAIL=stop steps start stop
expect_status "a failed stop fails the run" 1
run_status env RD_SHOTS_WORK="$SCRATCH/work" "$TOOL/shots.sh" fresh nope
expect_status "an unknown step is refused before any step runs" 2
expect "and nothing ran" "unknown step nope" "$output"

# --- the target manifest -----------------------------------------------------------------------
run_status python3 - "$TOOL/targets.json" "$SCRATCH/node-views" <<'PY'
import json, sys
manifest = json.load(open(sys.argv[1]))
views = set(open(sys.argv[2]).read().split())
bad = []
for name, spec in manifest["targets"].items():
    if spec["views"] != "*":
        bad += [f"{name}: {v}" for v in spec["views"] if v not in views]
bad += [f"by_hand: {v} is a view" for v in manifest["by_hand"] if v in views]
print("\n".join(bad))
sys.exit(1 if bad else 0)
PY
expect_status "targets.json maps only views, and nothing it takes by hand is one" 0
expect_true "the wiki takes 1440x900 at scale 1, the website scale 2 WebP" \
    'python3 -c "import json,sys; m=json.load(open(sys.argv[1])); c=m[\"capture\"]; t=m[\"targets\"]; sys.exit(0 if (c[\"width\"],c[\"height\"],t[\"wiki\"][\"scale\"],t[\"website\"][\"scale\"],t[\"website\"][\"format\"])==(1440,900,1,2,\"webp\") else 1)" "$TOOL/targets.json"'

# --- alt texts ---------------------------------------------------------------------------------
run_status python3 "$TOOL/alts.py" --lint
expect_status "no alt text names a number" 0
cp -r "$TOOL" "$SCRATCH/tool"
python3 - "$SCRATCH/tool/alts.json" <<'PY'
import json, sys
alts = json.load(open(sys.argv[1]))
alts["05-downloads"] = "The Downloads view with 3 packages"
alts["40-settings-transfers"] = "FTP, SFTP, WebDAV & S3"
json.dump(alts, open(sys.argv[1], "w"))
PY
run_status python3 "$SCRATCH/tool/alts.py" --lint
expect_status "a count is refused" 1
expect_output "naming the view" "05-downloads: '3'"
expect_true "a digit inside a name is not a number" '! grep -q "40-settings-transfers" <<< "$output"'

mkdir -p "$SCRATCH/wiki/guide"
printf '# Downloads\n\n![old text](../images/screenshots/05-downloads.png)\n![keep](../images/screenshots/99-other.png)\n' \
    > "$SCRATCH/wiki/guide/downloads.md"
run_status python3 "$TOOL/alts.py" "$SCRATCH/wiki" --check
expect_status "--check names a page whose alt text differs" 1
expect_true "and writes nothing" 'grep -q "old text" "$SCRATCH/wiki/guide/downloads.md"'
run_status python3 "$TOOL/alts.py" "$SCRATCH/wiki"
expect_status "the write" 0
expect_true "puts the text in" 'grep -q "!\[The Downloads view: the sidebar" "$SCRATCH/wiki/guide/downloads.md"'
expect_true "and leaves an image without one alone" 'grep -q "!\[keep\](../images/screenshots/99-other.png)" "$SCRATCH/wiki/guide/downloads.md"'
run_status python3 "$TOOL/alts.py" "$SCRATCH/wiki" --check
expect_status "--check after the write" 0

# --- delivery ----------------------------------------------------------------------------------
png() {  # png <file> <width> <height>: the header deliver.py reads
    python3 -c 'import struct, sys; open(sys.argv[1], "wb").write(b"\x89PNG\r\n\x1a\n" + struct.pack(">I", 13) + b"IHDR" + struct.pack(">II", int(sys.argv[2]), int(sys.argv[3])) + b"\x08\x06\x00\x00\x00")' "$@"
}
mkdir -p "$SCRATCH/bin" "$SCRATCH/out/1x" "$SCRATCH/out/2x" "$SCRATCH/w-1.27/images/screenshots" \
    "$SCRATCH/site-1.27/public/screenshots" "$SCRATCH/repo/.github/readme"
printf '#!/usr/bin/env bash\ncp "$1" "${@: -1}"\necho "$*" >> "%s/magick.log"\n' "$SCRATCH" > "$SCRATCH/bin/magick"
chmod +x "$SCRATCH/bin/magick"
for view in 05-downloads 19-logs 62-downloads-download-window; do
    png "$SCRATCH/out/1x/$view.png" 1440 900
    png "$SCRATCH/out/2x/$view.png" 2880 1800
done
echo old > "$SCRATCH/w-1.27/images/screenshots/05-downloads.png"
echo old > "$SCRATCH/w-1.27/images/screenshots/19-logs.png"
echo old > "$SCRATCH/site-1.27/public/screenshots/05-downloads.webp"
deliver() {
    run_status env PATH="$SCRATCH/bin:$PATH" RD_SHOTS_WORK="$SCRATCH" RD_WIKI_SRC="$SCRATCH/w" \
        RD_WEBSITE_DIR="$SCRATCH/site" python3 "$TOOL/deliver.py" --repo "$SCRATCH/repo" "$@"
}

deliver --views 05,19 --line 1.27 --dry-run
expect_status "a dry run" 0
expect_true "writes nothing" 'grep -qx old "$SCRATCH/w-1.27/images/screenshots/05-downloads.png"'
deliver --views 05,19 --line 1.27
expect_status "delivery into the release worktrees" 0
expect_true "the wiki image is the 1x shot" 'cmp -s "$SCRATCH/out/1x/05-downloads.png" "$SCRATCH/w-1.27/images/screenshots/05-downloads.png"'
expect_true "the website's WebP comes from the 2x shot" 'grep -q "out/2x/05-downloads.png -quality 85 $SCRATCH/site-1.27/public/screenshots/05-downloads.webp" "$SCRATCH/magick.log"'
expect_true "a view the website does not show stays off it" '[[ ! -e "$SCRATCH/site-1.27/public/screenshots/19-logs.webp" ]]'
expect_true "the README image is the 1x Downloads shot" 'cmp -s "$SCRATCH/out/1x/05-downloads.png" "$SCRATCH/repo/.github/readme/downloads.png"'

png "$SCRATCH/out/2x/05-downloads.png" 1440 900
echo old > "$SCRATCH/w-1.27/images/screenshots/19-logs.png"
deliver --views 19,05 --line 1.27
expect_status "a shot at the wrong size is refused" 1
expect_output "naming both sizes" "is 1440x900, website wants 2880x1800"
expect_true "before anything is written" 'grep -qx old "$SCRATCH/w-1.27/images/screenshots/19-logs.png"'
deliver --views 05 --line 1.27 --targets wiki
expect_status "--targets leaves the website out" 0

echo taken > "$SCRATCH/w-1.27/images/screenshots/62-something-else.png"
deliver --views 62 --line 1.27 --targets wiki
expect_status "a new wiki view whose number is taken is refused" 1
expect_output "naming the image" "62-something-else.png already has its number"
deliver --views 50 --line 1.27
expect_status "a view taken by hand is refused" 1
expect_output "with its reason" "50-grafana-dashboard is taken by hand"
deliver --line 1.27
expect_status "delivery without --views is refused" 2
deliver --views 05 --targets wiki
expect_status "without --line the main checkout is the target, here missing" 1
expect_output "and named" "$SCRATCH/w is not a directory"

# --- no machine in the tool --------------------------------------------------------------------
expect "the tool names no home, session or version path" "" \
    "$(grep -nE '/home/|/tmp/claude|shots-1[0-9]{2}|rdownloader-demo1|claude-1000' "$TOOL"/* || true)"

finish_tests screenshots
