// RD-130-16: an NZB, torrent or ZIP only the browser could load goes to rDownloader after all —
// for a site the person allowed, and for no other.
//
// What these hold: without a consent nothing changes against RD-120-63; the consent is the
// browser's grant for that one host plus a row naming it, and it can be taken back; only the
// cookies the browser would send to the download's own address leave it; the browser's copy is
// removed only once rDownloader took the file; and a copy Firefox holds back is given to the
// browser in full when rDownloader refuses it.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  MAX_FILE_BYTES,
  SITES_KEY,
  allowSite,
  createFileHandover,
  holdsCookieConsent,
  readAddressCookies,
  revokeSite,
  siteConsented,
  siteHost,
  sitePattern,
  submitFileAddress,
  submitFileBytes
} from '../src/files.js'
import { filesOutcome } from '../src/popup.js'
import { createSessionSharer } from '../src/session.js'

import { CONFIG, names, setupInterceptor } from './fakes.mjs'

const CART = 'https://indexer.test/getnzb/abc'
const HOST = 'indexer.test'

/** The cookies the fake browser would send to the cart: the session, and nothing else. */
const SESSION = [
  { name: 'uid', value: '7', domain: 'indexer.test', path: '/', secure: true },
  { name: 'sess', value: 's3cr3t', domain: '.indexer.test', path: '/', secure: true, httpOnly: true }
]

/**
 * A browser whose grants and `storage.local` are real state: what `permissions.request` grants,
 * `permissions.contains` answers, and `permissions.remove` takes away again.
 */
function browser(calls, { grant = true, sites = [], granted = [], filter = null } = {}) {
  const local = { [SITES_KEY]: sites }
  const grants = new Set(granted)
  const api = {
    runtime: { id: 'self', getURL: (path) => `moz-extension://self/${path}` },
    storage: {
      local: {
        get: async (defaults) => ({ ...defaults, ...structuredClone(local) }),
        set: async (values) => { calls.push(['store', structuredClone(values)]); Object.assign(local, structuredClone(values)) }
      }
    },
    permissions: {
      request: async (value) => {
        calls.push(['request', value])
        if (grant) for (const entry of [...value.permissions, ...value.origins]) grants.add(entry)
        return grant
      },
      contains: async (value) => [...(value.permissions ?? []), ...value.origins].every((entry) => grants.has(entry)),
      remove: async (value) => {
        calls.push(['remove', value])
        for (const entry of [...value.permissions, ...value.origins]) grants.delete(entry)
        return true
      }
    },
    cookies: { getAll: async (query) => { calls.push(['getAll', query]); return SESSION } },
    downloads: {
      pause: async (id) => calls.push(['pause', id]),
      resume: async (id) => calls.push(['resume', id]),
      cancel: async (id) => calls.push(['cancel', id]),
      removeFile: async (id) => calls.push(['removeFile', id]),
      erase: async (query) => calls.push(['erase', query.id])
    },
    webRequest: filter ? { filterResponseData: (requestId) => { calls.push(['filter', requestId]); return filter } } : {}
  }
  return { api, local, grants }
}

const ALLOWED = { sites: [HOST], granted: ['cookies', sitePattern(HOST)] }

/** A StreamFilter that records what the browser would receive. */
function streamFilter(calls) {
  return {
    written: [],
    write(data) { this.written.push(data); calls.push(['write', data.byteLength]) },
    close() { calls.push(['close']) },
    disconnect() { calls.push(['disconnect']) }
  }
}

function setup(calls, { api, bytes, address } = {}) {
  const sent = { bytes: [], address: [] }
  const handover = createFileHandover({
    api,
    loadConfig: async () => ({ ...CONFIG, interceptDownloads: true }),
    message: (key, substitutions) => (substitutions ? `${key}:${substitutions.join(',')}` : key),
    notify: async (body) => { calls.push(['notify', body]); return 'n1' },
    userAgent: 'Mozilla/5.0 (X11; rv:156.0) Firefox/156.0',
    submitBytes: async (config, body) => {
      calls.push(['submitBytes'])
      sent.bytes.push({ config, body })
      return bytes ?? { ok: true, status: 201, kind: 'nzb' }
    },
    submitAddress: async (config, body) => {
      calls.push(['submitAddress'])
      sent.address.push({ config, body })
      return address ?? { ok: true, status: 201, kind: 'nzb' }
    },
    wait: 50
  })
  return { handover, sent }
}

const DOWNLOAD = { id: 9, url: CART, filename: '/home/me/Downloads/Cart.Release.nzb', mime: 'application/x-nzb', referrer: 'https://indexer.test/cart' }

const RESPONSE = {
  requestId: 'r1',
  url: CART,
  type: 'main_frame',
  statusCode: 200,
  responseHeaders: [
    { name: 'Content-Type', value: 'application/x-nzb' },
    { name: 'Content-Disposition', value: 'attachment; filename="Cart.Release.nzb"' }
  ]
}

test('a consent names one host, both schemes, and nothing below it', () => {
  assert.equal(siteHost('https://Indexer.Test./getnzb/abc?zip=1'), HOST)
  assert.equal(siteHost('ftp://indexer.test/x'), null)
  assert.equal(siteHost('not an address'), null)
  assert.equal(sitePattern(HOST), '*://indexer.test/*')
})

test('allowing asks the browser first and records the site only once it granted', async () => {
  const calls = []
  const { api, local } = browser(calls)
  const result = await allowSite(api, CART)
  assert.deepEqual(result, { ok: true, code: null, host: HOST })
  // The request is the first thing the click does, so it is still the click's gesture.
  assert.deepEqual(calls[0], ['request', { permissions: ['cookies'], origins: ['*://indexer.test/*'] }])
  assert.deepEqual(local[SITES_KEY], [HOST])
  assert.equal(await siteConsented(api, CART), true)
  assert.equal(await siteConsented(api, 'https://www.indexer.test/getnzb/abc'), false, 'one host, not its neighbours')
})

test('a refused permission records nothing', async () => {
  const calls = []
  const { api, local } = browser(calls, { grant: false })
  assert.deepEqual(await allowSite(api, CART), { ok: false, code: 'denied', host: HOST })
  assert.deepEqual(local[SITES_KEY], [])
  assert.ok(!names(calls).includes('store'))
  assert.deepEqual(filesOutcome({ ok: false, code: 'denied', host: HOST }), { key: 'filesDenied', substitutions: [HOST], ok: false })
})

test('revoking takes the row and the grant back, and cookies with the last site', async () => {
  const calls = []
  const { api, local, grants } = browser(calls, {
    sites: [HOST, 'other.test'],
    granted: ['cookies', sitePattern(HOST), sitePattern('other.test')]
  })
  await revokeSite(api, HOST)
  assert.deepEqual(local[SITES_KEY], ['other.test'])
  assert.deepEqual(calls.at(-1), ['remove', { permissions: [], origins: ['*://indexer.test/*'] }])
  assert.equal(await siteConsented(api, CART), false)
  assert.ok(grants.has('cookies'), 'another site still stands on cookies')
  await revokeSite(api, 'other.test')
  assert.deepEqual(calls.at(-1), ['remove', { permissions: ['cookies'], origins: ['*://other.test/*'] }])
  assert.equal(await holdsCookieConsent(api), false)
  assert.deepEqual(filesOutcome({ ok: true, host: HOST, revoked: true }), { key: 'filesRevoked', substitutions: [HOST], ok: true })
})

test('only what the browser would send to that address is read, as name and value', async () => {
  const calls = []
  const { api } = browser(calls, ALLOWED)
  const cookies = await readAddressCookies(api, CART)
  assert.deepEqual(calls, [['getAll', { url: CART }]], 'no domain-wide read, nothing below the host')
  assert.deepEqual(cookies, SESSION)
  const { api: without } = browser([], { sites: [HOST], granted: [sitePattern(HOST)] })
  assert.equal(await readAddressCookies(without, CART), null, 'no cookies grant, no read')
})

test('the two requests carry what the route expects', async () => {
  const seen = []
  const fetchImpl = async (url, init) => {
    seen.push({ url, init })
    return { ok: true, status: 201, json: async () => ({ kind: 'nzb_zip' }) }
  }
  const byAddress = await submitFileAddress(CONFIG, { url: CART, cookies: SESSION, referrer: 'https://indexer.test/cart', userAgent: 'UA', fileName: 'cart.zip' }, fetchImpl)
  assert.deepEqual(byAddress, { ok: true, status: 201, code: null, message: null, kind: 'nzb_zip' })
  assert.equal(seen[0].url, 'http://127.0.0.1:8710/api/v1/capture/file')
  assert.deepEqual(JSON.parse(seen[0].init.body), {
    url: CART,
    // The rows `capture/cookies` takes, with what the service checks against the address.
    cookies: 'indexer.test\tFALSE\t/\tTRUE\t0\tuid\t7\n#HttpOnly_.indexer.test\tTRUE\t/\tTRUE\t0\tsess\ts3cr3t',
    referrer: 'https://indexer.test/cart',
    user_agent: 'UA',
    file_name: 'cart.zip'
  })
  assert.equal(seen[0].init.headers.authorization, 'Bearer capture-token')
  await submitFileBytes(CONFIG, { bytes: new Blob(['<nzb/>']), fileName: 'Release.nzb' }, fetchImpl)
  await submitFileAddress(CONFIG, { url: CART, cookies: [] }, fetchImpl)
  assert.deepEqual(JSON.parse(seen[2].init.body), { url: CART }, 'no cookies, no field')
  const form = seen[1].init.body
  assert.ok(form instanceof FormData)
  assert.equal(await form.get('file').text(), '<nzb/>')
  assert.equal(form.get('file_name'), 'Release.nzb')
  const refused = await submitFileBytes(CONFIG, { bytes: new Blob(['x']) }, async () => ({ ok: false, status: 400, json: async () => ({ code: 'capture.file_unsupported', error: 'no' }) }))
  assert.equal(refused.code, 'capture.file_unsupported')
})

test('without a consent an NZB stays with the browser, untouched and silent', async () => {
  const calls = []
  const { api } = browser(calls)
  const { handover } = setup(calls, { api })
  assert.equal(await handover.onBrowserOnlyDownload(DOWNLOAD), false)
  assert.deepEqual(calls, [], 'no read, no pause, no request, no notice')
})

test('with a consent the address goes over with its cookies, and the browser copy goes', async () => {
  const calls = []
  const { api } = browser(calls, ALLOWED)
  const { handover, sent } = setup(calls, { api })
  assert.equal(await handover.onBrowserOnlyDownload(DOWNLOAD), true)
  assert.deepEqual(names(calls), ['getAll', 'pause', 'submitAddress', 'cancel', 'removeFile', 'erase', 'notify'])
  assert.deepEqual(sent.address[0].body, {
    url: CART,
    cookies: SESSION,
    referrer: 'https://indexer.test/cart',
    userAgent: 'Mozilla/5.0 (X11; rv:156.0) Firefox/156.0',
    fileName: 'Cart.Release.nzb'
  })
  assert.deepEqual(calls.at(-1), ['notify', 'fileHandedOver'])
  assert.ok(!JSON.stringify(calls.filter(([name]) => name === 'notify')).includes('s3cr3t'), 'no cookie value in a notice')
})

test('a refused hand-over leaves the browser its copy and says why, unless it was no file for us', async () => {
  const calls = []
  const { api } = browser(calls, ALLOWED)
  const { handover } = setup(calls, { api, address: { ok: false, status: 502, code: 'capture.fetch_failed', message: 'The address could not be fetched' } })
  assert.equal(await handover.onBrowserOnlyDownload(DOWNLOAD), false)
  assert.deepEqual(names(calls), ['getAll', 'pause', 'submitAddress', 'resume', 'notify'])
  assert.deepEqual(calls.at(-1), ['notify', 'fileKept:The address could not be fetched'])

  const quiet = []
  const { api: again } = browser(quiet, ALLOWED)
  const { handover: other } = setup(quiet, { api: again, address: { ok: false, status: 400, code: 'capture.file_unsupported', message: 'no' } })
  await other.onBrowserOnlyDownload(DOWNLOAD)
  assert.ok(!names(quiet).includes('notify'), 'a ZIP of photos is simply the browser\'s')
  assert.ok(!names(quiet).includes('cancel'))
})

test('a consent whose cookie grant is gone says so and hands nothing over', async () => {
  const calls = []
  const { api } = browser(calls, { sites: [HOST], granted: [sitePattern(HOST)] })
  const { handover } = setup(calls, { api })
  assert.equal(await handover.onBrowserOnlyDownload(DOWNLOAD), false)
  assert.deepEqual(calls, [['notify', `fileConsentIncomplete:${HOST}`]])
})

test('Firefox holds the copied bytes back, hands them over and gives the browser nothing', async () => {
  const calls = []
  const filter = streamFilter(calls)
  const { api } = browser(calls, { ...ALLOWED, filter })
  const { handover, sent } = setup(calls, { api })
  assert.deepEqual(await handover.onHeadersReceived(RESPONSE), {})
  filter.ondata({ data: new TextEncoder().encode('<nzb>').buffer })
  filter.ondata({ data: new TextEncoder().encode('</nzb>').buffer })
  filter.onstop()
  assert.equal(await handover.onBrowserOnlyDownload(DOWNLOAD), true)
  assert.equal(await sent.bytes[0].body.bytes.text(), '<nzb></nzb>')
  assert.equal(sent.bytes[0].body.fileName, 'Cart.Release.nzb')
  assert.ok(!names(calls).includes('write'), 'the browser receives none of it')
  assert.ok(!names(calls).includes('submitAddress'), 'and the address is not fetched again')
  assert.deepEqual(names(calls).slice(-4), ['cancel', 'removeFile', 'erase', 'notify'])
})

test('a copy rDownloader refuses reaches the browser in full', async () => {
  const calls = []
  const filter = streamFilter(calls)
  const { api } = browser(calls, { ...ALLOWED, filter })
  const { handover } = setup(calls, { api, bytes: { ok: false, status: 400, code: 'capture.file_unsupported' } })
  await handover.onHeadersReceived(RESPONSE)
  filter.ondata({ data: new Uint8Array(3).buffer })
  filter.ondata({ data: new Uint8Array(4).buffer })
  filter.onstop()
  assert.equal(await handover.onBrowserOnlyDownload(DOWNLOAD), false)
  assert.deepEqual(filter.written.map((chunk) => chunk.byteLength), [3, 4])
  assert.ok(names(calls).includes('close'))
  assert.ok(!names(calls).includes('cancel'))
})

/** Mozilla bug 1787119: a response that turns into a download can lose its filter. */
test('a copy that breaks off falls back to the address', async () => {
  const calls = []
  const filter = streamFilter(calls)
  const { api } = browser(calls, { ...ALLOWED, filter })
  const { handover } = setup(calls, { api })
  await handover.onHeadersReceived(RESPONSE)
  filter.onerror()
  assert.equal(await handover.onBrowserOnlyDownload(DOWNLOAD), true)
  assert.ok(!names(calls).includes('submitBytes'))
  assert.equal(names(calls).filter((name) => name === 'submitAddress').length, 1)
})

test('a response over the limit is passed through and left to the browser', async () => {
  const calls = []
  const filter = streamFilter(calls)
  const { api } = browser(calls, { ...ALLOWED, filter })
  const { handover } = setup(calls, { api })
  await handover.onHeadersReceived(RESPONSE)
  filter.ondata({ data: new ArrayBuffer(MAX_FILE_BYTES) })
  filter.ondata({ data: new ArrayBuffer(1) })
  filter.ondata({ data: new ArrayBuffer(2) })
  filter.onstop()
  assert.deepEqual(filter.written.map((chunk) => chunk.byteLength), [MAX_FILE_BYTES, 1, 2])
  assert.ok(names(calls).includes('disconnect'))
  assert.equal(await handover.onBrowserOnlyDownload(DOWNLOAD), false)
  assert.ok(!names(calls).some((name) => name === 'submitBytes' || name === 'submitAddress'))
})

test('only an NZB, torrent or ZIP navigation from an allowed site is ever filtered', async () => {
  const calls = []
  const filter = streamFilter(calls)
  const { api } = browser(calls, { ...ALLOWED, filter })
  const { handover } = setup(calls, { api })
  // Answered synchronously, without a promise: nothing else waits on this listener.
  assert.equal(handover.onHeadersReceived({ ...RESPONSE, type: 'xmlhttprequest' }), undefined)
  assert.equal(handover.onHeadersReceived({ ...RESPONSE, statusCode: 302 }), undefined)
  assert.equal(handover.onHeadersReceived({ ...RESPONSE, url: 'https://indexer.test/page', responseHeaders: [{ name: 'content-type', value: 'text/html' }] }), undefined)
  await handover.onHeadersReceived({ ...RESPONSE, url: 'https://elsewhere.test/getnzb/abc' })
  assert.ok(!names(calls).includes('filter'), 'a site nobody allowed is never filtered')
  // Chrome: no filterResponseData, no filter, whatever arrives.
  const { api: chrome } = browser([], ALLOWED)
  assert.equal(setup([], { api: chrome }).handover.onHeadersReceived(RESPONSE), undefined)
})

test('the interceptor hands an NZB to the file handover and still never pauses it itself', async () => {
  const calls = []
  const seen = []
  const { createInterceptor } = await import('../src/downloads.js')
  const handled = createInterceptor({
    api: { downloads: { pause: async (id) => calls.push(['pause', id]) }, storage: {} },
    loadConfig: async () => ({ ...CONFIG, interceptDownloads: true }),
    submit: async () => ({ ok: true }),
    ping: async () => ({ ok: true, captureVersion: 2 }),
    message: (key) => key,
    userAgent: 'UA',
    ownExtensionId: 'self',
    // Wired the way background.js wires it.
    files: { onBrowserOnlyDownload: async (item) => { seen.push(item.id); return true } }
  })
  await handled.onDownloadCreated(DOWNLOAD)
  assert.deepEqual(seen, [9])
  assert.deepEqual(calls, [], 'the interceptor itself touches nothing')
  const { interceptor } = setupInterceptor({ calls })
  await interceptor.onDownloadCreated(DOWNLOAD)
  assert.deepEqual(calls, [], 'without a file handover nothing changes against RD-120-63')
})

test('a session share or handover leaves cookies granted while a site stands on them', async () => {
  const calls = []
  const { api } = browser(calls, ALLOWED)
  const sharer = createSessionSharer({
    api,
    loadConfig: async () => CONFIG,
    submit: async () => ({ ok: true }),
    message: (key) => key,
    notify: async () => {}
  })
  await sharer.share(CART, { includeSubdomains: false })
  // The share's own origin is this site's, which the consent holds: neither goes.
  assert.deepEqual(calls.find(([name]) => name === 'remove'), ['remove', { permissions: [], origins: [] }])
  assert.equal(await siteConsented(api, CART), true)
  assert.ok((await readAddressCookies(api, CART)) !== null, 'the cookie grant survived the share')
})
