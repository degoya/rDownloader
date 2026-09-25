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

const root = join(dirname(fileURLToPath(import.meta.url)), '..')

const registered = []
const attempts = []
const calls = []
const session = {}
const state = {
  config: { server: 'http://127.0.0.1:8710', token: 'capture-token', interceptDownloads: true },
  respond: async () => ({ ok: true, status: 201, json: async () => ({ candidates: [{}, {}] }) }),
  notificationsWork: true
}

function handlerFor(name) {
  const entry = registered.find((candidate) => candidate.name === name)
  assert.ok(entry, `nothing is registered for ${name}`)
  return entry.handler
}

function handlersFor(name) {
  return registered.filter((candidate) => candidate.name === name).map((candidate) => candidate.handler)
}

function event(name) {
  return { addListener: (handler, ...rest) => registered.push({ name, handler, rest }) }
}

/**
 * Firefox refuses `extraHeaders` on these events, which cost a release once (RD-108-22). This
 * one refuses it too, so the retry in `addRequestListener` is walked at load rather than taken
 * on trust.
 */
function pickyEvent(name) {
  return {
    addListener: (handler, filter, spec) => {
      attempts.push({ name, spec })
      if (spec?.includes('extraHeaders')) throw new Error('extraHeaders is not supported')
      registered.push({ name, handler, rest: [filter, spec] })
    }
  }
}

/**
 * Every asynchronous browser call in this fake settles on a *later* task than module
 * evaluation. That is what makes the registration assertion deterministic: a registration that
 * waits for one of them cannot slip into the snapshot taken right after the import.
 */
const later = (value) => new Promise((resolve) => setTimeout(() => resolve(value), 0))

globalThis.fetch = async (url, init) => {
  calls.push(['fetch', String(url)])
  return state.respond(url, init)
}

globalThis.chrome = {
  runtime: {
    id: 'self',
    getURL: (path) => `chrome-extension://self/${path}`,
    openOptionsPage: () => calls.push(['openOptionsPage']),
    onInstalled: event('runtime.onInstalled'),
    onStartup: event('runtime.onStartup'),
    onMessage: event('runtime.onMessage')
  },
  contextMenus: {
    create: (options) => calls.push(['menu', options.id, options.title, options.contexts.join(',')]),
    removeAll: (done) => { calls.push(['removeAllMenus']); done() },
    onClicked: event('contextMenus.onClicked')
  },
  storage: {
    local: { get: (defaults) => later({ ...defaults, ...state.config }), set: () => later() },
    session: {
      get: (defaults) => later(Object.fromEntries(Object.keys(defaults).map((key) => [key, session[key] ?? defaults[key]]))),
      set: (values) => { Object.assign(session, values); return later() }
    },
    onChanged: event('storage.onChanged')
  },
  alarms: { create: () => later(), clear: () => later(), onAlarm: event('alarms.onAlarm') },
  tabs: { create: () => later({ id: 1 }), update: () => later(), remove: () => later(), onUpdated: event('tabs.onUpdated'), onRemoved: event('tabs.onRemoved') },
  downloads: { onCreated: event('downloads.onCreated') },
  notifications: {
    create: (options) => {
      if (!state.notificationsWork) return Promise.reject(new Error('notifications are off'))
      calls.push(['notify', options.title, options.message])
      return later('n1')
    },
    clear: () => later(),
    onButtonClicked: event('notifications.onButtonClicked'),
    onClicked: event('notifications.onClicked')
  },
  webRequest: {
    onBeforeRequest: event('webRequest.onBeforeRequest'),
    onSendHeaders: pickyEvent('webRequest.onSendHeaders'),
    onHeadersReceived: pickyEvent('webRequest.onHeadersReceived')
  },
  permissions: {
    contains: () => later(true),
    request: () => later(true),
    remove: () => later(true),
    onAdded: event('permissions.onAdded')
  },
  cookies: {
    getAll: (query) => {
      calls.push(['getAll', query.url ?? query.domain])
      return later([{ name: 'session', value: 'abc', domain: '.hoster.test', path: '/', secure: true }])
    }
  },
  action: {
    setBadgeText: (options) => { calls.push(['badge', options.text]); return later() },
    setBadgeBackgroundColor: () => later()
  },
  i18n: { getMessage: (key, substitutions) => (substitutions ? `${key}:${substitutions.join(',')}` : key) }
}

const background = await import('../src/background.js')
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

/**
 * Lets every dispatched handler finish before the assertions look at what it did.
 *
 * Each faked browser call settles on its own task, so a handler that makes eight of them in a
 * row needs eight turns of the loop; draining a fixed number of turns is what makes that
 * deterministic rather than a guess at a duration.
 */
async function settle(turns = 40) {
  for (let turn = 0; turn < turns; turn += 1) await new Promise((resolve) => setTimeout(resolve, 0))
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

test('without filterResponseData nothing is registered as blocking', () => {
  // Chrome: the response copy of RD-130-16 does not exist there, and a blocking listener would
  // be refused outright. `files.test.mjs` drives the Firefox half.
  for (const entry of registered.filter((candidate) => candidate.name.startsWith('webRequest.'))) {
    assert.ok(!(entry.rest[1] ?? []).includes('blocking'), `${entry.name} is registered as blocking`)
  }
})

test('a browser that refuses extraHeaders still gets its listener', () => {
  // The Firefox path RD-108-22 paid for: ask for `extraHeaders`, and register without it when
  // the browser throws rather than losing the listener.
  const tries = attempts.filter((attempt) => attempt.name === 'webRequest.onSendHeaders')
  assert.deepEqual(tries.map((attempt) => attempt.spec), [
    ['requestHeaders', 'extraHeaders'],
    ['requestHeaders']
  ])
  assert.equal(registered.filter((entry) => entry.name === 'webRequest.onSendHeaders').length, 1)
  // `onBeforeRequest` asks for nothing at all — no `extraHeaders`, and no request body.
  const before = registered.find((entry) => entry.name === 'webRequest.onBeforeRequest')
  assert.equal(before.rest[1], undefined)
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
  assert.deepEqual(calls, [
    ['removeAllMenus'],
    ['menu', 'rdownloader-link', 'menuLink', 'link'],
    ['menu', 'rdownloader-page', 'menuPage', 'page'],
    ['menu', 'rdownloader-selection', 'menuSelection', 'selection'],
    // Sharing a session needs the click as its user gesture, so it lives on the page context.
    ['menu', 'rdownloader-session', 'menuSession', 'page']
  ])
})

test('each menu entry sends what it is about', async () => {
  const sent = []
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
  clicked({ menuItemId: 'rdownloader-session', pageUrl: 'https://hoster.test/' }, { title: 'A page' })
  await settle()
  assert.ok(calls.some(([name, url]) => name === 'fetch' && url.endsWith('/api/v1/capture/cookies')))

  // A menu entry with nothing to send sends nothing.
  calls.length = 0
  clicked({ menuItemId: 'rdownloader-link' }, {})
  clicked({ menuItemId: 'something-else', linkUrl: 'https://hoster.test/c' }, {})
  await settle()
  assert.deepEqual(calls, [])
  state.respond = async () => ({ ok: true, status: 201, json: async () => ({ candidates: [{}, {}] }) })
})

test('a send reports the count, the token and a plain failure apart', async () => {
  const sendMessage = handlerFor('runtime.onMessage')

  calls.length = 0
  let answer = null
  assert.equal(sendMessage({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, {}, (value) => { answer = value }), true)
  await settle()
  assert.deepEqual(answer, { ok: true })
  assert.ok(calls.some(([name, , body]) => name === 'notify' && body === 'sentLinks:2'))
  assert.deepEqual(calls.filter(([name]) => name === 'badge'), [], 'a success does not repaint a badge nobody changed')

  calls.length = 0
  state.respond = async () => ({ ok: false, status: 401, json: async () => ({ error: 'no', code: 'capture.token_required' }) })
  sendMessage({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, {}, () => {})
  await settle()
  assert.ok(calls.some(([name, , body]) => name === 'notify' && body === 'sendFailed:errorUnauthorized'))
  assert.deepEqual(calls.filter(([name]) => name === 'badge'), [['badge', '!']])

  calls.length = 0
  state.respond = async () => ({ ok: false, status: 500, json: async () => ({ error: 'server exploded' }) })
  sendMessage({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, {}, () => {})
  await settle()
  assert.ok(calls.some(([name, , body]) => name === 'notify' && body === 'sendFailed:server exploded'))

  // Back to success: the badge gives the "!" up again rather than keeping it for good.
  calls.length = 0
  state.respond = async () => ({ ok: true, status: 201, json: async () => ({ candidates: [{}, {}] }) })
  sendMessage({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, {}, () => {})
  await settle()
  assert.deepEqual(calls.filter(([name]) => name === 'badge'), [['badge', '']])
})

test('an unpaired extension opens its options instead of sending', async () => {
  calls.length = 0
  state.config = { ...state.config, token: '' }
  handlerFor('runtime.onMessage')({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, {}, () => {})
  await settle()
  assert.ok(!calls.some(([name]) => name === 'fetch'), 'nothing is posted without a token')
  assert.ok(calls.some(([name, , body]) => name === 'notify' && body === 'notConfigured'))
  assert.ok(calls.some(([name]) => name === 'openOptionsPage'))
  state.config = { ...state.config, token: 'capture-token' }
})

test('a browser without notifications still does the work', async () => {
  calls.length = 0
  state.notificationsWork = false
  handlerFor('runtime.onMessage')({ type: 'rdownloader:send', text: 'https://hoster.test/a' }, {}, () => {})
  await settle()
  assert.ok(calls.some(([name]) => name === 'fetch'), 'the links still go out')
  state.notificationsWork = true
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
