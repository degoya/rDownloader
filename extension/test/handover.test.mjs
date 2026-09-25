// RD-120-45: a browser's session at a provider, handed over to one rDownloader account.
//
// What these hold: nothing is read without the person's click on that one request; the site is
// the one the service names, never one a page or the popup supplies; and no cookie of any other
// domain leaves the browser.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  CONSENT_MESSAGE,
  DECLINE_MESSAGE,
  DELIVER_MESSAGE,
  POLL_MESSAGE,
  cookieInScope,
  createHandover,
  deliverHandover,
  listHandovers,
  readScopeCookies,
  scopeOrigin
} from '../src/handover.js'
import { handoverListFailure, handoverOutcome } from '../src/popup.js'

import { CONFIG, POPUP, fakeApi, names } from './fakes.mjs'

const HANDOVER = {
  id: '019d0000-0000-7000-8000-0000000000a1',
  provider: 'ddownload',
  provider_name: 'DDownload',
  account_label: 'Main',
  scope: 'https://ddownload.com/',
  host: 'ddownload.com',
  expires_at: '2026-09-23T20:05:00Z'
}

/** Every cookie the fake browser holds: the scope's, and a good many that must stay here. */
const JAR = [
  { name: 'xfss', value: 'session-value', domain: '.ddownload.com', path: '/', secure: true, httpOnly: true },
  { name: 'lang', value: 'en', domain: 'ddownload.com', path: '/account', secure: true },
  { name: 'sid', value: 'google-session', domain: '.google.com', path: '/', secure: true },
  { name: 'cdn', value: 'cdn-token', domain: 'files.ddownload.com', path: '/', secure: true },
  { name: 'fake', value: 'lookalike', domain: 'evil-ddownload.com', path: '/', secure: true },
  { name: 'suffix', value: 'suffix', domain: 'ddownload.com.evil.tld', path: '/', secure: true }
]

/** The fake browser, with a cookie store that answers every query with the whole jar. */
function browser(calls, { granted = true, holds = true, jar = JAR } = {}) {
  const api = fakeApi(calls, { granted })
  api.cookies = { getAll: async (query) => { calls.push(['getAll', query]); return jar } }
  api.permissions.contains = async (value) => { calls.push(['contains', value]); return holds }
  return api
}

function setup({ calls, api, handovers = [HANDOVER], deliver } = {}) {
  const delivered = []
  const handover = createHandover({
    api,
    loadConfig: async () => CONFIG,
    message: (key, substitutions) => (substitutions ? `${key}:${substitutions.join(',')}` : key),
    notify: async (body) => { calls.push(['notify', body]); return 'n1' },
    list: async () => { calls.push(['list']); return { ok: true, status: 200, handovers } },
    deliver: deliver ?? (async (_config, id, cookies) => {
      delivered.push({ id, cookies })
      calls.push(['deliver', id])
      return { ok: true, status: 200, code: 'browser_session.delivered' }
    }),
    decline: async (_config, id) => { calls.push(['decline', id]); return { ok: true, status: 200, code: 'browser_session.declined' } }
  })
  // The popup's messages cross a real message boundary to the background's handler.
  api.messageRouter = (payload, sender) => handover.onMessage(payload, sender)
  return { handover, delivered }
}

test('only the scope host and the domains above it pass; neighbours, look-alikes and subdomains do not', () => {
  const kept = JAR.filter((cookie) => cookieInScope(cookie, 'ddownload.com')).map((cookie) => cookie.name)
  assert.deepEqual(kept, ['xfss', 'lang'])
  // A login cookie on the parent domain is what a www host is signed in with.
  assert.equal(cookieInScope({ domain: '.hoster.com' }, 'www.hoster.com'), true)
  assert.equal(cookieInScope({ domain: '' }, 'hoster.com'), false)
  assert.equal(cookieInScope({ domain: 'hoster.com' }, ''), false)
})

test('the read keeps only the scope\'s cookies, whatever the browser answers', async () => {
  const calls = []
  const api = browser(calls)
  const cookies = await readScopeCookies(api, 'https://ddownload.com/')
  assert.deepEqual(cookies.map((cookie) => cookie.name).sort(), ['lang', 'xfss'])
  assert.deepEqual(calls.filter(([name]) => name === 'getAll').map(([, query]) => query), [
    { url: 'https://ddownload.com/' },
    { domain: 'ddownload.com' }
  ])
})

test('a scope that is not https names no origin, and nothing is read for it', async () => {
  assert.equal(scopeOrigin('https://ddownload.com/'), 'https://ddownload.com/*')
  assert.equal(scopeOrigin('http://ddownload.com/'), null)
  assert.equal(scopeOrigin('not a url'), null)
  const calls = []
  const api = browser(calls)
  const { handover } = setup({ calls, api })
  assert.deepEqual(await handover.consent({ ...HANDOVER, scope: 'http://ddownload.com/' }), { ok: false, code: 'scope' })
  assert.ok(!names(calls).includes('request'))
})

test('a consented handover sends the scope\'s cookies and no other domain\'s, then gives the grant back', async () => {
  const calls = []
  const api = browser(calls)
  const { handover, delivered } = setup({ calls, api })

  const result = await handover.consent(HANDOVER)

  assert.deepEqual(result, { ok: true, code: 'browser_session.delivered' })
  // The browser's own prompt, for `cookies` and exactly the one origin, and nothing broader.
  assert.deepEqual(calls.find(([name]) => name === 'request')[1], { permissions: ['cookies'], origins: ['https://ddownload.com/*'] })
  assert.equal(delivered.length, 1)
  const rows = delivered[0].cookies.split('\n')
  assert.equal(rows.length, 2)
  for (const row of rows) {
    const domain = row.replace(/^#HttpOnly_/, '').split('\t')[0]
    assert.ok(domain === '.ddownload.com' || domain === 'ddownload.com', `${domain} left the browser`)
  }
  for (const foreign of ['google-session', 'cdn-token', 'lookalike', 'suffix']) {
    assert.ok(!delivered[0].cookies.includes(foreign), `${foreign} left the browser`)
  }
  // `cookies` goes back, and the origin too: it was not held before this consent.
  assert.deepEqual(calls.find(([name]) => name === 'remove')[1], { permissions: ['cookies'], origins: ['https://ddownload.com/*'] })
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'handoverDone:ddownload.com'))
  // No cookie value appears in anything but the delivery itself.
  assert.ok(!calls.some(([, value]) => typeof value === 'string' && value.includes('session-value')))
})

test('an origin that was already granted is kept after the handover', async () => {
  const calls = []
  const api = browser(calls)
  const { handover } = setup({ calls, api })
  await handover.consent(HANDOVER, { hadOrigin: true })
  assert.deepEqual(calls.find(([name]) => name === 'remove')[1], { permissions: ['cookies'], origins: [] })
})

test('the permission request is the first thing the click does, before any await', async () => {
  const calls = []
  const api = browser(calls)
  const { handover } = setup({ calls, api })
  const pending = handover.consent(HANDOVER)
  // Synchronously after the call: the request is out, and the consent message is not yet.
  assert.deepEqual(names(calls), ['request'])
  await pending
})

test('a refused permission reads nothing and delivers nothing', async () => {
  const calls = []
  const api = browser(calls, { granted: false, holds: false })
  const { handover, delivered } = setup({ calls, api })
  assert.deepEqual(await handover.consent(HANDOVER), { ok: false, code: 'denied' })
  assert.ok(!names(calls).includes('getAll'))
  assert.deepEqual(delivered, [])
})

test('without a recorded consent nothing is read, even with the grant held', async () => {
  const calls = []
  const api = browser(calls)
  const { handover, delivered } = setup({ calls, api })
  assert.deepEqual(await handover.onMessage({ type: DELIVER_MESSAGE, id: HANDOVER.id }, POPUP), { ok: false, code: 'consent' })
  assert.ok(!names(calls).includes('getAll'))
  assert.deepEqual(delivered, [])
})

test('a page script or a content script cannot consent, deliver, decline or list', async () => {
  const calls = []
  const api = browser(calls)
  const { handover, delivered } = setup({ calls, api })
  const page = { id: 'self', tab: { id: 7 }, url: 'https://ddownload.com/account' }
  const stranger = { id: 'another-extension' }
  for (const sender of [page, stranger, undefined]) {
    for (const type of [CONSENT_MESSAGE, DELIVER_MESSAGE, DECLINE_MESSAGE, POLL_MESSAGE]) {
      const payload = { type, id: HANDOVER.id, origin: 'https://ddownload.com/*' }
      assert.equal(await handover.onMessage(payload, sender), false, `${type} from ${JSON.stringify(sender)}`)
    }
  }
  assert.deepEqual(api.session.handoverConsents ?? {}, {})
  assert.ok(!names(calls).includes('getAll'))
  assert.ok(!names(calls).includes('decline'))
  assert.deepEqual(delivered, [])
})

test('a consent for another origin than the service names is not a consent for this scope', async () => {
  const calls = []
  const api = browser(calls)
  const { handover, delivered } = setup({ calls, api })
  // The popup is ours, but what it named is not what the service says this request is for.
  await handover.onMessage({ type: CONSENT_MESSAGE, id: HANDOVER.id, origin: 'https://evil.tld/*' }, POPUP)
  assert.deepEqual(await handover.onMessage({ type: DELIVER_MESSAGE, id: HANDOVER.id }, POPUP), { ok: false, code: 'scope' })
  assert.ok(!names(calls).includes('getAll'))
  assert.deepEqual(delivered, [])
  // And an origin that is not an https origin pattern is not even recorded.
  assert.deepEqual(await handover.onMessage({ type: CONSENT_MESSAGE, id: 'x', origin: '<all_urls>' }, POPUP), { ok: false, code: 'scope' })
})

test('a consent is used once', async () => {
  const calls = []
  const api = browser(calls)
  const { handover, delivered } = setup({ calls, api })
  await handover.consent(HANDOVER)
  assert.deepEqual(await handover.onMessage({ type: DELIVER_MESSAGE, id: HANDOVER.id }, POPUP), { ok: false, code: 'consent' })
  assert.equal(delivered.length, 1)
})

test('a request that stopped waiting is not answered, and its grant is given back on the next poll', async () => {
  const calls = []
  const api = browser(calls)
  let waiting = [HANDOVER]
  const handover = createHandover({
    api,
    loadConfig: async () => CONFIG,
    message: (key) => key,
    notify: async () => null,
    list: async () => ({ ok: true, status: 200, handovers: waiting }),
    deliver: async () => { calls.push(['deliver']); return { ok: true } },
    decline: async () => ({ ok: true })
  })
  await handover.onMessage({ type: CONSENT_MESSAGE, id: HANDOVER.id, origin: 'https://ddownload.com/*', hadOrigin: false }, POPUP)
  waiting = []
  assert.deepEqual(await handover.onMessage({ type: DELIVER_MESSAGE, id: HANDOVER.id }, POPUP), { ok: false, code: 'not_waiting' })
  assert.ok(!names(calls).includes('getAll'))
  await handover.poll()
  assert.deepEqual(calls.find(([name]) => name === 'remove')[1], { permissions: ['cookies'], origins: ['https://ddownload.com/*'] })
  assert.deepEqual(api.session.handoverConsents, {})
})

test('a browser without cookies for the scope says so and sends nothing', async () => {
  const calls = []
  const api = browser(calls, { jar: [JAR[2]] })
  const { handover, delivered } = setup({ calls, api })
  assert.deepEqual(await handover.consent(HANDOVER), { ok: false, code: 'empty' })
  assert.deepEqual(delivered, [])
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'handoverEmpty:ddownload.com'))
  assert.ok(names(calls).includes('remove'), 'the grant is given back on this path too')
})

test('declining from the popup tells the server and reads nothing', async () => {
  const calls = []
  const api = browser(calls)
  const { handover } = setup({ calls, api })
  const result = await handover.onMessage({ type: DECLINE_MESSAGE, id: HANDOVER.id }, POPUP)
  assert.equal(result.ok, true)
  assert.deepEqual(calls.filter(([name]) => name === 'decline'), [['decline', HANDOVER.id]])
  assert.ok(!names(calls).includes('getAll'))
})

test('a waiting request is announced once, and its notification opens the popup', async () => {
  const calls = []
  const api = browser(calls)
  const { handover } = setup({ calls, api })
  await handover.poll()
  await handover.poll()
  const announced = calls.filter(([name]) => name === 'notify')
  assert.deepEqual(announced, [['notify', 'handoverWaiting:ddownload.com,Main']])
  assert.equal(await handover.onNotificationClicked('n1'), true)
  assert.ok(names(calls).includes('openPopup'))
  assert.equal(await handover.onNotificationClicked('n1'), false, 'handled once')
})

test('the popup has its own words for each outcome', () => {
  assert.deepEqual(handoverOutcome({ ok: true }, 'ddownload.com'), { key: 'handoverDone', substitutions: ['ddownload.com'], ok: true })
  assert.equal(handoverOutcome({ ok: false, code: 'denied' }, 'h').key, 'handoverDenied')
  assert.equal(handoverOutcome({ ok: false, code: 'empty' }, 'h').key, 'handoverEmpty')
  assert.deepEqual(handoverOutcome({ ok: false, code: 'browser_session.cookie_outside_scope' }, 'h'), {
    key: 'handoverFailed',
    substitutions: ['h', 'browser_session.cookie_outside_scope'],
    ok: false
  })
})

test('the capture calls go to the configured rDownloader with the capture token, and return no cookie', async () => {
  const seen = []
  const fetchImpl = async (url, init) => {
    seen.push({ url, init })
    return { ok: true, status: 200, json: async () => (init.method === 'GET' ? [HANDOVER] : { code: 'browser_session.delivered', params: { count: '2' } }) }
  }
  const listed = await listHandovers(CONFIG, fetchImpl)
  assert.deepEqual(listed.handovers, [HANDOVER])
  const result = await deliverHandover(CONFIG, HANDOVER.id, 'row-with-session-value', fetchImpl)
  assert.equal(result.ok, true)
  assert.ok(!JSON.stringify(result).includes('session-value'))
  assert.equal(seen[0].url, 'http://127.0.0.1:8710/api/v1/capture/browser-sessions')
  assert.equal(seen[1].url, `http://127.0.0.1:8710/api/v1/capture/browser-sessions/${HANDOVER.id}`)
  for (const { init } of seen) assert.equal(init.headers.authorization, 'Bearer capture-token')
  assert.deepEqual(JSON.parse(seen[1].init.body), { cookies: 'row-with-session-value' })
})

test('an older rDownloader without the feature, or an unpaired extension, gets no error line', () => {
  assert.equal(handoverListFailure({ ok: false, status: 404, message: 'HTTP 404' }), null)
  assert.equal(handoverListFailure({ ok: false, status: 401 }), null)
  assert.equal(handoverListFailure({ ok: true, handovers: [] }), null)
  assert.deepEqual(handoverListFailure({ ok: false, status: 0, message: 'connection refused' }), {
    key: 'handoverListFailed',
    substitutions: ['connection refused']
  })
})
