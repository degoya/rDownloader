import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createSessionSharer, originPattern, scopeHost, submitCookies, toNetscape } from '../src/session.js'

import { fakeBrowser } from './fake-browser.mjs'

const CONFIG = { server: 'http://127.0.0.1:8710', token: 'capture-token' }

function cookie(overrides = {}) {
  return { name: 'session', value: 'abc', domain: '.example.com', path: '/', secure: true, httpOnly: false, expirationDate: 2000000000.7, ...overrides }
}

test('scopeHost accepts bare hosts and full URLs alike', () => {
  assert.equal(scopeHost('https://Files.Example.COM/reports?x=1'), 'files.example.com')
  assert.equal(scopeHost('example.com'), 'example.com')
  assert.equal(scopeHost('example.com.'), 'example.com')
  assert.equal(scopeHost('  '), null)
  assert.equal(scopeHost('not a url at all /'), null)
})

test('permission is requested for one origin, never for all sites', () => {
  assert.equal(originPattern('example.com'), '*://*.example.com/*')
  // The pattern says exactly what the read does: no subdomains asked for, none covered.
  assert.equal(originPattern('example.com', { includeSubdomains: false }), '*://example.com/*')
})

test('cookies serialise to the Netscape format the server parses', () => {
  const rows = toNetscape([cookie(), cookie({ name: 'pref', domain: 'example.com', secure: false, httpOnly: true, expirationDate: undefined })], 'example.com').split('\n')
  assert.equal(rows[0], '.example.com\tTRUE\t/\tTRUE\t2000000000\tsession\tabc')
  // httpOnly rows carry the prefix the importer understands, and a session cookie is 0.
  assert.equal(rows[1], '#HttpOnly_example.com\tFALSE\t/\tFALSE\t0\tpref\tabc')
})

test('prefixed cookies travel like any other', () => {
  // `__Host-` and `__Secure-` are ordinary transferable cookies under extra attribute rules,
  // not something only the setting page can use. A hoster whose session cookie is called
  // `__Secure-session` used to get an exported profile that authenticated nobody (RD-109-22).
  const rows = toNetscape(
    [cookie({ name: '__Secure-session' }), cookie({ name: '__Host-id', domain: 'example.com' })],
    'example.com'
  ).split('\n')
  assert.equal(rows[0], '.example.com\tTRUE\t/\tTRUE\t2000000000\t__Secure-session\tabc')
  assert.equal(rows[1], 'example.com\tFALSE\t/\tTRUE\t2000000000\t__Host-id\tabc')
})

/** One share in a browser holding `jar`, clicked as a person does. */
async function share(target, { scope = 'https://example.com/files', options, jar = [cookie()], config = CONFIG, submit, answer, prepare } = {}) {
  const calls = []
  const browser = fakeBrowser(target, { calls, answer, cookies: jar })
  prepare?.(browser)
  const posted = []
  const sharer = createSessionSharer({
    api: browser.api,
    loadConfig: async () => config,
    submit: submit ?? (async (_config, body) => { posted.push(body); return { ok: true } }),
    message: (key, substitutions) => (substitutions ? `${key}:${substitutions.join(',')}` : key),
    notify: async (body) => calls.push(['notify', body])
  })
  const result = browser.gesture(() => sharer.share(scope, options))
  const prompted = browser.prompts.length
  return { browser, calls, posted, prompted, sharer, result: await result, notices: browser.notices() }
}

const BUILT = { firefox: fakeBrowser('firefox').held(), chrome: fakeBrowser('chrome').held() }

for (const target of ['firefox', 'chrome']) {
  test(`${target}: sharing asks in the click itself, posts the cookies and drops the grant again`, async () => {
    // The settings used to be read before the request; Firefox then refused it, and the share
    // ended as refused without a word (RD-1480-01).
    const { browser, calls, posted, prompted, result, notices } = await share(target)
    assert.equal(prompted, 1, 'the prompt is there before the click handler returned')
    assert.deepEqual(result, { ok: true, code: null })
    const asked = { permissions: ['cookies'], origins: ['*://*.example.com/*'], ...(target === 'firefox' ? { data_collection: ['authenticationInfo'] } : {}) }
    assert.deepEqual(calls.slice(0, 4), [
      ['request', asked],
      ['getAll', { url: 'https://example.com/files' }],
      ['getAll', { domain: 'example.com' }],
      // Keeping the permission afterwards would be a standing grant nobody asked for.
      ['remove', asked]
    ])
    assert.deepEqual(browser.held(), BUILT[target])
    // The same cookie from both queries is one row, not two.
    assert.equal(posted[0].cookies.split('\n').length, 1)
    assert.equal(posted[0].host, 'example.com')
    assert.match(posted[0].cookies, /session\tabc$/)
    assert.deepEqual(notices, ['sessionShared:example.com'])
  })

  test(`${target}: a refused permission reads no cookies at all, and is said`, async () => {
    const { calls, posted, result, notices } = await share(target, { answer: () => false })
    assert.deepEqual(result, { ok: false, code: 'denied' })
    assert.deepEqual(notices, ['handoverDenied:example.com'])
    assert.ok(!calls.some(([name]) => name === 'getAll'))
    assert.deepEqual(posted, [])
  })

  test(`${target}: an unpaired extension never touches cookies and keeps no grant`, async () => {
    const { browser, calls, posted, result, notices } = await share(target, { config: { ...CONFIG, token: '' } })
    assert.deepEqual(result, { ok: false, code: 'unconfigured' })
    assert.deepEqual(notices, ['sessionUnconfigured'])
    assert.ok(!calls.some(([name]) => name === 'getAll'))
    assert.deepEqual(posted, [])
    assert.deepEqual(browser.held(), BUILT[target], 'what was granted is given back')
  })
}

test('a request the browser refuses outright is a refusal, never a crash', async () => {
  const { sharer, notices, calls } = await share('firefox', { answer: () => false })
  // Firefox, once the click is over.
  assert.deepEqual(await sharer.share('https://example.com/files'), { ok: false, code: 'denied' })
  assert.equal(notices.length, 1)
  assert.equal(calls.filter(([name]) => name === 'notify').length, 2, 'said as well')
})

/** A jar that answers the two queries apart, as a browser does. */
function jarOf(browser, { forUrl = [], forDomain = [] }) {
  const calls = browser.calls
  Object.defineProperty(browser.api, 'cookies', {
    get: () => ({ getAll: async (query) => { calls.push(['getAll', query]); return query.url ? forUrl : forDomain } })
  })
}

test('a share of one page carries the cookie its parent domain set', async () => {
  // `getAll({ domain })` matches that domain and everything below it, never above, so a share
  // of https://www.hoster.com/files missed the login cookie on .hoster.com. `getAll({ url })`
  // is the browser answering which cookies it would send there (RD-109-22).
  const { calls, posted, result } = await share('chrome', {
    scope: 'https://www.hoster.com/files',
    prepare: (browser) => jarOf(browser, { forUrl: [cookie({ name: 'login', domain: '.hoster.com' })], forDomain: [cookie({ name: 'ui', domain: 'www.hoster.com' })] })
  })
  assert.deepEqual(result, { ok: true, code: null })
  assert.deepEqual(calls.filter(([name]) => name === 'getAll'), [['getAll', { url: 'https://www.hoster.com/files' }], ['getAll', { domain: 'www.hoster.com' }]])
  assert.match(posted[0].cookies, /^\.hoster\.com\t.*\tlogin\tabc$/m)
  assert.match(posted[0].cookies, /^www\.hoster\.com\t.*\tui\tabc$/m)
})

test('includeSubdomains bounds the read it names', async () => {
  const { calls, posted } = await share('chrome', {
    scope: 'https://hoster.com/files',
    options: { includeSubdomains: false },
    prepare: (browser) => jarOf(browser, { forUrl: [cookie({ name: 'login', domain: '.hoster.com' })], forDomain: [cookie({ name: 'sub', domain: 'files.hoster.com' })] })
  })
  assert.deepEqual(calls.filter(([name]) => name !== 'remove' && name !== 'notify').map(([name]) => name), ['request', 'getAll'], 'without subdomains the domain query is not made at all')
  // The permission asked for says the same thing the read does.
  assert.deepEqual(calls[0][1].origins, ['*://hoster.com/*'])
  assert.doesNotMatch(posted[0].cookies, /files\.hoster\.com/)
  assert.equal(posted[0].includeSubdomains, false)
})

test('a site with no cookies is reported, not posted, and keeps no grant', async () => {
  for (const target of ['firefox', 'chrome']) {
    const { browser, result, notices, posted } = await share(target, { scope: 'example.com', jar: [] })
    assert.deepEqual(result, { ok: false, code: 'empty' })
    assert.deepEqual(notices, ['sessionEmpty'])
    assert.deepEqual(posted, [])
    assert.deepEqual(browser.held(), BUILT[target], 'the grant goes back here as on every other way out')
  }
})

test('a rejected upload is reported, and a permission that will not go still lets it be', async () => {
  const { result, notices } = await share('chrome', {
    scope: 'example.com',
    submit: async () => ({ ok: false, code: 'authprofile.credentials_invalid', message: 'bad' }),
    // Chrome refuses to drop a permission another grant still needs.
    prepare: (browser) => { browser.api.permissions.remove = async () => { throw new Error('cannot remove') } }
  })
  assert.deepEqual(result, { ok: false, code: 'authprofile.credentials_invalid' })
  assert.deepEqual(notices, ['sessionFailed:bad'], 'the outcome of the upload is reported even though the permission would not go')
})

test('submitCookies talks to the capture endpoint with the capture token', async () => {
  let seen = null
  const response = { ok: true, status: 201, json: async () => ({ id: 'x', enabled: false }) }
  const result = await submitCookies(CONFIG, { host: 'example.com', includeSubdomains: true, cookies: 'a\tb', name: 'Example' },
    async (url, init) => { seen = { url, init }; return response })
  assert.equal(result.ok, true)
  assert.equal(seen.url, 'http://127.0.0.1:8710/api/v1/capture/cookies')
  assert.equal(seen.init.headers.authorization, 'Bearer capture-token')
  assert.deepEqual(JSON.parse(seen.init.body), {
    name: 'Example', scope: 'example.com', include_subdomains: true, cookies: 'a\tb'
  })
  // The server decides the profile stays disabled; the extension just reports it back.
  assert.equal(result.profile.enabled, false)
})

test('a rejected upload surfaces the server code', async () => {
  const result = await submitCookies(CONFIG, { host: 'example.com', includeSubdomains: true, cookies: 'x' },
    async () => ({ ok: false, status: 400, json: async () => ({ code: 'authprofile.credentials_invalid', error: 'bad' }) }))
  assert.deepEqual(result, { ok: false, status: 400, code: 'authprofile.credentials_invalid', message: 'bad' })
})
