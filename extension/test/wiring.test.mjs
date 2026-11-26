// `background.js` loaded twice, as Firefox and as Chrome, in the fake browser that keeps each
// one's rules (RD-1480-02). What these hold is the wiring: the flows have their own suites, but a
// listener that is never registered, registered with a filter that matches nothing, or called once
// the click is over leaves every one of those green.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { RESOURCES } from '../src/cnl.js'
import { SITES_KEY, sitePattern } from '../src/files.js'

import { fakeBrowser, loadIn, settle } from './fake-browser.mjs'

const HOST = 'hoster.test'
const PAGE = `https://${HOST}/folder/1`
const PAIRED = { server: 'http://127.0.0.1:8710', token: 'capture-token' }
const COOKIE = { name: 'session', value: 'abc', domain: `.${HOST}`, path: '/', secure: true }

/** rDownloader answers its capture routes; nothing answers the agent's port. */
const respond = async (url) => (url.includes(':9666') ? null : { ok: true, status: 201, json: async () => ({ candidates: [{}, {}], kind: 'nzb' }) })

async function background(target, options = {}) {
  const browser = fakeBrowser(target, { local: PAIRED, cookies: [COOKIE], respond, ...options })
  await loadIn(browser, 'background.js')
  return browser
}

const ALLOWED = { local: { ...PAIRED, [SITES_KEY]: [HOST] }, grants: ['cookies', 'authenticationInfo', sitePattern(HOST)] }
const posts = (browser, path) => browser.fetches.filter(({ url }) => url.endsWith(path))

function shareSession(browser) {
  browser.gesture(() => browser.fire('contextMenus.onClicked', { menuItemId: 'rdownloader-session', pageUrl: PAGE }, { title: 'A folder' }))
}

for (const target of ['firefox', 'chrome']) {
  test(`${target}: every listener is registered while the module is evaluated`, async () => {
    const browser = fakeBrowser(target, { respond })
    await loadIn(browser, 'background.js')
    const atLoad = browser.listeners.length
    await settle()
    assert.equal(browser.listeners.length, atLoad)
    for (const name of ['runtime.onMessage', 'contextMenus.onClicked', 'alarms.onAlarm', 'downloads.onCreated', 'webRequest.onSendHeaders', 'webRequest.onHeadersReceived', 'webRequest.onBeforeRequest']) {
      assert.ok(browser.listeners.some((entry) => entry.name === name), `${name} is not registered`)
    }
    const blocking = browser.listeners.filter((entry) => entry.spec?.includes('blocking')).map((entry) => entry.name)
    assert.deepEqual(blocking, target === 'firefox' ? ['webRequest.onBeforeRequest', 'webRequest.onHeadersReceived'] : [])
    // Firefox refuses `extraHeaders`, which cost a release once (RD-108-22): the listener is
    // registered without it there rather than lost.
    const [observer] = browser.listeners.filter((entry) => entry.name === 'webRequest.onSendHeaders')
    assert.deepEqual(observer.spec, target === 'firefox' ? ['requestHeaders'] : ['requestHeaders', 'extraHeaders'])
    // The first listener asks for nothing at all, no request body above all.
    assert.deepEqual(browser.listeners.find((entry) => entry.name === 'webRequest.onBeforeRequest').spec, [])
  })

  test(`${target}: sharing a session asks within the click, posts the cookies and gives the grant back`, async () => {
    const browser = await background(target)
    shareSession(browser)
    assert.equal(browser.prompts.length, 1, 'the prompt is there before the click handler returned')
    await settle()
    assert.deepEqual(browser.notices(), [browser.text('sessionShared', [HOST])])
    const [posted] = posts(browser, '/api/v1/capture/cookies')
    assert.match(JSON.parse(posted.init.body).cookies, /\tsession\tabc$/)
    assert.ok(!browser.held().includes('cookies'), 'nothing is kept after the read')
    assert.ok(!browser.held().some((entry) => entry.includes(HOST)))
  })

  test(`${target}: a session share the person declines is said, and reads nothing`, async () => {
    const browser = await background(target, { answer: () => false })
    shareSession(browser)
    await settle()
    assert.deepEqual(browser.notices(), [browser.text('handoverDenied', [HOST])])
    assert.ok(!browser.calls.some(([name]) => name === 'getAll'))
    assert.deepEqual(posts(browser, '/api/v1/capture/cookies'), [])
  })

  test(`${target}: an unpaired extension reads no cookie, gives the grant back and says what is missing`, async () => {
    const browser = await background(target, { local: { ...PAIRED, token: '' } })
    shareSession(browser)
    await settle()
    assert.deepEqual(browser.notices(), [browser.text('sessionUnconfigured')])
    assert.ok(!browser.calls.some(([name]) => name === 'getAll'))
    assert.ok(!browser.held().includes('cookies'))
  })

  test(`${target}: a page's Click'n'Load reaches the service, from the agent's port only`, async () => {
    const browser = await background(target, ALLOWED)
    browser.fire('runtime.onInstalled')
    await settle()
    const from = target === 'firefox' ? { originUrl: PAGE } : { initiator: `https://${HOST}` }
    const request = (url, extra = {}) => ({ url, method: 'GET', type: 'script', ...from, ...extra })
    const form = { method: 'POST', type: 'sub_frame', requestBody: { formData: { urls: ['https://a.test/x'] } } }
    for (const host of ['127.0.0.1', 'localhost']) {
      const answered = await browser.webRequest('onBeforeRequest', request(`http://${host}:9666/jdcheck.js`))
      if (target === 'firefox') assert.deepEqual(answered, [{ redirectUrl: browser.api.runtime.getURL(RESOURCES.check) }], host)
      else assert.equal(browser.rules().length, 3, 'the session rules answer the page')
      const before = posts(browser, '/api/v1/capture/click-and-load').length
      await browser.webRequest('onBeforeRequest', request(`http://${host}:9666/flash/add`, form))
      await settle()
      assert.equal(posts(browser, '/api/v1/capture/click-and-load').length, before + 1, host)
      // Another port is another program, whatever its path.
      assert.deepEqual(await browser.webRequest('onBeforeRequest', request(`http://${host}:8080/flash/add`, form)), [])
      assert.deepEqual(await browser.webRequest('onBeforeRequest', request(`http://${host}:8080/jdcheck.js`)), [])
      await settle()
      assert.equal(posts(browser, '/api/v1/capture/click-and-load').length, before + 1, `${host}:8080 is not the agent's`)
    }
  })

  test(`${target}: rDownloader on this computer is reached by a host grant, not by its CORS headers`, async () => {
    for (const host of ['127.0.0.1', 'localhost']) {
      const browser = await background(target, { local: { ...PAIRED, server: `http://${host}:8710` } })
      browser.fire('contextMenus.onClicked', { menuItemId: 'rdownloader-link', linkUrl: 'https://hoster.test/a' }, {})
      await settle()
      assert.deepEqual(browser.fetches.map(({ url, hostAccess }) => [url, hostAccess]), [[`http://${host}:8710/api/v1/capture/batches`, true]])
    }
  })
}

test("the capture token's area is closed to content scripts on every start", async () => {
  const chrome = await background('chrome')
  await settle()
  assert.deepEqual(chrome.calls.filter(([name]) => name === 'setAccessLevel'), [['setAccessLevel', 'local', { accessLevel: 'TRUSTED_CONTEXTS' }]])
  // A browser without the call starts as before.
  const firefox = await background('firefox')
  await settle()
  assert.ok(firefox.listeners.length > 0)
})

test("the port's own listener reads no other request's body", async () => {
  // Firefox's filter cannot name the port, so it names the paths the agent serves.
  for (const target of ['firefox', 'chrome']) {
    const browser = await background(target, ALLOWED)
    const forms = browser.listeners.filter((entry) => entry.spec?.includes('requestBody'))
    assert.equal(forms.length, 1, target)
    for (const pattern of forms[0].filter.urls) {
      assert.match(pattern, target === 'firefox' ? /^http:\/\/(127\.0\.0\.1|localhost)\/(jdcheck\.js|flash)\*$/ : /^http:\/\/(127\.0\.0\.1|localhost):9666\/\*$/)
    }
  }
})

test("Firefox copies an allowed site's NZB while it arrives and hands it over", async () => {
  const browser = await background('firefox', ALLOWED)
  const response = {
    requestId: 'r1',
    url: `https://${HOST}/getnzb/abc`,
    type: 'main_frame',
    statusCode: 200,
    responseHeaders: [{ name: 'content-type', value: 'application/x-nzb' }, { name: 'content-disposition', value: 'attachment; filename="a.nzb"' }]
  }
  await browser.webRequest('onHeadersReceived', response)
  assert.equal(browser.filters.length, 1, 'the response is copied')
  browser.filters[0].ondata({ data: new TextEncoder().encode('<nzb/>').buffer })
  browser.filters[0].onstop()
  await settle()
  assert.equal(posts(browser, '/api/v1/capture/file').length, 1)
  assert.equal(browser.filters[0].closed, true)
  // Without the consent nothing is copied.
  const other = await background('firefox')
  await other.webRequest('onHeadersReceived', response)
  assert.deepEqual(other.filters, [])
})
