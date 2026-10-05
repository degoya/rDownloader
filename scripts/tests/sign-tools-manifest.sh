#!/usr/bin/env bash
#
# scripts/sign-tools-manifest.sh on a copy of the embedded manifest with a throwaway key
# (RD-1110-14): `--set`, a changes file and `--bump` each raise `sequence` by one, date
# `issued_at` today and apply the change; the signature verifies with the throwaway pair's public
# key, and a changed payload does not; a dead download, a missing key and an unknown field stop
# the run with the file untouched. The URL check is a stub (no network), the key comes from
# `rdownloader plugin keygen --role tool-manifest` into a scratch directory, and HOME points
# there too, so no key under ~/.config is ever named.
#
# Builds nothing: it takes `rd-pack` and `rdownloader` from the checkout's target directory and
# reports "skipped" with the reason when either is missing.
#
#   scripts/tests/sign-tools-manifest.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/sign-tools-manifest.sh"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/lanes.sh
source "$ROOT/scripts/lib/lanes.sh"
TARGET_DIR="$(rd_target_dir "$ROOT")"

# The newest build of binary $1 in the target directory, or nothing.
newest() {
    local candidate best=""
    for candidate in "$TARGET_DIR/debug/$1" "$TARGET_DIR/release-test/$1"; do
        [[ -x "$candidate" ]] || continue
        [[ -z "$best" || "$candidate" -nt "$best" ]] && best="$candidate"
    done
    printf '%s' "$best"
}
PACK="$(newest rd-pack)"
SERVICE="$(newest rdownloader)"
if [[ -z "$PACK" || -z "$SERVICE" ]]; then
    echo "sign-tools-manifest: skipped — no rd-pack or rdownloader under $TARGET_DIR/{debug,release-test}"
    exit 0
fi
if ! "$PACK" tools sign-manifest --help > /dev/null 2>&1; then
    echo "sign-tools-manifest: skipped — $PACK predates \`rd-pack tools sign-manifest\`; rebuild it"
    exit 0
fi

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
mkdir -p "$SCRATCH/home" "$SCRATCH/keys"
"$SERVICE" plugin keygen --role tool-manifest --output "$SCRATCH/keys" > /dev/null
cp "$ROOT/crates/rd-tools/resources/tools-manifest.json" "$SCRATCH/manifest.json"

# The stub keeps what it was asked to check and answers as STUB_CHECK_STATUS says.
cat > "$SCRATCH/check.sh" <<'EOF'
#!/usr/bin/env bash
cp "$1" "$STUB_CHECKED"
[[ "${STUB_CHECK_STATUS:-0}" -eq 0 ]] || echo "DEAD https://example.test/gone: HTTP 404"
exit "${STUB_CHECK_STATUS:-0}"
EOF
chmod +x "$SCRATCH/check.sh"

export HOME="$SCRATCH/home"
export RD_PACK="$PACK"
export RD_TOOLS_MANIFEST="$SCRATCH/manifest.json"
export RD_TOOLS_MANIFEST_CHECK="$SCRATCH/check.sh"
export RDOWNLOADER_TOOLS_KEY="$SCRATCH/keys/rdownloader-tools.key"
# Not the embedded root's id: rd-pack refuses a key that is not the root under the root's name.
export RDOWNLOADER_TOOLS_KEY_ID="test-tools-key"
export STUB_CHECKED="$SCRATCH/checked.json"
export STUB_CHECK_STATUS=0

# `field` of the signed payload's entry `name:platform`, or a top-level field without $3.
read_field() {
    python3 - "$RD_TOOLS_MANIFEST" "$@" <<'EOF'
import json, sys
payload = json.load(open(sys.argv[1], encoding="utf-8"))["payload"]
if len(sys.argv) == 3:
    print(payload[sys.argv[2]])
else:
    name, platform = sys.argv[3].split(":")
    entry = [e for e in payload["tools"] if e["name"] == name and e["platform"] == platform]
    print(entry[0][sys.argv[2]] if entry else "missing")
EOF
}

# Whether the document $1 carries an Ed25519 signature over its payload that the public key in
# $2 verifies — rd-sign's digest (`DigestBuilder`: each field length-prefixed, big-endian u64,
# over the domain and the payload's raw bytes), checked by openssl, not by the code under test.
signature_verifies() {
    python3 - "$1" "$2" "$SCRATCH/verify" <<'EOF' || return 1
import base64, hashlib, json, os, struct, sys
text = open(sys.argv[1], encoding="utf-8").read()
start = text.index('"payload":') + len('"payload":')
while text[start] in " \n\t":
    start += 1
_, end = json.JSONDecoder().raw_decode(text, start)
raw = text[start:end].encode()
domain = b"rdownloader.tool-manifest.v1"
digest = hashlib.sha256(struct.pack(">Q", len(domain)) + domain
                        + struct.pack(">Q", len(raw)) + raw).digest()
public = base64.b64decode(open(sys.argv[2]).read().strip())
der = bytes.fromhex("302a300506032b6570032100") + public
out = sys.argv[3]
os.makedirs(out, exist_ok=True)
open(f"{out}/digest", "wb").write(digest)
open(f"{out}/signature", "wb").write(
    base64.b64decode(json.loads(text)["signatures"][0]["signature"]))
open(f"{out}/public.pem", "w").write(
    "-----BEGIN PUBLIC KEY-----\n" + base64.b64encode(der).decode() + "\n-----END PUBLIC KEY-----\n")
EOF
    openssl pkeyutl -verify -pubin -inkey "$SCRATCH/verify/public.pem" -rawin \
        -in "$SCRATCH/verify/digest" -sigfile "$SCRATCH/verify/signature" > /dev/null 2>&1
}

TODAY="$(date -u +%Y-%m-%dT00:00:00Z)"
SHA="$(printf '%064d' 0 | tr 0 b)"
sequence="$(read_field sequence)"
ffmpeg_url="$(read_field url ffmpeg:x86_64-unknown-linux-gnu)"

run_status "$SCRIPT" --set yt-dlp:x86_64-unknown-linux-gnu version=2026.09.30 \
    url=https://example.test/yt-dlp_linux "sha256=$SHA" size=4242
expect_status "--set signs" 0
expect "--set raises the sequence by one" "$((sequence + 1))" "$(read_field sequence)"
expect "issued_at is today, midnight UTC" "$TODAY" "$(read_field issued_at)"
expect "--set changes the url" "https://example.test/yt-dlp_linux" "$(read_field url yt-dlp:x86_64-unknown-linux-gnu)"
expect "--set changes the size" "4242" "$(read_field size yt-dlp:x86_64-unknown-linux-gnu)"
expect "another entry stays" "$ffmpeg_url" "$(read_field url ffmpeg:x86_64-unknown-linux-gnu)"
expect_true "the URL check saw the new payload" "grep -qF 'https://example.test/yt-dlp_linux' '$STUB_CHECKED'"
expect_output "the payload's diff is shown" '+      "url": "https://example.test/yt-dlp_linux",'
expect_true "the signature verifies with the throwaway public key" \
    "signature_verifies '$RD_TOOLS_MANIFEST' '$SCRATCH/keys/rdownloader-tools.pub'"
sed 's/"size":4242/"size":4243/' "$RD_TOOLS_MANIFEST" > "$SCRATCH/tampered.json"
expect_true "a changed payload does not verify" \
    "! signature_verifies '$SCRATCH/tampered.json' '$SCRATCH/keys/rdownloader-tools.pub'"

cat > "$SCRATCH/changes.json" <<'EOF'
[{"name": "ffprobe", "platform": "x86_64-unknown-linux-gnu", "members": ["bin/ffprobe"]}]
EOF
run_status "$SCRIPT" "$SCRATCH/changes.json"
expect_status "a changes file signs" 0
expect "a changes file raises the sequence again" "$((sequence + 2))" "$(read_field sequence)"
expect "a changes file is merged by name and platform" "['bin/ffprobe']" "$(read_field members ffprobe:x86_64-unknown-linux-gnu)"

run_status "$SCRIPT" --bump
expect_status "--bump signs" 0
expect "--bump raises the sequence" "$((sequence + 3))" "$(read_field sequence)"
expect "--bump changes no entry" "4242" "$(read_field size yt-dlp:x86_64-unknown-linux-gnu)"

cp "$RD_TOOLS_MANIFEST" "$SCRATCH/before.json"
STUB_CHECK_STATUS=1 run_status "$SCRIPT" --bump
expect_status "a dead download stops the run" 1
expect_output "the dead download is named" "DEAD https://example.test/gone"
expect_true "a dead download leaves the file as it was" "cmp -s '$SCRATCH/before.json' '$RD_TOOLS_MANIFEST'"

RDOWNLOADER_TOOLS_KEY="$SCRATCH/keys/absent.key" run_status "$SCRIPT" --bump
expect_status "a missing key stops the run" 1
expect_output "the missing key is named" "signing key not found at $SCRATCH/keys/absent.key"
expect_true "a missing key leaves the file as it was" "cmp -s '$SCRATCH/before.json' '$RD_TOOLS_MANIFEST'"

run_status "$SCRIPT" --set yt-dlp:x86_64-unknown-linux-gnu colour=red
expect_status "an unknown field is refused" 2
expect_output "the field is named" "unknown field(s) colour"
run_status "$SCRIPT" --set yt-dlp:riscv64gc-unknown-linux-gnu url=https://example.test/x
expect_status "a new entry without its required fields is refused" 2
expect_output "what it lacks is named" "a new entry needs version, sha256, size"
run_status "$SCRIPT"
expect_status "no change at all is a usage error" 2
expect_true "a refused change leaves the file as it was" "cmp -s '$SCRATCH/before.json' '$RD_TOOLS_MANIFEST'"

finish_tests "sign-tools-manifest"
