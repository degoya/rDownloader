// RD-1460-02: a browser-only file a page builds itself and downloads from a `blob:` address goes
// to rDownloader from a site the person allowed — under the same rules as every other hand-over
// of such a file — and the browser gets it whenever rDownloader does not take it.

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import vm from 'node:vm'

import { SITES_KEY, sitePattern } from '../src/files.js'
import { BROWSER_ONLY_EXTENSIONS, BROWSER_ONLY_TYPES } from '../src/intercept.js'
import { PAGE_FILE_MAX_BYTES, PAGE_FILE_MESSAGE, PAGE_SCRIPT_IDS, TAB_LIMIT, TAB_WINDOW_MS, createPageFiles, decodeBytes, pageScripts } from '../src/page-files.js'

import { fakeBrowser } from './fake-browser.mjs'
import { CONFIG, clock } from './fakes.mjs'

const HOST = 'hide.test'
const PAGE = `https://${HOST}/container/1`
const SENDER = { id: 'self', tab: { id: 4 }, url: PAGE }
const DLC = { type: PAGE_FILE_MESSAGE, name: 'Release.dlc', mime: 'application/octet-stream', data: btoa('dlc-bytes') }

function browser(calls, { sites = [HOST], granted = [sitePattern(HOST)], registered = [] } = {}) {
  return fakeBrowser('chrome', { calls, local: { [SITES_KEY]: sites }, grants: granted, scripts: registered })
}

function setup(calls, { config = {}, result = { ok: true, status: 201 }, time = clock(), ...options } = {}) {
  const { api, scripts } = browser(calls, options)
  const pageFiles = createPageFiles({
    api,
    loadConfig: async () => ({ ...CONFIG, interceptDownloads: true, ...config }),
    message: (key, substitutions) => (substitutions ? `${key}:${substitutions.join(',')}` : key),
    notify: async (body) => calls.push(['notify', body]),
    log: () => {},
    now: time.now,
    submitBytes: async (_config, body) => {
      calls.push(['submit', body.fileName, await body.bytes.text()])
      return result
    }
  })
  return { pageFiles, scripts }
}

test('a .dlc from an allowed site goes to rDownloader as bytes', async () => {
  const calls = []
  const { pageFiles } = setup(calls)
  assert.deepEqual(await pageFiles.onMessage(DLC, SENDER), { taken: true })
  assert.deepEqual(calls, [['submit', 'Release.dlc', 'dlc-bytes'], ['notify', 'fileHandedOver']])
})

test('the browser keeps the file whenever a rule of the hand-over says so', async () => {
  const cases = [
    ['no token', { config: { token: '' } }, DLC, SENDER],
    ['taking over switched off', { config: { interceptDownloads: false } }, DLC, SENDER],
    ['the type filter leaves .dlc out', { config: { interceptMode: 'only', interceptTypes: ['.nzb'] } }, DLC, SENDER],
    ['a site without consent', { sites: [] }, DLC, SENDER],
    ['a consent whose grant is gone', { granted: [] }, DLC, SENDER],
    ['not a browser-only file', {}, { ...DLC, name: 'cover.jpg' }, SENDER],
    ['not base64', {}, { ...DLC, data: '%%%' }, SENDER],
    ['no bytes at all', {}, { ...DLC, data: '' }, SENDER],
    ['another extension', {}, DLC, { ...SENDER, id: 'other' }],
    ['no tab', {}, DLC, { id: 'self', url: PAGE }]
  ]
  for (const [why, options, request, sender] of cases) {
    const calls = []
    const { pageFiles } = setup(calls, options)
    assert.deepEqual(await pageFiles.onMessage(request, sender), { taken: false }, why)
    assert.deepEqual(calls, [], why)
  }
})

test("the site comes from the browser's address of the frame, not from the page", async () => {
  const calls = []
  const { pageFiles } = setup(calls)
  const forged = { ...DLC, url: PAGE, host: HOST }
  assert.deepEqual(await pageFiles.onMessage(forged, { ...SENDER, url: 'https://elsewhere.test/' }), { taken: false })
  assert.deepEqual(calls, [])
})

test('one tab hands over so many files a minute and no more (RD-1480-03)', async () => {
  const calls = []
  const time = clock()
  const { pageFiles } = setup(calls, { time })
  const taken = async (sender) => (await pageFiles.onMessage(DLC, sender)).taken
  for (let file = 0; file < TAB_LIMIT; file += 1) assert.equal(await taken(SENDER), true, `file ${file + 1}`)
  assert.equal(await taken(SENDER), false, 'the browser keeps the next one')
  assert.equal(await taken({ ...SENDER, tab: { id: 5 } }), true, 'another tab has its own share')
  time.advance(TAB_WINDOW_MS)
  assert.equal(await taken(SENDER), true, 'a minute later it goes on')
  assert.equal(calls.filter(([name]) => name === 'submit').length, TAB_LIMIT + 2)
})

test('a refusal is said, and the browser keeps the file', async () => {
  const calls = []
  const refused = { ok: false, status: 422, code: 'capture.container_invalid', message: 'broken container' }
  const { pageFiles } = setup(calls, { result: refused })
  assert.deepEqual(await pageFiles.onMessage(DLC, SENDER), { taken: false })
  assert.deepEqual(calls.at(-1), ['notify', 'fileKept:broken container'])
})

test('a container named only by its type gets its extension', async () => {
  const calls = []
  const { pageFiles } = setup(calls)
  await pageFiles.onMessage({ ...DLC, name: 'Release', mime: 'application/dlc' }, SENDER)
  assert.equal(calls[0][1], 'Release.dlc')
})

test('the scripts are registered for exactly the allowed sites with a standing grant', async () => {
  const calls = []
  const { pageFiles, scripts } = setup(calls, { sites: [HOST, 'revoked.test'], granted: [sitePattern(HOST)] })
  await pageFiles.sync()
  assert.deepEqual([...scripts.keys()], PAGE_SCRIPT_IDS)
  const [main, relay] = calls.find(([name]) => name === 'register')[1]
  assert.deepEqual(main.matches, ['*://hide.test/*'])
  assert.equal(main.world, 'MAIN')
  assert.equal(relay.world, undefined, "the relay runs in the extension's world")
  assert.deepEqual(relay.matches, main.matches)
})

test('a new sync replaces the registration, and none is left without a site', async () => {
  const calls = []
  const { pageFiles, scripts } = setup(calls, { sites: [], registered: PAGE_SCRIPT_IDS })
  await Promise.all([pageFiles.sync(), pageFiles.sync()])
  assert.equal(scripts.size, 0)
  assert.ok(!calls.some(([name]) => name === 'register'))

  const again = setup([], { registered: PAGE_SCRIPT_IDS })
  await Promise.all([again.pageFiles.sync(), again.pageFiles.sync()])
  assert.deepEqual([...again.scripts.keys()], PAGE_SCRIPT_IDS, 'two syncs at once never register one id twice')
})

test('pageScripts point at files that exist', () => {
  for (const script of pageScripts([HOST])) {
    for (const file of script.js) assert.ok(readFileSync(new URL(`../${file}`, import.meta.url), 'utf8').length > 0, file)
  }
})

test('decodeBytes reads base64 and refuses what is too large', () => {
  assert.equal(new TextDecoder().decode(decodeBytes(btoa('abc'))), 'abc')
  assert.equal(decodeBytes(null), null)
  assert.equal(decodeBytes(''), null, 'nothing is not a file')
  assert.equal(decodeBytes('A'.repeat(Math.ceil(PAGE_FILE_MAX_BYTES / 3) * 4 + 8)), null)
})

// --- the page script, in a small fake of a page ---------------------------------------------

const MAIN = readFileSync(new URL('../src/page-files-main.js', import.meta.url), 'utf8')

test('the page script carries the lists and the limit of intercept.js and page-files.js', () => {
  const list = (name) => JSON.parse(MAIN.match(new RegExp(`const ${name} = (\\[[^\\]]*\\])`))[1].replaceAll("'", '"'))
  assert.deepEqual(list('NAMES'), BROWSER_ONLY_EXTENSIONS)
  assert.deepEqual(list('TYPES'), BROWSER_ONLY_TYPES)
  assert.match(MAIN, /const MAX_BYTES = 16 \* 1024 \* 1024/)
  assert.equal(PAGE_FILE_MAX_BYTES, 16 * 1024 * 1024)
})

/** A page with just what the page script touches; `downloads` is what the browser would save. */
function page() {
  const listeners = { click: [], message: [] }
  const downloads = []
  const posted = []
  let next = 0
  class Element {
    closest(selector) {
      return selector === 'a' && this instanceof HTMLAnchorElement ? this : null
    }
  }
  class HTMLAnchorElement extends Element {
    constructor() {
      super()
      this.attributes = new Map()
      this.isConnected = false
      this.href = ''
    }
    set download(value) {
      this.attributes.set('download', value)
    }
    hasAttribute(name) {
      return this.attributes.has(name)
    }
    getAttribute(name) {
      return this.attributes.get(name) ?? null
    }
    click() {
      const event = { target: this, defaultPrevented: false, preventDefault() { this.defaultPrevented = true } }
      if (this.isConnected) for (const listener of listeners.click) listener(event)
      if (!event.defaultPrevented) downloads.push({ name: this.getAttribute('download'), url: this.href })
    }
  }
  const live = new Map()
  const urls = {
    createObjectURL: (blob) => {
      const url = `blob:https://${HOST}/${++next}`
      live.set(url, blob)
      return url
    },
    revokeObjectURL: (url) => live.delete(url)
  }
  const window = {
    addEventListener: (type, listener) => listeners[type].push(listener),
    postMessage: (data) => posted.push(data)
  }
  const context = vm.createContext({
    URL: urls,
    Blob,
    Element,
    HTMLAnchorElement,
    window,
    location: { origin: `https://${HOST}` },
    document: { createElement: () => new HTMLAnchorElement() },
    crypto: { randomUUID: () => `id-${++next}` },
    btoa,
    setTimeout: () => 0,
    clearTimeout: () => {}
  })
  vm.runInContext(MAIN, context)
  const answer = (id, taken) => {
    for (const listener of listeners.message) listener({ source: window, data: { type: PAGE_FILE_MESSAGE, phase: 'answer', id, taken } })
  }
  return { context, downloads, posted, live, answer }
}

/** What a container page does: build the blob, click the anchor, revoke the address at once. */
function siteDownload({ context }, name, blob, { connected = true } = {}) {
  const url = context.URL.createObjectURL(blob)
  const anchor = context.document.createElement('a')
  anchor.href = url
  anchor.download = name
  anchor.isConnected = connected
  anchor.click()
  context.URL.revokeObjectURL(url)
}

const settle = () => new Promise((resolve) => setImmediate(resolve))

test('a container download of a revoked blob is held and its bytes posted', async () => {
  const fake = page()
  siteDownload(fake, 'Release.dlc', new Blob(['dlc-bytes']))
  await settle()
  assert.deepEqual(fake.downloads, [], 'the browser does not save it')
  assert.equal(fake.posted.length, 1)
  const [message] = fake.posted
  assert.equal(message.phase, 'file')
  assert.equal(message.name, 'Release.dlc')
  // Text, not an ArrayBuffer: Firefox hides a page's ArrayBuffer from the relay (RD-1470-03).
  assert.equal(typeof message.data, 'string')
  assert.equal(atob(message.data), 'dlc-bytes')
  assert.equal(message.bytes, undefined)
  fake.answer(message.id, true)
  assert.deepEqual(fake.downloads, [], 'taken: nothing more')
})

test('refused, the browser gets the same file from a fresh address', async () => {
  const fake = page()
  siteDownload(fake, 'Release.dlc', new Blob(['dlc-bytes']), { connected: false })
  await settle()
  assert.equal(fake.posted.length, 1, 'a detached anchor is held as well')
  fake.answer(fake.posted[0].id, false)
  assert.equal(fake.downloads.length, 1)
  assert.equal(fake.downloads[0].name, 'Release.dlc')
  assert.equal(await fake.live.get(fake.downloads[0].url).text(), 'dlc-bytes')
  fake.answer(fake.posted[0].id, false)
  assert.equal(fake.downloads.length, 1, 'a second answer changes nothing')
})

test('other blob downloads are left alone, a browser-only type is held by its type alone', async () => {
  const fake = page()
  siteDownload(fake, 'export.csv', new Blob(['a,b']))
  siteDownload(fake, 'cart', new Blob(['<nzb/>'], { type: 'application/x-nzb' }))
  await settle()
  assert.deepEqual(fake.downloads.map(({ name }) => name), ['export.csv'])
  assert.equal(fake.posted.length, 1)
})

// --- the relay, in the same kind of fake --------------------------------------------------

/** Objects of the fake's own realm, compared by their content. */
const plain = (value) => JSON.parse(JSON.stringify(value))

const RELAY = readFileSync(new URL('../src/page-files-relay.js', import.meta.url), 'utf8')

/** The relay on a page; `sent` is what reaches the background, `posted` what goes back. */
function relayPage(answer, { active = true } = {}) {
  const listeners = []
  const sent = []
  const posted = []
  const window = { addEventListener: (_type, listener) => listeners.push(listener), postMessage: (data) => posted.push(data) }
  const chrome = { runtime: { sendMessage: async (message) => (sent.push(message), answer) } }
  vm.runInContext(RELAY, vm.createContext({ window, chrome, location: { origin: `https://${HOST}` }, navigator: { userActivation: { isActive: active } }, console: { info: () => {}, warn: () => {} } }))
  const post = async (data) => {
    for (const listener of listeners) await listener({ source: window, data: { type: PAGE_FILE_MESSAGE, phase: 'file', id: 'id-1', ...data } })
  }
  return { sent, posted, post }
}

test('the relay forwards the base64 text and posts the answer back', async () => {
  const relay = relayPage({ taken: true })
  await relay.post({ name: 'Release.dlc', mime: 'application/octet-stream', data: btoa('dlc-bytes') })
  assert.deepEqual(plain(relay.sent), [{ type: PAGE_FILE_MESSAGE, name: 'Release.dlc', mime: 'application/octet-stream', data: btoa('dlc-bytes') }])
  assert.deepEqual(plain(relay.posted), [{ type: PAGE_FILE_MESSAGE, phase: 'answer', id: 'id-1', taken: true }])
})

test('without a click on the page the relay forwards nothing and the browser keeps the file (RD-1480-03)', async () => {
  // A script on an allowed site posts what `page-files-main.js` would: no gesture, no hand-over.
  const relay = relayPage({ taken: true }, { active: false })
  await relay.post({ name: 'Release.dlc', mime: '', data: btoa('dlc-bytes') })
  assert.deepEqual(relay.sent, [])
  assert.deepEqual(plain(relay.posted), [{ type: PAGE_FILE_MESSAGE, phase: 'answer', id: 'id-1', taken: false }])
})

test('without text the relay asks nothing and the browser keeps the file', async () => {
  // What Firefox showed the relay of 1.46.0: the page's ArrayBuffer filtered out (RD-1470-03).
  const relay = relayPage({ taken: true })
  await relay.post({ name: 'Release.dlc', mime: '', bytes: undefined })
  assert.deepEqual(relay.sent, [])
  assert.deepEqual(plain(relay.posted), [{ type: PAGE_FILE_MESSAGE, phase: 'answer', id: 'id-1', taken: false }])
})
