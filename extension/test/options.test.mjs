// The options page's decisions, without a DOM: which host permission a server needs, and what
// the person is told when the answer is no.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { connectionStatus, ensurePermission, saveStatus } from '../src/options.js'

function permissions({ held = false, grant = false } = {}) {
  const calls = []
  return {
    calls,
    contains: async (value) => { calls.push(['contains', value.origins[0]]); return held },
    request: async (value) => { calls.push(['request', value.origins[0]]); return grant }
  }
}

test('an address the browser cannot parse is named, not thrown', async () => {
  // `new URL('http://::1')` throws: an unbracketed IPv6 literal is not a host. The save
  // listener used to reject unhandled — no status text, nothing saved, no explanation
  // (RD-109-24).
  for (const input of ['::1', '[::1', 'http://', 'not a url at all']) {
    const api = permissions()
    assert.equal(await ensurePermission(input, api), 'invalid', input)
    assert.deepEqual(api.calls, [], 'nothing is asked for an address that is not one')
    assert.deepEqual(saveStatus('invalid'), { key: 'serverInvalid', ok: false })
  }
  // The bracketed spelling is a perfectly good address and goes the normal way.
  assert.equal(await ensurePermission('http://[::1]:8710', permissions({ held: true })), 'granted')
})

test('a loopback server on another port is asked for like any other host', async () => {
  // The manifest declares loopback on port 8710 and nothing else, so the old shortcut left a
  // service on 127.0.0.1:9000 with no host permission at all — it worked only because the
  // server answers `Access-Control-Allow-Origin: *` (RD-109-24).
  const declared = permissions({ held: true })
  assert.equal(await ensurePermission('http://127.0.0.1:8710', declared), 'granted')
  assert.deepEqual(declared.calls, [['contains', 'http://127.0.0.1:8710/*']], 'nothing is asked twice')

  const other = permissions({ held: false, grant: true })
  assert.equal(await ensurePermission('http://127.0.0.1:9000', other), 'granted')
  assert.deepEqual(other.calls, [
    ['contains', 'http://127.0.0.1:9000/*'],
    ['request', 'http://127.0.0.1:9000/*']
  ])
})

test('a refused permission says which refusal it was', async () => {
  assert.equal(await ensurePermission('http://127.0.0.1:9000', permissions()), 'deniedLoopback')
  assert.equal(await ensurePermission('http://nas.local:8710', permissions()), 'denied')
  assert.deepEqual(saveStatus('deniedLoopback'), { key: 'permissionDeniedLoopback', ok: false })
  assert.deepEqual(saveStatus('denied'), { key: 'permissionDenied', ok: false })
  assert.deepEqual(saveStatus('granted'), { key: 'saved', ok: true })
})

test('a browser that rejects the permission call is an invalid address, not a crash', async () => {
  const throwing = {
    contains: async () => { throw new Error('no such permission') },
    request: async () => true
  }
  assert.equal(await ensurePermission('http://nas.local:8710', throwing), 'invalid')
})

test('the connection test reports version, token and failure apart', () => {
  assert.deepEqual(connectionStatus('http://nas.local:8710', { ok: true, version: '1.0.9' }), {
    key: 'connected', substitutions: ['1.0.9'], ok: true
  })
  assert.deepEqual(connectionStatus('http://nas.local:8710', { ok: false, status: 401 }), {
    key: 'errorUnauthorized', substitutions: undefined, ok: false
  })
  assert.deepEqual(connectionStatus('http://nas.local:8710', { ok: false, status: 0, message: 'refused' }), {
    key: 'sendFailed', substitutions: ['refused'], ok: false
  })
  assert.deepEqual(connectionStatus('::1', { ok: false, status: 0, message: 'refused' }), {
    key: 'serverInvalid', substitutions: undefined, ok: false
  })
})
