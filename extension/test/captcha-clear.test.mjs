import assert from 'node:assert/strict'
import { test } from 'node:test'

import { WIDGET, fakeApi, setupAnswerer as setup } from './fakes.mjs'

// RD-1260-04: "captcha waiting" stood on screen after the captcha was answered elsewhere or ran
// out, until somebody clicked it away. The announcement now leaves with its captcha.
test('the announcement of a captcha that stopped waiting is cleared from the screen', async () => {
  const calls = []
  let waiting = [WIDGET]
  const api = fakeApi(calls)
  const { answerer } = setup({ calls, api, list: async () => ({ ok: true, status: 200, widgets: waiting }) })
  await answerer.poll()
  const [announced] = Object.values(api.session.captchaNotifications)
  assert.ok(announced, 'the waiting captcha was announced')
  assert.ok(!calls.some(([name]) => name === 'clearNotification'), 'a waiting captcha keeps its announcement')

  waiting = []
  await answerer.poll()
  const cleared = calls.filter(([name]) => name === 'clearNotification')
  assert.deepEqual(cleared, [['clearNotification', announced]])
})
