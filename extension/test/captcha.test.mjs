import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  DECLINE_MESSAGE,
  GRANT_MESSAGE,
  HARVEST_DEADLINE_MS,
  NO_WIDGET_AFTER_MS,
  NO_WIDGET_MESSAGE,
  OPEN_MESSAGE,
  POLL_ALARM,
  POLL_MESSAGE,
  STATE_KEYS,
  TOKEN_MESSAGE,
  WIDGET_MARKERS,
  answerFieldFor,
  answerWidget,
  createCaptchaAnswerer,
  harvestAnswer,
  hostOf,
  listWidgets,
  pageOriginPattern,
  skipWidget
} from '../src/captcha.js'

import { CONFIG, POPUP, WIDGET, fakeApi, names, setupAnswerer as setup } from './fakes.mjs'

test('the permission pattern names exactly the page origin, not its subdomains or all sites', () => {
  assert.equal(pageOriginPattern('https://ddownload.com/login.html?x=1'), 'https://ddownload.com/*')
  assert.equal(pageOriginPattern('https://www.example.com:8443/file'), 'https://www.example.com:8443/*')
  assert.equal(pageOriginPattern('ftp://example.com/x'), null)
  assert.equal(pageOriginPattern('not a url'), null)
  assert.equal(hostOf(WIDGET.page_url), 'ddownload.com')
})

test('each widget vendor has its own answer field and an unknown kind watches all of them', () => {
  assert.equal(answerFieldFor('turnstile'), 'input[name="cf-turnstile-response"]')
  assert.equal(answerFieldFor('recaptcha_v2'), 'textarea[name="g-recaptcha-response"]')
  assert.equal(answerFieldFor('h_captcha'), 'textarea[name="h-captcha-response"]')
  assert.match(answerFieldFor('something-new'), /cf-turnstile-response.*g-recaptcha-response.*h-captcha-response/)
})

test('an unpaired extension polls nothing and schedules no alarm', async () => {
  const calls = []
  const { answerer } = setup({ calls, config: { server: CONFIG.server, token: '' } })
  assert.deepEqual(await answerer.poll(), { ok: false, code: 'unconfigured', widgets: [] })
  assert.equal(await answerer.schedule(), false)
  assert.ok(!names(calls).includes('list'))
  assert.ok(names(calls).includes('clearAlarm'))
})

test('polling announces a waiting widget once, sets the badge and reschedules on the alarm', async () => {
  const calls = []
  const { answerer } = setup({ calls })
  assert.equal(await answerer.schedule(), true)
  assert.deepEqual(calls.find(([name]) => name === 'alarm'), ['alarm', POLL_ALARM, { periodInMinutes: 0.5 }])

  // `schedule` starts a poll it does not wait for; a second one while it runs shares it.
  await answerer.poll()
  await answerer.onAlarm({ name: POLL_ALARM })
  await answerer.onAlarm({ name: 'something-else' })

  const notified = calls.filter(([name]) => name === 'notify')
  assert.equal(notified.length, 1, 'the same captcha is announced once, not on every poll')
  assert.equal(notified[0][1], 'captchaWaiting:ddownload.com')
  assert.equal(notified[0].length, 2, 'nothing is handed to notify that notify discards')
  assert.ok(calls.some(([name, text]) => name === 'badge' && text === '1'))
})

test('a link send no longer wipes the count of waiting captchas off the badge', async () => {
  // RD-109-24, finding 4: the background painted the badge directly on a send while this module
  // kept its own count. A successful send cleared the number, and because `badge` returned early
  // when the count had not changed, the next poll left it cleared.
  const calls = []
  const other = { ...WIDGET, id: '019d0000-0000-7000-8000-000000000002' }
  const { answerer } = setup({ calls, widgets: [WIDGET, other] })
  await answerer.poll()
  assert.deepEqual(calls.filter(([name]) => name === 'badge').at(-1), ['badge', '2'])

  await answerer.setSendFailure(false)
  await answerer.poll()
  assert.deepEqual(
    calls.filter(([name]) => name === 'badge').at(-1),
    ['badge', '2'],
    'a successful send says nothing about the captchas that are waiting'
  )

  // A failed send outranks the count, and the count comes back once it is cleared.
  await answerer.setSendFailure(true)
  assert.deepEqual(calls.filter(([name]) => name === 'badge').at(-1), ['badge', '!'])
  await answerer.setSendFailure(false)
  assert.deepEqual(calls.filter(([name]) => name === 'badge').at(-1), ['badge', '2'])
})

test('opening asks for the page origin only, then the background opens and records the tab before activating it', async () => {
  const calls = []
  const { answerer, api } = setup({ calls })

  const opened = await answerer.open(WIDGET)

  assert.equal(opened.ok, true)
  // The popup asks first — that origin only, synchronously inside the click — while the grant
  // announcement goes out on the next microtask, and the open request once the answer is in.
  // Both announcements cross a real `runtime.sendMessage`: the fake clones the payload on the
  // way, as a browser does, so a payload that cannot be serialised does not arrive (RD-109-26).
  assert.deepEqual(names(calls).filter((name) => name === 'sendMessage' || name === 'request'), ['request', 'sendMessage', 'sendMessage'])
  assert.deepEqual(calls.find(([name]) => name === 'request')[1], { origins: ['https://ddownload.com/*'] })
  assert.deepEqual(calls.filter(([name]) => name === 'sendMessage').map(([, type]) => type), [GRANT_MESSAGE, OPEN_MESSAGE])
  // The tab is created in the background and recorded before it is brought to the front.
  assert.deepEqual(calls.find(([name]) => name === 'createTab')[1], { url: WIDGET.page_url, active: false })
  assert.deepEqual(calls.find(([name]) => name === 'activateTab').slice(1), [opened.tabId, { active: true }, true])
  assert.deepEqual(api.session.captchaTabs[String(opened.tabId)], {
    id: WIDGET.id, origin: 'https://ddownload.com/*', kind: 'turnstile', host: 'ddownload.com'
  })
  assert.deepEqual(api.session.captchaGrants, {}, 'the tab entry carries the origin from here on')
})

test('the permission request is the first thing the click does, before any await', async () => {
  const calls = []
  const { answerer } = setup({ calls })
  // Started, not abandoned: the test used to leave this promise pending and its work running
  // after the test had ended (RD-109-26).
  const opening = answerer.open(WIDGET)
  assert.equal(calls[0]?.[0], 'request', 'nothing awaited before the prompt, so the gesture still counts')
  await opening
})

test('a second click for the same widget focuses its tab instead of opening another', async () => {
  const calls = []
  const { answerer, api } = setup({ calls })
  const first = await answerer.open(WIDGET)

  const second = await answerer.open(WIDGET)

  assert.deepEqual(second, { ok: true, code: null, tabId: first.tabId, reused: true })
  assert.equal(calls.filter(([name]) => name === 'createTab').length, 1)
  assert.equal(calls.filter(([name]) => name === 'activateTab').length, 2)
  assert.equal(Object.keys(api.session.captchaTabs).length, 1, 'closing the one tab declines once')
})

test('a grant or open request from anything but our own page is ignored', async () => {
  const calls = []
  const { answerer, api } = setup({ calls })
  const grant = { type: GRANT_MESSAGE, id: WIDGET.id, origin: 'https://evil.example/*' }
  const open = { type: OPEN_MESSAGE, widget: { ...WIDGET, page_url: 'https://evil.example/phish' } }

  // A content script always carries a tab; another extension carries another id.
  for (const sender of [{ id: 'self', tab: { id: 5 }, url: 'https://ddownload.com/login.html' }, { id: 'someone-else' }, undefined]) {
    assert.equal(await answerer.onMessage(grant, sender), false)
    assert.equal(await answerer.onMessage(open, sender), false)
  }
  assert.deepEqual(api.session.captchaGrants ?? {}, {})
  assert.ok(!names(calls).includes('createTab'))

  // The popup opened as a tab (the notification fallback) is still our own page.
  const asTab = { id: 'self', tab: { id: 6 }, url: 'chrome-extension://self/src/popup.html' }
  assert.deepEqual(await answerer.onMessage({ type: GRANT_MESSAGE, id: WIDGET.id, origin: 'https://ddownload.com/*' }, asTab), { ok: true })
  assert.deepEqual(api.session.captchaGrants, { [WIDGET.id]: 'https://ddownload.com/*' })
})

test('a stale tab entry is replaced by a fresh tab instead of reporting success over nothing', async () => {
  const calls = []
  const api = fakeApi(calls)
  const { answerer } = setup({ calls, api })
  const first = await answerer.open(WIDGET)

  // The tab vanished without `onTabRemoved` ever running (a worker restart in between).
  const update = api.tabs.update
  let rejected = false
  api.tabs.update = async (id, options) => {
    if (!rejected) { rejected = true; throw new Error('No tab with id') }
    return update(id, options)
  }
  const second = await answerer.open(WIDGET)

  assert.equal(second.ok, true)
  assert.equal(second.reused, undefined)
  assert.notEqual(second.tabId, first.tabId)
  assert.equal(calls.filter(([name]) => name === 'createTab').length, 2)
  assert.deepEqual(Object.keys(api.session.captchaTabs), [String(second.tabId)], 'the stale entry is gone')
})

test('an origin shared by two waiting widgets is released only with the last of them', async () => {
  const calls = []
  const other = { ...WIDGET, id: '019d0000-0000-7000-8000-000000000002', page_url: 'https://ddownload.com/other.html' }
  const { answerer } = setup({ calls, widgets: [WIDGET, other] })
  const first = await answerer.open(WIDGET)
  const second = await answerer.open(other)

  await answerer.onTabRemoved(first.tabId)
  assert.ok(!names(calls).includes('remove'), 'the other tab still needs the origin')

  await answerer.onMessage({ type: TOKEN_MESSAGE, id: other.id, token: 't' }, { tab: { id: second.tabId } })
  assert.equal(calls.filter(([name]) => name === 'remove').length, 1)
})

test('a refused permission opens nothing and withdraws the announced grant', async () => {
  const calls = []
  const api = fakeApi(calls, { granted: false })
  const { answerer } = setup({ calls, api })
  assert.deepEqual(await answerer.open(WIDGET), { ok: false, code: 'denied' })
  assert.ok(!names(calls).includes('createTab'))
  assert.deepEqual(api.session.captchaGrants, {})
})

test('the reader is injected once the page has loaded, for that captcha and its field', async () => {
  const calls = []
  const { answerer } = setup({ calls })
  const { tabId } = await answerer.open(WIDGET)

  await answerer.onTabUpdated(tabId, { status: 'loading' })
  assert.ok(!names(calls).includes('inject'), 'nothing is injected into a page still loading')
  await answerer.onTabUpdated(tabId + 1, { status: 'complete' })
  assert.ok(!names(calls).includes('inject'), 'a tab this extension did not open is left alone')

  await answerer.onTabUpdated(tabId, { status: 'complete' })
  const injected = calls.find(([name]) => name === 'inject')[1]
  assert.deepEqual(injected.target, { tabId })
  assert.equal(injected.func, harvestAnswer)
  assert.deepEqual(injected.args, [
    WIDGET.id,
    'input[name="cf-turnstile-response"]',
    TOKEN_MESSAGE,
    HARVEST_DEADLINE_MS,
    { messageType: NO_WIDGET_MESSAGE, afterMs: NO_WIDGET_AFTER_MS, markers: WIDGET_MARKERS }
  ])
})

test('a token from the tab is posted, the tab closed and the permission released', async () => {
  const calls = []
  const { answerer, posted, api } = setup({ calls })
  const { tabId } = await answerer.open(WIDGET)

  const result = await answerer.onMessage({ type: TOKEN_MESSAGE, id: WIDGET.id, token: '0.token' }, { tab: { id: tabId } })

  assert.deepEqual(result, { ok: true, code: 'captcha.solved' })
  assert.deepEqual(posted, [{ id: WIDGET.id, token: '0.token' }])
  assert.ok(calls.some(([name, id]) => name === 'removeTab' && id === tabId))
  assert.deepEqual(calls.find(([name]) => name === 'remove')[1], { origins: ['https://ddownload.com/*'] })
  assert.deepEqual(api.session.captchaTabs, {})
  // The report names the hoster and says the server took the answer. The tab closing says
  // nothing at all: it closes on a rejected token exactly as it does on an accepted one.
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'captchaSent:ddownload.com'))
  // The token itself reaches the server and nothing else.
  assert.ok(!calls.some(([, value]) => typeof value === 'string' && value.includes('0.token')))
})

test('a token from a tab this extension did not open, or for another captcha, is not posted', async () => {
  const calls = []
  const { answerer, posted } = setup({ calls })
  const { tabId } = await answerer.open(WIDGET)

  assert.deepEqual(await answerer.onMessage({ type: TOKEN_MESSAGE, id: WIDGET.id, token: 'x' }, { tab: { id: 999 } }), { ok: false, code: 'untracked' })
  assert.deepEqual(await answerer.onMessage({ type: TOKEN_MESSAGE, id: 'other', token: 'x' }, { tab: { id: tabId } }), { ok: false, code: 'untracked' })
  assert.equal(await answerer.onMessage({ type: 'rdownloader:send', text: 'x' }, { tab: { id: tabId } }), false)
  assert.deepEqual(posted, [])
})

test('closing the tab without answering declines the captcha and releases the permission', async () => {
  const calls = []
  const { answerer, posted } = setup({ calls })
  const { tabId } = await answerer.open(WIDGET)

  await answerer.onTabRemoved(tabId)
  await answerer.onTabRemoved(tabId)

  assert.deepEqual(calls.filter(([name]) => name === 'skip'), [['skip', WIDGET.id]], 'declined once, not twice')
  assert.deepEqual(posted, [])
  assert.deepEqual(calls.find(([name]) => name === 'remove')[1], { origins: ['https://ddownload.com/*'] })
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'captchaSkipped:ddownload.com'))
})

test('a captcha that stopped waiting closes its tab on the next poll without declining it', async () => {
  const calls = []
  let waiting = [WIDGET]
  const api = fakeApi(calls)
  const { answerer } = setup({ calls, api, list: async () => ({ ok: true, status: 200, widgets: waiting }) })
  await answerer.poll()
  const { tabId } = await answerer.open(WIDGET)
  assert.deepEqual(api.session.captchaNotifications, { [WIDGET.id]: 'n1' })

  waiting = []
  await answerer.poll()
  assert.deepEqual(api.session.captchaNotifications, {}, 'the announcement is pruned with its captcha')

  assert.ok(calls.some(([name, id]) => name === 'removeTab' && id === tabId))
  assert.ok(calls.some(([name]) => name === 'remove'))
  assert.ok(!names(calls).includes('skip'), 'an expired or already answered captcha is not declined again')
  assert.ok(calls.some(([name, text]) => name === 'badge' && text === ''))
})

test('a grant whose tab never opened is released once the captcha is gone', async () => {
  const calls = []
  let waiting = [WIDGET]
  const api = fakeApi(calls)
  // The browser tears the popup down under the permission prompt: the grant was announced,
  // the code after `request` never runs. The prompt is held open by a deferred the test
  // resolves at the end, rather than by a promise nobody ever settles (RD-109-26).
  let dismiss
  const prompt = new Promise((resolve) => { dismiss = resolve })
  api.permissions.request = async (value) => { calls.push(['request', value]); return prompt }
  const { answerer } = setup({ calls, api, list: async () => ({ ok: true, status: 200, widgets: waiting }) })
  await answerer.poll()
  const opening = answerer.open(WIDGET)
  await new Promise((resolve) => setTimeout(resolve, 0))
  assert.deepEqual(api.session.captchaGrants, { [WIDGET.id]: 'https://ddownload.com/*' })
  assert.ok(!names(calls).includes('createTab'))

  await answerer.poll()
  assert.ok(!names(calls).includes('remove'), 'the grant stays while the captcha still waits')

  waiting = []
  await answerer.poll()
  assert.deepEqual(calls.find(([name]) => name === 'remove')[1], { origins: ['https://ddownload.com/*'] })
  assert.deepEqual(api.session.captchaGrants, {})

  // The popup is gone by now; closing the prompt as refused is what the browser reports.
  dismiss(false)
  assert.deepEqual(await opening, { ok: false, code: 'denied' })
})

test('declining from the popup skips on the server and closes an open tab', async () => {
  const calls = []
  const { answerer } = setup({ calls })
  const { tabId } = await answerer.open(WIDGET)

  const result = await answerer.decline(WIDGET.id)

  assert.equal(result.ok, true)
  assert.deepEqual(calls.filter(([name]) => name === 'skip'), [['skip', WIDGET.id]])
  assert.ok(calls.some(([name, id]) => name === 'removeTab' && id === tabId))
})

test('clicking the announcement opens the popup, even from a restarted worker, and other notifications are left to their owner', async () => {
  const calls = []
  const api = fakeApi(calls)
  const { answerer } = setup({ calls, api })
  await answerer.poll()
  const announced = calls.find(([name]) => name === 'notify')
  assert.ok(announced)
  assert.deepEqual(api.session.captchaNotifications, { [WIDGET.id]: 'n1' })

  // A fresh answerer over the same session storage stands for a restarted service worker.
  const { answerer: restarted } = setup({ calls, api })
  assert.equal(await restarted.onNotificationClicked('n1'), true)
  assert.ok(names(calls).includes('openPopup'))
  assert.equal(await answerer.onNotificationClicked('n1'), false, 'handled once')
  assert.equal(await answerer.onNotificationClicked('somebody-elses'), false)
})

test('listWidgets names the extension to the server and carries the capture token', async () => {
  let seen = null
  const result = await listWidgets(CONFIG, async (url, init) => {
    seen = { url, init }
    return { ok: true, status: 200, json: async () => [WIDGET] }
  })
  assert.equal(result.ok, true)
  assert.deepEqual(result.widgets, [WIDGET])
  assert.equal(seen.url, 'http://127.0.0.1:8710/api/v1/capture/captchas?client=browser_extension')
  assert.equal(seen.init.headers.authorization, 'Bearer capture-token')
})

test('answerWidget posts the token to the capture route and echoes nothing back', async () => {
  let seen = null
  const result = await answerWidget(CONFIG, WIDGET.id, '0.token', async (url, init) => {
    seen = { url, init }
    return { ok: true, status: 200, json: async () => ({ code: 'captcha.solved', message: 'Captcha answer submitted' }) }
  })
  assert.deepEqual(result, { ok: true, status: 200, code: 'captcha.solved', message: null })
  assert.equal(seen.url, `http://127.0.0.1:8710/api/v1/capture/captchas/${WIDGET.id}/token`)
  assert.equal(seen.init.method, 'POST')
  assert.deepEqual(JSON.parse(seen.init.body), { token: '0.token' })
})

test('skipWidget and a rejected answer surface the server code', async () => {
  const skipped = await skipWidget(CONFIG, WIDGET.id, async (url, init) => {
    assert.equal(url, `http://127.0.0.1:8710/api/v1/capture/captchas/${WIDGET.id}/skip`)
    assert.equal(init.method, 'POST')
    return { ok: true, status: 200, json: async () => ({ code: 'captcha.skipped' }) }
  })
  assert.equal(skipped.code, 'captcha.skipped')

  const gone = await answerWidget(CONFIG, WIDGET.id, 'late', async () => ({
    ok: false, status: 404, json: async () => ({ code: 'captcha.not_waiting', error: 'gone' })
  }))
  assert.deepEqual(gone, { ok: false, status: 404, code: 'captcha.not_waiting', message: 'gone' })
})

test('the injected reader reads one field, sends the token once and writes nothing', async () => {
  const sent = []
  const field = { value: '' }
  globalThis.document = { querySelector: () => field }
  globalThis.chrome = { runtime: { sendMessage: (payload) => sent.push(payload) } }
  const timers = []
  let cleared = null
  const realSetInterval = globalThis.setInterval
  const realClearInterval = globalThis.clearInterval
  globalThis.setInterval = (fn) => { timers.push(fn); return 7 }
  // `clearInterval` used to be replaced by `timers.splice(0)`, which emptied the array the test
  // then polled — so the second tick was a no-op and a harvester that sends twice would have
  // passed (RD-109-26). It records the id instead and leaves the timer where it is.
  globalThis.clearInterval = (id) => { cleared = id }
  try {
    harvestAnswer('c1', 'input[name="cf-turnstile-response"]', TOKEN_MESSAGE)
    assert.equal(timers.length, 1, 'one poll, not one per look')
    assert.deepEqual(sent, [], 'nothing is sent while the field is empty')
    field.value = ' 0.token '
    timers[0]()
    assert.equal(cleared, 7, 'the poll it started is the poll it stops')
    // The timer is still callable here, exactly as a real one would be until the event loop
    // gets to the cancellation. A second tick must change nothing.
    timers[0]()
    timers[0]()
    assert.deepEqual(sent, [{ type: TOKEN_MESSAGE, id: 'c1', token: '0.token' }])
    assert.equal(field.value, ' 0.token ', 'the page is read, never written')
  } finally {
    globalThis.setInterval = realSetInterval
    globalThis.clearInterval = realClearInterval
    delete globalThis.document
    delete globalThis.chrome
    delete globalThis.__rdownloaderCaptchaHarvester
  }
})

/**
 * The page the harvester runs in, with a poll the test drives by hand.
 *
 * `clearInterval` records the id it was given and leaves the timer callable, exactly as a real
 * one is until the event loop gets to the cancellation: a fake that removed the timer would
 * answer its own question (RD-109-26).
 */
function injectedPage(run) {
  const sent = []
  const field = { value: '' }
  const timers = []
  const cleared = []
  const real = { setInterval: globalThis.setInterval, clearInterval: globalThis.clearInterval }
  globalThis.document = { querySelector: () => field }
  globalThis.chrome = { runtime: { sendMessage: (payload) => sent.push(payload) } }
  let nextTimer = 0
  globalThis.setInterval = (fn) => { nextTimer += 1; timers.push({ id: nextTimer, fn }); return nextTimer }
  globalThis.clearInterval = (id) => cleared.push(id)
  try {
    return run({ sent, field, timers, cleared })
  } finally {
    globalThis.setInterval = real.setInterval
    globalThis.clearInterval = real.clearInterval
    delete globalThis.document
    delete globalThis.chrome
    delete globalThis.__rdownloaderCaptchaHarvester
  }
}

test('an unanswered widget does not leave a poll running in the page for ever', () => {
  // RD-109-23, finding 3: the 500 ms interval was cleared only when a token was found, so a
  // widget nobody solves left it ticking until the tab was closed.
  injectedPage(({ sent, timers, cleared }) => {
    harvestAnswer('c1', 'input', TOKEN_MESSAGE, 10_000)
    timers[0].fn()
    assert.deepEqual(cleared, [], 'it keeps looking while the widget may still be answered')
    assert.deepEqual(sent, [])
  })
  injectedPage(({ sent, timers, cleared }) => {
    // The same poll once its deadline has passed: it stops itself, answered or not.
    harvestAnswer('c1', 'input', TOKEN_MESSAGE, 0)
    assert.deepEqual(cleared, [1], 'an unanswered poll stops itself, and it is the one it started')
    assert.deepEqual(sent, [], 'stopping is not answering')
    timers[0].fn()
    assert.deepEqual(cleared, [1], 'and it does not try to stop it twice')
  })
})

test('a second injection into the same document starts no second poll and sends no second token', () => {
  // `onTabUpdated` injects on every `complete`, deliberately, so a navigation is followed. In
  // the same document that used to mean a second timer and a token sent twice — the second send
  // answered with `untracked`, because the first had already taken the tab entry (RD-109-23).
  injectedPage(({ sent, field, timers }) => {
    const first = harvestAnswer('c1', 'input', TOKEN_MESSAGE, 10_000)
    const second = harvestAnswer('c1', 'input', TOKEN_MESSAGE, 10_000)
    assert.equal(timers.length, 1, 'one poll per document, not one per injection')

    field.value = '0.token'
    for (const timer of timers) timer.fn()
    assert.deepEqual(sent, [{ type: TOKEN_MESSAGE, id: 'c1', token: '0.token' }], 'one token, once')
    assert.deepEqual([first, second], [true, false], 'and the second injection says it took nothing on')
  })
})

test('a payload that cannot cross the message boundary does not quietly cross it', async () => {
  // `captcha.test.mjs` used to wire `send` straight into `onMessage`, so there was no boundary
  // at all: a payload a browser could never serialise would have passed (RD-109-26).
  const calls = []
  const { api } = setup({ calls })
  await assert.rejects(() => api.runtime.sendMessage({ type: OPEN_MESSAGE, widget: () => 'not data' }))
  // And an ordinary payload arrives, answered by the handler on the other side.
  assert.deepEqual(
    await api.runtime.sendMessage({ type: GRANT_MESSAGE, id: WIDGET.id, origin: 'https://ddownload.com/*' }),
    { ok: true }
  )
})

// ---------------------------------------------------------------------------------------------
// RD-109-23: what the flow reports must be what the server was told.

test('a decline the server never heard of is not reported as a decline', async () => {
  // Finding 1: `onTabRemoved` discarded `skip`'s result and announced `captchaSkipped` whatever
  // came back. With an expired capture token the POST answers 401, the waiting download runs
  // into a timeout instead of failing with `captcha.skipped`, and the person was told the
  // opposite of what happened.
  const calls = []
  const { answerer } = setup({
    calls,
    skip: async (_config, id) => { calls.push(['skip', id]); return { ok: false, status: 401, code: 'auth.unauthorized', message: 'unauthorized' } }
  })
  const { tabId } = await answerer.open(WIDGET)

  await answerer.onTabRemoved(tabId)

  const reported = calls.filter(([name]) => name === 'notify').map(([, body]) => body)
  assert.ok(!reported.includes('captchaSkipped:ddownload.com'), 'nothing was declined')
  assert.ok(
    reported.includes('captchaSkipFailed:ddownload.com,errorUnauthorized'),
    `the refusal is what the person hears, not a decline: ${JSON.stringify(reported)}`
  )
  // The origin is still given back: the tab is gone either way.
  assert.deepEqual(calls.find(([name]) => name === 'remove')[1], { origins: ['https://ddownload.com/*'] })
})

test('a closed tab on an unpaired extension, or one the call threw on, says so too', async () => {
  const calls = []
  const config = { ...CONFIG }
  const { answerer } = setup({ calls, config })
  const first = await answerer.open(WIDGET)
  config.token = ''
  await answerer.onTabRemoved(first.tabId)
  assert.ok(!names(calls).includes('skip'), 'there is nothing to tell the server with')
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'captchaSkipFailed:ddownload.com,unconfigured'))

  const thrown = []
  const { answerer: broken } = setup({
    calls: thrown,
    skip: async () => { throw new Error('the worker died under the call') }
  })
  const second = await broken.open(WIDGET)
  await broken.onTabRemoved(second.tabId)
  assert.ok(thrown.some(([name, body]) => name === 'notify' && body === 'captchaSkipFailed:ddownload.com,the worker died under the call'))
})

test('a forget during a slow openTab survives, and the origin is released with the last tab', async () => {
  // Finding 2: `openTab` read the tab map, awaited `tabs.create` and wrote the whole map back
  // afterwards. A `forget` in that window was overwritten; the resurrected entry carries the
  // origin, `release` reads it as "still needed", and the hoster permission never comes back.
  const calls = []
  const api = fakeApi(calls)
  const A_ORIGIN = 'https://hoster-a.test/*'
  const first = { ...WIDGET, id: 'widget-a-1', page_url: 'https://hoster-a.test/one' }
  const other = { ...WIDGET, id: 'widget-b', page_url: 'https://hoster-b.test/one' }
  const later = { ...WIDGET, id: 'widget-a-2', page_url: 'https://hoster-a.test/two' }
  const { answerer } = setup({ calls, api, widgets: [first, other, later] })
  const firstTab = await answerer.open(first)

  // `tabs.create` is the slow part, and the test holds it exactly there.
  const create = api.tabs.create
  let reached
  let proceed
  const arrived = new Promise((resolve) => { reached = resolve })
  const held = new Promise((resolve) => { proceed = resolve })
  api.tabs.create = async (options) => { reached(); await held; return create(options) }
  const opening = answerer.open(other)
  await arrived

  // The person closes the first hoster's tab while the second one is still being opened.
  await answerer.onTabRemoved(firstTab.tabId)
  proceed()
  const otherTab = await opening
  api.tabs.create = create

  assert.deepEqual(
    Object.keys(api.session.captchaTabs),
    [String(otherTab.tabId)],
    'the entry for the closed tab was written back over its own removal'
  )

  // And the consequence the finding names: a later widget on the same hoster can give the
  // origin back, which a ghost entry carrying that origin would prevent for ever.
  const laterTab = await answerer.open(later)
  await answerer.onTabRemoved(laterTab.tabId)
  const released = calls.filter(([name, value]) => name === 'remove' && value?.origins?.[0] === A_ORIGIN)
  assert.equal(released.length, 2, 'the hoster origin is held by an entry for a tab that is gone')
})

test('without storage.session the captcha state stays in memory, and an older version\'s is dropped', async () => {
  // Finding 4: `api.storage.session ?? api.storage.local` put tab ids, grants and notification
  // ids on the disk wherever the session area is missing. After a browser restart `pollOnce`
  // closed tabs by ids that belong to somebody else's windows by then.
  const calls = []
  const api = fakeApi(calls)
  const disk = {}
  api.storage = {
    local: {
      get: async (defaults) => ({ ...defaults }),
      set: async (values) => { calls.push(['localSet', Object.keys(values)]); Object.assign(disk, values) },
      remove: async (keys) => calls.push(['localRemove', keys])
    }
  }
  const { answerer } = setup({ calls, api })

  const opened = await answerer.open(WIDGET)

  assert.equal(opened.ok, true)
  assert.deepEqual(disk, {}, 'no tab id, grant or notification id reaches a store that outlives the browser')
  assert.ok(!names(calls).includes('localSet'))
  // In memory it is all still there, so the flow works for this worker's lifetime.
  await answerer.onTabRemoved(opened.tabId)
  assert.deepEqual(calls.filter(([name]) => name === 'skip'), [['skip', WIDGET.id]])
  // And what an older version left behind goes at startup, before anything can act on it.
  await answerer.schedule()
  assert.deepEqual(calls.find(([name]) => name === 'localRemove')?.[1], STATE_KEYS)
})

test('a storage area that throws is survived rather than reported over', async () => {
  // The `read`/`write` catch blocks, which no test had ever entered.
  const calls = []
  const api = fakeApi(calls)
  api.storage.session = {
    get: async () => { throw new Error('the session area is gone') },
    set: async () => { throw new Error('the session area is gone') }
  }
  const { answerer } = setup({ calls, api })

  const opened = await answerer.open(WIDGET)
  assert.equal(opened.ok, true)
  // Nothing can be remembered, so that tab's token is not accepted — and that is what is said.
  assert.deepEqual(
    await answerer.onMessage({ type: TOKEN_MESSAGE, id: WIDGET.id, token: 'x' }, { tab: { id: opened.tabId } }),
    { ok: false, code: 'untracked' }
  )
  assert.deepEqual(await answerer.poll(), { ok: true, status: 200, widgets: [WIDGET] })
})

test('the popup listing is the background\'s own run: it announces, prunes and paints the badge', async () => {
  // Finding 5: `popup.js` called `listWidgets` directly, so its listing announced nothing,
  // removed no stale notification id and never touched the badge. The list and the number
  // beside it could disagree for as long as the count itself did not move.
  const calls = []
  let waiting = [WIDGET]
  const api = fakeApi(calls)
  const { answerer } = setup({ calls, api, list: async () => { calls.push(['list']); return { ok: true, status: 200, widgets: waiting } } })

  const listed = await answerer.requestPoll()

  assert.deepEqual(listed.widgets, [WIDGET])
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'captchaWaiting:ddownload.com'))
  assert.deepEqual(calls.filter(([name]) => name === 'badge').at(-1), ['badge', '1'])
  assert.deepEqual(api.session.captchaNotifications, { [WIDGET.id]: 'n1' })

  waiting = []
  const again = await answerer.requestPoll()

  assert.deepEqual(again.widgets, [])
  assert.deepEqual(api.session.captchaNotifications, {}, 'the popup prunes the announcement it is showing')
  assert.deepEqual(calls.filter(([name]) => name === 'badge').at(-1), ['badge', ''])
})

test('a poll or a decline from anything but our own page is ignored', async () => {
  // Finding 6: `rdownloader:captcha-decline` was answered by a branch of its own in
  // `background.js`, without the `fromOwnPage` check its two siblings get.
  const calls = []
  const { answerer } = setup({ calls })
  for (const sender of [{ id: 'self', tab: { id: 5 }, url: 'https://ddownload.com/login.html' }, { id: 'someone-else' }, undefined]) {
    assert.equal(await answerer.onMessage({ type: POLL_MESSAGE }, sender), false)
    assert.equal(await answerer.onMessage({ type: DECLINE_MESSAGE, id: WIDGET.id }, sender), false)
  }
  assert.ok(!names(calls).includes('skip'), 'a page on the hoster cannot decline the captcha it is showing')
  assert.ok(!names(calls).includes('list'))

  // The popup itself is answered, over a real message boundary.
  const declined = await answerer.onMessage({ type: DECLINE_MESSAGE, id: WIDGET.id }, POPUP)
  assert.equal(declined.ok, true)
  assert.deepEqual(calls.filter(([name]) => name === 'skip'), [['skip', WIDGET.id]])
})

test('a tab closed by a poll because its captcha stopped waiting is not silence', async () => {
  // The tab closing by itself was the whole report, and `captchaOpened` promised it meant the
  // answer had gone through. It is also what an expired captcha looks like (RD-109-23).
  const calls = []
  let waiting = [WIDGET]
  const api = fakeApi(calls)
  const { answerer } = setup({ calls, api, list: async () => ({ ok: true, status: 200, widgets: waiting }) })
  await answerer.poll()
  await answerer.open(WIDGET)

  waiting = []
  await answerer.poll()

  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'captchaGone:ddownload.com'))
  assert.ok(!names(calls).includes('skip'), 'and it is still not declined a second time')
})

test('the error branches of the flow report their own failure', async () => {
  // Finding 7, the six branches no test had ever entered.
  // 1. The listing itself fails: the failure is returned, nothing is announced or painted.
  const failing = []
  const { answerer: unreachable } = setup({
    calls: failing,
    list: async () => ({ ok: false, status: 0, code: 'network', message: 'connection refused', widgets: [] })
  })
  assert.deepEqual(await unreachable.poll(), { ok: false, status: 0, code: 'network', message: 'connection refused', widgets: [] })
  assert.ok(!names(failing).includes('notify'))
  assert.ok(!names(failing).includes('badge'))
  // And the popup gets the same answer through the background rather than a cheerful empty list.
  assert.deepEqual(await unreachable.requestPoll(), { ok: false, status: 0, code: 'network', message: 'connection refused', widgets: [] })

  // 2. `permissions.request` throws synchronously — no user gesture left to spend.
  const refused = []
  const api = fakeApi(refused)
  api.permissions.request = () => { throw new Error('may only be called from a user gesture') }
  const { answerer: gestureless } = setup({ calls: refused, api })
  assert.deepEqual(await gestureless.open(WIDGET), { ok: false, code: 'denied' })
  assert.ok(!names(refused).includes('createTab'))
  assert.deepEqual(api.session.captchaGrants, {}, 'the announced grant is withdrawn again')

  // 3. The background cannot be reached: `code: 'background'`, never a reported success.
  const mute = []
  const { answerer: orphaned } = setup({ calls: mute, send: async () => { throw new Error('receiving end does not exist') } })
  assert.deepEqual(await orphaned.open(WIDGET), { ok: false, code: 'background' })
  assert.deepEqual(await orphaned.requestPoll(), { ok: false, status: 0, code: 'background', message: 'receiving end does not exist', widgets: [] })

  // 4. The page cannot be injected into (an error page, a redirect off the origin).
  const blind = []
  const blindApi = fakeApi(blind)
  blindApi.scripting.executeScript = async () => { throw new Error('cannot access contents of the page') }
  const { answerer: uninjectable } = setup({ calls: blind, api: blindApi })
  const { tabId } = await uninjectable.open(WIDGET)
  await uninjectable.onTabUpdated(tabId, { status: 'complete' })
  assert.ok(blindApi.session.captchaTabs[String(tabId)], 'the tab stays tracked, so closing it still declines')

  // 5. Declining without a capture token says which state it is in.
  const unpaired = []
  const { answerer: unconfigured } = setup({ calls: unpaired, config: { server: CONFIG.server, token: '' } })
  assert.deepEqual(await unconfigured.decline(WIDGET.id), { ok: false, code: 'unconfigured' })
  assert.ok(!names(unpaired).includes('skip'))

  // 6. `action.openPopup` throws where the browser has none: the popup opens as a tab instead.
  const clicked = []
  const clickedApi = fakeApi(clicked)
  const { answerer: announcer } = setup({ calls: clicked, api: clickedApi })
  await announcer.poll()
  clickedApi.action.openPopup = async () => { throw new Error('openPopup is not a function') }
  assert.equal(await announcer.onNotificationClicked('n1'), true)
  assert.deepEqual(clicked.find(([name]) => name === 'createTab')[1], { url: 'chrome-extension://self/src/popup.html', active: true })
})
