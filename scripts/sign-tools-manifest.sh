#!/usr/bin/env bash
#
# Re-signs the embedded tool manifest (RD-1110-14): the next payload — the changes applied,
# `sequence` + 1, `issued_at` today at midnight UTC — is checked for a dead download, signed with
# the tool-manifest key and written over `crates/rd-tools/resources/tools-manifest.json`, then
# the payload's diff is shown. Until 1.11 this was a throwaway payload script and a signing
# command typed by hand with the key's path in it (RD-1101-13); like `build-plugins.sh`, this
# script finds the key itself, so nobody writes a key path into a command line.
#
# Usage:
#   scripts/sign-tools-manifest.sh --bump               # a new sequence, nothing else changes
#   scripts/sign-tools-manifest.sh <changes.json>       # entries merged by `name` + `platform`
#   scripts/sign-tools-manifest.sh --set <name>:<platform> <field>=<value>... [--set …]
#
# A changes file is a JSON list of entries, or an object with such a list under `tools`. Each
# entry names `name` and `platform` and the fields it changes: `version`, `url`, `sha256`, `size`,
# `archive`, `members`, `min_app_version`, `max_app_version`. An entry with no counterpart is
# added and then needs `version`, `url`, `sha256` and `size`. On the command line `size` is a
# number, `members` a comma-separated list and an empty `min_app_version=`/`max_app_version=`
# removes the bound:
#
#   scripts/sign-tools-manifest.sh --set ffmpeg:x86_64-pc-windows-msvc url=https://… sha256=… size=…
#
# Nothing is written unless every step holds: a dead URL or a wrong size
# (`scripts/tools-manifest-check.sh`), a missing key, an entry an installation would refuse
# (`rd-pack tools sign-manifest` verifies what it signed) each stop the run with the file as it
# was. The packager is `rd-pack` in `release-test`, built under the build lock as in
# `build-plugins.sh`.
#
# Environment:
#   RDOWNLOADER_TOOLS_KEY     the tool-manifest key (default ~/.config/rdownloader/rdownloader-tools.key)
#   RDOWNLOADER_TOOLS_KEY_ID  the `key_id` signed under (default: rd-pack's, the embedded root's)
#   RD_PACK                   a built `rd-pack` to sign with; nothing is built and no lock taken
#   RD_TOOLS_MANIFEST         the signed manifest read and replaced (default: the embedded one)
#   RD_TOOLS_MANIFEST_CHECK   the URL check, run with the new payload (default
#                             scripts/tools-manifest-check.sh); only the script test sets it
#
# Exit codes: 0 signed, 1 a check refused or signing failed, 2 a usage or change error.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KEY="${RDOWNLOADER_TOOLS_KEY:-$HOME/.config/rdownloader/rdownloader-tools.key}"
MANIFEST="${RD_TOOLS_MANIFEST:-$ROOT/crates/rd-tools/resources/tools-manifest.json}"
CHECK="${RD_TOOLS_MANIFEST_CHECK:-$ROOT/scripts/tools-manifest-check.sh}"
# shellcheck source=lib/jobs.sh
source "$ROOT/scripts/lib/jobs.sh"
# shellcheck source=lib/lock.sh
source "$ROOT/scripts/lib/lock.sh"

usage() {
    sed -n '/^# Usage:/,/^# Exit codes/p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

# Checked before the lock and the build, so a mistyped argument costs nothing. Python reads the
# changes themselves; here only their shape. The lock re-runs this script with ARGUMENTS.
ARGUMENTS=("$@")
bump=0
changes_file=""
sets=()
set_target=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        -h | --help) usage ;;
        --bump) bump=1; set_target="" ;;
        --set)
            [[ $# -ge 2 && "$2" == *:* ]] || { echo "--set wants <name>:<platform>" >&2; exit 2; }
            set_target="$2"
            shift
            ;;
        -*) echo "unknown argument: $1" >&2; exit 2 ;;
        *=*)
            [[ -n "$set_target" ]] || { echo "$1 follows no --set <name>:<platform>" >&2; exit 2; }
            sets+=("$set_target"$'\t'"$1")
            ;;
        *)
            [[ -z "$changes_file" ]] || { echo "one changes file at a time" >&2; exit 2; }
            [[ -f "$1" ]] || { echo "no changes file at $1" >&2; exit 2; }
            changes_file="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
            set_target=""
            ;;
    esac
    shift
done
if [[ "$bump" -eq 0 && -z "$changes_file" && "${#sets[@]}" -eq 0 ]]; then
    echo "nothing to do: name --bump, a changes file or --set" >&2
    usage
fi
[[ -f "$MANIFEST" ]] || { echo "no manifest at $MANIFEST" >&2; exit 2; }
if [[ ! -f "$KEY" ]]; then
    echo "signing key not found at $KEY" >&2
    echo "set RDOWNLOADER_TOOLS_KEY to the tool-manifest key (which one: the release skill)" >&2
    exit 1
fi

if [[ -n "${RD_PACK:-}" ]]; then
    PACKAGER="$RD_PACK"
else
    rd_take_lock "${ARGUMENTS[@]}"
    PACKAGER_PROFILE="release-test"
    echo "==> building the packager (rd-pack, $PACKAGER_PROFILE)"
    (cd "$ROOT" && CARGO_BUILD_JOBS="$JOBS" cargo build --quiet --profile "$PACKAGER_PROFILE" -j "$JOBS" -p rd-pack)
    PACKAGER="$CARGO_TARGET_DIR/$PACKAGER_PROFILE/rd-pack"
fi
[[ -x "$PACKAGER" ]] || { echo "!! no packager at $PACKAGER" >&2; exit 1; }

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

if ! python3 - "$MANIFEST" "$SCRATCH/old.json" "$SCRATCH/new.json" "$changes_file" "${sets[@]}" <<'EOF'
import copy, datetime, json, sys

manifest_path, old_path, new_path, changes_path, *sets = sys.argv[1:]
FIELDS = ("version", "url", "sha256", "size", "archive", "members",
          "min_app_version", "max_app_version")
REQUIRED = ("version", "url", "sha256", "size")


def fail(message):
    sys.exit(f"{message}; nothing was signed")


def checked(field, value, origin):
    if field == "size":
        ok = isinstance(value, int) and not isinstance(value, bool)
    elif field == "members":
        ok = isinstance(value, list) and all(isinstance(member, str) for member in value)
    elif field in ("min_app_version", "max_app_version"):
        ok = value is None or isinstance(value, str)
    else:
        ok = isinstance(value, str) and value != ""
    if not ok:
        fail(f"{origin}: {field} cannot be {value!r}")
    return value


def merge(tools, change, origin):
    name, platform = change.get("name"), change.get("platform")
    if not isinstance(name, str) or not isinstance(platform, str):
        fail(f"{origin}: a change names `name` and `platform`")
    unknown = sorted(set(change) - set(FIELDS) - {"name", "platform"})
    if unknown:
        fail(f"{origin}: unknown field(s) {', '.join(unknown)} (known: {', '.join(FIELDS)})")
    fields = {field: checked(field, change[field], origin) for field in FIELDS if field in change}
    for entry in tools:
        if entry["name"] == name and entry["platform"] == platform:
            entry.update(fields)
            return
    missing = [field for field in REQUIRED if field not in fields]
    if missing:
        fail(f"{origin}: the manifest has no {name}:{platform}, and a new entry needs "
             + ", ".join(missing))
    tools.append({"name": name, "version": fields["version"], "platform": platform,
                  "url": fields["url"], "sha256": fields["sha256"], "size": fields["size"],
                  "archive": fields.get("archive", "raw"), "members": fields.get("members", []),
                  "min_app_version": fields.get("min_app_version"),
                  "max_app_version": fields.get("max_app_version")})


def from_argument(target, assignment):
    name, platform = target.split(":", 1)
    field, value = assignment.split("=", 1)
    if field == "size":
        if not value.isdigit():
            fail(f"--set {target}: size={value} is not a number")
        value = int(value)
    elif field == "members":
        value = [member for member in value.split(",") if member]
    elif field in ("min_app_version", "max_app_version") and value == "":
        value = None
    return {"name": name, "platform": platform, field: value}


document = json.load(open(manifest_path, encoding="utf-8"))
if not isinstance(document, dict) or "payload" not in document or "signatures" not in document:
    fail(f"{manifest_path} is not a signed document")
payload = document["payload"]
old = copy.deepcopy(payload)
tools = payload.setdefault("tools", [])

if changes_path:
    changes = json.load(open(changes_path, encoding="utf-8"))
    if isinstance(changes, dict):
        changes = changes.get("tools")
    if not isinstance(changes, list) or not all(isinstance(change, dict) for change in changes):
        fail(f"{changes_path} is neither a list of entries nor an object with one under `tools`")
    for change in changes:
        merge(tools, change, changes_path)
for spec in sets:
    target, assignment = spec.split("\t", 1)
    merge(tools, from_argument(target, assignment), f"--set {target}")

if (changes_path or sets) and tools == old.get("tools"):
    print("note: the changes change no entry; only the sequence moves", file=sys.stderr)
payload["sequence"] = int(old["sequence"]) + 1
payload["issued_at"] = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT00:00:00Z")

for path, value in ((old_path, old), (new_path, payload)):
    with open(path, "w", encoding="utf-8") as out:
        json.dump(value, out, indent=2)
        out.write("\n")
EOF
then
    exit 2
fi

echo "==> checking every download the new payload names"
if ! "$CHECK" "$SCRATCH/new.json"; then
    echo "!! a download in the new payload is dead or has another size; nothing was signed" >&2
    exit 1
fi

echo "==> signing"
sign_arguments=(tools sign-manifest --input "$SCRATCH/new.json" --output "$SCRATCH/signed.json" --key "$KEY")
[[ -n "${RDOWNLOADER_TOOLS_KEY_ID:-}" ]] && sign_arguments+=(--key-id "$RDOWNLOADER_TOOLS_KEY_ID")
if ! "$PACKAGER" "${sign_arguments[@]}"; then
    echo "!! signing failed; $MANIFEST is unchanged" >&2
    exit 1
fi
mv "$SCRATCH/signed.json" "$MANIFEST"

# The diff of what was signed, read back from the document, not of what was asked for.
python3 - "$MANIFEST" "$SCRATCH/signed-payload.json" <<'EOF'
import json, sys
payload = json.load(open(sys.argv[1], encoding="utf-8"))["payload"]
with open(sys.argv[2], "w", encoding="utf-8") as out:
    json.dump(payload, out, indent=2)
    out.write("\n")
EOF
echo
diff -u --label "payload before" --label "payload signed" "$SCRATCH/old.json" "$SCRATCH/signed-payload.json" || true
echo
echo "signed $MANIFEST; commit it and run cargo nextest run -p rd-tools"
