// What crosses into a page or between the extension's halves, with nothing but what is there.
//
// `harvestAnswer` is serialised into the hoster's page (`scripting.executeScript`), so it may use
// nothing of its module; every other suite calls it as a module's function, where a constant of
// `captcha-page.js` is in reach and a reader that uses one passes (RD-1480-02). And a runtime
// message crosses as JSON in Chrome, structurally cloned in Firefox.

import assert from 'node:assert/strict'
import { test } from 'node:test'
import vm from 'node:vm'

import { NO_WIDGET_MESSAGE, TOKEN_MESSAGE, WIDGET_MARKERS, harvestAnswer } from '../src/captcha.js'

import { crossBoundary } from './boundary.mjs'

/** The reader as a page gets it: its source, in a context that holds a page and nothing else. */
function inject(...args) {
  const sent = []
  const timers = []
  const page = { field: null, now: 1_000 }
  const context = vm.createContext({
    chrome: { runtime: { sendMessage: (payload) => sent.push(payload) } },
    document: { querySelector: () => page.field },
    Date: { now: () => page.now },
    setInterval: (look) => timers.push(look),
    clearInterval: () => { timers.length = 0 }
  })
  const reader = vm.runInContext(`(${harvestAnswer.toString()})`, context)
  const started = reader(...args)
  return { sent, page, started, tick: () => timers.forEach((look) => look()), polling: () => timers.length > 0 }
}

/** An object of the page's realm, compared by its content. */
const plain = (value) => JSON.parse(JSON.stringify(value))

test('the injected reader needs nothing of its module to send a token', () => {
  const run = inject('c1', 'input[name="cf-turnstile-response"]', TOKEN_MESSAGE)
  assert.equal(run.started, true)
  run.tick()
  assert.deepEqual(run.sent, [])
  run.page.field = { value: ' 0.token ' }
  run.tick()
  assert.deepEqual(plain(run.sent), [{ type: TOKEN_MESSAGE, id: 'c1', token: '0.token' }])
  assert.equal(run.polling(), false)
})

test('nor to report a page without a widget, nor to give up at its deadline', () => {
  const missing = { messageType: NO_WIDGET_MESSAGE, afterMs: 15_000, markers: WIDGET_MARKERS }
  const reported = inject('c1', 'input', TOKEN_MESSAGE, 600_000, missing)
  reported.page.now += 15_000
  reported.tick()
  assert.deepEqual(plain(reported.sent), [{ type: NO_WIDGET_MESSAGE, id: 'c1' }])

  const given = inject('c1', 'input', TOKEN_MESSAGE)
  given.page.now += 599_999
  given.tick()
  assert.equal(given.polling(), true)
  given.page.now += 1
  given.tick()
  assert.equal(given.polling(), false, 'the default deadline is a literal of the function')
  assert.deepEqual(given.sent, [])
})

test('Chrome carries JSON, Firefox a structured clone, and the fakes say so', () => {
  // Measured in Chromium 159: no error, the value simply arrives as something else.
  for (const value of [{ data: new ArrayBuffer(2) }, { map: new Map([[1, 2]]) }, { set: new Set([1]) }, { date: new Date(0) }, { number: Number.NaN }, [undefined]]) {
    assert.throws(() => crossBoundary('chrome', value), /carries JSON/)
    assert.deepEqual(crossBoundary('firefox', value), value)
  }
  const payload = { type: 'x', name: 'Release.dlc', data: 'ZGxj', list: [1, null, { deep: true }], gone: undefined }
  assert.deepEqual(crossBoundary('chrome', payload), { type: 'x', name: 'Release.dlc', data: 'ZGxj', list: [1, null, { deep: true }] })
  assert.notEqual(crossBoundary('chrome', payload).list, payload.list, 'a copy, as storage hands out')
  assert.throws(() => crossBoundary('firefox', { run: () => {} }))
  assert.throws(() => crossBoundary('chrome', { run: () => {} }))
})
