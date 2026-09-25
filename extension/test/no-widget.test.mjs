// RD-120-45: a hoster page that shows no widget at all.
//
// The service met a Turnstile on the hoster's sign-in page, fetched with an empty cookie jar; the
// person's browser holds a session there and is sent straight past the form. The reader used to
// watch that page for its whole deadline while the sign-in waited in silence.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { NO_WIDGET_MESSAGE, TOKEN_MESSAGE, WIDGET_MARKERS, harvestAnswer, reportPageWithoutWidget } from '../src/captcha.js'

import { CONFIG, WIDGET, fakeApi, names, setupAnswerer as setup } from './fakes.mjs'

const MISSING = { messageType: NO_WIDGET_MESSAGE, afterMs: 15_000, markers: WIDGET_MARKERS }

/** A page with a clock and a DOM the test controls; `present` says what `querySelector` finds. */
function page(run) {
  const sent = []
  const timers = []
  const cleared = []
  const state = { present: new Set(), now: 1_000 }
  const real = { setInterval: globalThis.setInterval, clearInterval: globalThis.clearInterval, now: Date.now }
  globalThis.document = { querySelector: (selector) => (state.present.has(selector) ? { value: '' } : null) }
  globalThis.chrome = { runtime: { sendMessage: (payload) => sent.push(payload) } }
  globalThis.setInterval = (fn) => { timers.push(fn); return timers.length }
  globalThis.clearInterval = (id) => cleared.push(id)
  Date.now = () => state.now
  try {
    return run({ sent, timers, cleared, state })
  } finally {
    globalThis.setInterval = real.setInterval
    globalThis.clearInterval = real.clearInterval
    Date.now = real.now
    delete globalThis.document
    delete globalThis.chrome
    delete globalThis.__rdownloaderCaptchaHarvester
  }
}

test('a page that never shows a widget is reported once, after the grace period, and the poll stops', () => {
  page(({ sent, timers, cleared, state }) => {
    harvestAnswer('c1', 'input[name="cf-turnstile-response"]', TOKEN_MESSAGE, 600_000, MISSING)
    state.now += 14_000
    timers[0]()
    assert.deepEqual(sent, [], 'a widget script gets its few seconds to render')
    state.now += 1_000
    timers[0]()
    timers[0]()
    assert.deepEqual(sent, [{ type: NO_WIDGET_MESSAGE, id: 'c1' }], 'reported once, carrying nothing but the id')
    assert.deepEqual(cleared, [1], 'and the poll it started is stopped')
  })
})

test('a widget that is on the page, answered or not, is never reported missing', () => {
  page(({ sent, timers, state }) => {
    // Only the widget's frame is there, as while Turnstile is still rendering its field.
    state.present.add(WIDGET_MARKERS)
    harvestAnswer('c1', 'input[name="cf-turnstile-response"]', TOKEN_MESSAGE, 600_000, MISSING)
    // Seen once is seen for good: a widget that resets or re-renders is not a missing one.
    state.present.clear()
    state.now += 60_000
    timers[0]()
    assert.deepEqual(sent, [])
  })
  page(({ sent, timers, state }) => {
    state.present.add('input[name="cf-turnstile-response"]')
    harvestAnswer('c1', 'input[name="cf-turnstile-response"]', TOKEN_MESSAGE, 600_000, MISSING)
    state.now += 60_000
    timers[0]()
    assert.deepEqual(sent, [], 'an empty answer field is a widget waiting for its answer')
  })
})

test('the reader without the missing-widget option behaves exactly as before', () => {
  page(({ sent, timers, state }) => {
    harvestAnswer('c1', 'input', TOKEN_MESSAGE, 600_000)
    state.now += 60_000
    timers[0]()
    assert.deepEqual(sent, [])
  })
})

test('a missing widget is reported to the server, the permission released and the tab left open', async () => {
  const calls = []
  const { answerer, api } = setup({ calls })
  const { tabId } = await answerer.open(WIDGET)

  const result = await answerer.onMessage({ type: NO_WIDGET_MESSAGE, id: WIDGET.id }, { tab: { id: tabId } })

  assert.deepEqual(result, { ok: true, code: 'captcha.page_without_widget' })
  assert.deepEqual(calls.filter(([name]) => name === 'noWidget'), [['noWidget', WIDGET.id]])
  assert.deepEqual(calls.find(([name]) => name === 'remove')[1], { origins: ['https://ddownload.com/*'] })
  // The tab shows the person their own signed-in page, which is what explains the message.
  assert.ok(!names(calls).includes('removeTab'))
  assert.deepEqual(api.session.captchaTabs, {})
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'captchaNoWidget:ddownload.com'))

  // Closing that tab later declines nothing: the captcha is already over.
  await answerer.onTabRemoved(tabId)
  assert.ok(!names(calls).includes('skip'))
})

test('only the tab opened for that captcha may report it missing', async () => {
  const calls = []
  const { answerer } = setup({ calls })
  const { tabId } = await answerer.open(WIDGET)

  assert.deepEqual(await answerer.onMessage({ type: NO_WIDGET_MESSAGE, id: WIDGET.id }, { id: 'self' }), { ok: false, code: 'sender' })
  assert.deepEqual(await answerer.onMessage({ type: NO_WIDGET_MESSAGE, id: WIDGET.id }, { tab: { id: 999 } }), { ok: false, code: 'untracked' })
  assert.deepEqual(await answerer.onMessage({ type: NO_WIDGET_MESSAGE, id: 'other' }, { tab: { id: tabId } }), { ok: false, code: 'untracked' })
  assert.ok(!names(calls).includes('noWidget'))
})

test('a report the server refused says so instead of claiming the sign-in stopped', async () => {
  const calls = []
  const api = fakeApi(calls)
  const { answerer } = setup({
    calls,
    api,
    reportMissing: async () => ({ ok: false, status: 404, code: 'captcha.not_waiting', message: 'gone' })
  })
  const { tabId } = await answerer.open(WIDGET)

  const result = await answerer.onMessage({ type: NO_WIDGET_MESSAGE, id: WIDGET.id }, { tab: { id: tabId } })

  assert.deepEqual(result, { ok: false, code: 'captcha.not_waiting' })
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'captchaFailed:ddownload.com,gone'))
})

test('reportPageWithoutWidget posts to the capture route with the capture token and nothing else', async () => {
  let seen = null
  const result = await reportPageWithoutWidget(CONFIG, WIDGET.id, async (url, init) => {
    seen = { url, init }
    return { ok: true, status: 200, json: async () => ({ code: 'captcha.page_without_widget' }) }
  })
  assert.deepEqual(result, { ok: true, status: 200, code: 'captcha.page_without_widget', message: null })
  assert.equal(seen.url, `http://127.0.0.1:8710/api/v1/capture/captchas/${WIDGET.id}/no-widget`)
  assert.equal(seen.init.method, 'POST')
  assert.equal(seen.init.headers.authorization, 'Bearer capture-token')
  assert.equal(seen.init.body, undefined)
})
