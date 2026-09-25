// The browser surface the extension touches, faked once.
//
// `downloads.test.mjs` and `captcha.test.mjs` each used to carry their own nearly identical
// `fakeApi` and `setup`, and `CONFIG` stood in two files (RD-109-26). One copy means a fake that
// gains a method gains it for everybody, and that the two suites cannot drift into describing
// two different browsers.
//
// Not named `*.test.mjs`, so `node --test extension/test/*.test.mjs` does not try to run it.

import { createInterceptor } from '../src/downloads.js'
import { createCaptchaAnswerer } from '../src/captcha.js'

export const SERVER = 'http://127.0.0.1:8710'
export const CONFIG = { server: SERVER, token: 'capture-token' }

export const WIDGET = {
  id: '019d0000-0000-7000-8000-000000000001',
  kind: 'turnstile',
  page_url: 'https://ddownload.com/login.html?next=/abc',
  site_key: '0x4AAA',
  expires_at: '2026-09-16T12:05:00Z'
}

/** The popup as the background sees it: our extension id, no tab. */
export const POPUP = { id: 'self' }

export const names = (calls) => calls.map(([name]) => name)

/**
 * A clock the test moves by hand.
 *
 * It used to be `now: () => 1_000` in every download test, so the correlation window could never
 * expire, the buffer could never evict and the capability cache could never go stale — the suite
 * would have stayed green with the window logic deleted outright (RD-109-18).
 */
export function clock(start = 1_000) {
  let value = start
  return { now: () => value, advance: (ms) => { value += ms } }
}

/**
 * One fake browser, recording every call into `calls`.
 *
 * `messageRouter` is what makes a `runtime.sendMessage` a real boundary: the payload is cloned
 * on the way, exactly as a browser would serialise it, so a payload that cannot cross does not
 * cross here either.
 */
export function fakeApi(calls, {
  pause = async () => {},
  cancel,
  buttons = true,
  hostAccess = false,
  granted = true,
  session = {},
  messageRouter = null
} = {}) {
  let notificationCount = 0
  let nextTab = 40
  const api = {
    calls,
    session,
    /** Set by a test to the handler a runtime message should be delivered to. */
    messageRouter,
    runtime: {
      id: 'self',
      getURL: (path) => `chrome-extension://self/${path}`,
      sendMessage: async (payload) => {
        calls.push(['sendMessage', payload?.type])
        // structuredClone is what a real message boundary does; it throws on what cannot cross.
        const delivered = structuredClone(payload)
        if (!api.messageRouter) return undefined
        return api.messageRouter(delivered, POPUP)
      }
    },
    storage: {
      session: {
        // A real `storage.session` serialises on the way in and on the way out, exactly as a
        // runtime message does. Handing the stored object back by reference made every
        // read-modify-write across an await look safe — two callers mutated one object, so
        // nothing could be lost — and hid RD-109-23's second finding completely.
        get: async (defaults) =>
          Object.fromEntries(Object.keys(defaults).map(
            (key) => [key, session[key] === undefined ? defaults[key] : structuredClone(session[key])]
          )),
        set: async (values) => Object.assign(session, structuredClone(values))
      }
    },
    permissions: {
      contains: async (value) => { calls.push(['contains', value.origins[0]]); return hostAccess },
      request: async (value) => { calls.push(['request', value]); return granted },
      remove: async (value) => { calls.push(['remove', value]); return true }
    },
    downloads: {
      pause: async (id) => { calls.push(['pause', id]); await pause(id) },
      cancel: async (id) => { calls.push(['cancel', id]); if (cancel) await cancel(id) },
      erase: async (query) => calls.push(['erase', query.id]),
      resume: async (id) => calls.push(['resume', id])
    },
    tabs: {
      create: async (options) => { nextTab += 1; calls.push(['createTab', options]); return { id: nextTab } },
      // Records whether the tab was already tracked when it was brought to the front.
      update: async (id, options) => calls.push(['activateTab', id, options, Boolean(session.captchaTabs?.[String(id)])]),
      remove: async (id) => calls.push(['removeTab', id])
    },
    scripting: { executeScript: async (options) => calls.push(['inject', options]) },
    alarms: {
      create: async (name, options) => calls.push(['alarm', name, options]),
      clear: async (name) => calls.push(['clearAlarm', name])
    },
    action: {
      setBadgeText: async (options) => calls.push(['badge', options.text]),
      setBadgeBackgroundColor: async () => {},
      openPopup: async () => calls.push(['openPopup'])
    },
    notifications: {
      create: async (options) => {
        // Firefox rejects a notification that carries buttons; the extension has to fall back
        // to a plain one rather than lose the notification entirely.
        if (options.buttons && !buttons) throw new Error('buttons are not supported')
        notificationCount += 1
        calls.push(['notify', options.message, options.buttons ? 'with-button' : 'plain'])
        return `n${notificationCount}`
      },
      clear: async (id) => calls.push(['clearNotification', id])
    }
  }
  return api
}

/** The download interceptor over the shared fake. */
export function setupInterceptor({
  calls,
  pause,
  cancel,
  buttons,
  captureVersion = 1,
  config = {},
  submit,
  ping,
  observing = false,
  hostAccess = false,
  time = clock(),
  session = {}
} = {}) {
  const bodies = []
  const pings = { count: 0 }
  const api = fakeApi(calls, { pause, cancel, buttons, hostAccess, session })
  const interceptor = createInterceptor({
    api,
    loadConfig: async () => ({ server: SERVER, token: 'tok', interceptDownloads: true, ...config }),
    submit:
      submit ??
      (async (_config, body) => {
        bodies.push(body)
        calls.push(['submit'])
        return { ok: true, status: 201, links: 1 }
      }),
    ping:
      ping ??
      (async () => {
        pings.count += 1
        return { ok: true, status: 200, version: '0.4.0', captureVersion }
      }),
    message: (key) => key,
    userAgent: 'Mozilla/5.0 (X11) Chrome/130',
    ownExtensionId: 'self',
    now: time.now,
    observing
  })
  return { interceptor, bodies, api, time, pings, session }
}

/**
 * The captcha answerer over the shared fake.
 *
 * `send` defaults to the module's own — `api.runtime.sendMessage` — and the fake routes that to
 * this answerer's `onMessage`, so the popup's messages really cross a message boundary instead
 * of being wired straight into the handler (RD-109-26).
 */
export function setupAnswerer({ calls, api, config = CONFIG, widgets = [WIDGET], list, answer, skip, reportMissing, send } = {}) {
  const posted = []
  let nextNotification = 0
  const browser = api ?? fakeApi(calls)
  const answerer = createCaptchaAnswerer({
    api: browser,
    loadConfig: async () => config,
    message: (key, substitutions) => (substitutions ? `${key}:${substitutions.join(',')}` : key),
    // The wrapper in `background.js` takes one argument. Anything else handed to it would be
    // dropped, which is what the `{ captcha: true }` that used to ride along was doing.
    notify: async (body, ...discarded) => {
      nextNotification += 1
      calls.push(['notify', body, ...discarded])
      return `n${nextNotification}`
    },
    list: list ?? (async () => { calls.push(['list']); return { ok: true, status: 200, widgets } }),
    answer: answer ?? (async (_config, id, token) => { posted.push({ id, token }); calls.push(['answer', id]); return { ok: true, status: 200, code: 'captcha.solved' } }),
    skip: skip ?? (async (_config, id) => { calls.push(['skip', id]); return { ok: true, status: 200, code: 'captcha.skipped' } }),
    reportMissing: reportMissing ?? (async (_config, id) => { calls.push(['noWidget', id]); return { ok: true, status: 200, code: 'captcha.page_without_widget' } }),
    ...(send ? { send } : {})
  })
  browser.messageRouter = (payload, sender) => answerer.onMessage(payload, sender)
  return { answerer, posted, api: browser }
}
