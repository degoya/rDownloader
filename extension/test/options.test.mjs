// The options page's decisions, without a DOM: which host permission a server needs, and what
// the person is told when the answer is no.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { connectionStatus, ensurePermission, saveStatus } from '../src/options.js'

import { fakeBrowser } from './fake-browser.mjs'

/** Saves `server` in a click in this browser. */
async function save(target, server, options) {
  const browser = fakeBrowser(target, options)
  const outcome = await browser.gesture(() => ensurePermission(server, browser.api.permissions, browser.manifest))
  return { browser, outcome, asked: browser.calls.map(([name, value]) => [name, value.origins]) }
}

test('an address the browser cannot parse is named, not thrown', async () => {
  // `new URL('http://::1')` throws: an unbracketed IPv6 literal is not a host. The save
  // listener used to reject unhandled — no status text, nothing saved, no explanation
  // (RD-109-24).
  for (const input of ['::1', '[::1', 'http://', 'not a url at all']) {
    const { outcome, asked } = await save('chrome', input)
    assert.equal(outcome, 'invalid', input)
    assert.deepEqual(asked, [], 'nothing is asked for an address that is not one')
    assert.deepEqual(saveStatus('invalid'), { key: 'serverInvalid', ok: false })
  }
  // The bracketed spelling is a perfectly good address and goes the normal way.
  assert.equal((await save('chrome', 'http://[::1]:8710')).outcome, 'granted')
})

test('the permission is requested in the click itself, as Firefox demands', async () => {
  // Firefox takes `permissions.request` only while it handles the click and refuses it after any
  // awaited call. `contains` used to be awaited first, so every address without a grant was
  // refused as "not a valid server address" and nothing could be saved.
  for (const target of ['firefox', 'chrome']) {
    for (const server of ['http://192.168.178.123:8710', 'https://dl.example.com']) {
      const { browser, outcome } = await save(target, server)
      assert.equal(outcome, 'granted', `${target}: ${server}`)
      assert.equal(browser.prompts.length, 1)
      // The grant covers what the extension then asks the server.
      const { browser: again, outcome: second } = await save(target, server, { grants: browser.prompts[0].origins })
      assert.equal(second, 'granted')
      assert.deepEqual(again.prompts, [], 'a grant already held asks nobody twice')
    }
  }
})

test('the addresses on this computer are declared, so saving them asks nothing', async () => {
  for (const target of ['firefox', 'chrome']) {
    for (const server of ['http://127.0.0.1:8710', 'http://localhost:8710']) {
      const { browser, outcome } = await save(target, server, { answer: () => false })
      assert.equal(outcome, 'granted', `${target}: ${server}`)
      assert.deepEqual(browser.prompts, [])
    }
  }
  // Chrome's grant names the port: another one on this computer is another service (RD-109-24).
  assert.equal((await save('chrome', 'http://127.0.0.1:9000', { answer: () => false })).outcome, 'deniedLoopback')
})

test('Firefox is asked without the port, Chrome with it', async () => {
  // A match pattern with a port matches nothing in Firefox (bugs 1362809, 1468162).
  for (const [server, firefox, chrome] of [
    ['http://192.168.178.123:8710', 'http://192.168.178.123/*', 'http://192.168.178.123:8710/*'],
    ['https://dl.example.com', 'https://dl.example.com/*', 'https://dl.example.com/*'],
    ['http://[::1]:8710', 'http://[::1]/*', 'http://[::1]:8710/*'],
    ['http://127.0.0.1:8710', 'http://127.0.0.1/*', 'http://127.0.0.1:8710/*']
  ]) {
    assert.deepEqual((await save('firefox', server)).asked, [['request', [firefox]]])
    assert.deepEqual((await save('chrome', server)).asked, [['request', [chrome]]])
  }
})

test('a refused permission says which refusal it was', async () => {
  assert.equal((await save('chrome', 'http://127.0.0.1:9000', { answer: () => false })).outcome, 'deniedLoopback')
  assert.equal((await save('firefox', 'http://nas.local:8710', { answer: () => false })).outcome, 'denied')
  assert.deepEqual(saveStatus('deniedLoopback'), { key: 'permissionDeniedLoopback', ok: false })
  assert.deepEqual(saveStatus('denied'), { key: 'permissionDenied', ok: false })
  assert.deepEqual(saveStatus('granted'), { key: 'saved', ok: true })
})

test('a permission call the browser rejects is no invalid address', async () => {
  const throwing = (held) => ({
    contains: async () => held(),
    request: async () => { throw new Error('no such permission') }
  })
  assert.equal(await ensurePermission('http://nas.local:8710', throwing(() => true)), 'granted')
  assert.equal(await ensurePermission('http://nas.local:8710', throwing(() => false)), 'denied')
  assert.equal(await ensurePermission('http://nas.local:8710', throwing(() => { throw new Error('no') })), 'denied')
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
