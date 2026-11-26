// Every host grant the extension asks for or asks about, on an address with a port of its own:
// Firefox reads no port in a match pattern, so a grant that names one covers nothing (RD-1480-01).

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { pageOriginPattern } from '../src/captcha.js'
import { manifestOf } from '../src/grants.js'
import { scopeOrigin } from '../src/handover.js'
import { originMatchPattern } from '../src/intercept.js'

import { fakeBrowser, loadIn, settle } from './fake-browser.mjs'
import { openPage } from './fake-page.mjs'
import { WIDGET } from './fakes.mjs'
import { matchesPattern } from './match-patterns.mjs'

const PAGE = 'https://hoster.test:8443/login'
const PAIRED = { server: 'http://127.0.0.1:8710', token: 'capture-token' }
const COOKIE = { name: 's', value: '1', domain: '.hoster.test', path: '/', secure: true }
const json = (payload) => ({ ok: true, status: 200, json: async () => payload })

async function service(url) {
  if (url.includes('/capture/captchas?')) return json([{ ...WIDGET, page_url: PAGE }])
  if (url.endsWith('/capture/browser-sessions')) return json([{ id: 'h1', scope: PAGE, host: 'hoster.test', account_label: 'Main' }])
  return json({})
}

test('the pattern of a page and of a scope covers that page in the browser it is asked of', () => {
  for (const [target, expected] of [['firefox', 'https://hoster.test/*'], ['chrome', 'https://hoster.test:8443/*']]) {
    const manifest = manifestOf(fakeBrowser(target).api)
    for (const pattern of [pageOriginPattern(PAGE, manifest), scopeOrigin(PAGE, manifest)]) {
      assert.equal(pattern, expected)
      assert.ok(matchesPattern(target, pattern, PAGE), `${target}: ${pattern}`)
    }
  }
  // Chrome's grant stays with the one service.
  assert.ok(!matchesPattern('chrome', 'https://hoster.test:8443/*', 'https://hoster.test/'))
  // The interceptor's question names no port in either browser.
  assert.equal(originMatchPattern(PAGE), 'https://hoster.test/*')
  assert.ok(matchesPattern('firefox', originMatchPattern(PAGE), PAGE))
  assert.equal(manifestOf({}), null)
  assert.equal(manifestOf({ runtime: { getManifest: () => { throw new Error('gone') } } }), null)
})

for (const target of ['firefox', 'chrome']) {
  test(`${target}: a captcha and a session on an address with a port get a grant that covers it`, async () => {
    const browser = fakeBrowser(target, { local: PAIRED, cookies: [COOKIE], respond: service })
    const covered = () => browser.held().some((entry) => entry.includes('hoster.test') && matchesPattern(target, entry, PAGE))
    await loadIn(browser, 'background.js')
    const page = await openPage(browser, 'popup')
    await settle()

    page.click(page.button('captcha-list', browser.text('captchaAnswer')))
    await settle()
    assert.deepEqual(page.status(), [browser.text('captchaOpened'), 'ok'])
    assert.ok(covered(), 'the reader reaches the page')
    page.click(page.button('captcha-list', browser.text('captchaDecline')))
    await settle()
    assert.ok(!covered(), 'and the grant goes with the captcha')

    page.click(page.button('handover-list', browser.text('handoverAllow')))
    await settle()
    const asked = browser.prompts.at(-1).origins[0]
    assert.ok(matchesPattern(target, asked, PAGE), asked)
    if (target === 'firefox') {
      // Chrome's half is as it was: its grant names the port, and what `cookies` then answers for
      // a domain without one is Chrome's business, not this fake's.
      const delivered = browser.fetches.find(({ url }) => url.endsWith('/browser-sessions/h1'))
      assert.match(JSON.parse(delivered.init.body).cookies, /\ts\t1$/)
      assert.deepEqual(page.status(), [browser.text('handoverDone', ['hoster.test']), 'ok'])
    }
    assert.ok(!browser.held().includes('cookies'))
  })
}
