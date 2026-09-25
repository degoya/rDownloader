import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  BUFFER_LIMIT,
  CAPABILITY_TTL_MS,
  CORRELATION_WINDOW_MS,
  NOTICE_TTL_MS
} from '../src/downloads.js'
import { SERVER, clock, setupInterceptor as setup } from './fakes.mjs'

const ITEM = { id: 7, url: 'https://files.test/report.pdf', filename: 'report.pdf' }

test('a successful handoff pauses, submits, cancels and erases', async () => {
  const calls = []
  const { interceptor, bodies } = setup({ calls })
  await interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(
    calls.filter(([name]) => name !== 'notify' && name !== 'clearNotification'),
    [
      ['pause', 7],
      ['submit'],
      ['cancel', 7],
      ['erase', 7]
    ]
  )
  assert.equal(bodies[0].source, 'browser_download')
  assert.equal(bodies[0].links[0].url, ITEM.url)
})

test('a failed submit resumes the browser download and never cancels it', async () => {
  const calls = []
  const { interceptor } = setup({
    calls,
    submit: async () => ({ ok: false, status: 401, code: 'auth.unauthorized', message: 'nope', links: 0 })
  })
  await interceptor.onDownloadCreated(ITEM)
  const names = calls.map(([name]) => name)
  assert.ok(names.includes('resume'))
  assert.ok(!names.includes('cancel'))
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'interceptFailed'))
})

test('a rejected pause aborts the handoff without submitting', async () => {
  const calls = []
  const { interceptor, bodies } = setup({
    calls,
    pause: async () => {
      throw new Error('download already complete')
    }
  })
  await interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(calls, [['pause', 7]])
  assert.deepStrictEqual(bodies, [])
})

test('keep in browser resumes and blocks a late success from cancelling', async () => {
  const calls = []
  let release
  let submitSeen
  const seen = new Promise((resolve) => {
    submitSeen = resolve
  })
  const { interceptor } = setup({
    calls,
    submit: () => {
      calls.push(['submit'])
      submitSeen()
      return new Promise((resolve) => {
        release = resolve
      })
    }
  })
  const running = interceptor.onDownloadCreated(ITEM)
  await seen
  await interceptor.onNotificationButtonClicked('n1', 0)
  release({ ok: true, status: 201, links: 1 })
  await running
  const names = calls.map(([name]) => name)
  assert.ok(names.includes('resume'))
  assert.ok(!names.includes('cancel'))
  assert.ok(!names.includes('erase'))
})

test('a server without capture support receives the legacy body', async () => {
  const calls = []
  const { interceptor, bodies } = setup({ calls, captureVersion: 0 })
  await interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(bodies, [
    { text: ITEM.url, source: 'browser_extension', source_label: 'Browser', package_name: 'report.pdf' }
  ])
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'interceptLegacyServer'))
})

test('downloads from the rDownloader web interface are ignored entirely', async () => {
  const calls = []
  const { interceptor, bodies } = setup({ calls })
  // Not a ZIP: that would be left to the browser by RD-120-63 and prove nothing about the origin.
  await interceptor.onDownloadCreated({ id: 9, url: `${SERVER}/api/v1/files/9`, filename: 'a.mkv' })
  await interceptor.onDownloadCreated({ id: 10, url: 'https://x.test/a.mkv', referrer: `${SERVER}/downloads` })
  assert.deepStrictEqual(calls, [])
  assert.deepStrictEqual(bodies, [])
})

test('interception stays off without a token or when disabled', async () => {
  const calls = []
  const { interceptor } = setup({ calls, config: { token: '' } })
  await interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(calls, [['notify', 'notConfigured', 'plain']])

  const other = []
  const disabled = setup({ calls: other, config: { interceptDownloads: false } })
  await disabled.interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(other, [])
})

test('a download the page started with a POST is kept by the browser, never handed over as a GET', async () => {
  // RD-109-20: the body path is gone, so a POST can never be reproduced. Handing it over as a
  // GET would fetch something else entirely and store it under the right file name.
  for (const observe of [
    (interceptor) => interceptor.onBeforeRequest({ url: ITEM.url, method: 'POST' }),
    (interceptor) => interceptor.onSendHeaders({ url: ITEM.url, method: 'POST', requestHeaders: [] })
  ]) {
    const calls = []
    const { interceptor, bodies } = setup({ calls })
    observe(interceptor)
    await interceptor.onDownloadCreated(ITEM)
    assert.deepStrictEqual(bodies, [], 'nothing is submitted for a POST')
    // RD-120-63: decided before the pause, so the browser's download is never touched at all.
    assert.deepStrictEqual(calls, [['notify', 'interceptPostUnsupported', 'plain']])
  }
})

test('an NZB, torrent or ZIP is left to the browser untouched and silently', async () => {
  // RD-120-63: rDownloader would fetch the address again without the browser's session; an
  // indexer cart answers that with an error and a one-time link is already spent.
  for (const item of [
    { ...ITEM, url: 'https://indexer.test/getnzb/abc', filename: '', mime: 'application/x-nzb' },
    { ...ITEM, url: 'https://indexer.test/getnzb?id=1&zip=1', filename: '', mime: 'application/zip' },
    { ...ITEM, url: 'https://tracker.test/dl/5', filename: '/home/me/Downloads/show.torrent' },
    { ...ITEM, url: 'https://files.test/archive.zip', filename: '' }
  ]) {
    const calls = []
    const { interceptor, bodies } = setup({ calls, observing: true, hostAccess: true })
    await interceptor.onDownloadCreated(item)
    assert.deepStrictEqual(calls, [], `${item.url} was touched`)
    assert.deepStrictEqual(bodies, [])
  }
})

test('an ordinary file is still handed over exactly as before', async () => {
  const calls = []
  const { interceptor, bodies } = setup({ calls })
  const item = { id: 8, url: 'https://files.test/get/5', filename: '/home/me/Downloads/movie.mkv', mime: 'video/x-matroska' }
  await interceptor.onDownloadCreated(item)
  assert.deepStrictEqual(
    calls.filter(([name]) => name !== 'notify' && name !== 'clearNotification'),
    [['pause', 8], ['submit'], ['cancel', 8], ['erase', 8]]
  )
  assert.equal(bodies[0].links[0].url, item.url)
  assert.equal(bodies[0].links[0].file_name, 'movie.mkv')
  assert.equal(bodies[0].links[0].request.method, 'GET')
})

test('an NZB named only by its observed Content-Disposition stays in the browser', async () => {
  const calls = []
  const item = { ...ITEM, url: 'https://indexer.test/getnzb/abc', filename: '' }
  const { interceptor, bodies } = setup({ calls })
  interceptor.onHeadersReceived({
    url: item.url,
    responseHeaders: [{ name: 'Content-Disposition', value: 'attachment; filename="Some.Show.S01E01.nzb"' }]
  })
  await interceptor.onDownloadCreated(item)
  assert.deepStrictEqual(calls, [])
  assert.deepStrictEqual(bodies, [])
})

test('an NZB stays in the browser even before a server is configured', async () => {
  const calls = []
  const { interceptor } = setup({ calls, config: { token: '' } })
  await interceptor.onDownloadCreated({ ...ITEM, mime: 'application/x-nzb' })
  assert.deepStrictEqual(calls, [], 'no "configure first" notice for a download that never goes')
})

test('a download we could have observed but did not stays with the browser', async () => {
  // RD-109-18: `correlateRequest` returning null used to be indistinguishable from an observed
  // GET, so a POST whose buffered entry had been evicted went over as `method: "GET"`.
  const calls = []
  const { interceptor, bodies } = setup({ calls, observing: true, hostAccess: true })
  await interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(bodies, [], 'nothing is submitted for a request we never saw')
  assert.deepStrictEqual(calls.find(([name]) => name === 'contains'), ['contains', 'https://files.test/*'])
  const names = calls.map(([name]) => name)
  assert.ok(names.includes('resume'))
  assert.ok(!names.includes('cancel'))
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'interceptRequestUnknown'))
})

test('a download we could not observe at all is still handed over', async () => {
  // The other half of the same decision: a plain GET download needs no observed request, and a
  // default install holds host access for no hoster, so refusing here would switch the whole
  // feature off. Both branches of "no correlated request" are deliberate.
  for (const options of [{ observing: false, hostAccess: true }, { observing: true, hostAccess: false }]) {
    const calls = []
    const { interceptor, bodies } = setup({ calls, ...options })
    await interceptor.onDownloadCreated(ITEM)
    assert.equal(bodies.length, 1)
    assert.equal(bodies[0].links[0].request.method, 'GET')
    assert.ok(calls.map(([name]) => name).includes('cancel'))
  }
})

test('a flood of GETs no longer evicts the buffered POST', async () => {
  const calls = []
  const { interceptor, bodies } = setup({ calls })
  interceptor.onSendHeaders({ url: ITEM.url, method: 'POST', requestHeaders: [] })
  for (let index = 0; index < BUFFER_LIMIT + 10; index += 1) {
    interceptor.onSendHeaders({ url: `https://poller.test/${index}`, method: 'GET', requestHeaders: [] })
  }
  await interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(bodies, [], 'the POST is still the newest entry for this address')
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'interceptPostUnsupported'))
})

test('merging two events of one request refreshes its age', async () => {
  const time = clock()
  const calls = []
  const { interceptor, bodies } = setup({ calls, time })
  interceptor.onSendHeaders({ url: ITEM.url, method: 'GET', requestHeaders: [{ name: 'Accept', value: '*/*' }] })
  time.advance(8_000)
  interceptor.onHeadersReceived({
    url: ITEM.url,
    responseHeaders: [{ name: 'Content-Disposition', value: 'attachment; filename="report.pdf"' }]
  })
  time.advance(8_000)
  // Sixteen seconds after the first event, eight after the last: inside the window that counts.
  await interceptor.onDownloadCreated(ITEM)
  assert.equal(bodies[0].links[0].request.content_disposition, 'attachment; filename="report.pdf"')
  assert.deepStrictEqual(bodies[0].links[0].request.headers, [{ name: 'accept', value: '*/*' }])
})

test('the correlation window really expires', async () => {
  const time = clock()
  const calls = []
  const { interceptor, bodies } = setup({ calls, time, observing: true, hostAccess: true })
  interceptor.onSendHeaders({ url: ITEM.url, method: 'GET', requestHeaders: [{ name: 'Accept', value: '*/*' }] })
  time.advance(CORRELATION_WINDOW_MS + 1)
  await interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(bodies, [], 'an aged-out entry is no entry')
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'interceptRequestUnknown'))
})

test('the negotiated capture version really goes stale', async () => {
  const time = clock()
  const calls = []
  const { interceptor, pings } = setup({ calls, time })
  await interceptor.onDownloadCreated(ITEM)
  await interceptor.onDownloadCreated({ ...ITEM, id: 8 })
  assert.equal(pings.count, 1, 'the second download rides on the cached answer')
  time.advance(CAPABILITY_TTL_MS + 1)
  await interceptor.onDownloadCreated({ ...ITEM, id: 9 })
  assert.equal(pings.count, 2)
})

test('the negotiated version and the one-off notices survive a service-worker teardown', async () => {
  // A fresh interceptor over the same session storage is what a restarted MV3 worker is.
  const session = {}
  const time = clock()
  const calls = []
  const first = setup({ calls, time, session })
  await first.interceptor.onDownloadCreated(ITEM)
  assert.equal(first.pings.count, 1)

  const second = setup({ calls, time, session })
  await second.interceptor.onDownloadCreated({ ...ITEM, id: 8 })
  assert.equal(second.pings.count, 0, 'the restarted worker reuses the negotiated version')

  const unconfigured = []
  const third = setup({ calls: unconfigured, time, session: {}, config: { token: '' } })
  await third.interceptor.onDownloadCreated(ITEM)
  const fourth = setup({ calls: unconfigured, time, session: third.session, config: { token: '' } })
  await fourth.interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(
    unconfigured.filter(([, body]) => body === 'notConfigured'),
    [['notify', 'notConfigured', 'plain']],
    'the hint is said once across the teardown, not on every download'
  )
})

test('surviving state expires, and an expired entry counts as none', async () => {
  const session = {}
  const time = clock()
  const calls = []
  const { interceptor, pings } = setup({ calls, time, session })
  await interceptor.onDownloadCreated(ITEM)
  assert.equal(pings.count, 1)
  time.advance(NOTICE_TTL_MS + 1)

  const later = setup({ calls, time, session })
  await later.interceptor.onDownloadCreated({ ...ITEM, id: 8 })
  assert.equal(later.pings.count, 1, 'a stale capability is renegotiated, not trusted')

  // An entry a future version wrote differently, or one without an expiry at all, is not
  // better than no entry.
  const broken = setup({ calls, time, session: { captureCapability: { value: 2 } } })
  await broken.interceptor.onDownloadCreated({ ...ITEM, id: 9 })
  assert.equal(broken.pings.count, 1)
})

test('a service that does not answer is not called too old', async () => {
  const calls = []
  const { interceptor, bodies } = setup({
    calls,
    ping: async () => ({ ok: false, status: 0, message: 'connection refused' })
  })
  await interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(bodies, [], 'nothing is submitted to a service that is not there')
  const names = calls.map(([name]) => name)
  assert.ok(names.includes('resume'), 'the browser keeps its own download')
  assert.ok(!names.includes('cancel'))
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'interceptServerUnreachable'))
  assert.ok(!calls.some(([name, body]) => name === 'notify' && body === 'interceptLegacyServer'))
})

test('a throwing cancel does not swallow the rest of the handoff', async () => {
  // The download finished while the capture POST was in flight. `cancel` was the one unguarded
  // browser call in `finish`, so the notification stayed up for good and the success was never
  // reported (RD-109-24).
  const calls = []
  const { interceptor } = setup({
    calls,
    cancel: async () => {
      throw new Error('download already complete')
    }
  })
  await interceptor.onDownloadCreated(ITEM)
  const names = calls.map(([name]) => name)
  assert.ok(names.includes('erase'))
  assert.ok(calls.some(([name, id]) => name === 'clearNotification' && id === 'n1'))
  assert.ok(calls.some(([name, body]) => name === 'notify' && body === 'interceptSent'))
})

test('a download the person took back gets no failure notice', async () => {
  const calls = []
  let release
  let submitSeen
  const seen = new Promise((resolve) => { submitSeen = resolve })
  const { interceptor } = setup({
    calls,
    submit: () => {
      submitSeen()
      return new Promise((resolve) => { release = resolve })
    }
  })
  const running = interceptor.onDownloadCreated(ITEM)
  await seen
  await interceptor.onNotificationButtonClicked('n1', 0)
  release({ ok: false, status: 500, message: 'server exploded', links: 0 })
  await running
  assert.ok(
    !calls.some(([name, body]) => name === 'notify' && body === 'interceptFailed'),
    'the download is doing exactly what they asked for'
  )
})

test('a browser without notification buttons still gets a notification', async () => {
  const calls = []
  const { interceptor } = setup({ calls, buttons: false })
  await interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(
    calls.filter(([name, body]) => name === 'notify' && body === 'interceptSending'),
    [['notify', 'interceptSending', 'plain']]
  )
  // The plain notification is still the handle for "keep in browser": in Firefox the whole
  // notification acts as that action.
  assert.ok(calls.some(([name, id]) => name === 'clearNotification' && id === 'n1'))
})

test('both notification handlers keep the download in the browser', async () => {
  for (const click of [
    (interceptor) => interceptor.onNotificationClicked('n1'),
    (interceptor) => interceptor.onNotificationButtonClicked('n1', 0)
  ]) {
    const calls = []
    let release
    let submitSeen
    const seen = new Promise((resolve) => { submitSeen = resolve })
    const { interceptor } = setup({
      calls,
      submit: () => {
        submitSeen()
        return new Promise((resolve) => { release = resolve })
      }
    })
    const running = interceptor.onDownloadCreated(ITEM)
    await seen
    await click(interceptor)
    release({ ok: true, status: 201, links: 1 })
    await running
    const names = calls.map(([name]) => name)
    assert.ok(names.includes('resume'))
    assert.ok(!names.includes('cancel'))
  }
  // A button other than "keep in browser" is not an answer to anything.
  const other = []
  const { interceptor } = setup({ calls: other })
  await interceptor.onNotificationButtonClicked('n1', 1)
  assert.deepStrictEqual(other, [])
})

test('captured request headers and content-disposition reach the payload', async () => {
  // Which headers survive the filter is `filterHeaders`' own business and is asserted once, in
  // `intercept.test.mjs`. What this one is about is that the observation reaches the payload
  // at all (RD-109-26).
  const calls = []
  const { interceptor, bodies } = setup({ calls })
  interceptor.onSendHeaders({
    url: ITEM.url,
    requestHeaders: [{ name: 'Accept', value: 'application/pdf' }]
  })
  interceptor.onHeadersReceived({
    url: ITEM.url,
    responseHeaders: [{ name: 'Content-Disposition', value: 'attachment; filename="report.pdf"' }]
  })
  await interceptor.onDownloadCreated(ITEM)
  assert.deepStrictEqual(bodies[0].links[0].request, {
    method: 'GET',
    user_agent: 'Mozilla/5.0 (X11) Chrome/130',
    content_disposition: 'attachment; filename="report.pdf"',
    headers: [{ name: 'accept', value: 'application/pdf' }]
  })
})
