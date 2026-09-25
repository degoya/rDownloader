import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createSessionSharer, originPattern, scopeHost, submitCookies, toNetscape } from '../src/session.js'

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

/** A cookie jar that answers `getAll` the way a browser does. */
function fakeCookies(calls, { forUrl = [], forDomain = [] } = {}) {
  return {
    getAll: async (query) => {
      calls.push(['getAll', query])
      return query.url ? forUrl : forDomain
    }
  }
}

test('a share of one page carries the cookie its parent domain set', async () => {
  // `getAll({ domain })` matches that domain and everything below it, never above, so a share
  // of https://www.hoster.com/files missed the login cookie on .hoster.com. `getAll({ url })`
  // is the browser answering which cookies it would send there (RD-109-22).
  const calls = []
  const parent = cookie({ name: 'login', domain: '.hoster.com' })
  const api = {
    permissions: { request: async () => true, remove: async () => true },
    cookies: fakeCookies(calls, { forUrl: [parent], forDomain: [cookie({ name: 'ui', domain: 'www.hoster.com' })] })
  }
  let posted = null
  const sharer = createSessionSharer({
    api,
    loadConfig: async () => CONFIG,
    submit: async (_config, body) => { posted = body; return { ok: true } },
    message: (key) => key,
    notify: async () => {}
  })

  assert.deepEqual(await sharer.share('https://www.hoster.com/files'), { ok: true, code: null })
  assert.deepEqual(calls[0], ['getAll', { url: 'https://www.hoster.com/files' }])
  assert.deepEqual(calls[1], ['getAll', { domain: 'www.hoster.com' }])
  assert.match(posted.cookies, /^\.hoster\.com\t.*\tlogin\tabc$/m)
  assert.match(posted.cookies, /^www\.hoster\.com\t.*\tui\tabc$/m)
})

test('includeSubdomains bounds the read it names', async () => {
  const calls = []
  const api = {
    permissions: { request: async (value) => { calls.push(['request', value]); return true }, remove: async () => true },
    cookies: fakeCookies(calls, {
      forUrl: [cookie({ name: 'login', domain: '.hoster.com' })],
      forDomain: [cookie({ name: 'sub', domain: 'files.hoster.com' })]
    })
  }
  let posted = null
  const sharer = createSessionSharer({
    api,
    loadConfig: async () => CONFIG,
    submit: async (_config, body) => { posted = body; return { ok: true } },
    message: (key) => key,
    notify: async () => {}
  })

  await sharer.share('https://hoster.com/files', { includeSubdomains: false })
  assert.deepEqual(
    calls.map(([name]) => name),
    ['request', 'getAll'],
    'without subdomains the domain query is not made at all'
  )
  // The permission asked for says the same thing the read does.
  assert.deepEqual(calls[0][1].origins, ['*://hoster.com/*'])
  assert.doesNotMatch(posted.cookies, /files\.hoster\.com/)
  assert.equal(posted.includeSubdomains, false)
})

test('sharing asks for permission, posts the cookies and drops the grant again', async () => {
  const calls = []
  const api = {
    permissions: {
      request: async (value) => { calls.push(['request', value]); return true },
      remove: async (value) => { calls.push(['remove', value]); return true }
    },
    cookies: fakeCookies(calls, { forUrl: [cookie()], forDomain: [cookie()] })
  }
  let posted = null
  const sharer = createSessionSharer({
    api,
    loadConfig: async () => CONFIG,
    submit: async (_config, body) => { posted = body; return { ok: true } },
    message: (key) => key,
    notify: async () => {}
  })

  const result = await sharer.share('https://example.com/files')
  assert.deepEqual(result, { ok: true, code: null })
  assert.deepEqual(calls[0], ['request', { permissions: ['cookies'], origins: ['*://*.example.com/*'] }])
  assert.deepEqual(calls[1], ['getAll', { url: 'https://example.com/files' }])
  assert.deepEqual(calls[2], ['getAll', { domain: 'example.com' }])
  // The same cookie from both queries is one row, not two.
  assert.equal(posted.cookies.split('\n').length, 1)
  // Keeping the permission afterwards would be a standing grant nobody asked for.
  assert.equal(calls[3][0], 'remove')
  assert.equal(posted.host, 'example.com')
  assert.match(posted.cookies, /session\tabc$/)
})

test('a site with no cookies is reported, not posted', async () => {
  const calls = []
  const sharer = createSessionSharer({
    api: {
      permissions: { request: async () => true, remove: async (value) => { calls.push(['remove', value]); return true } },
      cookies: fakeCookies(calls)
    },
    loadConfig: async () => CONFIG,
    submit: async () => { throw new Error('must not submit') },
    message: (key) => key,
    notify: async (body) => calls.push(['notify', body])
  })
  assert.deepEqual(await sharer.share('example.com'), { ok: false, code: 'empty' })
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'sessionEmpty'))
})

test('a rejected upload is reported, and a permission that will not go still lets it be', async () => {
  const calls = []
  const sharer = createSessionSharer({
    api: {
      permissions: {
        request: async () => true,
        // Chrome refuses to drop a permission another grant still needs.
        remove: async () => { throw new Error('cannot remove') }
      },
      cookies: fakeCookies(calls, { forUrl: [cookie()], forDomain: [cookie()] })
    },
    loadConfig: async () => CONFIG,
    submit: async () => ({ ok: false, code: 'authprofile.credentials_invalid', message: 'bad' }),
    message: (key, substitutions) => (substitutions ? `${key}:${substitutions.join(',')}` : key),
    notify: async (body) => calls.push(['notify', body])
  })
  assert.deepEqual(await sharer.share('example.com'), { ok: false, code: 'authprofile.credentials_invalid' })
  assert.ok(
    calls.some(([name, body]) => name === 'notify' && body === 'sessionFailed:bad'),
    'the outcome of the upload is reported even though the permission would not go'
  )
})

test('a refused permission reads no cookies at all', async () => {
  let read = false
  const sharer = createSessionSharer({
    api: {
      permissions: { request: async () => false, remove: async () => true },
      cookies: { getAll: async () => { read = true; return [cookie()] } }
    },
    loadConfig: async () => CONFIG,
    submit: async () => { throw new Error('must not submit') },
    message: (key) => key,
    notify: async () => {}
  })
  assert.deepEqual(await sharer.share('example.com'), { ok: false, code: 'denied' })
  assert.equal(read, false)
})

test('an unpaired extension never touches cookies', async () => {
  let requested = false
  const sharer = createSessionSharer({
    api: {
      permissions: { request: async () => { requested = true; return true }, remove: async () => true },
      cookies: { getAll: async () => [cookie()] }
    },
    loadConfig: async () => ({ server: CONFIG.server, token: '' }),
    submit: async () => { throw new Error('must not submit') },
    message: (key) => key,
    notify: async () => {}
  })
  assert.deepEqual(await sharer.share('example.com'), { ok: false, code: 'unconfigured' })
  assert.equal(requested, false)
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
