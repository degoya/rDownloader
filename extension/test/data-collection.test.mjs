// RD-1270-02: Firefox's optional `authenticationInfo` data consent for the cookies that leave.
//
// What these hold: the Firefox build asks for the category in the same request as `cookies` and
// the site, the Chrome build never names it (Chrome refuses the key), and in Firefox no cookie is
// read or sent — session share, session handover, a site's file hand-over — while the person does
// not hold it, even with `cookies` and the site granted.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { COOKIE_DATA, declaresCookieData, holdsCookieData, withCookieData } from '../src/data-collection.js'
import { SITES_KEY, allowSite, createFileHandover, revokeSite, sitePattern } from '../src/files.js'
import { DELIVER_MESSAGE, createHandover } from '../src/handover.js'
import { createSessionSharer } from '../src/session.js'

import { fakeBrowser } from './fake-browser.mjs'
import { CONFIG, POPUP, names } from './fakes.mjs'

const HOST = 'indexer.test'
const CART = `https://${HOST}/getnzb/abc`
const JAR = [{ name: 'sess', value: 's3cr3t', domain: `.${HOST}`, path: '/', secure: true, httpOnly: true }]

/**
 * A browser running one of the two builds. `withhold` is a person who declines the data consent
 * (or took it back in `about:addons`): the request grants the rest.
 */
function browser(calls, { target = 'firefox', granted = [], sites = [], withhold = false } = {}) {
  return fakeBrowser(target, { calls, answer: () => (withhold ? COOKIE_DATA : true), local: { [SITES_KEY]: sites }, grants: granted, cookies: JAR })
}

test('only the Firefox build names the cookie category, and only it is asked for it', () => {
  const request = { permissions: ['cookies'], origins: [sitePattern(HOST)] }
  const { api: firefox } = browser([])
  const { api: chrome } = browser([], { target: 'chrome' })
  assert.equal(declaresCookieData(firefox), true)
  assert.deepEqual(withCookieData(firefox, request), { ...request, data_collection: ['authenticationInfo'] })
  // Chrome's `permissions` API refuses an unknown key: the request must stay as it was.
  assert.equal(declaresCookieData(chrome), false)
  assert.deepEqual(withCookieData(chrome, request), request)
  assert.deepEqual(withCookieData({}, request), request, 'no manifest to read: nothing is added')
})

test('allowing a site in Firefox asks for the data consent in the same request, revoking gives it back', async () => {
  const calls = []
  const { api, gesture, held } = browser(calls)
  assert.equal((await gesture(() => allowSite(api, CART))).ok, true)
  assert.deepEqual(calls[0], ['request', { permissions: ['cookies'], origins: [sitePattern(HOST)], data_collection: ['authenticationInfo'] }])
  assert.equal(await holdsCookieData(api), true)
  await revokeSite(api, HOST)
  assert.deepEqual(calls.at(-1), ['remove', { permissions: ['cookies'], origins: [sitePattern(HOST)], data_collection: ['authenticationInfo'] }])
  assert.ok(!held().includes('authenticationInfo'))
})

test('Firefox without the data consent hands a site\'s file over without a single cookie read', async () => {
  const calls = []
  const { api } = browser(calls, { sites: [HOST], granted: ['cookies', sitePattern(HOST)] })
  const sent = []
  const handover = createFileHandover({
    api,
    loadConfig: async () => ({ ...CONFIG, interceptDownloads: true }),
    message: (key, substitutions) => (substitutions ? `${key}:${substitutions.join(',')}` : key),
    notify: async (body) => { calls.push(['notify', body]) },
    submitAddress: async (_config, body) => { sent.push(body); return { ok: true, status: 201 } },
    wait: 50
  })
  const download = { id: 9, url: CART, filename: '/tmp/Cart.nzb', mime: 'application/x-nzb', referrer: '' }
  assert.equal(await handover.onBrowserOnlyDownload(download), false)
  assert.ok(!names(calls).includes('getAll'), 'no cookie was read')
  assert.deepEqual(sent, [], 'nothing went to rDownloader')
  assert.deepEqual(calls.at(-1), ['notify', `fileConsentIncomplete:${HOST}`])
})

test('a Firefox session share the data consent was declined for reads and sends no cookie', async () => {
  const calls = []
  const { api, gesture, held } = browser(calls, { withhold: true })
  const posted = []
  const sharer = createSessionSharer({
    api,
    loadConfig: async () => CONFIG,
    submit: async (_config, body) => { posted.push(body); return { ok: true } },
    message: (key) => key,
    notify: async () => {}
  })
  assert.deepEqual(await gesture(() => sharer.share('https://www.hoster.com/files')), { ok: false, code: 'denied' })
  assert.deepEqual(calls[0][1].data_collection, ['authenticationInfo'])
  assert.ok(!names(calls).includes('getAll'))
  assert.deepEqual(posted, [])
  // What the request did grant is given back at once.
  assert.ok(!held().includes('cookies'))
  assert.ok(!held().includes('*://*.www.hoster.com/*'))
})

test('a Firefox session handover without the data consent delivers no cookie', async () => {
  const calls = []
  const { api, gesture } = browser(calls, { withhold: true })
  const scope = 'https://ddownload.com/'
  const delivered = []
  const handover = createHandover({
    api,
    loadConfig: async () => CONFIG,
    message: (key) => key,
    notify: async () => null,
    list: async () => ({ ok: true, status: 200, handovers: [{ id: 'h1', scope, host: 'ddownload.com' }] }),
    deliver: async (_config, id, cookies) => { delivered.push(cookies); return { ok: true } },
    decline: async () => ({ ok: true })
  })
  // Wired the way background.js wires it.
  api.runtime.onMessage.addListener((request, sender, sendResponse) => {
    handover.onMessage(request, sender).then(sendResponse)
    return true
  })
  assert.deepEqual(await gesture(() => handover.consent({ id: 'h1', scope })), { ok: false, code: 'denied' })
  assert.deepEqual(calls.find(([name]) => name === 'request')[1].data_collection, ['authenticationInfo'])
  assert.ok(!names(calls).includes('getAll'))
  assert.deepEqual(delivered, [])
  // A second attempt straight at the background fares no better.
  assert.deepEqual(await handover.onMessage({ type: DELIVER_MESSAGE, id: 'h1' }, POPUP), { ok: false, code: 'denied' })
  assert.deepEqual(delivered, [])
})
