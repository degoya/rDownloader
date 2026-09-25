// The POST-body path is removed and has to stay removed (RD-109-20).
//
// The extension used to read the body of a form-triggered download and hand it over so
// rDownloader could repeat the request. It never once did so in a default install: no part of
// the extension ever requested the broad host access `webRequest` needs to see a hoster's
// requests, so the listener was never registered. Authenticated POST downloads are no longer an
// advertised feature. The server still accepts a body under capture contract v2 — the extension
// no longer produces one, and these tests are what keeps it that way.

import assert from 'node:assert/strict'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import * as intercept from '../src/intercept.js'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')

function sources() {
  return readdirSync(join(root, 'src'))
    .filter((name) => name.endsWith('.js'))
    .map((name) => [name, readFileSync(join(root, 'src', name), 'utf8')])
}

test('intercept.js exports no body encoder and no second payload builder', () => {
  for (const gone of ['encodeBody', 'encodeBase64', 'buildIntakePayloadV2', 'BODY_CONTENT_TYPES', 'MAX_BODY_BYTES']) {
    assert.equal(intercept[gone], undefined, `${gone} is part of the removed POST-body path`)
  }
  // What is left builds one shape, and that shape is a GET.
  const payload = intercept.buildIntakePayload({
    item: { url: 'https://hoster.test/dl', filename: 'movie.mkv' },
    captured: { method: 'POST', headers: [{ name: 'Content-Type', value: 'application/json' }] },
    userAgent: 'UA'
  })
  assert.equal(payload.links[0].request.method, 'GET')
  assert.equal(payload.links[0].request.body_b64, undefined)
  assert.equal(payload.links[0].request.has_file_upload, undefined)
})

test('no listener asks the browser for a request body', () => {
  // The quoted spellings are the ones that do something: `'requestBody'` is the extraInfoSpec
  // flag a webRequest listener is registered with, `details.requestBody` is what it then yields,
  // and `body_b64` is the field the capture contract would carry it in. Prose about the removed
  // path is allowed to name it; code is not.
  for (const [name, text] of sources()) {
    assert.ok(!/['"]requestBody['"]/.test(text), `${name} registers a listener with requestBody`)
    assert.ok(!/\.requestBody\b/.test(text), `${name} reads details.requestBody`)
    assert.ok(!/['"]?body_b64['"]?\s*[:=]/.test(text), `${name} still fills body_b64`)
  }
})

test('webRequest is used for observation only', () => {
  const background = readFileSync(join(root, 'src', 'background.js'), 'utf8')
  // The three listeners that stay, and the extraInfoSpec each of them may ask for.
  assert.match(background, /onSendHeaders, interceptor\.onSendHeaders, \['requestHeaders'\]/)
  assert.match(background, /onHeadersReceived, interceptor\.onHeadersReceived, \['responseHeaders'\]/)
  assert.match(background, /onBeforeRequest\.addListener\(interceptor\.onBeforeRequest, REQUEST_FILTER\)/)
  // The gate that used to wait for a hand-granted wildcard is gone with the path it guarded.
  assert.ok(!/registerBodyListener/.test(background), 'the body listener must not come back')
  assert.ok(!/permissions\?\.onAdded/.test(background), 'nothing re-registers a listener on a new grant')
})

test('the header allowlist carries what a download needs and no credential', () => {
  assert.ok(intercept.HEADER_ALLOWLIST.includes('content-type'))
  // The denylist is a substring matcher, so a new allowlist entry can be silently dropped.
  for (const name of intercept.HEADER_ALLOWLIST) {
    assert.ok(!intercept.HEADER_DENYLIST.test(name), `${name} is refused by the denylist`)
  }
})

test('the contract version is the one the service declares', () => {
  // This used to assert `CONTRACT_VERSION === 2` against nothing but itself, so a server-side
  // version bump could not break it (RD-109-26). It reads the service's own constant instead,
  // which is the only way the two can be held together from here: the extension has no build
  // step and shares no code with the Rust workspace.
  const capture = readFileSync(join(root, '..', 'crates', 'rd-core', 'src', 'capture.rs'), 'utf8')
  const declared = /pub const CAPTURE_CONTRACT_VERSION:\s*u32\s*=\s*(\d+);/.exec(capture)
  assert.ok(declared, 'rd-core no longer declares CAPTURE_CONTRACT_VERSION where this test looks')
  assert.equal(
    intercept.CONTRACT_VERSION,
    Number(declared[1]),
    'the extension and rd_core::CAPTURE_CONTRACT_VERSION have drifted apart'
  )
  // And it has a reader: a server announcing more is answered with what this build produces.
  assert.match(readFileSync(join(root, 'src', 'downloads.js'), 'utf8'), /Math\.min\(result\.captureVersion \?\? 0, CONTRACT_VERSION\)/)
})
