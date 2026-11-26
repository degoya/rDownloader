// One fake browser with the browsers' own rules (RD-1480-02).
//
// Every suite used to bring a browser that said yes to everything, and each of the three Firefox
// failures of RD-1480-01 passed all of them: a permission asked for after the click was over, a
// filter and four host permissions with a port Firefox matches nothing with. This one enforces
// what the real ones do, from the manifest the build writes:
//
// - `permissions.request` needs the click (Firefox: synchronously inside the handler), accepts
//   only what the manifest declares, and prompts only for what is not held yet;
// - match patterns mean what they mean in that browser (`match-patterns.mjs`);
// - a `webRequest` listener is refused or fed by its filter, its extra info and the host grants;
// - messages and storage cross what they cross there (`boundary.mjs`);
// - an API appears with its permission: `cookies` after the grant, `declarativeNetRequest` in
//   Chrome only, `filterResponseData` in Firefox only.
//
// Not named `*.test.mjs`, so `node --test extension/test/*.test.mjs` does not try to run it.

import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { manifestFor } from '../build.mjs'
import { crossBoundary } from './boundary.mjs'
import { matchesPattern, subsumes } from './match-patterns.mjs'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const base = JSON.parse(readFileSync(join(root, 'manifest.base.json'), 'utf8'))

/** The extra info each browser takes; anything else makes `addListener` throw. */
const EXTRA_INFO = {
  firefox: {
    onBeforeRequest: ['blocking', 'requestBody'],
    onSendHeaders: ['requestHeaders'],
    onHeadersReceived: ['blocking', 'responseHeaders']
  },
  chrome: {
    onBeforeRequest: ['requestBody'],
    onSendHeaders: ['requestHeaders', 'extraHeaders'],
    onHeadersReceived: ['responseHeaders', 'extraHeaders']
  }
}

/**
 * A browser answers on a later task, never within the module's evaluation: a listener registered
 * once an answer came is not registered at load, and a suite that looks then has to see that.
 */
const later = () => new Promise((resolve) => setTimeout(resolve, 0))

/** Lets everything the extension started run to its end. */
export async function settle(turns = 60) {
  for (let turn = 0; turn < turns; turn += 1) await new Promise((resolve) => setTimeout(resolve, 0))
}

/**
 * A browser running one of the two builds.
 *
 * - `answer(request)` is the person at the prompt: true, false, or a list of the entries they
 *   withhold while agreeing to the rest (Firefox's data consent).
 * - `local` is what `storage.local` holds, `grants` what was granted before.
 * - `respond(url, init)` is the network; without one every request fails, as without a server.
 * - `keys` words a message as its key and substitutions; a key no catalogue has is '' either way.
 */
export function fakeBrowser(target, { answer = () => true, local = {}, grants = [], cookies = [], scripts: registered = [], calls = [], respond = null, language = 'en', keys = false } = {}) {
  const manifest = manifestFor(target, base)
  const catalogue = JSON.parse(readFileSync(join(root, '_locales', language, 'messages.json'), 'utf8'))
  const own = target === 'firefox' ? 'moz-extension://self/' : 'chrome-extension://self/'
  const prompts = []
  const fetches = []
  const listeners = []
  const filters = []
  const session = {}
  const stored = crossBoundary(target, local)
  const held = new Set([...manifest.permissions, ...manifest.host_permissions, ...grants])
  let handlingInput = false
  let sessionRules = []
  const scripts = new Map(registered.map((id) => [id, { id }]))
  const cross = (value) => crossBoundary(target, value)
  const isOrigin = (entry) => entry.includes('://') || entry === '<all_urls>'

  /** Events fire on the registered listeners; `name` is `<namespace>.<event>`. */
  function event(name) {
    return { addListener: (handler) => listeners.push({ name, handler }) }
  }

  function fire(name, ...args) {
    return listeners.filter((entry) => entry.name === name).map((entry) => entry.handler(...args))
  }

  /** Runs `work` as the browser runs a handler of a click. */
  function gesture(work) {
    handlingInput = true
    try {
      return work()
    } finally {
      // Chrome's activation outlives the handler by seconds; Firefox's ends with it.
      if (target === 'firefox') handlingInput = false
    }
  }

  function holds(entry) {
    return isOrigin(entry) ? [...held].some((other) => isOrigin(other) && subsumes(target, other, entry)) : held.has(entry)
  }

  function entriesOf(request) {
    const known = target === 'firefox' ? ['permissions', 'origins', 'data_collection'] : ['permissions', 'origins']
    for (const key of Object.keys(request)) if (!known.includes(key)) throw new TypeError(`Unexpected property: '${key}'.`)
    return known.flatMap((key) => request[key] ?? [])
  }

  function declared(entry) {
    const hosts = [...manifest.host_permissions, ...manifest.optional_host_permissions]
    if (isOrigin(entry)) return hosts.some((pattern) => subsumes(target, pattern, entry))
    const data = manifest.browser_specific_settings?.gecko?.data_collection_permissions
    return [...manifest.permissions, ...manifest.optional_permissions, ...(data?.optional ?? [])].includes(entry)
  }

  async function request(value) {
    if (!handlingInput) throw new Error('permissions.request may only be called from a user input handler')
    const entries = entriesOf(value)
    for (const entry of entries) if (!declared(entry)) throw new Error(`Cannot request ${entry}: the manifest does not declare it`)
    const wanted = entries.filter((entry) => !holds(entry))
    if (wanted.length === 0) return true
    prompts.push(value)
    const said = await answer(value)
    if (said === false) return false
    for (const entry of wanted) if (!(Array.isArray(said) && said.includes(entry))) held.add(entry)
    return true
  }

  const permissions = {
    request(value) {
      calls.push(['request', value])
      return request(value)
    },
    contains: async (value) => {
      const entries = entriesOf(value)
      await later()
      return entries.every(holds)
    },
    remove: async (value) => {
      calls.push(['remove', value])
      const entries = entriesOf(value)
      for (const entry of entries) if (manifest.permissions.includes(entry)) throw new Error('You cannot remove required permissions.')
      for (const entry of entries) held.delete(entry)
      fire('permissions.onRemoved', value)
      return true
    },
    onRemoved: event('permissions.onRemoved')
  }

  /** Whether the extension may see a request to `url`: a host grant covers it. */
  function hostAccess(url) {
    return [...held].some((entry) => isOrigin(entry) && matchesPattern(target, entry, url))
  }

  function webRequestEvent(name) {
    return {
      addListener(handler, filter, spec = []) {
        for (const pattern of filter.urls) matchesPattern(target, pattern, 'http://example.test/')
        for (const flag of spec) {
          if (!EXTRA_INFO[target][name].includes(flag)) throw new Error(`${flag} is not supported by webRequest.${name}`)
        }
        listeners.push({ name: `webRequest.${name}`, handler, filter, spec })
      }
    }
  }

  /**
   * One request of a page. Only listeners whose filter takes it are called, only with host access
   * to its address and to the page that made it, and a body only where it was asked for.
   */
  async function webRequest(name, details) {
    const page = details.initiator ?? details.originUrl
    const results = []
    for (const { name: registered, handler, filter, spec } of listeners) {
      if (registered !== `webRequest.${name}`) continue
      if (!filter.urls.some((pattern) => matchesPattern(target, pattern, details.url))) continue
      if (filter.types && !filter.types.includes(details.type)) continue
      if (!hostAccess(details.url) || (page && details.type !== 'main_frame' && !hostAccess(page))) continue
      const { requestBody, ...seen } = details
      const result = await handler(spec.includes('requestBody') ? { ...seen, requestBody } : seen)
      if (spec.includes('blocking') && result && Object.keys(result).length) results.push(result)
    }
    return results
  }

  function area(values, name) {
    return {
      get: async (defaults) => {
        await later()
        return Object.fromEntries(Object.entries(defaults).map(([key, fallback]) => [key, cross(values[key] === undefined ? fallback : values[key])]))
      },
      set: async (changed) => {
        if (name === 'local') calls.push(['store', cross(changed)])
        Object.assign(values, cross(changed))
        fire('storage.onChanged', Object.fromEntries(Object.keys(changed).map((key) => [key, { newValue: cross(changed[key]) }])), name)
      },
      // Chrome's way of keeping an area from the content scripts; Firefox has none.
      ...(target === 'chrome' ? { setAccessLevel: async (options) => { calls.push(['setAccessLevel', name, options]) } } : {})
    }
  }

  function getMessage(key, substitutions = []) {
    const entry = catalogue[key]
    if (!entry) return ''
    if (key === 'pageLanguage') return entry.message
    if (keys) return substitutions.length ? `${key}:${substitutions.join(',')}` : key
    const slots = entry.message.replace(/\$([A-Z_]+)\$/g, (_whole, name) => entry.placeholders[name.toLowerCase()].content)
    return slots.replace(/\$(\d)/g, (_whole, slot) => substitutions[Number(slot) - 1] ?? '')
  }

  /** Delivers a message to the background's listeners, as `sender`, and answers what they answer. */
  function deliver(payload, sender) {
    return new Promise((resolve, reject) => {
      const answered = listeners
        .filter((entry) => entry.name === 'runtime.onMessage')
        .map((entry) => entry.handler(cross(payload), sender, (response) => resolve(cross(response))))
      if (!answered.length) reject(new Error('Could not establish connection. Receiving end does not exist.'))
      else if (!answered.includes(true)) resolve(undefined)
    })
  }

  let notificationCount = 0
  const record = (name, result) => async (...args) => { calls.push([name, ...args]); return typeof result === 'function' ? result(...args) : result }

  const api = {
    runtime: {
      id: 'self',
      getURL: (path) => `${own}${path}`,
      getManifest: () => structuredClone(manifest),
      openOptionsPage: () => { calls.push(['openOptionsPage']) },
      sendMessage: (payload) => deliver(payload, { id: 'self', url: `${own}src/popup.html` }),
      onInstalled: event('runtime.onInstalled'),
      onStartup: event('runtime.onStartup'),
      onMessage: event('runtime.onMessage')
    },
    i18n: { getMessage, getUILanguage: () => language },
    storage: { local: area(stored, 'local'), session: area(session, 'session'), onChanged: event('storage.onChanged') },
    permissions,
    get cookies() {
      if (!held.has('cookies')) return undefined
      return {
        getAll: async (query) => {
          calls.push(['getAll', query])
          return cookies.filter((cookie) => hostAccess(`https://${cookie.domain.replace(/^\./, '')}/`))
        }
      }
    },
    contextMenus: { create: (options) => calls.push(['menu', options.id, options.title, options.contexts.join(',')]), removeAll: (done) => done(), onClicked: event('contextMenus.onClicked') },
    alarms: { create: record('alarm'), clear: record('clearAlarm'), onAlarm: event('alarms.onAlarm') },
    tabs: {
      create: record('createTab', { id: 41 }),
      update: record('activateTab'),
      remove: record('removeTab'),
      query: async () => [{ id: 7, url: 'https://hoster.test/folder', title: 'A folder' }],
      onUpdated: event('tabs.onUpdated'),
      onRemoved: event('tabs.onRemoved')
    },
    downloads: {
      pause: record('pause'),
      resume: record('resume'),
      cancel: record('cancel'),
      removeFile: record('removeFile'),
      erase: async (query) => { calls.push(['erase', query.id]) },
      onCreated: event('downloads.onCreated')
    },
    notifications: {
      create: async (options) => {
        // Firefox refuses a notification that carries buttons.
        if (options.buttons && target === 'firefox') throw new Error('buttons are not supported')
        calls.push(['notify', options.message, options.buttons ? 'with-button' : 'plain'])
        notificationCount += 1
        return `n${notificationCount}`
      },
      clear: record('clearNotification'),
      onClicked: event('notifications.onClicked'),
      ...(target === 'chrome' ? { onButtonClicked: event('notifications.onButtonClicked') } : {})
    },
    action: { setBadgeText: async (options) => { calls.push(['badge', options.text]) }, setBadgeBackgroundColor: async () => {} },
    scripting: {
      executeScript: record('inject'),
      getRegisteredContentScripts: async ({ ids }) => ids.filter((id) => scripts.has(id)).map((id) => scripts.get(id)),
      unregisterContentScripts: async ({ ids }) => {
        calls.push(['unregister', ids])
        for (const id of ids) scripts.delete(id)
      },
      registerContentScripts: async (list) => {
        calls.push(['register', list])
        for (const script of list) {
          if (scripts.has(script.id)) throw new Error(`Duplicate script ID '${script.id}'`)
          scripts.set(script.id, script)
        }
      }
    },
    webRequest: {
      onBeforeRequest: webRequestEvent('onBeforeRequest'),
      onSendHeaders: webRequestEvent('onSendHeaders'),
      onHeadersReceived: webRequestEvent('onHeadersReceived')
    }
  }
  if (manifest.permissions.includes('webRequestFilterResponse')) {
    api.webRequest.filterResponseData = (requestId) => {
      const filter = { requestId, written: [], closed: false, write: (data) => filter.written.push(data), close: () => { filter.closed = true }, disconnect: () => {} }
      filters.push(filter)
      return filter
    }
  }
  if (manifest.permissions.includes('declarativeNetRequestWithHostAccess')) {
    api.declarativeNetRequest = {
      updateSessionRules: async ({ removeRuleIds, addRules }) => {
        calls.push(['rules', addRules.length])
        sessionRules = [...sessionRules.filter((rule) => !removeRuleIds.includes(rule.id)), ...addRules]
      },
      getSessionRules: async () => sessionRules
    }
  }

  /** The extension's own `fetch`; `hostAccess` says whether it needs the server's CORS headers. */
  async function fetchFake(url, init = {}) {
    calls.push(['fetch', String(url)])
    fetches.push({ url: String(url), init, hostAccess: hostAccess(String(url)) })
    const response = await respond?.(String(url), init)
    if (!response) throw new TypeError('NetworkError when attempting to fetch resource.')
    return response
  }

  return {
    target, api, manifest, calls, prompts, fetches, filters, listeners, scripts, stored,
    held: () => [...held],
    rules: () => sessionRules,
    notices: () => calls.filter(([name]) => name === 'notify').map(([, text]) => text),
    text: getMessage,
    fire,
    gesture,
    webRequest,
    deliver,
    fetch: fetchFake
  }
}

/**
 * Loads a module of the extension in this browser, with a module graph of its own: `browser.js`
 * reads the browser's namespace once, so two loads from one directory would share the first.
 */
export async function loadIn(browser, file) {
  const dir = mkdtempSync(join(tmpdir(), 'rdownloader-extension-src-'))
  process.on('exit', () => rmSync(dir, { recursive: true, force: true }))
  cpSync(join(root, 'src'), join(dir, 'src'), { recursive: true })
  writeFileSync(join(dir, 'package.json'), '{"type":"module"}')
  globalThis.chrome = browser.api
  if (browser.target === 'firefox') globalThis.browser = browser.api
  else delete globalThis.browser
  globalThis.fetch = browser.fetch
  return import(pathToFileURL(join(dir, 'src', file)).href)
}
