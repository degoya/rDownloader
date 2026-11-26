// The file that wires everything together, which no test imported at all (RD-109-26).
//
// Two things are being checked here. First, *when* the listeners are registered: under Manifest
// V3 Chrome wakes a terminated service worker only for events whose listener was registered
// synchronously during the first evaluation of the worker script, so one added from inside a
// `.then()` is there until the first teardown — about thirty idle seconds — and then never
// again, without a word (RD-109-19). Second, what those listeners then do: the context menu,
// the send, the notifications, the message routing.

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import { MESSAGE_TYPES } from '../src/captcha.js'

import { fakeBrowser, loadIn, settle } from './fake-browser.mjs'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')

const PAIRED = { server: 'http://127.0.0.1:8710', token: 'capture-token', interceptDownloads: true }
const calls = []
const state = { respond: async () => ({ ok: true, status: 201, json: async () => ({ candidates: [{}, {}] }) }) }

// Chrome's build; `wiring.test.mjs` loads both. The fake answers on a later task, so a
// registration that waits for an answer cannot slip into `atLoad`.
const browser = fakeBrowser('chrome', {
  calls,
  local: PAIRED,
  cookies: [{ name: 'session', value: 'abc', domain: '.hoster.test', path: '/', secure: true }],
  keys: true,
  respond: (url, init) => state.respond(url, init)
})
const registered = browser.listeners

function handlerFor(name) {
  const entry = registered.find((candidate) => candidate.name === name)
  assert.ok(entry, `nothing is registered for ${name}`)
  return entry.handler
}

function handlersFor(name) {
  return registered.filter((candidate) => candidate.name === name).map((candidate) => candidate.handler)
}

const background = await loadIn(browser, 'background.js')
const atLoad = registered.map((entry) => entry.name)
// Drain the microtask queue and a macrotask: anything that waited for a browser answer lands here.
await new Promise((resolve) => setTimeout(resolve, 5))
const afterwards = registered.map((entry) => entry.name)

/** Runs `work` with console.error captured, and gives back what it logged. */
async function withLoggedErrors(work) {
  const logged = []
  const real = console.error
  console.error = (...parts) => logged.push(parts.map(String).join(' '))
  try {
    await work()
  } finally {
    console.error = real
  }
  return logged
}

test('every listener is registered while the worker script is first evaluated', () => {
  assert.deepStrictEqual(afterwards, atLoad, 'a listener appeared after the module body had run')
})

test('the worker registers the events it has to be woken for', () => {
  for (const name of [
    'runtime.onInstalled',
    'runtime.onStartup',
    'runtime.onMessage',
    'contextMenus.onClicked',
    'storage.onChanged',
    'alarms.onAlarm',
    'tabs.onUpdated',
    'tabs.onRemoved',
    'downloads.onCreated',
    'notifications.onButtonClicked',
    'notifications.onClicked',
    'webRequest.onSendHeaders',
    'webRequest.onHeadersReceived',
    'webRequest.onBeforeRequest'
  ]) {
    assert.ok(atLoad.includes(name), `${name} is never registered`)
  }
  assert.ok(!atLoad.includes('permissions.onAdded'), 'nothing re-registers a listener on a new grant')
})

test('no registration waits on a promise in the source either', () => {
  const source = readFileSync(join(root, 'src', 'background.js'), 'utf8')
  // A `.then(...)` in this file may only appear inside a handler, never around a registration.
  for (const [index, line] of source.split('\n').entries()) {
    if (!/\.then\(/.test(line)) continue
    const tail = source.split('\n').slice(index, index + 6).join('\n')
    assert.ok(!/addListener\(/.test(tail), `a registration follows a .then() at line ${index + 1}`)
  }
})

test('installing rebuilds the context menu from scratch', async () => {
  calls.length = 0
  handlersFor('runtime.onInstalled')[0]()
  await settle()
  assert.deepEqual(calls.filter(([name]) => name === 'menu'), [
    ['menu', 'rdownloader-link', 'menuLink', 'link'],
    ['menu', 'rdownloader-page', 'menuPage', 'page'],
    ['menu', 'rdownloader-selection', 'menuSelection', 'selection'],
    // Sharing a session needs the click as its user gesture, so it lives on the page context.
    ['menu', 'rdownloader-session', 'menuSession', 'page'],
    // Local files, from any page and from the toolbar button (RD-1260-11).
    ['menu', 'rdownloader-file', 'menuSendFile', 'page,action']
  ])
})

test('each menu entry sends what it is about', async () => {
  const sent = []
  const links = state.respond
  state.respond = async (_url, init) => {
    sent.push(JSON.parse(init.body))
    return { ok: true, status: 201, json: async () => ({ candidates: [{}] }) }
  }
  const clicked = handlerFor('contextMenus.onClicked')
  clicked({ menuItemId: 'rdownloader-link', linkUrl: 'https://hoster.test/a' }, {})
  await settle()
  clicked({ menuItemId: 'rdownloader-page', pageUrl: 'https://hoster.test/' }, { title: 'A page' })
  await settle()
  clicked({ menuItemId: 'rdownloader-selection', selectionText: 'https://hoster.test/b' }, {})
  await settle()
  assert.deepEqual(sent.map((body) => body.text), [
    'https://hoster.test/a',
    'https://hoster.test/',
    'https://hoster.test/b'
  ])
  assert.equal(sent[1].package_name, 'A page')

  // The session entry goes to the sharer, not to the link intake.
  calls.length = 0
  browser.gesture(() => clicked({ menuItemId: 'rdownloader-session', pageUrl: 'https://hoster.test/' }, { title: 'A page' }))
  await settle()
  assert.ok(calls.some(([name, url]) => name === 'fetch' && url.endsWith('/api/v1/capture/cookies')))

  // "Send file to rDownloader…" opens the extension's own page and sends nothing yet.
  calls.length = 0
  clicked({ menuItemId: 'rdownloader-file', pageUrl: 'https://hoster.test/' }, {})
  await settle()
  assert.deepEqual(calls, [['createTab', { url: 'chrome-extension://self/src/send-file.html' }]])

  // A menu entry with nothing to send sends nothing.
  calls.length = 0
  clicked({ menuItemId: 'rdownloader-link' }, {})
  clicked({ menuItemId: 'something-else', linkUrl: 'https://hoster.test/c' }, {})
  await settle()
  assert.deepEqual(calls, [])
  state.respond = links
})

/** The popup as a popup: our own extension, no tab. */
const POPUP = { id: 'self' }

test('a send is answered only for our own pages', async () => {
  const sendMessage = handlerFor('runtime.onMessage')
  calls.length = 0

  // A content script carries a tab and the page's own address; another extension its own id.
  const contentScript = { id: 'self', tab: { id: 4 }, url: 'https://hoster.test/page' }
  assert.equal(sendMessage({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, contentScript, () => {}), false)
  assert.equal(sendMessage({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, { id: 'other' }, () => {}), false)
  await settle()
  assert.ok(!calls.some(([name]) => name === 'fetch'), 'nothing is posted for a foreign sender')

  // The popup opened as a tab is still our own page.
  const popupTab = { id: 'self', tab: { id: 5 }, url: 'chrome-extension://self/src/popup.html' }
  assert.equal(sendMessage({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, popupTab, () => {}), true)
  await settle()
  assert.ok(calls.some(([name]) => name === 'fetch'))
})

test('a send reports the count, the token and a plain failure apart', async () => {
  const sendMessage = handlerFor('runtime.onMessage')
  const clicked = handlerFor('contextMenus.onClicked')
  const notices = () => calls.filter(([name]) => name === 'notify').map(([, body]) => body)

  // RD-1270-05: the popup is answered with what really happened, and shows it itself -- one
  // notice per hand-over, so the background raises none for it.
  calls.length = 0
  let answer = null
  assert.equal(sendMessage({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, POPUP, (value) => { answer = value }), true)
  await settle()
  assert.deepEqual(answer, { ok: true, status: 201, code: null, message: null, links: 2 })
  assert.deepEqual(notices(), [], 'the popup shows the outcome; no second notice')
  assert.deepEqual(calls.filter(([name]) => name === 'badge'), [], 'a success does not repaint a badge nobody changed')

  // The context menu has no status line: exactly one desktop notification says it.
  calls.length = 0
  clicked({ menuItemId: 'rdownloader-link', linkUrl: 'https://hoster.test/a' }, {})
  await settle()
  assert.deepEqual(notices(), ['sentLinks:2'])

  calls.length = 0
  state.respond = async () => ({ ok: false, status: 401, json: async () => ({ error: 'no', code: 'capture.token_required' }) })
  sendMessage({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, POPUP, (value) => { answer = value })
  await settle()
  assert.equal(answer.ok, false, 'a refusal is never answered as a success')
  assert.equal(answer.status, 401)
  assert.equal(answer.code, 'capture.token_required')
  assert.deepEqual(notices(), [])
  assert.deepEqual(calls.filter(([name]) => name === 'badge'), [['badge', '!']])
  calls.length = 0
  clicked({ menuItemId: 'rdownloader-link', linkUrl: 'https://hoster.test/a' }, {})
  await settle()
  assert.deepEqual(notices(), ['sendFailed:errorUnauthorized'])

  calls.length = 0
  state.respond = async () => ({ ok: false, status: 500, json: async () => ({ error: 'server exploded' }) })
  sendMessage({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, POPUP, (value) => { answer = value })
  await settle()
  assert.equal(answer.ok, false)
  assert.equal(answer.message, 'server exploded')
  clicked({ menuItemId: 'rdownloader-link', linkUrl: 'https://hoster.test/a' }, {})
  await settle()
  assert.deepEqual(notices(), ['sendFailed:server exploded'])

  // Back to success: the badge gives the "!" up again rather than keeping it for good.
  calls.length = 0
  state.respond = async () => ({ ok: true, status: 201, json: async () => ({ candidates: [{}, {}] }) })
  sendMessage({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, POPUP, () => {})
  await settle()
  assert.deepEqual(calls.filter(([name]) => name === 'badge'), [['badge', '']])
})

test('an unpaired extension opens its options instead of sending', async () => {
  calls.length = 0
  browser.stored.token = ''
  let answer = null
  handlerFor('runtime.onMessage')({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, POPUP, (value) => { answer = value })
  await settle()
  assert.ok(!calls.some(([name]) => name === 'fetch'), 'nothing is posted without a token')
  assert.equal(answer.ok, false)
  assert.equal(answer.code, 'unconfigured')
  assert.ok(calls.some(([name]) => name === 'openOptionsPage'))
  calls.length = 0
  handlerFor('contextMenus.onClicked')({ menuItemId: 'rdownloader-link', linkUrl: 'https://hoster.test/a' }, {})
  await settle()
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'notConfigured'))
  browser.stored.token = PAIRED.token
})

test('a browser without notifications still does the work', async () => {
  calls.length = 0
  const create = browser.api.notifications.create
  browser.api.notifications.create = async () => { throw new Error('notifications are off') }
  handlerFor('runtime.onMessage')({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, POPUP, () => {})
  await settle()
  assert.ok(calls.some(([name]) => name === 'fetch'), 'the links still go out')
  browser.api.notifications.create = create
})

test('onMessage answers the types it owns and declines the rest', async () => {
  const sendMessage = handlerFor('runtime.onMessage')
  assert.equal(sendMessage({ type: 'something-else' }, {}, () => {}), false)
  assert.equal(sendMessage(undefined, {}, () => {}), false)

  for (const type of MESSAGE_TYPES) {
    assert.equal(sendMessage({ type }, {}, () => {}), true, `${type} is not routed`)
  }

  // Declining used to be a branch of its own here, without the sender check its two siblings
  // got; it is one of `MESSAGE_TYPES` now and goes through the same handler (RD-109-23).
  let fromNowhere = null
  assert.equal(sendMessage({ type: 'rdownloader:captcha-decline', id: 'x' }, {}, (value) => { fromNowhere = value }), true)
  await settle()
  assert.equal(fromNowhere, false, 'a sender that is not one of our own pages decides nothing')

  let declined = null
  assert.equal(
    sendMessage({ type: 'rdownloader:captcha-decline', id: 'x' }, { id: 'self' }, (value) => { declined = value }),
    true
  )
  await settle()
  assert.ok(declined, 'the popup is answered')
})

test('a handler that throws is logged, not lost', async () => {
  // A bare `void p` at every dispatch turned every throw into an unhandled rejection with
  // nothing naming the handler it came from (RD-109-24). node:test fails the run on an
  // unhandled rejection, so this test passing at all is half the assertion.
  const logged = await withLoggedErrors(async () => {
    background.dispatch('downloads.onCreated', () => Promise.reject(new Error('boom')))
    background.dispatch('downloads.onCreated', () => { throw new Error('synchronous boom') })
    await settle()
  })
  assert.equal(logged.length, 2)
  for (const line of logged) assert.match(line, /rdownloader: downloads\.onCreated failed/)
})

test('a throwing notification owner leaves the fallback untouched', async () => {
  const asked = []
  const logged = await withLoggedErrors(async () => {
    const handled = await background.offerNotification('n1', [
      () => { throw new Error('captcha side is broken') },
      (id) => { asked.push(id); return false }
    ])
    assert.equal(handled, false, 'nobody claimed it')
  })
  assert.deepStrictEqual(asked, ['n1'], 'the download side was asked anyway')
  assert.match(logged[0], /a notification owner failed/)
})

test('the first owner that claims a notification ends the chain', async () => {
  const asked = []
  const handled = await background.offerNotification('n2', [
    (id) => { asked.push(['first', id]); return true },
    (id) => { asked.push(['second', id]); return true }
  ])
  assert.equal(handled, true)
  assert.deepStrictEqual(asked, [['first', 'n2']])
})
