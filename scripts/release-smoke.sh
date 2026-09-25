#!/usr/bin/env bash
#
# Starts the binary that was just built and proves it actually serves, over real HTTP and a real
# browser. Nothing here talks to the source tree: it runs artifacts/linux/rdownloader against a
# throwaway database on a free port, and everything it asserts is what a user would get.
#
# The assertions are written to fail loudly rather than to pass quietly. Every HTTP call captures
# its status code explicitly and compares it; no response is piped through a filter that could
# turn a 500 into a match. The negative case matters as much as the positive one — an install
# that answers /api/v1/settings without a session is a broken install, not a passing smoke test.
#
# Usage:
#   scripts/release-smoke.sh 1.0.1
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="${1:-}"
[[ -n "$VERSION" ]] || { echo "usage: scripts/release-smoke.sh <version>" >&2; exit 2; }

BINARY="$ROOT/artifacts/linux/rdownloader"
[[ -x "$BINARY" ]] || { echo "no built binary at $BINARY — run the packaging step first" >&2; exit 1; }

WORK="$(mktemp -d -t rd-smoke-XXXXXX)"
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
BASE="http://127.0.0.1:$PORT"
PASSWORD="smoke-$(head -c 12 /dev/urandom | base64 | tr -dc 'A-Za-z0-9')"
SERVER_LOG="$WORK/server.log"
SERVER_PID=""
FAILURES=0

cleanup() {
    if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2> /dev/null; then
        kill "$SERVER_PID" 2> /dev/null || true
        wait "$SERVER_PID" 2> /dev/null || true
    fi
    if [[ "$FAILURES" -ne 0 && -f "$SERVER_LOG" ]]; then
        echo "--- server log (last 60 lines) ---" >&2
        tail -60 "$SERVER_LOG" >&2 || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

fail() { echo "FAIL $*" >&2; FAILURES=$((FAILURES + 1)); }
pass() { echo "ok   $*"; }

# One HTTP call. Body to $WORK/body, status returned on stdout, and the exit status of curl
# itself is checked — a connection refused must not read as an empty status string.
http() {
    local method="$1" path="$2" data="${3:-}" code
    local -a args=(-sS -o "$WORK/body" -w '%{http_code}' -X "$method" -b "$WORK/cookies" -c "$WORK/cookies")
    [[ -n "$data" ]] && args+=(-H 'Content-Type: application/json' -d "$data")
    if ! code="$(curl "${args[@]}" "$BASE$path")"; then
        echo "000"
        return 0
    fi
    echo "$code"
}

expect() {
    local want="$1" method="$2" path="$3" data="${4:-}" got
    got="$(http "$method" "$path" "$data")"
    if [[ "$got" == "$want" ]]; then
        pass "$method $path -> $got"
    else
        fail "$method $path -> $got (expected $want): $(head -c 300 "$WORK/body" 2>/dev/null)"
    fi
}

echo "==> smoke-testing $VERSION on $BASE"
echo "    binary:   $BINARY"
echo "    workdir:  $WORK"

# ---------------------------------------------------------------------------------------------
# Start
# ---------------------------------------------------------------------------------------------
"$BINARY" serve \
    --database "$WORK/rdownloader.sqlite3" \
    --downloads "$WORK/downloads" \
    --listen "127.0.0.1:$PORT" > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

ready=0
for _ in $(seq 1 60); do
    if ! kill -0 "$SERVER_PID" 2> /dev/null; then
        echo "the service exited during startup" >&2
        tail -40 "$SERVER_LOG" >&2
        exit 1
    fi
    if [[ "$(http GET /api/v1/health)" == "200" ]]; then ready=1; break; fi
    sleep 1
done
[[ "$ready" -eq 1 ]] || { echo "the service never answered /api/v1/health" >&2; exit 1; }
pass "service came up (pid $SERVER_PID)"

# ---------------------------------------------------------------------------------------------
# The API, as a client sees it
# ---------------------------------------------------------------------------------------------
reported="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$WORK/body")"
if [[ "$reported" == "$VERSION" ]]; then
    pass "/api/v1/health reports $reported"
else
    fail "/api/v1/health reports $reported, expected $VERSION"
fi

expect 200 GET /api/v1/auth/status
setup_required="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["setup_required"])' "$WORK/body")"
if [[ "$setup_required" == "True" ]]; then
    pass "a fresh install asks for setup"
else
    fail "a fresh install reports setup_required=$setup_required"
fi

# Before any session exists, a protected route must refuse. If this passes with a 200 the rest
# of the suite is meaningless, so it is checked first.
guarded="$(http GET /api/v1/settings)"
if [[ "$guarded" == "401" || "$guarded" == "403" ]]; then
    pass "GET /api/v1/settings without a session -> $guarded"
else
    fail "GET /api/v1/settings without a session -> $guarded (expected 401 or 403)"
fi

expect 200 POST /api/v1/auth/setup "{\"password\":\"$PASSWORD\"}"
expect 200 POST /api/v1/auth/login "{\"password\":\"$PASSWORD\"}"

# The same route again, now that there is a session.
expect 200 GET /api/v1/settings
expect 200 GET /api/v1/packages
expect 200 GET /api/v1/system/media
expect 200 GET /api/v1/collector/candidates

# The embedded web UI has to be in the binary; a release built without web/dist serves nothing.
index_code="$(http GET /)"
if [[ "$index_code" == "200" ]] && grep -qi '<div id="app"' "$WORK/body"; then
    pass "the embedded web UI is served ($(stat -c %s "$WORK/body") bytes)"
else
    fail "GET / -> $index_code, and no app root in the body"
fi

# ---------------------------------------------------------------------------------------------
# The UI, in a real browser
#
# Playwright is not a dependency of this repository and the MCP browser is not usable on this
# machine, so the browsers already in ~/.cache/ms-playwright are driven through the copy of
# playwright in the npx cache. Missing either is a failure, not a skip: a smoke test that
# silently drops its UI half is exactly the false green this pipeline exists to prevent.
# ---------------------------------------------------------------------------------------------
PW_ROOT="$(find "$HOME/.npm/_npx" -maxdepth 3 -type d -name playwright 2>/dev/null \
    | head -1 | xargs -r dirname)"
if [[ -z "$PW_ROOT" ]]; then
    fail "no playwright in the npx cache (npx playwright@latest --version once, then re-run)"
elif [[ ! -d "$HOME/.cache/ms-playwright" ]]; then
    fail "no browsers in ~/.cache/ms-playwright"
else
    cat > "$WORK/ui.mjs" <<'JS'
import { chromium } from 'playwright'

const base = process.env.SMOKE_BASE
const password = process.env.SMOKE_PASSWORD
const shot = process.env.SMOKE_SHOT
const problems = []

const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })

// An uncaught exception or a failed request is a finding, not noise: the point of loading the
// real bundle is to catch what unit tests cannot.
page.on('pageerror', (error) => problems.push(`pageerror: ${error.message}`))
page.on('console', (message) => {
  if (message.type() === 'error') problems.push(`console: ${message.text()}`)
})
page.on('response', (response) => {
  if (response.status() >= 500) problems.push(`${response.status()} ${response.url()}`)
})

try {
  const response = await page.goto(base, { waitUntil: 'networkidle', timeout: 45000 })
  if (!response || !response.ok()) throw new Error(`GET / -> ${response && response.status()}`)

  // The SPA has to have mounted. An empty #app means the bundle loaded and then died.
  await page.waitForFunction(() => {
    const root = document.querySelector('#app')
    return root && root.children.length > 0
  }, { timeout: 30000 })
  console.log(`mounted: ${await page.title()}`)

  // The API setup already ran, so this is the sign-in screen.
  const field = page.locator('input[type="password"]').first()
  await field.waitFor({ state: 'visible', timeout: 20000 })
  await field.fill(password)
  await field.press('Enter')

  // Signed in means the password field is gone and the session is real. Asked of the page
  // rather than of curl, so it is the browser's own cookie that is being proven.
  await field.waitFor({ state: 'detached', timeout: 30000 }).catch(async () => {
    if (await field.isVisible()) throw new Error('still on the sign-in screen after submitting')
  })
  const status = await page.evaluate(async (url) => {
    const r = await fetch(`${url}/api/v1/settings`, { credentials: 'include' })
    return r.status
  }, base)
  if (status !== 200) throw new Error(`the browser session cannot read /api/v1/settings: ${status}`)
  console.log('signed in, and the browser session reaches the API')

  await page.screenshot({ path: shot, fullPage: false })
  console.log(`screenshot: ${shot}`)
} finally {
  await browser.close()
}

if (problems.length) {
  console.error('browser problems:')
  for (const problem of problems) console.error(`  ${problem}`)
  process.exit(1)
}
JS
    # NODE_PATH is a CommonJS mechanism and does nothing for an `import` in a .mjs, so the
    # script resolved nothing and the UI half failed with ERR_MODULE_NOT_FOUND. ESM resolution
    # instead walks up from the importing file looking for node_modules, so lending the work
    # directory the npx cache's copy is what actually makes `import ... from 'playwright'` work.
    ln -sfn "$PW_ROOT" "$WORK/node_modules"
    echo "==> UI pass (playwright from $PW_ROOT)"
    if NODE_PATH="$PW_ROOT" SMOKE_BASE="$BASE" SMOKE_PASSWORD="$PASSWORD" \
       SMOKE_SHOT="$ROOT/artifacts/release-smoke-$VERSION.png" \
       node "$WORK/ui.mjs"; then
        pass "the UI signs in and renders"
    else
        fail "the Playwright UI pass"
    fi
fi

# ---------------------------------------------------------------------------------------------
echo
if [[ "$FAILURES" -eq 0 ]]; then
    echo "==> smoke test passed"
else
    echo "==> smoke test failed: $FAILURES check(s)" >&2
fi
exit "$FAILURES"
